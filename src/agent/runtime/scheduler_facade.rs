//! Scheduler control facades
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub fn scheduler(&self) -> &Arc<Scheduler> {
        &self.scheduler
    }

    pub fn storage(&self) -> &Arc<SqliteStorage> {
        &self.storage
    }

    pub fn event_bus(&self) -> &EventBus {
        &self.event_bus
    }

    pub fn config(&self) -> Arc<tokio::sync::RwLock<Config>> {
        Arc::clone(&self.config)
    }

    pub async fn start_scheduler(self: &Arc<Self>) -> Result<()> {
        self.scheduler.start().await?;

        let rx = {
            let mut guard = self.run_prompt_rx.lock().await;
            guard.take()
        };

        if let Some(mut rx) = rx {
            let this = Arc::clone(self);
            tokio::spawn(async move {
                while let Some(req) = rx.recv().await {
                    let requested_session_id = req.session_id.clone();
                    let created_session = requested_session_id.is_none();
                    let session_id = match requested_session_id {
                        Some(sid) => sid,
                        None => {
                            let session = match this.create_session().await {
                                Ok(s) => s,
                                Err(e) => {
                                    warn!("Scheduler run_prompt: failed to create session: {}", e);
                                    if let Some(tx) = req.response_tx {
                                        let _ = tx
                                            .send(Err(format!("Failed to create session: {}", e)));
                                    }
                                    continue;
                                }
                            };
                            info!("Scheduler run_prompt: created session {}", session.id);
                            session.id
                        }
                    };

                    if created_session {
                        if let Ok(Some(mut session)) = this.get_session(&session_id).await {
                            if !session.metadata.is_object() {
                                session.metadata = serde_json::json!({});
                            }
                            if let Some(meta) = session.metadata.as_object_mut() {
                                let label = match req.job_type.as_str() {
                                    "reminder" => "Scheduled reminder",
                                    "daily_briefing" => "Daily briefing",
                                    _ => "Scheduled task",
                                };
                                let preview = req.prompt.chars().take(60).collect::<String>();
                                meta.insert(
                                    "name".to_string(),
                                    serde_json::Value::String(format!("{}: {}", label, preview)),
                                );
                                meta.insert(
                                    "source".to_string(),
                                    serde_json::Value::String("scheduler".to_string()),
                                );
                                if let Some(source) = &req.source {
                                    meta.insert(
                                        "delivery_source".to_string(),
                                        serde_json::Value::String(source.clone()),
                                    );
                                }
                                meta.insert(
                                    "scheduled_job_id".to_string(),
                                    serde_json::Value::String(req.job_id.clone()),
                                );
                                meta.insert(
                                    "scheduled_job_type".to_string(),
                                    serde_json::Value::String(req.job_type.clone()),
                                );
                            }
                            if let Err(e) = this.session_manager.update_session(&session).await {
                                warn!("Failed to label scheduled session: {}", e);
                            }
                        }
                    }

                    info!("Scheduler job executing in session {}", session_id);

                    // Pin the job's workspace before the run so prompt jobs act
                    // in the workspace the user chose instead of the session's
                    // (or the active one). A chat that already has messages is
                    // workspace-locked; log and fall back rather than fail the
                    // whole job.
                    if req.run_agent {
                        if let Some(workspace_id) = req.workspace_id.as_deref() {
                            if let Err(e) =
                                this.set_session_workspace(&session_id, workspace_id).await
                            {
                                warn!(
                                    "Scheduler job {}: could not apply workspace '{}' to session {}: {}",
                                    req.job_id, workspace_id, session_id, e
                                );
                            }
                        }
                    }

                    let result: std::result::Result<String, String> = if req.run_agent {
                        match tokio::time::timeout(
                            std::time::Duration::from_secs(290),
                            this.process_message(&session_id, req.prompt, "scheduler".to_string()),
                        )
                        .await
                        {
                            Ok(Ok(response)) => Ok(response),
                            Ok(Err(e)) => Err(e.to_string()),
                            Err(_) => Err("Agent execution timed out".to_string()),
                        }
                    } else {
                        match this.get_session(&session_id).await {
                            Ok(Some(mut session)) => {
                                session.messages.push(crate::storage::Message::assistant(
                                    req.prompt.clone(),
                                    None,
                                ));
                                session.updated_at = Utc::now();
                                match this.session_manager.update_session(&session).await {
                                    Ok(()) => Ok(req.prompt.clone()),
                                    Err(e) => Err(e.to_string()),
                                }
                            }
                            Ok(None) => Err("Scheduled session was not found".to_string()),
                            Err(e) => Err(e.to_string()),
                        }
                    };

                    if let Some(tx) = req.response_tx {
                        match result {
                            Ok(response) => {
                                let _ = tx.send(Ok(RunPromptResponse {
                                    session_id: session_id.clone(),
                                    response,
                                }));
                            }
                            Err(e) => {
                                let _ = tx.send(Err(e.clone()));
                                error!("Scheduler job failed for session {}: {}", session_id, e);
                            }
                        }
                    } else if let Err(e) = result {
                        error!("Scheduler job failed for session {}: {}", session_id, e);
                    }
                }
            });
        }

        Ok(())
    }

    pub async fn stop_scheduler(&self) {
        self.scheduler.stop().await;
    }
}
