//! Tool dispatch: script bridge, batch/parallel execution, progress events
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    /// `tool_script` runs outside the normal tool path for the same
    /// reason `batch` does: it needs the registry itself, which a tool
    /// owned by that registry cannot hold.
    pub(super) async fn handle_tool_script_call(
        &self,
        session_id: &str,
        active_workspace: &WorkspaceConfig,
        args: &serde_json::Value,
    ) -> Result<ToolResult> {
        let config = self.config.read().await.clone();
        let context = crate::tools::tool_script::ScriptContext {
            registry: self.tool_registry.clone(),
            config,
            workspace_path: active_workspace.resolved_path(),
            workspace_id: active_workspace.id.clone(),
            event_bus: Some(self.event_bus.clone()),
            session_id: session_id.to_string(),
        };
        crate::tools::tool_script::run_script(context, args).await
    }

    pub(super) async fn handle_batch_tool_call(
        &self,
        session: &mut Session,
        active_workspace: &WorkspaceConfig,
        args: &serde_json::Value,
        message_index: i32,
    ) -> Result<String> {
        let tool_calls = args["tool_calls"]
            .as_array()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'tool_calls' array".to_string()))?;

        if tool_calls.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Batch requires at least one tool call".to_string(),
            ));
        }

        if tool_calls.len() > 25 {
            return Err(OSAgentError::ToolExecution(
                "Batch supports at most 25 tool calls".to_string(),
            ));
        }

        const SAFE_BATCH_TOOLS: &[&str] = &[
            "read_file",
            "list_files",
            "grep",
            "glob",
            "bash",
            "web_fetch",
            "web_search",
            "reflect",
            "codesearch",
            "lsp",
            "todoread",
            "skill",
            "skill_list",
        ];

        for call in tool_calls {
            let tool_name = call["tool"].as_str().unwrap_or("");
            if tool_name.eq_ignore_ascii_case("batch") {
                return Err(OSAgentError::ToolExecution(
                    "Nested batch calls are not allowed".to_string(),
                ));
            }
            if !SAFE_BATCH_TOOLS
                .iter()
                .any(|allowed| allowed.eq_ignore_ascii_case(tool_name))
            {
                return Err(OSAgentError::ToolExecution(format!(
                    "Tool '{}' is not allowed in batch. Batch is limited to independent read-only tools.",
                    tool_name
                )));
            }
            if tool_name.eq_ignore_ascii_case("bash") {
                let read_only = call["parameters"]["read_only"].as_bool().unwrap_or(false);
                if !read_only {
                    return Err(OSAgentError::ToolExecution(
                        "Batch bash calls require parameters.read_only=true".to_string(),
                    ));
                }
                let command = call["parameters"]["command"].as_str().ok_or_else(|| {
                    OSAgentError::ToolExecution(
                        "Batch bash calls require a 'command' string".to_string(),
                    )
                })?;
                let args_list = call["parameters"]["args"]
                    .as_array()
                    .map(|arr| {
                        arr.iter()
                            .filter_map(|value| value.as_str().map(|value| value.to_string()))
                            .collect::<Vec<_>>()
                    })
                    .unwrap_or_default();
                let full_command = BashTool::build_command(command, &args_list);
                BashTool::validate_explicit_read_only(&full_command)?;
            }
        }

        let workspace_path = active_workspace.resolved_path();
        let session_id = session.id.clone();
        let event_bus = self.event_bus.clone();

        let batch_internal_ids: Vec<String> = (0..tool_calls.len())
            .map(|i| format!("batch-{}-{}", Uuid::new_v4(), i))
            .collect();

        let futures =
            tool_calls
                .iter()
                .zip(batch_internal_ids.iter())
                .map(|(call, internal_id)| {
                    let tool_name = call["tool"].as_str().unwrap_or("").to_string();
                    let params = call["parameters"].clone();
                    let registry = self.tool_registry.clone();
                    let workspace_path = workspace_path.clone();
                    let session_id = session_id.clone();
                    let internal_id = internal_id.clone();
                    let event_bus = event_bus.clone();
                    let msg_idx = message_index;

                    async move {
                        event_bus.emit(AgentEvent::ToolStart {
                            session_id: session_id.clone(),
                            sequence: 0,
                            tool_call_id: internal_id.clone(),
                            tool_name: tool_name.clone(),
                            arguments: params.clone(),
                            message_index: msg_idx,
                            timestamp: SystemTime::now(),
                        });

                        let start = Instant::now();
                        let result = if tool_name.is_empty() {
                            Err(OSAgentError::ToolExecution("Missing tool name".to_string()))
                        } else if !registry.is_allowed(&tool_name) {
                            Err(OSAgentError::ToolExecution(
                                "Tool is not allowed in this session".to_string(),
                            ))
                        } else {
                            registry
                                .execute_in_workspace_result(
                                    &tool_name,
                                    params,
                                    Some(workspace_path),
                                )
                                .await
                        };
                        let duration_ms = start.elapsed().as_millis() as u64;

                        let (success, output) = match result {
                            Ok(out) => (out.outcome.is_success(), out),
                            Err(e) => (false, ToolResult::failure(e.to_string())),
                        };

                        event_bus.emit(AgentEvent::ToolComplete {
                            session_id,
                            sequence: 0,
                            tool_call_id: internal_id,
                            tool_name,
                            success,
                            output: output.output.clone(),
                            title: output.title.clone(),
                            metadata: Self::non_empty_tool_metadata(&output.metadata),
                            duration_ms,
                            timestamp: SystemTime::now(),
                        });

                        (success, output)
                    }
                });

        let results: Vec<(bool, ToolResult)> = join_all(futures).await;
        let mut lines = Vec::new();
        let mut success_count = 0usize;

        for (idx, (success, output)) in results.iter().enumerate() {
            let call = &tool_calls[idx];
            let tool_name = call["tool"].as_str().unwrap_or("");
            if *success {
                success_count += 1;
            }
            lines.push(format!(
                "[{}] {}\n{}",
                if *success { "ok" } else { "error" },
                tool_name,
                Self::summarize_tool_output_for_context(tool_name, output.outcome, &output.output,)
            ));
        }

        self.record_session_event(
            session,
            "batch",
            serde_json::json!({
                "total": tool_calls.len(),
                "successful": success_count,
                "failed": tool_calls.len() - success_count,
            }),
        )?;

        Ok(format!(
            "Batch executed {}/{} tool calls successfully.\n\n{}",
            success_count,
            tool_calls.len(),
            lines.join("\n\n")
        ))
    }

    pub(super) async fn execute_parallel_tool_calls(
        &self,
        session_id: &str,
        _session: &Session,
        active_workspace: &WorkspaceConfig,
        tool_calls: &[ToolCall],
        _runtime_config: &Config,
        _recent_tool_signatures: &mut Vec<String>,
        _recent_tool_intents: &mut Vec<String>,
        _recent_tool_outcomes: &mut Vec<(String, bool, String)>,
        tool_success_count: &mut usize,
        tool_failure_count: &mut usize,
        _loop_guard_triggered: &mut bool,
        _iteration: usize,
        _user: &str,
        message_index: i32,
    ) -> Result<
        Vec<(
            ToolResult,
            bool,
            u64,
            String,
            Option<String>,
            Option<String>,
        )>,
    > {
        let session_id = session_id.to_string();
        let workspace_path = active_workspace.resolved_path();
        let registry = self.tool_registry.clone();
        let event_bus = self.event_bus.clone();

        for tool_call in tool_calls {
            self.event_bus.emit(AgentEvent::ToolStart {
                session_id: session_id.clone(),
                sequence: 0,
                tool_call_id: tool_call.id.clone(),
                tool_name: tool_call.name.clone(),
                arguments: tool_call.arguments.clone(),
                message_index,
                timestamp: SystemTime::now(),
            });
        }

        let cancel_fut = self.wait_for_cancellation(&session_id);
        tokio::select! {
            _ = cancel_fut => {
                for tool_call in tool_calls {
                    self.event_bus.emit(AgentEvent::ToolComplete {
                        session_id: session_id.clone(),
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
                }
                return Err(OSAgentError::Session("Operation cancelled".to_string()));
            }
            _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
        }

        let futures = tool_calls.iter().map(|tool_call| {
            let tool_name = tool_call.name.clone();
            let args = tool_call.arguments.clone();
            let tool_id = tool_call.id.clone();
            let session_id = session_id.clone();
            let workspace_path = workspace_path.clone();
            let registry = registry.clone();
            let event_bus = event_bus.clone();
            async move {
                let start = Instant::now();

                Self::emit_tool_progress(
                    &event_bus,
                    &session_id,
                    &tool_id,
                    &tool_name,
                    ToolStatus::Preparing,
                    "Preparing parallel execution...",
                    10,
                );

                Self::emit_tool_progress(
                    &event_bus,
                    &session_id,
                    &tool_id,
                    &tool_name,
                    ToolStatus::Executing,
                    "Executing in parallel...",
                    50,
                );

                let cancel_fut = self.wait_for_cancellation(&session_id);
                tokio::select! {
                    _ = cancel_fut => {
                        Self::emit_tool_complete(
                            &event_bus,
                            &session_id,
                            &tool_id,
                            &tool_name,
                            false,
                            ToolResult::failure("Cancelled by user"),
                            0,
                        );
                        return (
                            ToolResult::failure("Cancelled by user"),
                            false,
                            0,
                            String::new(),
                            None,
                            None,
                        );
                    }
                    _ = tokio::time::sleep(std::time::Duration::from_millis(1)) => {}
                }

                let result = tokio::select! {
                    biased;
                    _ = self.wait_for_cancellation(&session_id) => Err(OSAgentError::Session("Operation cancelled".to_string())),
                    result = registry.execute_in_workspace_result(&tool_name, args, Some(workspace_path)) => result,
                };

                let duration_ms = start.elapsed().as_millis() as u64;

                Self::emit_tool_progress(
                    &event_bus,
                    &session_id,
                    &tool_id,
                    &tool_name,
                    ToolStatus::Finalizing,
                    "Finalizing...",
                    90,
                );

                match result {
                    Ok(output) => {
                        let success = output.outcome.is_success();
                        Self::emit_tool_complete(
                            &event_bus,
                            &session_id,
                            &tool_id,
                            &tool_name,
                            success,
                            output.clone(),
                            duration_ms,
                        );
                        (
                            output,
                            success,
                            duration_ms,
                            String::new(),
                            None::<String>,
                            None,
                        )
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        let error_result = ToolResult::failure(error_msg.clone());
                        Self::emit_tool_complete(
                            &event_bus,
                            &session_id,
                            &tool_id,
                            &tool_name,
                            false,
                            error_result.clone(),
                            duration_ms,
                        );
                        (
                            error_result,
                            false,
                            duration_ms,
                            String::new(),
                            None::<String>,
                            Some(e.to_string()),
                        )
                    }
                }
            }
        });

        let results = join_all(futures).await;
        let mut outputs = Vec::new();

        for (tool_call, result) in tool_calls.iter().zip(results) {
            let (tool_result, success, duration_ms, _, _, error_detail) = result;

            let tool_signature = Self::tool_call_signature(&tool_call.name, &tool_call.arguments);
            let tool_intent = Self::tool_intent_signature(&tool_call.name, &tool_call.arguments);

            if success {
                *tool_success_count += 1;
            } else {
                *tool_failure_count += 1;
            }

            outputs.push((
                tool_result,
                success,
                duration_ms,
                tool_signature,
                tool_intent,
                error_detail,
            ));
        }

        Ok(outputs)
    }

    pub(super) fn emit_tool_progress(
        event_bus: &EventBus,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        status: ToolStatus,
        message: &str,
        progress_percent: u8,
    ) {
        event_bus.emit(AgentEvent::ToolProgress {
            session_id: session_id.to_string(),
            sequence: 0,
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            status,
            message: Some(message.to_string()),
            progress_percent: Some(progress_percent),
            timestamp: SystemTime::now(),
        });
    }

    pub(super) fn emit_tool_complete(
        event_bus: &EventBus,
        session_id: &str,
        tool_call_id: &str,
        tool_name: &str,
        success: bool,
        result: ToolResult,
        duration_ms: u64,
    ) {
        let metadata = Self::non_empty_tool_metadata(&result.metadata);
        event_bus.emit(AgentEvent::ToolComplete {
            session_id: session_id.to_string(),
            sequence: 0,
            tool_call_id: tool_call_id.to_string(),
            tool_name: tool_name.to_string(),
            success,
            output: result.output,
            title: result.title,
            metadata,
            duration_ms,
            timestamp: SystemTime::now(),
        });
    }

    pub(super) fn non_empty_tool_metadata(
        metadata: &serde_json::Value,
    ) -> Option<serde_json::Value> {
        match metadata {
            serde_json::Value::Object(map) if map.is_empty() => None,
            _ => Some(metadata.clone()),
        }
    }

    pub(super) fn session_workspace_id(session: &Session) -> Option<String> {
        session
            .metadata
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
    }

    pub(super) fn session_archived(session: &Session) -> bool {
        session
            .metadata
            .get("archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    pub(super) fn set_session_workspace_id(
        session: &mut Session,
        workspace_id: &str,
    ) -> Result<()> {
        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }

        let metadata = session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Parse("Session metadata must be a JSON object".to_string())
        })?;
        metadata.insert(
            "workspace_id".to_string(),
            serde_json::Value::String(workspace_id.to_string()),
        );
        Ok(())
    }

    pub(super) fn resolve_workspace_for_session(
        session: &Session,
        config: &Config,
    ) -> Option<WorkspaceConfig> {
        let workspace_id = Self::session_workspace_id(session)?;
        config.get_workspace(&workspace_id)
    }

    pub(super) fn write_active_persona_to_session(
        session: &mut Session,
        active_persona: Option<ActivePersona>,
    ) -> Result<()> {
        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }

        let metadata = session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Parse("Session metadata must be a JSON object".to_string())
        })?;

        match active_persona {
            Some(persona) => {
                let value = serde_json::to_value(persona)
                    .map_err(|e| OSAgentError::Parse(format!("Failed to encode persona: {}", e)))?;
                metadata.insert("active_persona".to_string(), value);
            }
            None => {
                metadata.remove("active_persona");
            }
        }

        Ok(())
    }

    pub(super) fn persona_status_text(session: &Session) -> String {
        if let Some(persona) = Self::active_persona_from_session(session) {
            format!(
                "Active persona: {} ({}) - {}",
                persona.id, persona.name, persona.summary
            )
        } else {
            "Active persona: default (none selected)".to_string()
        }
    }

    pub(super) fn handle_persona_tool_call(
        &self,
        session: &mut Session,
        args: &serde_json::Value,
    ) -> Result<String> {
        let action = args["action"]
            .as_str()
            .unwrap_or("list")
            .trim()
            .to_lowercase();

        match action.as_str() {
            "list" => {
                let mut out = persona::list_personas_text();
                out.push_str("\n\n");
                out.push_str(&Self::persona_status_text(session));
                Ok(out)
            }
            "get" | "status" => Ok(Self::persona_status_text(session)),
            "set" => {
                let persona_id = args["persona_id"].as_str().ok_or_else(|| {
                    OSAgentError::ToolExecution("Missing 'persona_id' for action=set".to_string())
                })?;

                let roleplay_character = args["roleplay_character"].as_str().map(|s| s.to_string());

                let active = persona::resolve_active_persona(persona_id, roleplay_character)
                    .map_err(OSAgentError::ToolExecution)?;
                let summary = active.summary.clone();
                let id = active.id.clone();
                Self::write_active_persona_to_session(session, Some(active))?;

                Ok(format!(
                    "Persona set to {}. {} This persona is now auto-enforced for this session.",
                    id, summary
                ))
            }
            "reset" | "clear" => {
                Self::write_active_persona_to_session(session, None)?;
                Ok("Persona reset to default assistant behavior. Auto-enforced persona is now off.".to_string())
            }
            _ => Err(OSAgentError::ToolExecution(format!(
                "Unknown persona action '{}'. Use list/get/set/reset.",
                action
            ))),
        }
    }
}
