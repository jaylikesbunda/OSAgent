//! Turn pipeline: process_message, stream consumption, chunk emission, retries
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn process_message(
        &self,
        session_id: &str,
        user_message: String,
        user: String,
    ) -> Result<String> {
        let run_guard = self.try_start_run(session_id, &user)?;
        let result = self
            .process_message_internal(
                session_id,
                user_message,
                user.clone(),
                None,
                None,
                None,
                None,
                None,
            )
            .await;
        drop(run_guard);
        self.dispatch_queued_messages(session_id, &user);
        result
    }

    pub(super) async fn process_message_internal(
        &self,
        session_id: &str,
        user_message: String,
        user: String,
        message_metadata: Option<serde_json::Value>,
        queue_entry_id: Option<String>,
        images: Option<Vec<MessageImage>>,
        attachment_context: Option<String>,
        attachments: Option<Vec<MessageAttachment>>,
    ) -> Result<String> {
        info!("process_message: Starting for session {}", session_id);

        // Acquire session lock to serialize requests
        let session_lock = self.get_session_lock(session_id);
        let _lock_guard = session_lock.lock().await;

        // Get cancellation notifier for this session
        let cancel_notify = self.get_cancellation_notify(session_id);
        let cancel_flag = self.get_cancel_flag(session_id);

        let active_run = self
            .active_runs
            .get(session_id)
            .map(|entry| entry.clone())
            .unwrap_or(ActiveRunInfo {
                started_at: SystemTime::now(),
                user: user.clone(),
            });

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| {
                error!("Session not found: {}", session_id);
                OSAgentError::Session("Session not found".to_string())
            })?;

        info!("process_message: Session loaded, adding user message");
        info!("Session metadata: {:?}", session.metadata);

        // Wake turns are started by `start_wake_turn` when background subagent
        // results are ready: the turn is driven entirely by the merged <task>
        // blocks below, with no visible user message of its own.
        let is_wake_turn = message_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("kind"))
            .and_then(|value| value.as_str())
            == Some("subagent_wake");
        let merged_subagent_results = self.merge_pending_subagent_results(&mut session, session_id);

        // Track streaks of auto-continuation turns so a model that keeps
        // spawning background work cannot loop forever without a human.
        if is_wake_turn {
            *self
                .auto_wake_turns
                .entry(session_id.to_string())
                .or_insert(0) += 1;
        } else {
            self.auto_wake_turns.remove(session_id);
        }

        let repo_exploration_request = is_repo_exploration_request(&user_message);
        let queued_client_message_id = message_metadata
            .as_ref()
            .and_then(|metadata| metadata.get("client_message_id"))
            .and_then(|value| value.as_str())
            .map(|value| value.to_string());

        let image_items = images.unwrap_or_default();
        let image_count = image_items.len();
        let attachment_items = attachments.unwrap_or_default();
        let visible_attachments = attachment_items
            .iter()
            .filter(|attachment| attachment.kind != "image")
            .cloned()
            .collect::<Vec<_>>();
        info!(
            "process_message: adding user message with {} image(s), {} attachment(s), and {} text chars",
            image_count,
            attachment_items.len(),
            user_message.chars().count()
        );

        if let Some(context) = attachment_context {
            if !context.trim().is_empty() {
                session
                    .messages
                    .push(Message::synthetic_user(context, "attachment_context"));
            }
        }

        if is_wake_turn {
            if merged_subagent_results == 0 {
                warn!(
                    "Wake turn for session {} found no pending background results; skipping",
                    session_id
                );
                return Ok(String::new());
            }
            info!(
                "process_message: wake turn for session {} driven by {} background result(s)",
                session_id, merged_subagent_results
            );
        } else {
            let mut message = Message::user_with_images(user_message.clone(), image_items.clone());
            if let Some(metadata) = message_metadata {
                message.metadata = metadata;
            }
            if !attachment_items.is_empty() {
                if let Some(obj) = message.metadata.as_object_mut() {
                    obj.insert(
                        "attachments".to_string(),
                        serde_json::to_value(&visible_attachments)
                            .unwrap_or_else(|_| serde_json::json!([])),
                    );
                }
            }
            session.messages.push(message);

            let is_community = session
                .metadata
                .get("discord_community")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            if !is_community {
                match self
                    .decision_memory
                    .maybe_capture_from_user_message(&user_message, &user)
                    .await
                {
                    Ok(DecisionCaptureOutcome::Ignored) => {}
                    Ok(DecisionCaptureOutcome::Recorded(entry)) => {
                        info!(
                            "Captured approved decision from user message: {}",
                            entry.key
                        );
                    }
                    Ok(DecisionCaptureOutcome::Suggested(suggestion)) => {
                        info!(
                            "Captured decision suggestion from user message: {} (id: {})",
                            suggestion.key, suggestion.id
                        );
                    }
                    Err(error) => {
                        warn!("Failed to capture approved decision memory: {}", error);
                    }
                }
            }
        }

        // Mark session as running so frontend can restore thinking indicator on switch
        session.task_status = "running".to_string();
        if let Err(e) = self.session_manager.update_session(&session).await {
            warn!("Failed to update session task_status to running: {}", e);
        }
        if let Some(queue_entry_id) = queue_entry_id.as_ref() {
            if let Err(error) = self.storage.delete_queued_message(queue_entry_id) {
                warn!(
                    "Failed to remove queued message {} after dispatch: {}",
                    queue_entry_id, error
                );
            }
            self.event_bus.emit(AgentEvent::QueuedMessageDispatched {
                session_id: session_id.to_string(),
                sequence: 0,
                queue_entry_id: queue_entry_id.clone(),
                client_message_id: queued_client_message_id
                    .clone()
                    .unwrap_or_else(|| queue_entry_id.clone()),
                content: user_message.clone(),
                images: image_items.clone(),
                attachments: visible_attachments.clone(),
                timestamp: SystemTime::now(),
            });
        }
        let _status_guard =
            TaskStatusGuard::new(session_id.to_string(), self.session_manager.clone());

        // Emit thinking event
        self.event_bus.emit(AgentEvent::Thinking {
            session_id: session_id.to_string(),
            sequence: 0,
            message: format!("Processing your request... started by {}", active_run.user),
            timestamp: SystemTime::now(),
        });

        let runtime_config = self.config.read().await.clone();
        let active_workspace = Self::resolve_workspace_for_session(&session, &runtime_config)
            .unwrap_or_else(|| runtime_config.get_active_workspace());

        let workspace_path = active_workspace.resolved_path();
        let workspace_root = PathBuf::from(shellexpand::tilde(&workspace_path).to_string());
        let config_dir = runtime_config.config_dir();
        let global_instruction_reminder =
            format_system_reminder(&global_instruction_blocks(&config_dir));
        let workspace_instruction_reminder =
            format_system_reminder(&workspace_instruction_blocks(&workspace_root));
        let git_context = git_workspace_context(&workspace_root).await;
        let is_git_repo = workspace_root.join(".git").is_dir() || git_context.is_some();
        let skill_summary_prompt = self.tool_registry.skill_summary_prompt();
        // The deferred built-in manifest is core to the agent's own
        // toolset, so it is always shown. The MCP manifest is read per
        // run, not at startup: servers connect in the background and the
        // user can add more from the UI mid-session.
        let mcp_manifest_prompt = if runtime_config.mcp.manifest_in_prompt {
            self.tool_registry.mcp_manifest_prompt()
        } else {
            None
        };
        let native_manifest_prompt = self.tool_registry.native_manifest_prompt();
        let context_provider = self.active_provider().await;
        let context_provider_type = context_provider.provider_type().to_string();
        let context_model = context_provider.current_model().await;

        info!(
            "Resolved workspace for session: {} (path: {})",
            active_workspace.id, workspace_path
        );

        let mut iteration = 0;
        let agent_settings = self.agent_settings.read().await;
        let max_iterations = agent_settings.max_iterations.max(1);
        drop(agent_settings);
        // Long agentic runs used to be cut off the moment the configured
        // budget was spent. Treat that value as the first checkpoint instead:
        // each time it is reached the model is nudged to continue and gets
        // another budget-sized chunk, up to a hard ceiling that bounds
        // worst-case cost.
        let hard_iteration_ceiling = max_iterations
            .saturating_mul(4)
            .max(max_iterations.saturating_add(100));
        let mut iteration_budget = max_iterations;
        let mut iteration_extensions = 0usize;
        let mut response_truncated = false;
        let mut max_iterations_reached = false;
        let mut tool_success_count = 0usize;
        let mut tool_failure_count = 0usize;
        let mut last_iteration_only_meta_tools = false;
        let mut meta_only_followup_attempts = 0usize;
        let mut recent_tool_intents: Vec<String> = Vec::new();
        let mut recent_tool_signatures: Vec<String> = Vec::new();
        let mut recent_tool_outcomes: Vec<(String, bool, String)> = Vec::new();
        // Rolling-window loop detector (generic repeats, ping-pong, poll with no
        // progress, global failure circuit breaker). Previously this module was
        // compiled but never instantiated, so only the narrow "two identical
        // calls in a row" guard below could fire.
        let mut tool_loop_detector = ToolLoopDetector::new(LoopDetectionConfig::default());

        // A fresh user prompt resets the advisory repeat-tool-call chain:
        // the loop the reminder was watching is over once a human steers.
        {
            let state = self
                .repeat_reminders
                .entry(session_id.to_string())
                .or_insert_with(|| {
                    std::sync::Mutex::new(crate::tools::repeat_reminder::RepeatReminderState::new())
                });
            let lock_result = state.lock();
            if let Ok(mut guard) = lock_result {
                guard.reset();
            }
        }
        let mut total_input_tokens: usize = 0;
        let mut total_output_tokens: usize = 0;
        let mut total_tokens: usize = 0;
        let mut total_cached_read_tokens: usize = 0;
        let mut total_cached_write_tokens: usize = 0;
        let mut cache_read_reported = false;
        let mut cache_write_reported = false;
        let mut cache_reasons: Vec<String> = Vec::new();
        let mut cache_reason_hint = "unknown".to_string();
        // `usage` on ResponseComplete is the final provider request. Keep the
        // aggregate separately so tool-heavy turns do not make the displayed
        // final-request cache rate look artificially low.
        let mut last_request_usage: Option<EventTokenUsage> = None;

        loop {
            iteration += 1;
            info!(
                "process_message: Iteration {} (budget {}, ceiling {})",
                iteration, iteration_budget, hard_iteration_ceiling
            );

            // Check for cancellation at the start of each iteration
            if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
                self.session_manager.update_session(&session).await?;
                warn!(
                    "Operation cancelled via flag for session {} at iteration {}",
                    session_id, iteration
                );
                self.event_bus.emit(AgentEvent::Cancelled {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    timestamp: SystemTime::now(),
                });
                return Err(OSAgentError::Session("Operation cancelled".to_string()));
            }
            let cancel_fut = self.wait_for_cancellation(session_id);
            tokio::select! {
                _ = cancel_fut => {
                    warn!("Operation cancelled for session {} at iteration {}", session_id, iteration);
                    self.session_manager.update_session(&session).await?;
                    self.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    return Err(OSAgentError::Session("Operation cancelled".to_string()));
                }
                _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {
                    // Continue processing
                }
            }

            if iteration > hard_iteration_ceiling {
                warn!(
                    "Hard iteration ceiling ({}) reached for session {}",
                    hard_iteration_ceiling, session_id
                );
                max_iterations_reached = true;
                break;
            }

            if iteration > iteration_budget {
                iteration_extensions += 1;
                iteration_budget = (iteration_budget + max_iterations).min(hard_iteration_ceiling);
                warn!(
                    "Iteration budget reached for session {} - extending to {} (extension {})",
                    session_id, iteration_budget, iteration_extensions
                );
                Self::push_session_event(
                    &mut session,
                    "iteration_budget_extended",
                    serde_json::json!({
                        "iteration": iteration,
                        "new_budget": iteration_budget,
                        "extensions": iteration_extensions,
                    }),
                )?;
                session.messages.push(Message::synthetic_user(
                    "You reached the per-turn tool budget. Keep going from where you left off: \
                     do not repeat completed steps, do not restate the plan, and only stop when \
                     the task is finished or you are genuinely blocked."
                        .to_string(),
                    "iteration_budget_nudge",
                ));
                self.session_manager.update_session(&session).await?;
                continue;
            }

            let active_persona = Self::active_persona_from_session(&session);
            let is_community = session
                .metadata
                .get("discord_community")
                .and_then(|value| value.as_bool())
                .unwrap_or(false);
            let tool_profile = if is_community {
                ToolProfile::Community
            } else {
                ToolProfile::from_persona_id(
                    active_persona.as_ref().map(|persona| persona.id.as_str()),
                )
            };
            let tools = self
                .tool_registry
                .get_tool_definitions_for_profile(tool_profile, session_id);
            info!(
                "process_message: Calling provider with {} tools (filtered from {} total)",
                tools.len(),
                self.tool_registry.get_tool_definitions(session_id).len()
            );
            debug!(
                "Selected {:?} profile tools for session {}: {}",
                tool_profile,
                session_id,
                tools
                    .iter()
                    .map(|tool| tool.function.name.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            );

            let is_roleplay = active_persona
                .as_ref()
                .map(|p| p.id == "custom")
                .unwrap_or(false);

            let mut api_messages = if is_community {
                let context = session
                    .metadata
                    .get("discord_community_context")
                    .and_then(|value| value.as_str())
                    .unwrap_or("You are a restricted community support assistant.");
                vec![Message::system(context.to_string())]
            } else if is_roleplay {
                vec![]
            } else {
                // Refresh dynamic prompt portions if the date has rolled over
                let today = chrono::Local::now().date_naive();
                let needs_refresh = {
                    let last_refresh = self.last_prompt_refresh.lock().unwrap();
                    *last_refresh != today
                };
                if needs_refresh {
                    let custom_id = self.config.read().await.agent.custom_identity.clone();
                    if let Ok(mut cache) = self.prompt_cache.write() {
                        cache.refresh_dynamic(custom_id.as_deref());
                    }
                    let mut last_refresh = self.last_prompt_refresh.lock().unwrap();
                    *last_refresh = today;
                }
                let prompt_text = {
                    let cache = self.prompt_cache.read().unwrap();
                    cache.prompt.clone()
                };
                vec![Message::system(prompt_text)]
            };

            // Voice mode is stored on the session (rather than passed per call)
            // so it survives reloads and applies to every surface, not just web.
            let voice_mode = session
                .metadata
                .get("voice_mode")
                .and_then(|v| v.as_bool())
                .unwrap_or(false);
            if voice_mode {
                api_messages.push(Message::system(prompt::build_voice_output_instructions()));
            }

            if !is_roleplay && !is_community {
                if let Some(goal) = self.goal_store.get(&session.id)? {
                    api_messages.push(Message::system(format!(
                        "# Current Goal\n{}\nGoal state is authoritative over older conversation notes. Only continue an active, armed goal. Complete it only when its objective is achieved and relevant checks support that conclusion; report remaining work honestly. Use get_goal/update_goal (load through tool_search if needed) to change state. A paused, blocked, or complete goal does not authorize more autonomous work.",
                        serde_json::to_string(&serde_json::json!({"goal": goal, "armed": self.goal_store.is_armed(&session.id)})).unwrap_or_default()
                    )));
                }
                match self.referenced_session_context(&session).await {
                    Ok(Some(context_block)) => api_messages.push(Message::system(context_block)),
                    Ok(None) => {}
                    Err(error) => warn!("Referenced chat context unavailable: {}", error),
                }
                match self.decision_memory.prompt_block().await {
                    Ok(Some(decision_block)) => api_messages.push(Message::system(decision_block)),
                    Ok(None) => {}
                    Err(error) => warn!("Decision memory unavailable for this turn: {}", error),
                }
                match self
                    .memory_store
                    .prompt_block(Some(&active_workspace.id), Some(&user_message))
                    .await
                {
                    Ok(Some(memory_block)) => api_messages.push(Message::system(memory_block)),
                    Ok(None) => {}
                    Err(error) => warn!("Memory unavailable for this turn: {}", error),
                }
            }

            if !is_community {
                if let Some(ref active_persona) = active_persona {
                    api_messages.push(Message::system(persona::build_persona_system_prompt(
                        active_persona,
                    )));
                }
            }
            api_messages.push(Message::system(format!(
                "# Tool Capability Profile\n- Profile: {:?}\n- The provider tool schemas are the authoritative available tools for this turn.",
                tool_profile
            )));

            if !is_roleplay && !is_community {
                let workspace_paths = active_workspace
                    .paths
                    .iter()
                    .map(|entry| {
                        format!(
                            "- {} ({})",
                            entry.path,
                            if entry.permission.allows_writes() {
                                "read-write"
                            } else {
                                "read-only"
                            }
                        )
                    })
                    .collect::<Vec<_>>()
                    .join("\n");
                api_messages.push(Message::system(format!(
                    "# Runtime Environment\n- Model: {} / {}\n- Workspace: {} ({})\n- Working directory: {}\n- Is directory a git repo: {}\n- Platform: {}\n- This workspace is fixed for the current turn; relative tool paths resolve from its primary path.\n- If an explicit absolute path outside these roots is necessary, request it with the relevant tool; OSA will ask the user for approval.\n# Workspace Roots\n{}",
                    context_provider_type,
                    context_model,
                    active_workspace.name,
                    active_workspace.id,
                    workspace_path,
                    if is_git_repo { "yes" } else { "no" },
                    std::env::consts::OS,
                    workspace_paths
                )));
                if let Some(git_context) = git_context.as_ref() {
                    api_messages.push(Message::system(git_context.clone()));
                }
                if let Some(reminder) = global_instruction_reminder.as_ref() {
                    api_messages.push(Message::system(reminder.clone()));
                }
                if let Some(reminder) = workspace_instruction_reminder.as_ref() {
                    api_messages.push(Message::system(reminder.clone()));
                }
                if let Some(skill_summary) = skill_summary_prompt.as_ref() {
                    api_messages.push(Message::system(skill_summary.clone()));
                }
                if let Some(manifest) = mcp_manifest_prompt.as_ref() {
                    api_messages.push(Message::system(manifest.clone()));
                }
                if let Some(manifest) = native_manifest_prompt.as_ref() {
                    api_messages.push(Message::system(manifest.clone()));
                }
                if runtime_config.compaction.notes_enabled {
                    api_messages.push(Message::system(WORKING_NOTES_INSTRUCTIONS.to_string()));
                    if let Some(notes) = session
                        .context_state
                        .as_ref()
                        .and_then(|state| state.working_notes.as_ref())
                        .map(|notes| notes.text.trim().to_string())
                        .filter(|text| !text.is_empty())
                    {
                        // A notes-first checkpoint already carries this exact
                        // text. Do not pay for or privilege a duplicate system
                        // copy until the rolling notes have changed again.
                        let checkpoint_has_notes = session.messages.iter().rev().any(|message| {
                            Self::message_kind(message) == Some("compaction_summary")
                                && message.content.contains(&notes)
                        });
                        if !checkpoint_has_notes {
                            api_messages.push(Message::system(format!(
                                "# Session Working Notes\n{}\n\nThese notes are your rolling handoff. Keep them current with `update_notes`; a fresh agent reads them first after compaction.",
                                notes
                            )));
                        }
                    }
                }
            }

            // Compaction markers accumulate in storage; forward at most the
            // newest one so prior summaries are not re-sent (and
            // re-summarized) on every turn, while other synthetic control
            // messages never reach the provider at all.
            api_messages.reserve(session.messages.len());
            for (index, msg) in session.messages.iter().enumerate() {
                if Self::is_superseded_summary(&session.messages, index) {
                    continue;
                }
                if Self::is_synthetic_message(msg)
                    && Self::message_kind(msg) != Some("compaction_summary")
                {
                    continue;
                }
                api_messages.push(msg.clone());
            }
            if response_truncated && !is_roleplay {
                api_messages.push(Message::user(
                    "Your previous response was cut off due to length. Continue exactly from where you left off. Do not repeat what you already said.".to_string(),
                ));
                response_truncated = false;
            }

            // Roleplay continuity enforcer
            if is_roleplay {
                api_messages.push(Message::user(
                    "Continue the scene. Stay in character. React naturally.".to_string(),
                ));
            }

            if repo_exploration_request && iteration == 1 && !is_roleplay {
                api_messages.push(Message::user(
                    "For codebase exploration, start broad, then narrow. Use list_files or glob to find likely paths, grep for symbols or keywords, then use read_file for focused file or directory reads with offset/limit paging. Stop once you have enough to answer.".to_string(),
                ));
            }

            let provider = self.active_provider().await;
            // Fall back to the configured window when the provider and the
            // catalog both come up empty: without a window the pressure
            // trigger below never runs and the session grows until the
            // provider hard-errors.
            let context_window = provider.model_context_window().await.or_else(|| {
                let fallback = runtime_config.compaction.fallback_context_window;
                if fallback > 0 {
                    Some(fallback)
                } else {
                    None
                }
            });
            if let Some(window) = context_window {
                let model_limit = self
                    .catalog
                    .lookup_model_limit(provider.provider_type(), &provider.current_model().await);
                let input_limit = model_limit.as_ref().and_then(|l| l.input);
                let output_limit = model_limit.as_ref().map(|l| l.output).unwrap_or(8192);

                // Message token usage is the provider usage for the request that
                // produced that message. It is cumulative across the session and
                // must not be used as the size of the current request context.
                // Use the current serialized messages for the preflight estimate;
                // provider-reported usage remains available separately below for
                // cumulative billing/usage totals.
                let estimated_pre_tokens: usize =
                    api_messages.iter().map(Self::message_tokens).sum();
                let tool_schema_tokens = serde_json::to_string(&tools)
                    .map(|schemas| schemas.chars().count().div_ceil(4))
                    .unwrap_or(0);
                let pre_tokens = estimated_pre_tokens.saturating_add(tool_schema_tokens);

                let tools_fingerprint = Self::fingerprint_tools(&tools);
                let previous_context = session.context_state.as_ref();
                if previous_context
                    .and_then(|state| state.actual_usage.as_ref())
                    .is_none()
                    && !session
                        .messages
                        .iter()
                        .any(|message| message.tokens.is_some())
                {
                    cache_reason_hint = "first_request".to_string();
                } else {
                    let mut changes = Vec::new();
                    if previous_context
                        .and_then(|state| state.cache_provider.as_deref())
                        .is_some_and(|previous| previous != context_provider_type)
                    {
                        changes.push("provider_changed");
                    }
                    if previous_context
                        .and_then(|state| state.cache_model.as_deref())
                        .is_some_and(|previous| previous != context_model)
                    {
                        changes.push("model_changed");
                    }
                    if previous_context
                        .and_then(|state| state.cache_tools_fingerprint)
                        .is_some_and(|previous| previous != tools_fingerprint)
                    {
                        changes.push("tools_changed");
                    }
                    cache_reason_hint = if changes.is_empty() {
                        "normal_miss".to_string()
                    } else {
                        changes.join("+")
                    };
                }

                let reserved_output = std::cmp::min(
                    output_limit,
                    runtime_config.compaction.reserved_output_tokens.max(1024),
                );
                let usable = input_limit
                    .unwrap_or(window.saturating_sub(reserved_output))
                    .saturating_sub(tool_schema_tokens);
                let threshold_ratio = if runtime_config.compaction.enabled {
                    runtime_config.compaction.threshold_ratio.clamp(0.5, 0.99)
                } else {
                    0.99
                };
                let budget = ((usable as f32) * threshold_ratio) as usize;

                if pre_tokens > budget {
                    self.emit_reasoning_event(
                        session_id,
                        format!(
                            "Context pressure detected ({} tokens over budget {}). Pruning or compacting earlier history.",
                            pre_tokens, budget
                        ),
                    );

                    let stored_conversation_tokens: usize =
                        session.messages.iter().map(Self::message_tokens).sum();
                    let conversation_budget = budget
                        .saturating_sub(pre_tokens.saturating_sub(stored_conversation_tokens));
                    if let Some((pruned, compacted, replayed)) = self
                        .compact_session_history(
                            &mut session,
                            &active_workspace,
                            &runtime_config.compaction,
                            None,
                            Some(conversation_budget),
                        )
                        .await?
                    {
                        if let Some(ref mut cs) = session.context_state {
                            cs.compaction_stats.total_compactions += 1;
                            cs.compaction_stats.total_pruned_messages += pruned;
                            cs.compaction_stats.total_compacted_messages += compacted;
                        }
                        // The pre-compact provider usage no longer describes
                        // the shrunken request: drop it so the next preflight
                        // reports the fresh estimate instead of stale occupancy.
                        last_request_usage = None;
                        self.record_session_event(
                            &mut session,
                            "compaction",
                            serde_json::json!({
                                "iteration": iteration,
                                "pruned_messages": pruned,
                                "compacted_messages": compacted,
                                "replayed": replayed,
                            }),
                        )?;
                        self.event_bus.emit(AgentEvent::Compaction {
                            session_id: session_id.to_string(),
                            sequence: 0,
                            pruned_messages: pruned,
                            compacted_messages: compacted,
                            replayed,
                            timestamp: SystemTime::now(),
                        });
                        self.session_manager.update_session(&session).await?;
                        continue;
                    }
                }

                let condensed_messages = Self::condense_messages(
                    &api_messages,
                    usable.saturating_sub(tool_schema_tokens),
                );
                let condensed = condensed_messages.len() != api_messages.len();
                if condensed {
                    cache_reason_hint = "compacted".to_string();
                    api_messages = condensed_messages;
                }
                let post_tokens: usize = api_messages
                    .iter()
                    .map(Self::message_tokens)
                    .sum::<usize>()
                    .saturating_add(tool_schema_tokens);

                let actual_usage = {
                    let mut total_input = 0usize;
                    let mut total_output = 0usize;
                    let mut total_total = 0usize;
                    let mut total_cached_read = 0usize;
                    let mut total_cached_write = 0usize;
                    let mut total_reasoning = 0usize;
                    for msg in session.messages.iter() {
                        if let Some(ref tokens) = msg.tokens {
                            total_input += tokens.input;
                            total_output += tokens.output;
                            total_total += tokens.total;
                            if let Some(cr) = tokens.cached_read {
                                total_cached_read += cr;
                            }
                            if let Some(cw) = tokens.cached_write {
                                total_cached_write += cw;
                            }
                            if let Some(r) = tokens.reasoning {
                                total_reasoning += r;
                            }
                        }
                    }
                    if total_total > 0 {
                        Some(EventTokenUsage {
                            input: total_input,
                            output: total_output,
                            total: total_total,
                            cached_read: if total_cached_read > 0 {
                                Some(total_cached_read)
                            } else {
                                None
                            },
                            cached_write: if total_cached_write > 0 {
                                Some(total_cached_write)
                            } else {
                                None
                            },
                            reasoning: if total_reasoning > 0 {
                                Some(total_reasoning)
                            } else {
                                None
                            },
                            cache_reason: None,
                        })
                    } else {
                        None
                    }
                };

                self.event_bus.emit(AgentEvent::ContextUpdate {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    context_window: window,
                    estimated_tokens: if condensed { post_tokens } else { pre_tokens },
                    budget_tokens: budget,
                    tool_schema_tokens,
                    condensed,
                    actual_usage: actual_usage.clone(),
                    last_request_usage: last_request_usage.clone(),
                    subagent_session_id: None,
                    timestamp: SystemTime::now(),
                });

                session.context_state = Some(crate::storage::models::SessionContextState {
                    estimated_tokens: if condensed { post_tokens } else { pre_tokens },
                    context_window: window,
                    budget_tokens: budget,
                    actual_usage: actual_usage.map(|u| crate::storage::models::MessageTokens {
                        input: u.input,
                        output: u.output,
                        total: u.total,
                        cached_read: u.cached_read,
                        cached_write: u.cached_write,
                        reasoning: u.reasoning,
                        cache_reason: None,
                    }),
                    working_notes: session
                        .context_state
                        .as_ref()
                        .and_then(|cs| cs.working_notes.clone()),
                    last_request_usage: last_request_usage.as_ref().map(|u| {
                        crate::storage::models::MessageTokens {
                            input: u.input,
                            output: u.output,
                            total: u.total,
                            cached_read: u.cached_read,
                            cached_write: u.cached_write,
                            reasoning: u.reasoning,
                            cache_reason: None,
                        }
                    }),
                    cache_provider: Some(context_provider_type.clone()),
                    cache_model: Some(context_model.clone()),
                    cache_tools_fingerprint: Some(tools_fingerprint),
                    tool_usage: session
                        .context_state
                        .as_ref()
                        .map(|cs| cs.tool_usage.clone())
                        .unwrap_or_default(),
                    compaction_stats: session
                        .context_state
                        .as_ref()
                        .map(|cs| cs.compaction_stats.clone())
                        .unwrap_or_default(),
                });
                self.session_manager.update_session(&session).await?;
            }

            let stream_attempt = tokio::select! {
                _ = self.wait_for_cancellation(session_id) => {
                    warn!("Operation cancelled for session {} during provider stream setup", session_id);
                    self.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    return Err(OSAgentError::Session("Operation cancelled".to_string()));
                }
                result = provider.complete_stream(Some(session_id), &api_messages, &tools) => result,
            };

            let (mut response, used_streaming) = match stream_attempt {
                Ok(stream) => {
                    let mut stream = stream;
                    let mut stream_retries: u32 = 0;
                    loop {
                        match self
                            .consume_provider_stream(
                                session_id,
                                &mut session,
                                stream,
                                cancel_notify.clone(),
                            )
                            .await
                        {
                            Ok(mut response) => {
                                response.retry_count =
                                    response.retry_count.saturating_add(stream_retries);
                                break (response, true);
                            }
                            Err(e)
                                if !e.is_context_limit()
                                    && Self::is_streaming_fallback_error(&e)
                                    && !(e.is_retryable() && stream_retries + 1 < MAX_RETRIES) =>
                            {
                                warn!(
                                    "Stream consumption failed with fallback-eligible error in session {}: {}. Falling back to non-streaming.",
                                    session_id, e
                                );
                                // Remove the empty assistant message pushed by consume_provider_stream
                                Self::reset_failed_stream_message(&mut session);
                                let response = tokio::select! {
                                    _ = self.wait_for_cancellation(session_id) => {
                                        warn!("Operation cancelled for session {} during provider fallback", session_id);
                                        self.event_bus.emit(AgentEvent::Cancelled {
                                            session_id: session_id.to_string(),
                                            sequence: 0,
                                            timestamp: SystemTime::now(),
                                        });
                                        return Err(OSAgentError::Session("Operation cancelled".to_string()));
                                    }
                                    result = provider.complete(Some(session_id), &api_messages, &tools) => {
                                        result.map_err(|e| {
                                            error!("Provider error in session {}: {}", session_id, e);
                                            self.event_bus.emit(AgentEvent::Error {
                                                session_id: session_id.to_string(),
                                                sequence: 0,
                                                error: e.user_facing_message(),
                                                recoverable: e.is_recoverable(),
                                                timestamp: SystemTime::now(),
                                            });
                                            e
                                        })?
                                    }
                                };
                                break (response, false);
                            }
                            // A transient failure while reading the stream body
                            // (truncated connection, undecodable SSE frame) is
                            // retried here rather than ending the turn. Only
                            // safe when nothing has been committed to the
                            // transcript yet, so a retry cannot duplicate text.
                            Err(e)
                                if !is_operation_cancelled(&e)
                                    && e.is_retryable()
                                    && stream_retries + 1 < MAX_RETRIES
                                    && Self::reset_failed_stream_message(&mut session) =>
                            {
                                stream_retries += 1;
                                let delay = OpenAICompatibleProvider::retry_delay_for_attempt(
                                    stream_retries,
                                    &e,
                                );
                                warn!(
                                    "Provider stream failed mid-response (attempt {}/{}): {}. Retrying in {}s...",
                                    stream_retries,
                                    MAX_RETRIES,
                                    e,
                                    delay.as_secs()
                                );
                                self.emit_provider_stream_retry(
                                    session_id,
                                    stream_retries,
                                    delay,
                                    &e.to_string(),
                                );
                                self.session_manager.update_session(&session).await?;
                                tokio::time::sleep(delay).await;
                                let retried = tokio::select! {
                                    _ = self.wait_for_cancellation(session_id) => {
                                        warn!("Operation cancelled for session {} during provider stream retry", session_id);
                                        self.event_bus.emit(AgentEvent::Cancelled {
                                            session_id: session_id.to_string(),
                                            sequence: 0,
                                            timestamp: SystemTime::now(),
                                        });
                                        return Err(OSAgentError::Session("Operation cancelled".to_string()));
                                    }
                                    result = provider.complete_stream(Some(session_id), &api_messages, &tools) => result,
                                };
                                match retried {
                                    Ok(next) => {
                                        stream = next;
                                        continue;
                                    }
                                    Err(setup_error) => {
                                        error!(
                                            "Provider stream retry failed in session {}: {}",
                                            session_id, setup_error
                                        );
                                        self.event_bus.emit(AgentEvent::Error {
                                            session_id: session_id.to_string(),
                                            sequence: 0,
                                            error: setup_error.user_facing_message(),
                                            recoverable: setup_error.is_recoverable(),
                                            timestamp: SystemTime::now(),
                                        });
                                        return Err(setup_error);
                                    }
                                }
                            }
                            Err(e) => {
                                // `consume_provider_stream` already emitted a
                                // `Cancelled` event when the stream was stopped by
                                // the user. Surface the cancellation as-is instead
                                // of re-classifying it as a provider `Error`
                                // ("Session error: Operation cancelled").
                                if is_operation_cancelled(&e) {
                                    return Err(e);
                                }
                                error!("Provider stream error in session {}: {}", session_id, e);
                                self.event_bus.emit(AgentEvent::Error {
                                    session_id: session_id.to_string(),
                                    sequence: 0,
                                    error: e.user_facing_message(),
                                    recoverable: e.is_recoverable(),
                                    timestamp: SystemTime::now(),
                                });
                                return Err(e);
                            }
                        }
                    }
                }
                Err(error) if Self::is_streaming_fallback_error(&error) => {
                    let response = tokio::select! {
                        _ = self.wait_for_cancellation(session_id) => {
                            warn!("Operation cancelled for session {} during provider call", session_id);
                            self.event_bus.emit(AgentEvent::Cancelled {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                timestamp: SystemTime::now(),
                            });
                            return Err(OSAgentError::Session("Operation cancelled".to_string()));
                        }
                        result = provider.complete(Some(session_id), &api_messages, &tools) => {
                            result.map_err(|e| {
                                error!("Provider error in session {}: {}", session_id, e);
                                self.event_bus.emit(AgentEvent::Error {
                                    session_id: session_id.to_string(),
                                    sequence: 0,
                                    error: e.user_facing_message(),
                                    recoverable: e.is_recoverable(),
                                    timestamp: SystemTime::now(),
                                });
                                e
                            })?
                        }
                    };
                    (response, false)
                }
                Err(error) => {
                    error!(
                        "Provider stream setup error in session {}: {}",
                        session_id, error
                    );
                    // A hard context-length rejection is recoverable by
                    // compaction: shrink history once and retry the request
                    // instead of failing the turn.
                    if error.is_context_limit() {
                        if let Some(recovered) = self
                            .compact_after_context_error(
                                session_id,
                                &mut session,
                                &active_workspace,
                                &runtime_config,
                                iteration,
                                &error,
                            )
                            .await?
                        {
                            if recovered {
                                continue;
                            }
                        }
                    }
                    self.event_bus.emit(AgentEvent::Error {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        error: error.user_facing_message(),
                        recoverable: error.is_recoverable(),
                        timestamp: SystemTime::now(),
                    });
                    return Err(error);
                }
            };

            if used_streaming
                && response.content.is_none()
                && response.tool_calls.is_none()
                && response.finish_reason != "length"
            {
                warn!(
                    "Streaming provider response for session {} had no content or tool calls; falling back to non-stream parsing",
                    session_id
                );

                let fallback = tokio::select! {
                    _ = self.wait_for_cancellation(session_id) => {
                        warn!("Operation cancelled for session {} during provider fallback call", session_id);
                        self.event_bus.emit(AgentEvent::Cancelled {
                            session_id: session_id.to_string(),
                            sequence: 0,
                            timestamp: SystemTime::now(),
                        });
                        return Err(OSAgentError::Session("Operation cancelled".to_string()));
                    }
                    result = provider.complete(Some(session_id), &api_messages, &tools) => {
                        result.map_err(|e| {
                            error!("Provider fallback error in session {}: {}", session_id, e);
                            self.event_bus.emit(AgentEvent::Error {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                error: e.user_facing_message(),
                                recoverable: e.is_recoverable(),
                                timestamp: SystemTime::now(),
                            });
                            e
                        })?
                    }
                };

                if let Some(last_message) = session.messages.last_mut() {
                    if last_message.role == "assistant" {
                        let had_thinking = last_message
                            .thinking
                            .as_deref()
                            .map(|value| !value.trim().is_empty())
                            .unwrap_or(false);
                        let had_content = !last_message.content.trim().is_empty();

                        if !had_thinking {
                            last_message.thinking = fallback.thinking.clone();
                        }
                        if !had_content {
                            last_message.content = fallback.content.clone().unwrap_or_default();
                        }
                        last_message.tool_calls = fallback.tool_calls.clone();
                        if let Some(ref usage) = fallback.usage {
                            last_message.tokens = Some(MessageTokens {
                                input: usage.input,
                                output: usage.output,
                                total: usage.total,
                                cached_read: usage.cached_read,
                                cached_write: usage.cached_write,
                                reasoning: usage.reasoning,
                                cache_reason: None,
                            });
                        }

                        self.session_manager.update_session(&session).await?;

                        if !had_thinking {
                            if let Some(thinking) = fallback.thinking.as_deref() {
                                self.emit_thinking_chunks(session_id, thinking);
                            }
                        }
                        let fallback_has_tool_calls = fallback
                            .tool_calls
                            .as_ref()
                            .map(|calls| !calls.is_empty())
                            .unwrap_or(false);
                        if !had_content && !fallback_has_tool_calls {
                            if let Some(_content) = fallback.content.as_deref() {
                                // Don't emit ResponseStart/ResponseChunk here.
                                // The non-streaming path below (lines 1505-1514)
                                // handles event emission uniformly to avoid
                                // duplicating events already sent by consume_provider_stream.
                            }
                        }
                    }
                }

                response = fallback;
            }

            info!(
                "process_message: Provider response received - content={}, tool_calls={:?}",
                response.content.as_ref().map(|c| c.len()).unwrap_or(0),
                response.tool_calls.as_ref().map(|t| t.len())
            );

            let response_cache_reason = response
                .usage
                .as_ref()
                .map(|usage| {
                    if usage.cached_read.is_some_and(|cached| cached > 0) {
                        "hit".to_string()
                    } else if response.context_compressed || cache_reason_hint == "compacted" {
                        "compacted".to_string()
                    } else if usage.cached_read.is_some() {
                        cache_reason_hint.clone()
                    } else {
                        "unreported".to_string()
                    }
                })
                .unwrap_or_else(|| "unreported".to_string());
            cache_reasons.push(response_cache_reason.clone());
            last_request_usage = response.usage.as_ref().map(|usage| EventTokenUsage {
                input: usage.input,
                output: usage.output,
                total: usage.total,
                cached_read: usage.cached_read,
                cached_write: usage.cached_write,
                reasoning: usage.reasoning,
                cache_reason: Some(response_cache_reason.clone()),
            });
            info!(
                "Session {} cache result: {} (input={}, cached_read={:?})",
                session_id,
                response_cache_reason,
                response
                    .usage
                    .as_ref()
                    .map(|usage| usage.input)
                    .unwrap_or(0),
                response.usage.as_ref().and_then(|usage| usage.cached_read)
            );

            if response.retry_count > 0 && response.context_compressed {
                self.record_session_event(
                    &mut session,
                    "retry",
                    serde_json::json!({
                        "scope": "provider",
                        "attempt_count": response.retry_count,
                        "context_compressed": response.context_compressed,
                    }),
                )?;
                self.event_bus.emit(AgentEvent::Retry {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    scope: "provider".to_string(),
                    attempt_count: response.retry_count,
                    max_attempts: MAX_RETRIES,
                    next_retry_in_ms: 0,
                    subagent_session_id: None,
                    reason: "provider retried after compressing request context".to_string(),
                    timestamp: SystemTime::now(),
                });
            } else if response.retry_count > 0 {
                self.record_session_event(
                    &mut session,
                    "retry",
                    serde_json::json!({
                        "scope": "provider",
                        "attempt_count": response.retry_count,
                        "context_compressed": response.context_compressed,
                    }),
                )?;
            }

            if response.context_compressed {
                if let Some(summary) = response.compressed_summary.clone() {
                    let provider_compaction = Self::plan_provider_compaction(&session, &summary);
                    if let Some((keep_head, keep_tail, dropped, framed)) = provider_compaction {
                        if let Err(error) = self.storage.archive_messages(&session.id, &dropped) {
                            warn!(
                                "Failed to archive provider-compacted messages for {}: {}; keeping full history",
                                session_id, error
                            );
                        } else {
                            let original_len = session.messages.len();
                            let tail = session.messages[original_len - keep_tail..].to_vec();
                            let mut compacted = session.messages[..keep_head].to_vec();
                            compacted.push(Message::synthetic_assistant(
                                framed.clone(),
                                "compaction_summary",
                            ));
                            compacted.extend(tail);
                            let compacted_count = original_len - compacted.len();
                            session.messages = compacted;
                            info!(
                            "Persisting provider-side context compression for session {}: replaced {} messages with summary",
                            session_id, compacted_count
                        );
                            if let Some(ref mut cs) = session.context_state {
                                cs.compaction_stats.total_compactions += 1;
                                cs.compaction_stats.total_compacted_messages += compacted_count;
                                cs.compaction_stats.estimated_tokens_saved += dropped
                                    .iter()
                                    .map(Self::message_tokens)
                                    .sum::<usize>()
                                    .saturating_sub(Self::estimate_tokens(&framed));
                            }
                            self.record_session_event(
                                &mut session,
                                "compaction",
                                serde_json::json!({
                                    "iteration": iteration,
                                    "pruned_messages": 0,
                                    "compacted_messages": compacted_count,
                                    "replayed": false,
                                    "source": "provider_fallback",
                                }),
                            )?;
                            self.event_bus.emit(AgentEvent::Compaction {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                pruned_messages: 0,
                                compacted_messages: compacted_count,
                                replayed: false,
                                timestamp: SystemTime::now(),
                            });
                            self.session_manager.update_session(&session).await?;
                        }
                    }
                }
            }

            let has_tool_calls = response
                .tool_calls
                .as_ref()
                .map(|calls| !calls.is_empty())
                .unwrap_or(false);

            if used_streaming {
                if let Some(last_message) = session.messages.last_mut() {
                    last_message.tool_calls = response.tool_calls.clone();
                    if has_tool_calls {
                        // Keep prelude visible (opencode-style parts): the
                        // transcript folds it into the tool card, but the
                        // history must retain it or the next turn loses the
                        // model's own stated intent to continue editing.
                        last_message.metadata = serde_json::json!({
                            "kind": "tool_prelude",
                        });
                    }
                    if let Some(ref usage) = response.usage {
                        last_message.tokens = Some(MessageTokens {
                            input: usage.input,
                            output: usage.output,
                            total: usage.total,
                            cached_read: usage.cached_read,
                            cached_write: usage.cached_write,
                            reasoning: usage.reasoning,
                            cache_reason: Some(response_cache_reason.clone()),
                        });
                        total_input_tokens += usage.input;
                        total_output_tokens += usage.output;
                        total_tokens += usage.total;
                        if let Some(cached_read) = usage.cached_read {
                            total_cached_read_tokens += cached_read;
                            cache_read_reported = true;
                        }
                        if let Some(cached_write) = usage.cached_write {
                            total_cached_write_tokens += cached_write;
                            cache_write_reported = true;
                        }
                    }
                }
                self.session_manager.update_session(&session).await?;
            } else {
                let mut assistant_message = Message::assistant(
                    response.content.clone().unwrap_or_default(),
                    response.tool_calls.clone(),
                );
                if has_tool_calls {
                    assistant_message.metadata = serde_json::json!({
                        "kind": "tool_prelude",
                    });
                }
                assistant_message.thinking = response.thinking.clone();
                if let Some(ref usage) = response.usage {
                    assistant_message.tokens = Some(MessageTokens {
                        input: usage.input,
                        output: usage.output,
                        total: usage.total,
                        cached_read: usage.cached_read,
                        cached_write: usage.cached_write,
                        reasoning: usage.reasoning,
                        cache_reason: Some(response_cache_reason.clone()),
                    });
                    total_input_tokens += usage.input;
                    total_output_tokens += usage.output;
                    total_tokens += usage.total;
                    if let Some(cached_read) = usage.cached_read {
                        total_cached_read_tokens += cached_read;
                        cache_read_reported = true;
                    }
                    if let Some(cached_write) = usage.cached_write {
                        total_cached_write_tokens += cached_write;
                        cache_write_reported = true;
                    }
                }
                session.messages.push(assistant_message);
                self.session_manager.update_session(&session).await?;

                if let Some(thinking) = response.thinking.as_deref() {
                    self.emit_thinking_chunks(session_id, thinking);
                }

                if !has_tool_calls {
                    self.event_bus.emit(AgentEvent::ResponseStart {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });

                    if let Some(content) = response.content.as_ref() {
                        self.emit_response_chunks(session_id, content);
                    }
                }
            }

            if let Some(tool_calls) = response.tool_calls {
                info!(
                    "process_message: Processing {} tool calls",
                    tool_calls.len()
                );
                let mut loop_guard_triggered = false;

                last_iteration_only_meta_tools = tool_calls.iter().all(|tool_call| {
                    matches!(tool_call.name.as_str(), "task" | "reflect" | "persona")
                });
                if !last_iteration_only_meta_tools {
                    meta_only_followup_attempts = 0;
                }

                let all_parallel_safe = tool_calls.iter().all(|tc| {
                    self.tool_registry.is_parallel_safe(&tc.name)
                        && Self::requested_absolute_path(
                            &tc.name,
                            &tc.arguments,
                            Some(Path::new(&workspace_path)),
                        )
                        .is_none()
                        && !matches!(
                            tc.name.as_str(),
                            "batch" | "persona" | "question" | "subagent"
                        )
                });

                if all_parallel_safe && tool_calls.len() > 1 {
                    info!(
                        "process_message: Executing {} tools in parallel",
                        tool_calls.len()
                    );
                    let message_index = session.messages.len().saturating_sub(1) as i32;
                    let parallel_results = match self
                        .execute_parallel_tool_calls(
                            session_id,
                            &session,
                            &active_workspace,
                            &tool_calls,
                            &runtime_config,
                            &mut recent_tool_signatures,
                            &mut recent_tool_intents,
                            &mut recent_tool_outcomes,
                            &mut tool_success_count,
                            &mut tool_failure_count,
                            &mut loop_guard_triggered,
                            iteration,
                            &user,
                            message_index,
                        )
                        .await
                    {
                        Ok(results) => results,
                        Err(e) => {
                            // The assistant message carrying these calls is
                            // already persisted, so leaving them without results
                            // sends the next request a `function_call` with no
                            // `function_call_output` — which the API rejects
                            // outright ("No tool output found for function
                            // call ...") and which leaves the session unable to
                            // continue at all. Record the cancellation for
                            // every call, exactly as the sequential path does.
                            for tool_call in &tool_calls {
                                session.messages.push(Message::tool_result(
                                    tool_call.id.clone(),
                                    format!("Tool: {}\nError: Cancelled by user", tool_call.name),
                                ));
                            }
                            self.session_manager.update_session(&session).await?;
                            return Err(e);
                        }
                    };

                    for (tool_call, result) in tool_calls.iter().zip(parallel_results) {
                        let (
                            mut tool_result,
                            success,
                            duration_ms,
                            tool_sig,
                            tool_intent,
                            error_detail,
                        ) = result;
                        if success {
                            crate::tools::spill::maybe_spill_tool_result(
                                &self.spill_store,
                                &session.id,
                                &tool_call.name,
                                &mut tool_result,
                                &runtime_config.spill,
                            );
                        }
                        let output = tool_result.output.clone();
                        if success {
                            tool_success_count += 1;
                        } else {
                            tool_failure_count += 1;
                        }

                        recent_tool_signatures.push(tool_sig.clone());
                        if recent_tool_signatures.len() > 8 {
                            recent_tool_signatures.remove(0);
                        }
                        if let Some(intent) = tool_intent {
                            recent_tool_intents.push(intent);
                            if recent_tool_intents.len() > 8 {
                                recent_tool_intents.remove(0);
                            }
                        }

                        if self.check_tool_loop(
                            &mut tool_loop_detector,
                            &mut session,
                            &tool_call.name,
                            &tool_call.arguments,
                            success,
                        )? {
                            loop_guard_triggered = true;
                        }

                        let context_window_tokens =
                            std::cmp::max(runtime_config.agent.max_tokens * 4, 131_072);
                        let truncated_output = truncation::maybe_truncate_tool_result(
                            &output,
                            context_window_tokens,
                            &TruncationOptions::default(),
                        );

                        let first_non_empty_line = truncated_output
                            .lines()
                            .map(|line| line.trim())
                            .find(|line| !line.is_empty())
                            .unwrap_or("(no output)")
                            .to_string();
                        recent_tool_outcomes.push((
                            tool_call.name.clone(),
                            success,
                            first_non_empty_line,
                        ));
                        if recent_tool_outcomes.len() > 5 {
                            recent_tool_outcomes.remove(0);
                        }

                        self.event_bus.emit(AgentEvent::ToolComplete {
                            session_id: session_id.to_string(),
                            sequence: 0,
                            tool_call_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            success,
                            output: truncated_output.clone(),
                            title: tool_result.title.clone(),
                            metadata: Self::non_empty_tool_metadata(&tool_result.metadata),
                            duration_ms,
                            timestamp: SystemTime::now(),
                        });

                        if runtime_config.logging.audit_enabled {
                            let audit_entry = AuditEntry {
                                id: Uuid::new_v4().to_string(),
                                timestamp: chrono::Utc::now(),
                                session_id: session.id.clone(),
                                tool: tool_call.name.clone(),
                                input: tool_call.arguments.to_string(),
                                output: output.clone(),
                                duration_ms,
                                user: user.clone(),
                            };
                            self.storage.log_audit(audit_entry)?;
                        }

                        let tool_message = Self::summarize_tool_output_for_context(
                            &tool_call.name,
                            tool_result.outcome,
                            &output,
                        );
                        let mut tool_meta = serde_json::json!({
                            "tool_name": tool_call.name,
                            "success": success,
                            "tool_result": {
                                "title": tool_result.title,
                                "metadata": tool_result.metadata,
                            }
                        });
                        if !success {
                            if let Some(err) = &error_detail {
                                tool_meta["error_message"] = serde_json::json!(err);
                            }
                        }
                        session.messages.push(Message::tool_result_with_metadata(
                            tool_call.id.clone(),
                            tool_message,
                            tool_meta,
                        ));
                    }
                } else {
                    for (idx, tool_call) in tool_calls.iter().enumerate() {
                        info!(
                            "process_message: Tool call {} - {}",
                            idx + 1,
                            tool_call.name
                        );
                        if tool_call.name != "batch" {
                            let message_index = session.messages.len().saturating_sub(1) as i32;
                            self.event_bus.emit(AgentEvent::ToolStart {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                tool_call_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                arguments: tool_call.arguments.clone(),
                                message_index,
                                timestamp: SystemTime::now(),
                            });
                        }

                        if !self.tool_registry.is_allowed(&tool_call.name) {
                            let error_msg =
                                format!("Tool '{}' is not in allowlist", tool_call.name);
                            warn!("{}", error_msg);

                            self.event_bus.emit(AgentEvent::ToolComplete {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                tool_call_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                success: false,
                                output: error_msg.clone(),
                                title: None,
                                metadata: None,
                                duration_ms: 0,
                                timestamp: SystemTime::now(),
                            });

                            let tool_message =
                                format!("Tool: {}\nError: {}", tool_call.name, error_msg);
                            session.messages.push(Message::tool_result_with_metadata(
                                tool_call.id.clone(),
                                tool_message,
                                serde_json::json!({
                                    "tool_name": tool_call.name,
                                    "success": false,
                                    "error_message": error_msg,
                                }),
                            ));
                            continue;
                        }

                        let tool_signature =
                            Self::tool_call_signature(&tool_call.name, &tool_call.arguments);
                        let tool_intent =
                            Self::tool_intent_signature(&tool_call.name, &tool_call.arguments);
                        let repeat_count = Self::consecutive_repeat_count(
                            &recent_tool_signatures,
                            &tool_signature,
                        );
                        let intent_repeat_count = tool_intent
                            .as_ref()
                            .map(|intent| {
                                Self::consecutive_repeat_count(&recent_tool_intents, intent)
                            })
                            .unwrap_or(0);
                        // The consecutive guard below misses a rotating set of
                        // near-identical attempts (for example cycling through
                        // probe scripts) that never repeats twice in a row.
                        // Flag a repeated intent anywhere in the recent window
                        // as an advisory nudge instead of a hard block.
                        let intent_window_count = tool_intent
                            .as_ref()
                            .map(|intent| {
                                recent_tool_intents
                                    .iter()
                                    .filter(|previous| *previous == intent)
                                    .count()
                            })
                            .unwrap_or(0);
                        if intent_window_count >= 2 {
                            let loop_msg = format!(
                                "Loop guard: '{}' has repeated the same intent {} times in the last {} calls. {}",
                                tool_call.name,
                                intent_window_count + 1,
                                recent_tool_intents.len() + 1,
                                Self::tool_loop_guidance(&tool_call.name)
                            );
                            warn!(
                                "Advisory repeated-intent loop in session {}: {}",
                                session_id, loop_msg
                            );
                            self.record_session_event(
                                &mut session,
                                "loop_detected",
                                serde_json::json!({
                                    "detector": "RepeatedIntent",
                                    "level": "warning",
                                    "count": intent_window_count + 1,
                                    "tool_name": tool_call.name,
                                    "message": &loop_msg,
                                }),
                            )?;
                            self.event_bus.emit(AgentEvent::LoopDetected {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                detector: "RepeatedIntent".to_string(),
                                level: "warning".to_string(),
                                count: intent_window_count + 1,
                                message: loop_msg,
                                timestamp: SystemTime::now(),
                            });
                            loop_guard_triggered = true;
                        }
                        if repeat_count >= 2 || intent_repeat_count >= 2 {
                            let loop_msg = format!(
                                "Loop guard: blocked repeated {} tool call '{}' after {} consecutive attempts. {}",
                                if repeat_count >= 2 { "identical" } else { "similar" },
                                tool_call.name,
                                repeat_count.max(intent_repeat_count) + 1,
                                Self::tool_loop_guidance(&tool_call.name)
                            );
                            warn!(
                                "Blocking repeated identical tool call in session {}: {}",
                                session_id, tool_signature
                            );

                            tool_failure_count += 1;
                            self.record_session_event(
                                &mut session,
                                "reasoning",
                                serde_json::json!({
                                    "summary": loop_msg,
                                    "iteration": iteration,
                                }),
                            )?;
                            self.emit_reasoning_event(session_id, loop_msg.clone());
                            recent_tool_outcomes.push((
                                tool_call.name.clone(),
                                false,
                                "Loop guard blocked repeated identical call".to_string(),
                            ));
                            if recent_tool_outcomes.len() > 5 {
                                recent_tool_outcomes.remove(0);
                            }

                            self.event_bus.emit(AgentEvent::ToolComplete {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                tool_call_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                success: false,
                                output: loop_msg.clone(),
                                title: None,
                                metadata: None,
                                duration_ms: 0,
                                timestamp: SystemTime::now(),
                            });

                            session.messages.push(Message::tool_result_with_metadata(
                                tool_call.id.clone(),
                                format!("Tool: {}\nError: {}", tool_call.name, loop_msg),
                                serde_json::json!({
                                    "tool_name": tool_call.name,
                                    "success": false,
                                    "error_message": loop_msg,
                                }),
                            ));
                            loop_guard_triggered = true;
                            continue;
                        }

                        let repeat_reminder = {
                            let state = self
                                .repeat_reminders
                                .entry(session_id.to_string())
                                .or_insert_with(|| {
                                    std::sync::Mutex::new(
                                        crate::tools::repeat_reminder::RepeatReminderState::new(),
                                    )
                                });
                            state.lock().ok().and_then(|mut guard| {
                                guard.note_tool_call(
                                    &tool_call.name,
                                    &tool_call.arguments,
                                    &runtime_config.tools.repeat_reminder,
                                )
                            })
                        };

                        let start = Instant::now();

                        // Emit tool progress - preparing
                        self.event_bus.emit(AgentEvent::ToolProgress {
                            session_id: session_id.to_string(),
                            sequence: 0,
                            tool_call_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            status: ToolStatus::Preparing,
                            message: Some("Preparing execution...".to_string()),
                            progress_percent: Some(10),
                            timestamp: SystemTime::now(),
                        });

                        // Emit tool progress - executing
                        self.event_bus.emit(AgentEvent::ToolProgress {
                            session_id: session_id.to_string(),
                            sequence: 0,
                            tool_call_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            status: ToolStatus::Executing,
                            message: Some("Executing...".to_string()),
                            progress_percent: Some(50),
                            timestamp: SystemTime::now(),
                        });

                        info!("process_message: Executing tool {}", tool_call.name);

                        // Check for cancellation before tool execution
                        let cancel_fut = self.wait_for_cancellation(session_id);
                        tokio::select! {
                            _ = cancel_fut => {
                                warn!("Operation cancelled before tool execution for session {}", session_id);
                                self.event_bus.emit(AgentEvent::ToolComplete {
                                    session_id: session_id.to_string(),
                                    sequence: 0,
                                    tool_call_id: tool_call.id.clone(),
                                    tool_name: tool_call.name.clone(),
                                    success: false,
                                    output: "Cancelled by user".to_string(),
                                    title: None,
                                    metadata: None,
                                    duration_ms: 0,
                                    timestamp: SystemTime::now(),
                                });
                                session.messages.push(Message::tool_result(
                                    tool_call.id.clone(),
                                    format!("Tool: {}\nError: Cancelled by user", tool_call.name),
                                ));
                                self.event_bus.emit(AgentEvent::Cancelled {
                                    session_id: session_id.to_string(),
                                    sequence: 0,
                                    timestamp: SystemTime::now(),
                                });
                                return Err(OSAgentError::Session("Operation cancelled".to_string()));
                            }
                            _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {
                                // Continue with tool execution
                            }
                        }

                        // Tool schemas are not a sufficient security boundary:
                        // providers can emit calls for tools they were not shown.
                        // Deny them with a recoverable tool error instead of
                        // killing the whole turn: a dead turn on a public
                        // channel looks like the bot dying, and the model
                        // just retries the same call next turn.
                        if !tool_profile.allows(&tool_call.name) {
                            let denial = format!(
                                "Tool '{}' is not available in this chat. Use only the tools from your tool list.",
                                tool_call.name
                            );
                            warn!(
                                "Profile {:?} denied tool '{}' for session {}",
                                tool_profile, tool_call.name, session_id
                            );
                            self.event_bus.emit(AgentEvent::ToolComplete {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                tool_call_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                success: false,
                                output: denial.clone(),
                                title: None,
                                metadata: None,
                                duration_ms: 0,
                                timestamp: SystemTime::now(),
                            });
                            session.messages.push(Message::tool_result_with_metadata(
                                tool_call.id.clone(),
                                format!("Tool: {}\nError: {}", tool_call.name, denial),
                                serde_json::json!({
                                    "tool_name": tool_call.name,
                                    "success": false,
                                    "error_message": denial,
                                }),
                            ));
                            continue;
                        }

                        let snapshot_id =
                            if tool_call.name != "persona" && tool_call.name != "batch" {
                                self.capture_file_snapshots(
                                    &session.id,
                                    &active_workspace,
                                    &tool_call.name,
                                    &tool_call.arguments,
                                )?
                            } else {
                                None
                            };
                        let execution = async {
                            if tool_call.name == "persona" {
                                self.handle_persona_tool_call(&mut session, &tool_call.arguments)
                                    .map(ToolResult::new)
                            } else if tool_call.name == "batch" {
                                let batch_message_index =
                                    session.messages.len().saturating_sub(1) as i32;
                                self.handle_batch_tool_call(
                                    &mut session,
                                    &active_workspace,
                                    &tool_call.arguments,
                                    batch_message_index,
                                    &tool_profile,
                                )
                                .await
                                .map(ToolResult::new)
                            } else if tool_call.name == "tool_script" {
                                self.handle_tool_script_call(
                                    &session.id,
                                    &active_workspace,
                                    &tool_call.arguments,
                                )
                                .await
                            } else {
                                let mut tool_args = tool_call.arguments.clone();
                                // Every tool call carries the session id so
                                // session-scoped machinery (tool_search
                                // activation, MCP auto-activation, todos,
                                // goals, questions) can find its session.
                                tool_args["session_id"] = serde_json::json!(session_id);
                                tool_args["workspace_id"] = serde_json::json!(active_workspace.id);
                                // Reading other conversations through the
                                // `sessions` tool is gated by the
                                // `session_access` policy (popup, rules) before
                                // anything executes.
                                let session_access = if tool_call.name == "sessions" {
                                    self.authorize_session_access(
                                        session_id,
                                        &tool_call.id,
                                        &tool_args,
                                    )
                                    .await
                                } else {
                                    Ok(())
                                };
                                let external_paths = match session_access {
                                    Ok(()) => {
                                        self.authorize_external_paths(
                                            session_id,
                                            &tool_call.name,
                                            &tool_call.id,
                                            &tool_args,
                                            &active_workspace,
                                        )
                                        .await
                                    }
                                    Err(error) => Err(error),
                                };
                                match external_paths {
                                    Ok((workspace_additions, resolved_path)) => {
                                        if let Some(resolved) = resolved_path {
                                            let path_key = match tool_call.name.as_str() {
                                                "read_file" => Some("filePath"),
                                                "write_file" | "edit_file" | "delete_file"
                                                | "list_files" | "grep" | "glob" => Some("path"),
                                                "bash" | "process" => Some("workdir"),
                                                _ => None,
                                            };
                                            if let Some(key) = path_key {
                                                tool_args[key] =
                                                    serde_json::Value::String(resolved);
                                            }
                                        }
                                        self.tool_registry
                                            .execute_in_workspace_with_external_result(
                                                &tool_call.name,
                                                tool_args,
                                                Some(workspace_path.clone()),
                                                &workspace_additions,
                                            )
                                            .await
                                    }
                                    Err(error) => Err(error),
                                }
                            }
                        };
                        let result: Result<ToolResult> = tokio::select! {
                            biased;
                            _ = self.wait_for_cancellation(session_id) => Err(OSAgentError::Session("Operation cancelled".to_string())),
                            result = execution => result,
                        };

                        let duration_ms = start.elapsed().as_millis() as u64;
                        info!(
                            "process_message: Tool {} completed in {}ms",
                            tool_call.name, duration_ms
                        );

                        // Emit tool progress - finalizing
                        self.event_bus.emit(AgentEvent::ToolProgress {
                            session_id: session_id.to_string(),
                            sequence: 0,
                            tool_call_id: tool_call.id.clone(),
                            tool_name: tool_call.name.clone(),
                            status: ToolStatus::Finalizing,
                            message: Some("Finalizing...".to_string()),
                            progress_percent: Some(90),
                            timestamp: SystemTime::now(),
                        });

                        let (mut tool_result, audit_output, success, error_detail) = match result {
                            Ok(tool_result) => {
                                let success = tool_result.outcome.is_success();
                                info!(
                                    "Tool {} completed with outcome {} in {}ms",
                                    tool_call.name,
                                    tool_result.outcome.as_str(),
                                    duration_ms
                                );
                                (
                                    tool_result.clone(),
                                    tool_result.output.clone(),
                                    success,
                                    None,
                                )
                            }
                            Err(e) => {
                                let error_msg = format!("Error: {}", e);
                                error!("Tool {} failed: {}", tool_call.name, error_msg);
                                (
                                    ToolResult::failure(error_msg.clone()),
                                    error_msg,
                                    false,
                                    Some(e.to_string()),
                                )
                            }
                        };

                        if success
                            && ["write_file", "edit_file", "apply_patch"]
                                .contains(&tool_call.name.as_str())
                        {
                            if let Some(snapshot_id) = snapshot_id.as_deref() {
                                if let Ok(file_diff_meta) = self.build_file_diff_metadata(
                                    &session.id,
                                    snapshot_id,
                                    &active_workspace,
                                ) {
                                    if let Some(base) = tool_result.metadata.as_object_mut() {
                                        if let Some(extra) = file_diff_meta.as_object() {
                                            for (key, value) in extra {
                                                base.insert(key.clone(), value.clone());
                                            }
                                        }
                                    }
                                }
                            }
                        }

                        // Oversized plain-text results are spilled to a
                        // session-scoped file and replaced with a bounded
                        // preview, so the truncation below never has to
                        // silently drop content.
                        if success {
                            crate::tools::spill::maybe_spill_tool_result(
                                &self.spill_store,
                                &session.id,
                                &tool_call.name,
                                &mut tool_result,
                                &runtime_config.spill,
                            );
                        }

                        let output = tool_result.output.clone();

                        let context_window_tokens =
                            std::cmp::max(runtime_config.agent.max_tokens * 4, 131_072);
                        let truncated_output = truncation::maybe_truncate_tool_result(
                            &output,
                            context_window_tokens,
                            &TruncationOptions::default(),
                        );

                        if success {
                            tool_success_count += 1;
                            if let Some(snapshot_id) = snapshot_id.clone() {
                                self.record_session_event(
                                    &mut session,
                                    "snapshot",
                                    serde_json::json!({
                                        "snapshot_id": snapshot_id,
                                        "tool_name": tool_call.name,
                                    }),
                                )?;
                            }
                        } else {
                            tool_failure_count += 1;
                        }

                        self.record_session_event(
                            &mut session,
                            "tool",
                            serde_json::json!({
                                "tool_name": tool_call.name.clone(),
                                "success": success,
                                "duration_ms": duration_ms,
                                "tool_call_id": tool_call.id.clone(),
                                "error": error_detail.clone(),
                            }),
                        )?;

                        recent_tool_signatures.push(tool_signature);
                        if recent_tool_signatures.len() > 8 {
                            recent_tool_signatures.remove(0);
                        }
                        if let Some(tool_intent) = tool_intent {
                            recent_tool_intents.push(tool_intent);
                            if recent_tool_intents.len() > 8 {
                                recent_tool_intents.remove(0);
                            }
                        }

                        if self.check_tool_loop(
                            &mut tool_loop_detector,
                            &mut session,
                            &tool_call.name,
                            &tool_call.arguments,
                            success,
                        )? {
                            loop_guard_triggered = true;
                        }

                        let first_non_empty_line = truncated_output
                            .lines()
                            .map(|line| line.trim())
                            .find(|line| !line.is_empty())
                            .unwrap_or("(no output)")
                            .to_string();
                        recent_tool_outcomes.push((
                            tool_call.name.clone(),
                            success,
                            first_non_empty_line,
                        ));
                        if recent_tool_outcomes.len() > 5 {
                            recent_tool_outcomes.remove(0);
                        }

                        if tool_call.name != "batch" {
                            self.event_bus.emit(AgentEvent::ToolComplete {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                tool_call_id: tool_call.id.clone(),
                                tool_name: tool_call.name.clone(),
                                success,
                                output: truncated_output.clone(),
                                title: tool_result.title.clone(),
                                metadata: Self::non_empty_tool_metadata(&tool_result.metadata),
                                duration_ms,
                                timestamp: SystemTime::now(),
                            });
                        }

                        if runtime_config.logging.audit_enabled && tool_call.name != "batch" {
                            let audit_entry = AuditEntry {
                                id: Uuid::new_v4().to_string(),
                                timestamp: chrono::Utc::now(),
                                session_id: session.id.clone(),
                                tool: tool_call.name.clone(),
                                input: tool_call.arguments.to_string(),
                                output: audit_output,
                                duration_ms,
                                user: user.clone(),
                            };

                            self.storage.log_audit(audit_entry)?;
                        }

                        // Every function call needs a matching result in the
                        // recorded history. This used to skip `batch`, which
                        // left its `function_call` permanently unanswered: the
                        // sub-tools report through the event bus (under their
                        // own `batch-*` ids) and never become messages, so
                        // nothing else supplied the result. The Responses API
                        // rejects that history outright ("No tool output found
                        // for function call ...") and every later request in the
                        // session inherits the breakage.
                        let tool_message = Self::summarize_tool_output_for_context(
                            &tool_call.name,
                            tool_result.outcome,
                            &output,
                        );
                        let mut tool_meta = serde_json::json!({
                            "tool_name": tool_call.name,
                            "success": success,
                            "tool_result": {
                                "title": tool_result.title,
                                "metadata": tool_result.metadata,
                            }
                        });
                        if !success {
                            if let Some(err) = &error_detail {
                                tool_meta["error_message"] = serde_json::json!(err);
                            }
                        }
                        session.messages.push(Message::tool_result_with_metadata(
                            tool_call.id.clone(),
                            tool_message,
                            tool_meta,
                        ));
                        if !tool_result.attachments.is_empty() {
                            if let Some(tool_msg) = session.messages.last_mut() {
                                for attachment in &tool_result.attachments {
                                    tool_msg.images.push(MessageImage {
                                        filename: attachment.filename.clone(),
                                        mime: attachment.mime.clone(),
                                        data_url: attachment.data_url.clone(),
                                    });
                                }
                            }
                        }

                        // Advisory repeat-call nudge travels as its
                        // own user-role message so the tool result
                        // above stays the tool's auditable output.
                        if let Some(reminder) = repeat_reminder {
                            session
                                .messages
                                .push(Message::synthetic_user(reminder, "repeat_tool_reminder"));
                        }
                    }
                }

                info!("process_message: All tool calls processed, updating session");
                if loop_guard_triggered {
                    session.messages.push(Message::synthetic_user(
                        "You are repeating yourself. Do not rerun the same tool call with the same arguments. Reuse earlier results, change strategy, inspect a file you already found, or explain the blocker."
                            .to_string(),
                        "loop_guard_nudge",
                    ));
                }
                self.record_session_event(
                    &mut session,
                    "step_finish",
                    serde_json::json!({
                        "iteration": iteration,
                        "finish_reason": "tool_calls_processed",
                        "tool_success_count": tool_success_count,
                        "tool_failure_count": tool_failure_count,
                    }),
                )?;
                self.event_bus.emit(AgentEvent::StepFinish {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    iteration,
                    tool_success_count,
                    tool_failure_count,
                    finish_reason: "tool_calls_processed".to_string(),
                    timestamp: SystemTime::now(),
                });
                self.session_manager.update_session(&session).await?;
            } else {
                info!("process_message: No tool calls, response complete");

                let finish = response.finish_reason.to_lowercase();
                if finish == "length" && iteration < hard_iteration_ceiling {
                    warn!(
                        "Response truncated (finish_reason=length) for session {} - requesting continuation",
                        session_id
                    );
                    response_truncated = true;
                    continue;
                }

                if last_iteration_only_meta_tools
                    && meta_only_followup_attempts < 2
                    && response
                        .content
                        .as_ref()
                        .map(|c| !c.trim().is_empty())
                        .unwrap_or(false)
                {
                    meta_only_followup_attempts += 1;
                    warn!(
                        "Post-tool response stopped after planning/meta tools for session {} - nudging model to continue substantive work",
                        session_id
                    );
                    session.messages.push(Message::synthetic_user(
                        "Planning is not completion. Continue with the next real step now, or explain the blocker plainly.".to_string(),
                        "planning_nudge",
                    ));
                    continue;
                }
                // No more tool calls, emit response complete
                let finish_reason = response.finish_reason.clone();
                self.record_session_event(
                    &mut session,
                    "step_finish",
                    serde_json::json!({
                        "iteration": iteration,
                        "finish_reason": finish_reason,
                        "tool_success_count": tool_success_count,
                        "tool_failure_count": tool_failure_count,
                    }),
                )?;
                self.event_bus.emit(AgentEvent::StepFinish {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    iteration,
                    tool_success_count,
                    tool_failure_count,
                    finish_reason,
                    timestamp: SystemTime::now(),
                });
                break;
            }
        }

        // Persist the usage from the final provider request. A turn that ends
        // without another iteration has no later preflight ContextUpdate to
        // copy this into session state, which otherwise leaves the next load
        // showing the old chars/4 estimate after compaction.
        if let Some(context_state) = session.context_state.as_mut() {
            context_state.last_request_usage =
                last_request_usage.as_ref().map(|usage| MessageTokens {
                    input: usage.input,
                    output: usage.output,
                    total: usage.total,
                    cached_read: usage.cached_read,
                    cached_write: usage.cached_write,
                    reasoning: usage.reasoning,
                    cache_reason: usage.cache_reason.clone(),
                });
        }

        // A reopened client may read the session immediately after receiving
        // ResponseComplete. Persist the full reply and idle status first so
        // that snapshot can never pair a complete event with a running turn.
        session.task_status = "active".to_string();
        info!("process_message: Loop complete, finalizing session");
        self.session_manager.update_session(&session).await?;
        self.event_bus.emit(AgentEvent::ResponseComplete {
            session_id: session_id.to_string(),
            sequence: 0,
            timestamp: SystemTime::now(),
            usage: last_request_usage,
            turn_usage: if total_tokens > 0 {
                Some(EventTokenUsage {
                    input: total_input_tokens,
                    output: total_output_tokens,
                    total: total_tokens,
                    cached_read: cache_read_reported.then_some(total_cached_read_tokens),
                    cached_write: cache_write_reported.then_some(total_cached_write_tokens),
                    reasoning: None,
                    cache_reason: Self::aggregate_cache_reason(&cache_reasons),
                })
            } else {
                None
            },
        });

        if runtime_config.agent.checkpoint_enabled {
            // Community Discord turns have a read-only tool profile. Running
            // `git add -A` over their workspace after every reply can hold the
            // Discord response behind a large repository scan even though no
            // files could have changed. Keep the session snapshot checkpoint,
            // but skip the workspace Git snapshot for these turns.
            let checkpoint_workspace = (session.metadata["source"] != "discord-community")
                .then(|| active_workspace.resolved_path());
            if let Err(error) = self
                .checkpoint_manager
                .create_checkpoint(
                    &session,
                    Some("assistant_turn_complete".to_string()),
                    None,
                    checkpoint_workspace,
                )
                .await
            {
                warn!("Failed to create end-of-turn checkpoint: {}", error);
            }
        }

        let last_assistant_message = session
            .messages
            .iter()
            .rev()
            .find(|m| Self::is_visible_assistant_message(m));

        let fallback_assistant_message = session.messages.iter().rev().find(|m| {
            m.role == "assistant"
                && !m.content.trim().is_empty()
                && !Self::looks_like_internal_tool_dump(&m.content)
        });

        let mut result = last_assistant_message
            .or(fallback_assistant_message)
            .map(|m| m.content.clone())
            .unwrap_or_else(|| {
                // Check if any tools were executed
                let has_tool_results = session.messages.iter().any(|m| m.role == "tool");
                if has_tool_results {
                    "Task completed successfully. The requested operations have been executed."
                        .to_string()
                } else {
                    String::new()
                }
            });

        if max_iterations_reached {
            let total_tools = tool_success_count + tool_failure_count;
            let base = if total_tools > 0 {
                "Status: partial. I finished running tools, but hit the iteration ceiling before finalizing.".to_string()
            } else {
                "Status: partial. I hit the iteration ceiling before completing the task."
                    .to_string()
            };

            let counts = if total_tools > 0 {
                format!(
                    " Tools run: {} ({} succeeded, {} failed).",
                    total_tools, tool_success_count, tool_failure_count
                )
            } else {
                String::new()
            };

            let last_tool = recent_tool_outcomes
                .last()
                .map(|(name, success, line)| {
                    let cleaned = line.replace(['\n', '\r'], " ");
                    let mut snippet: String = cleaned.chars().take(180).collect();
                    if cleaned.chars().count() > 180 {
                        snippet.push_str("...");
                    }
                    format!(
                        " Completed: last tool {} ({}) - {}.",
                        name,
                        if *success { "success" } else { "failed" },
                        snippet
                    )
                })
                .unwrap_or_default();

            let remaining = if tool_failure_count > 0 {
                " Remaining: fix failed tool steps, then produce a final summary of applied changes."
            } else {
                " Remaining: produce a final summary of applied changes."
            };

            result = format!("{}{}{}{}", base, counts, last_tool, remaining)
                .trim()
                .to_string();
        }

        let has_tool_messages = session.messages.iter().any(|m| m.role == "tool");

        if has_tool_messages && result.trim().is_empty() {
            let mut tool_summary = String::from("Tool Results:\n");
            // Only the last three tool results render below; clone just
            // those instead of every tool output in the transcript.
            let mut recent: Vec<String> = session
                .messages
                .iter()
                .rev()
                .filter(|m| m.role == "tool")
                .take(3)
                .map(|m| m.content.clone())
                .collect();
            recent.reverse();

            for message in recent {
                let char_count = message.chars().count();
                let mut snippet: String = message.chars().take(1500).collect();
                if char_count > 1500 {
                    snippet.push_str("\n...[truncated]");
                }
                tool_summary.push_str("```tool\n");
                tool_summary.push_str(&snippet);
                tool_summary.push_str("\n```\n");
            }

            result = tool_summary.trim().to_string();
        }

        if result.is_empty() {
            warn!(
                "Empty response for session {} - last assistant message had no content",
                session_id
            );
        } else {
            info!(
                "process_message: Returning {} char response for session {}",
                result.len(),
                session_id
            );
        }

        if let Ok(elapsed) = active_run.started_at.elapsed() {
            info!(
                "process_message: Session {} finished in {}ms",
                session_id,
                elapsed.as_millis()
            );
        }

        // Goal round continuation: when an armed active goal has rounds
        // left, reserve the next round and queue it. The queue is
        // dispatched by the caller once this run's guard drops.
        if let Err(error) = self.maybe_queue_goal_round(&session).await {
            warn!(
                "Goal round driver failed for session {}: {}",
                session_id, error
            );
        }

        Ok(result)
    }

    /// Reserve and enqueue the next goal round, if one is due. Mirrors
    /// DSH's goal-round-driver: the reservation is CAS-fenced on the
    /// goal revision and rounds run as separate queued turns, never
    /// auto-continuing after a process restart (arming is in-memory).
    pub(super) async fn maybe_queue_goal_round(&self, session: &Session) -> Result<()> {
        if session.agent_type != "primary" {
            return Ok(());
        }
        let Some((round, goal)) = self.goal_store.reserve_round(&session.id)? else {
            return Ok(());
        };
        let objective = serde_json::to_string(&goal.objective)
            .unwrap_or_else(|_| format!("\"{}\"", goal.objective));
        let message = format!(
            "<goal_round>\nObjective: {}\nRound: {} of {}\nContinue working toward the objective. \
             When the objective is met, call update_goal with action=\"complete\". If you cannot \
             make progress, report the blocker to the user.\n</goal_round>",
            objective, round, goal.max_rounds
        );
        self.enqueue_message(
            &session.id,
            &format!("goal-round-{}-{}-{}", goal.id, goal.revision, round),
            &message,
            &[],
            None,
            &[],
        )
        .await?;
        info!(
            "Goal round driver: reserved round {} of {} for session {}",
            round, goal.max_rounds, session.id
        );
        Ok(())
    }

    /// Dispatch pending queued messages for a session, if any. Safe to
    /// call after every run; the queue claim is a no-op when empty. Also
    /// honors deferred background-completion wakes once the queue settles.
    pub(super) fn dispatch_queued_messages(&self, session_id: &str, user: &str) {
        let Some(runtime) = self.self_arc.get().and_then(|weak| weak.upgrade()) else {
            return;
        };
        let session_id = session_id.to_string();
        let user = user.to_string();
        tokio::spawn(async move {
            tokio::time::sleep(std::time::Duration::from_millis(100)).await;
            let dispatch_runtime = runtime.clone();
            if let Err(error) =
                dispatch_runtime.spawn_next_queued_message_run(session_id.clone(), user)
            {
                tracing::error!(
                    "Failed to dispatch queued messages for session {}: {}",
                    session_id,
                    error
                );
            }
            runtime.check_pending_wake(&session_id);
        });
    }

    pub(super) fn emit_chunked_text<F>(&self, text: &str, chunk_size: usize, mut emit: F)
    where
        F: FnMut(String),
    {
        if text.trim().is_empty() {
            return;
        }

        let mut chunk_start = 0usize;
        let mut chunk_chars = 0usize;

        for (idx, ch) in text.char_indices() {
            if chunk_chars == chunk_size {
                emit(text[chunk_start..idx].to_string());
                chunk_start = idx;
                chunk_chars = 0;
            }
            chunk_chars += 1;
            let next_idx = idx + ch.len_utf8();
            if next_idx == text.len() {
                emit(text[chunk_start..next_idx].to_string());
            }
        }
    }

    pub(super) fn emit_thinking_chunks(&self, session_id: &str, thinking: &str) {
        const CHUNK_SIZE: usize = 180;

        if thinking.trim().is_empty() {
            return;
        }

        self.event_bus.emit(AgentEvent::ThinkingStart {
            session_id: session_id.to_string(),
            sequence: 0,
            timestamp: SystemTime::now(),
        });

        self.emit_chunked_text(thinking, CHUNK_SIZE, |content| {
            self.event_bus.emit(AgentEvent::ThinkingDelta {
                session_id: session_id.to_string(),
                sequence: 0,
                content,
                timestamp: SystemTime::now(),
            });
        });

        self.event_bus.emit(AgentEvent::ThinkingEnd {
            session_id: session_id.to_string(),
            sequence: 0,
            timestamp: SystemTime::now(),
        });
    }

    pub(super) fn emit_response_chunks(&self, session_id: &str, content: &str) {
        const CHUNK_SIZE: usize = 180;

        self.emit_chunked_text(content, CHUNK_SIZE, |chunk_text| {
            self.event_bus.emit(AgentEvent::ResponseChunk {
                session_id: session_id.to_string(),
                sequence: 0,
                content: chunk_text,
                timestamp: SystemTime::now(),
            });
        });
    }

    /// Builds the provider retry listener: emits a live `Retry` event before
    /// each retry sleep. When the retrying session is a subagent, the event is
    /// also emitted against the parent session (with `subagent_session_id`
    /// set) so the parent chat can surface it on the subagent card.
    pub(super) fn provider_retry_listener(
        event_bus: Arc<EventBus>,
        storage: Arc<SqliteStorage>,
    ) -> Arc<dyn RetryListener> {
        Arc::new(
            move |session_id: Option<&str>,
                  attempt: u32,
                  max_attempts: u32,
                  delay: std::time::Duration,
                  reason: &str| {
                let Some(sid) = session_id else { return };
                let reason_text = format!(
                    "Provider request failed: {} — retrying in ~{}s",
                    reason,
                    delay.as_secs()
                );
                let parent_id = storage
                    .get_session(sid)
                    .ok()
                    .flatten()
                    .and_then(|session| session.parent_id);
                if let Some(parent) = parent_id {
                    event_bus.emit(AgentEvent::Retry {
                        session_id: parent,
                        sequence: 0,
                        scope: "provider".to_string(),
                        attempt_count: attempt,
                        max_attempts,
                        next_retry_in_ms: delay.as_millis() as u64,
                        subagent_session_id: Some(sid.to_string()),
                        reason: reason_text.clone(),
                        timestamp: SystemTime::now(),
                    });
                }
                event_bus.emit(AgentEvent::Retry {
                    session_id: sid.to_string(),
                    sequence: 0,
                    scope: "provider".to_string(),
                    attempt_count: attempt,
                    max_attempts,
                    next_retry_in_ms: delay.as_millis() as u64,
                    subagent_session_id: None,
                    reason: reason_text,
                    timestamp: SystemTime::now(),
                });
            },
        )
    }

    /// Removes the placeholder assistant message pushed by
    /// `consume_provider_stream` when the stream failed before delivering any
    /// content. Returns true when it was safe to retry: nothing user-visible
    /// (text, thinking, or tool call) had been committed.
    pub(super) fn reset_failed_stream_message(session: &mut Session) -> bool {
        match session.messages.last() {
            Some(last)
                if last.role == "assistant"
                    && last.content.is_empty()
                    && last.tool_calls.is_none()
                    && last
                        .thinking
                        .as_deref()
                        .map(|text| text.trim().is_empty())
                        .unwrap_or(true) =>
            {
                session.messages.pop();
                true
            }
            _ => false,
        }
    }

    /// Emits the same `Retry` event the provider-level retry listener does, for
    /// a retry the runtime performs itself after a mid-stream failure.
    pub(super) fn emit_provider_stream_retry(
        &self,
        session_id: &str,
        attempt: u32,
        delay: std::time::Duration,
        reason: &str,
    ) {
        let reason_text = format!(
            "Provider stream failed mid-response: {}; retrying in ~{}s",
            reason,
            delay.as_secs()
        );
        let parent_id = self
            .storage
            .get_session(session_id)
            .ok()
            .flatten()
            .and_then(|session| session.parent_id);
        if let Some(parent) = parent_id {
            self.event_bus.emit(AgentEvent::Retry {
                session_id: parent,
                sequence: 0,
                scope: "provider".to_string(),
                attempt_count: attempt,
                max_attempts: MAX_RETRIES,
                next_retry_in_ms: delay.as_millis() as u64,
                subagent_session_id: Some(session_id.to_string()),
                reason: reason_text.clone(),
                timestamp: SystemTime::now(),
            });
        }
        self.event_bus.emit(AgentEvent::Retry {
            session_id: session_id.to_string(),
            sequence: 0,
            scope: "provider".to_string(),
            attempt_count: attempt,
            max_attempts: MAX_RETRIES,
            next_retry_in_ms: delay.as_millis() as u64,
            subagent_session_id: None,
            reason: reason_text,
            timestamp: SystemTime::now(),
        });
    }

    pub(super) fn is_streaming_fallback_error(error: &OSAgentError) -> bool {
        match error {
            OSAgentError::Provider(message) => {
                message.contains("Streaming currently supports only")
                    || message.contains("Invalid status code: 4")
                    || message.contains("Invalid status code: 5")
            }
            OSAgentError::ProviderStructured(info) => {
                info.message.contains("Streaming currently supports only")
                    || info.status_code.is_some_and(|status| status >= 400)
            }
            _ => false,
        }
    }

    pub(super) async fn maybe_persist_streaming_message(
        &self,
        session: &Session,
        last_flush: &mut Instant,
        dirty_chars: &mut usize,
        force: bool,
    ) -> Result<()> {
        const FLUSH_CHARS: usize = 384;
        const FLUSH_INTERVAL_MS: u64 = 250;

        if !force
            && *dirty_chars < FLUSH_CHARS
            && last_flush.elapsed().as_millis() < FLUSH_INTERVAL_MS as u128
        {
            return Ok(());
        }

        self.session_manager.update_session(session).await?;
        *last_flush = Instant::now();
        *dirty_chars = 0;
        Ok(())
    }

    pub(super) async fn consume_provider_stream(
        &self,
        session_id: &str,
        session: &mut Session,
        mut stream: futures::stream::BoxStream<'static, Result<StreamEvent>>,
        _cancel_notify: Arc<Notify>,
    ) -> Result<crate::agent::provider::ProviderResponse> {
        let assistant_index = session.messages.len();
        session
            .messages
            .push(Message::assistant(String::new(), None));
        self.session_manager.update_session(session).await?;

        let mut response_started = false;
        let mut thinking_started = false;
        let mut finish_reason = "completed".to_string();
        let mut usage: Option<crate::agent::provider::TokenUsage> = None;
        let mut tool_calls: HashMap<usize, PendingToolCall> = HashMap::new();
        let mut last_flush = Instant::now();
        let mut dirty_chars = 0usize;
        let mut finish_deadline = None;

        loop {
            let next_event = tokio::select! {
                _ = self.wait_for_cancellation(session_id) => {
                    self.session_manager.update_session(session).await?;
                    self.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    return Err(OSAgentError::Session("Operation cancelled".to_string()));
                }
                event = next_provider_stream_event(
                    &mut stream,
                    finish_deadline,
                    std::time::Duration::from_secs(120),
                ) => event,
            };

            let Some(event) = next_event else {
                break;
            };

            let event = event?;

            if let Some(reason) = event.finish_reason.as_ref() {
                finish_reason = reason.clone();
                if !reason.is_empty() && finish_deadline.is_none() {
                    finish_deadline =
                        Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2));
                }
            }
            if event.usage.is_some() {
                usage = event.usage.clone();
            }

            if let Some(delta) = event.thinking.as_ref() {
                let assistant = &mut session.messages[assistant_index];
                let thinking = assistant.thinking.get_or_insert_with(String::new);
                thinking.push_str(delta);
                dirty_chars += delta.len();

                if !thinking_started {
                    self.event_bus.emit(AgentEvent::ThinkingStart {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    thinking_started = true;
                }

                self.event_bus.emit(AgentEvent::ThinkingDelta {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    content: delta.clone(),
                    timestamp: SystemTime::now(),
                });
            }

            if let Some(delta) = event.content.as_ref() {
                if thinking_started {
                    self.event_bus.emit(AgentEvent::ThinkingEnd {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    thinking_started = false;
                }
                if !response_started {
                    self.event_bus.emit(AgentEvent::ResponseStart {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    response_started = true;
                }

                let assistant = &mut session.messages[assistant_index];
                assistant.content.push_str(delta);
                dirty_chars += delta.len();

                self.event_bus.emit(AgentEvent::ResponseChunk {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    content: delta.clone(),
                    timestamp: SystemTime::now(),
                });
            }

            if !event.tool_call_deltas.is_empty() {
                if thinking_started {
                    self.event_bus.emit(AgentEvent::ThinkingEnd {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    thinking_started = false;
                }
                if !response_started {
                    self.event_bus.emit(AgentEvent::ResponseStart {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    response_started = true;
                }

                for delta in event.tool_call_deltas {
                    let entry = tool_calls.entry(delta.index).or_default();
                    if let Some(id) = delta.id {
                        entry.id = id;
                    }
                    if let Some(name) = delta.name {
                        entry.name = name;
                    }
                    if let Some(arguments) = delta.arguments {
                        entry.arguments.push_str(&arguments);
                        dirty_chars += arguments.len();
                    }
                }
            }

            self.maybe_persist_streaming_message(session, &mut last_flush, &mut dirty_chars, false)
                .await?;

            if event.done {
                break;
            }
        }

        if thinking_started {
            self.event_bus.emit(AgentEvent::ThinkingEnd {
                session_id: session_id.to_string(),
                sequence: 0,
                timestamp: SystemTime::now(),
            });
        }

        let tool_calls = if tool_calls.is_empty() {
            None
        } else {
            let mut ordered = tool_calls.into_iter().collect::<Vec<_>>();
            ordered.sort_by_key(|(index, _)| *index);
            let built = ordered
                .into_iter()
                .filter_map(|(_, call)| {
                    if call.name.is_empty() {
                        return None;
                    }
                    Some(ToolCall {
                        id: if call.id.is_empty() {
                            Uuid::new_v4().to_string()
                        } else {
                            call.id
                        },
                        name: call.name,
                        arguments: serde_json::from_str(&call.arguments)
                            .unwrap_or_else(|_| serde_json::json!({})),
                    })
                })
                .collect::<Vec<_>>();
            if built.is_empty() {
                None
            } else {
                Some(built)
            }
        };

        session.messages[assistant_index].tool_calls = tool_calls.clone();
        self.maybe_persist_streaming_message(session, &mut last_flush, &mut dirty_chars, true)
            .await?;

        Ok(crate::agent::provider::ProviderResponse {
            content: Some(session.messages[assistant_index].content.clone())
                .filter(|content| !content.is_empty()),
            thinking: session.messages[assistant_index]
                .thinking
                .clone()
                .filter(|thinking| !thinking.is_empty()),
            tool_calls,
            finish_reason,
            retry_count: 0,
            context_compressed: false,
            compressed_summary: None,
            usage,
        })
    }
}
