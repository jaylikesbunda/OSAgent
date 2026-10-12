//! Run lifecycle: guards, busy checks, spawn/stop/cancel, cancellation plumbing
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn answer_question(&self, question_id: &str, answers: Vec<Vec<String>>) -> bool {
        self.event_bus.answer_question(question_id, answers).await
    }

    /// Get or create a lock for a specific session
    pub(super) fn get_session_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.session_locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Get or create a cancellation notifier for a specific session
    pub(super) fn get_cancellation_notify(&self, session_id: &str) -> Arc<Notify> {
        self.session_cancellation
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    pub(super) fn get_cancel_flag(&self, session_id: &str) -> Arc<std::sync::atomic::AtomicBool> {
        self.session_cancel_flags
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
            .clone()
    }

    pub(super) fn clear_cancel_flag(&self, session_id: &str) {
        self.session_cancel_flags.remove(session_id);
    }

    pub(super) async fn wait_for_cancellation(&self, session_id: &str) {
        let notify = self.get_cancellation_notify(session_id);
        let flag = self.get_cancel_flag(session_id);
        wait_for_cancel_signal(&notify, &flag).await;
    }

    /// Explicit Stop pauses autonomous work but retains queued user messages.
    pub async fn stop_session(&self, session_id: &str) -> Result<()> {
        self.stopped_sessions.insert(session_id.to_string(), ());
        self.cancel_session(session_id);
        let goal_result = (|| {
            if let Some(goal) = self.goal_store.get(session_id)? {
                if goal.phase == crate::storage::GoalPhase::Active {
                    self.goal_store
                        .apply_action(session_id, goal.revision, "pause", None, None)?;
                }
            }
            self.remove_pending_goal_rounds(session_id)
        })();
        self.cancel_subagents_for_parent(session_id).await;
        goal_result
    }

    pub fn resume_session_queue(&self, session_id: &str) {
        self.stopped_sessions.remove(session_id);
    }

    /// Cancel any in-progress operation for a session
    pub fn cancel_session(&self, session_id: &str) {
        if let Some(flag) = self.session_cancel_flags.get(session_id) {
            flag.store(true, std::sync::atomic::Ordering::Release);
        }
        if let Some(notify) = self.session_cancellation.get(session_id) {
            notify.notify_waiters();
            info!("Cancelled in-progress operation for session {}", session_id);
        }
        // A run parked on an unanswered question never polls the
        // cancellation flag; drop its pending questions so the tool
        // future returns and the loop can observe the cancel.
        let event_bus = self.event_bus.clone();
        let session_id = session_id.to_string();
        tokio::spawn(async move {
            let cancelled = event_bus.cancel_questions_for_session(&session_id).await;
            if cancelled > 0 {
                info!(
                    "Cancelled {} pending question(s) for session {}",
                    cancelled, session_id
                );
            }
        });
    }

    /// Cancel all subagents belonging to a parent session
    pub async fn cancel_subagents_for_parent(&self, session_id: &str) {
        self.subagent_manager
            .cancel_all_for_parent(session_id)
            .await;
    }

    /// Emit the terminal event for a run that returned an error. Cancellation
    /// is emitted as a single `Cancelled`; everything else as `Error`.
    pub(super) fn emit_run_error_event(&self, session_id: &str, error: &OSAgentError) {
        if is_operation_cancelled(error) {
            self.event_bus.emit(AgentEvent::Cancelled {
                session_id: session_id.to_string(),
                sequence: 0,
                timestamp: SystemTime::now(),
            });
        } else {
            self.event_bus.emit(AgentEvent::Error {
                session_id: session_id.to_string(),
                sequence: 0,
                error: error.to_string(),
                recoverable: error.is_recoverable(),
                timestamp: SystemTime::now(),
            });
        }
    }

    /// Called by the SubagentManager when a background subagent reaches a
    /// terminal state. Starts a continuation turn on the parent right away if
    /// it is idle; otherwise parks the request so it is honored when the
    /// current run ends (see [`Self::check_pending_wake`]).
    pub fn notify_background_task_finished(&self, parent_session_id: String) {
        let Some(runtime) = self.self_arc.get().and_then(|weak| weak.upgrade()) else {
            return;
        };
        tokio::spawn(async move {
            runtime.wake_for_subagent_results(parent_session_id).await;
        });
    }

    /// Start a continuation turn for undelivered background subagent results,
    /// unless auto-resume is disabled, queued user messages are waiting (they
    /// will carry the results on their own turn), or the session is busy.
    pub(super) async fn wake_for_subagent_results(self: Arc<Self>, session_id: String) {
        if !self.agent_settings.read().await.subagent_auto_resume {
            return;
        }
        if self
            .storage
            .get_session(&session_id)
            .ok()
            .flatten()
            .is_none()
        {
            return;
        }
        // Queued user messages take priority: merging happens at the start of
        // every turn, so the next queued message delivers the results anyway.
        match self.storage.list_queued_messages(&session_id) {
            Ok(items) if !items.is_empty() => return,
            Ok(_) => {}
            Err(e) => {
                warn!(
                    "Wake check for {}: failed to list queued messages: {}",
                    session_id, e
                );
                return;
            }
        }
        self.start_wake_turn(session_id).await;
    }

    /// Acquire the run guard and spawn a continuation turn. Parks the request
    /// when another run is already active.
    pub(super) async fn start_wake_turn(self: Arc<Self>, session_id: String) {
        if self.stopped_sessions.contains_key(&session_id) {
            self.pending_wakes.insert(session_id, ());
            return;
        }
        if self.active_runs.contains_key(&session_id) {
            self.pending_wakes.insert(session_id, ());
            return;
        }

        // Loop guard: too many consecutive background-driven turns without a
        // real user message means we stop waking until the user chimes in.
        let max_auto_turns = self
            .agent_settings
            .read()
            .await
            .subagent_auto_resume_max_turns;
        {
            let counter = self.auto_wake_turns.entry(session_id.clone()).or_insert(0);
            if *counter >= max_auto_turns {
                warn!(
                    "Session {}: {} consecutive auto-continuation turns (cap {}) — leaving remaining background result(s) for the user's next message",
                    session_id, *counter, max_auto_turns
                );
                return;
            }
        }

        // Double-check there is still something to deliver (another consumer
        // may have merged it between notification and now).
        match self
            .storage
            .list_unnotified_completed_for_parent(&session_id)
        {
            Ok(tasks) if tasks.is_empty() => return,
            Ok(_) => {}
            Err(_) => return,
        }

        let run_guard = match self.try_start_run(&session_id, "background-task-wake") {
            Ok(guard) => guard,
            Err(OSAgentError::Session(message)) if message.contains("already in progress") => {
                self.pending_wakes.insert(session_id, ());
                return;
            }
            Err(error) => {
                warn!("Wake turn for {} could not start: {}", session_id, error);
                return;
            }
        };

        if self.stopped_sessions.contains_key(&session_id) {
            self.pending_wakes.insert(session_id, ());
            return;
        }

        info!(
            "Background subagent result(s) ready for session {} — starting continuation turn",
            session_id
        );

        let runtime = self.clone();
        tokio::spawn(async move {
            let result = runtime
                .process_message_internal(
                    &session_id,
                    String::new(),
                    "background-task-wake".to_string(),
                    Some(serde_json::json!({"kind": "subagent_wake"})),
                    None,
                    None,
                    None,
                    None,
                )
                .await;

            if let Err(error) = &result {
                error!("Wake turn failed for session {}: {}", session_id, error);
                runtime.emit_run_error_event(&session_id, error);
            }

            drop(run_guard);
            runtime.dispatch_queued_messages(&session_id, "system");
        });
    }

    /// Called at run end: if a background subagent finished while this run was
    /// active, start the deferred continuation turn.
    pub fn check_pending_wake(&self, session_id: &str) {
        if self.stopped_sessions.contains_key(session_id) {
            return;
        }
        if self.pending_wakes.remove(session_id).is_some() {
            info!(
                "Pending background-result wake found for session {}",
                session_id
            );
            if let Some(runtime) = self.self_arc.get().and_then(|weak| weak.upgrade()) {
                let sid = session_id.to_string();
                tokio::spawn(async move {
                    // Give the queue dispatcher's delayed claim a head start.
                    tokio::time::sleep(std::time::Duration::from_millis(150)).await;
                    runtime.wake_for_subagent_results(sid).await;
                });
            }
        }
    }

    /// Merge results of background subagents that finished since the parent's
    /// last turn into the parent session as synthetic user messages, so the
    /// parent notices them on its next turn. Marks them notified so they are
    /// delivered exactly once. Returns how many results were merged.
    pub(super) fn merge_pending_subagent_results(
        &self,
        session: &mut Session,
        session_id: &str,
    ) -> usize {
        let tasks = match self
            .storage
            .list_unnotified_completed_for_parent(session_id)
        {
            Ok(tasks) => tasks,
            Err(e) => {
                warn!(
                    "Failed to list completed background subagents for {}: {}",
                    session_id, e
                );
                return 0;
            }
        };
        for task in &tasks {
            let state = match task.status.as_str() {
                "completed" => "completed",
                "partial" => "partial",
                "timeout" => "timeout",
                "cancelled" => "cancelled",
                _ => "error",
            };
            let result = task
                .result
                .clone()
                .unwrap_or_else(|| "No result".to_string());
            let body = format!(
                "<task id=\"{}\" state=\"{}\">\n<summary>Background subagent '{}' finished: {}</summary>\n<task_result>\n{}\n</task_result>\n</task>",
                task.session_id, state, task.description, task.status, result
            );
            session
                .messages
                .push(Message::synthetic_user(body, "subagent_completed"));
            if let Err(e) = self.storage.mark_subagent_notified(&task.id) {
                warn!("Failed to mark subagent task {} notified: {}", task.id, e);
            }
        }
        if !tasks.is_empty() {
            info!(
                "Merged {} completed background subagent result(s) into session {}",
                tasks.len(),
                session_id
            );
        }
        tasks.len()
    }

    /// Check if a session has an active operation
    pub fn is_session_busy(&self, session_id: &str) -> bool {
        self.active_runs.contains_key(session_id)
    }

    pub(super) fn try_start_run(&self, session_id: &str, user: &str) -> Result<RunGuard> {
        let entry = match self.active_runs.entry(session_id.to_string()) {
            dashmap::mapref::entry::Entry::Vacant(entry) => entry,
            dashmap::mapref::entry::Entry::Occupied(_) => {
                return Err(OSAgentError::Session(
                    "A run is already in progress for this session".to_string(),
                ))
            }
        };
        self.clear_cancel_flag(session_id);
        self.get_cancel_flag(session_id);
        entry.insert(ActiveRunInfo {
            started_at: SystemTime::now(),
            user: user.to_string(),
        });

        Ok(RunGuard {
            session_id: session_id.to_string(),
            active_runs: self.active_runs.clone(),
        })
    }

    pub async fn spawn_message_run(
        self: Arc<Self>,
        session_id: String,
        user_message: String,
        user: String,
    ) -> Result<()> {
        if self.get_session(&session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }

        let run_guard = self.try_start_run(&session_id, &user)?;
        let runtime = self.clone();

        tokio::spawn(async move {
            let result = runtime
                .process_message_internal(
                    &session_id,
                    user_message,
                    user.clone(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .await;
            if let Err(error) = &result {
                error!(
                    "Background run failed for session {} (user {}): {}",
                    session_id, user, error
                );
                runtime.emit_run_error_event(&session_id, error);
            }
            drop(run_guard);
            runtime.dispatch_queued_messages(&session_id, &user);
        });

        Ok(())
    }

    pub fn spawn_next_queued_message_run(
        self: Arc<Self>,
        session_id: String,
        user: String,
    ) -> Result<Option<String>> {
        if self.stopped_sessions.contains_key(&session_id) {
            return Ok(None);
        }
        if self.storage.get_session(&session_id)?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }

        let run_guard = match self.try_start_run(&session_id, &user) {
            Ok(guard) => guard,
            Err(OSAgentError::Session(message)) if message.contains("already in progress") => {
                return Ok(None);
            }
            Err(error) => return Err(error),
        };

        let queued_message = loop {
            if self.stopped_sessions.contains_key(&session_id) {
                return Ok(None);
            }
            match self.storage.claim_next_queued_message(&session_id)? {
                Some(item) if item.client_message_id.starts_with("goal-round-") => {
                    let current = self.goal_store.get(&session_id)?;
                    let valid = current.as_ref().is_some_and(|goal| {
                        goal.phase == crate::storage::GoalPhase::Active
                            && self.goal_store.is_armed(&session_id)
                            && item
                                .client_message_id
                                .starts_with(&format!("goal-round-{}-", goal.id))
                    });
                    if valid {
                        break item;
                    }
                    self.storage.delete_queued_message(&item.id)?;
                }
                Some(item) => break item,
                None => {
                    drop(run_guard);
                    // Queue fully drained: honor any deferred background-completion
                    // wake before the session goes idle.
                    self.check_pending_wake(&session_id);
                    return Ok(None);
                }
            }
        };

        let started_queue_id = queued_message.id.clone();
        let runtime = self.clone();
        tokio::spawn(async move {
            let queue_entry_id = queued_message.id.clone();
            let client_message_id = queued_message.client_message_id.clone();
            let result = runtime
                .process_message_internal(
                    &session_id,
                    queued_message.content.clone(),
                    user.clone(),
                    Some(serde_json::json!({
                        "queued": true,
                        "queue_entry_id": queue_entry_id,
                        "client_message_id": client_message_id,
                    })),
                    Some(queued_message.id.clone()),
                    Some(queued_message.images.clone()),
                    queued_message.attachment_context.clone(),
                    Some(queued_message.attachments.clone()),
                )
                .await;

            if let Err(error) = &result {
                error!(
                    "Queued run failed for session {} (user {}): {}",
                    session_id, user, error
                );
                runtime.emit_run_error_event(&session_id, error);
            }

            drop(run_guard);

            // Always hand the queue another chance once the run guard drops.
            // Ok, cancellation and terminal errors all fall through here so a
            // message that was enqueued mid-run can never be left stranded by
            // a chain that stopped advancing. `dispatch_queued_messages` is a
            // delayed no-op when the queue is empty and also honors deferred
            // background-completion wakes.
            runtime.dispatch_queued_messages(&session_id, &user);
        });

        Ok(Some(started_queue_id))
    }
}
