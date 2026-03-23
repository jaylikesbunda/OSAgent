use crate::agent::events::{AgentEvent, EventBus};
use crate::agent::prompt::{self, PromptMode};
use crate::agent::provider::{OpenAICompatibleProvider, Provider};
use crate::agent::session::SessionManager;
use crate::config::Config;
use crate::error::Result;
use crate::storage::{Message, SqliteStorage, SubagentTask};
use crate::tools::registry::ToolRegistry;
use chrono::Utc;
use dashmap::DashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tracing::{error, info, warn};
use uuid::Uuid;

fn truncate_tool_output(tool_name: &str, output: &str) -> String {
    const MAX_CHARS: usize = 4_000;
    const MAX_LINES: usize = 80;

    let normalized = output.replace('\r', "");
    let line_count = normalized.lines().count();
    let mut selected_lines: Vec<&str> = normalized.lines().take(MAX_LINES).collect();

    if selected_lines.is_empty() && !normalized.is_empty() {
        selected_lines.push(normalized.as_str());
    }

    let mut compact = selected_lines.join("\n");
    if compact.chars().count() > MAX_CHARS {
        compact = compact.chars().take(MAX_CHARS).collect::<String>();
        compact.push_str("\n...[truncated for context]");
    } else if line_count > MAX_LINES {
        compact.push_str(&format!(
            "\n...[truncated {} more lines for context]",
            line_count - MAX_LINES
        ));
    }

    if compact.trim().is_empty() {
        compact = "(no output)".to_string();
    }

    match tool_name {
        "read_file" => format!(
            "Tool: {}\nOutput summary (trimmed file content for context):\n{}",
            tool_name, compact
        ),
        "list_files" | "glob" | "grep" => format!(
            "Tool: {}\nOutput summary (trimmed search results for context):\n{}",
            tool_name, compact
        ),
        _ => format!("Tool: {}\nOutput:\n{}", tool_name, compact),
    }
}

pub struct SubagentManager {
    storage: Arc<SqliteStorage>,
    event_bus: Arc<EventBus>,
    session_manager: Arc<SessionManager>,
    active_subagents: Arc<DashMap<String, SubagentHandle>>,
    config: Arc<tokio::sync::RwLock<Config>>,
}

struct SubagentHandle {
    task_id: String,
    parent_session_id: String,
    session_id: String,
    handle: JoinHandle<()>,
    cancel_tx: mpsc::Sender<()>,
}

impl SubagentManager {
    pub fn new(
        storage: Arc<SqliteStorage>,
        event_bus: Arc<EventBus>,
        session_manager: Arc<SessionManager>,
        config: Arc<tokio::sync::RwLock<Config>>,
    ) -> Self {
        Self {
            storage,
            event_bus,
            session_manager,
            active_subagents: Arc::new(DashMap::new()),
            config,
        }
    }

    pub fn get_allowed_tools_for_agent_type(agent_type: &str) -> Vec<String> {
        let all_tools = vec![
            "bash",
            "batch",
            "read_file",
            "write_file",
            "edit_file",
            "apply_patch",
            "list_files",
            "delete_file",
            "code_python",
            "code_node",
            "code_bash",
            "grep",
            "glob",
            "web_fetch",
            "web_search",
            "task",
            "reflect",
            "question",
            "skill",
            "skill_list",
            "lsp",
            "persona",
            "process",
            "todowrite",
            "todoread",
        ];

        let general_tools: HashSet<String> = all_tools.iter().map(|s| s.to_string()).collect();

        let explore_tools: HashSet<String> = [
            "read_file",
            "list_files",
            "grep",
            "glob",
            "web_fetch",
            "web_search",
            "reflect",
            "skill",
            "skill_list",
            "lsp",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let allowed = match agent_type {
            "general" => general_tools,
            "explore" => explore_tools,
            _ => general_tools,
        };

        allowed.into_iter().collect()
    }

    pub async fn spawn_subagent(
        &self,
        parent_session_id: String,
        description: String,
        prompt: String,
        agent_type: String,
    ) -> Result<String> {
        let parent_session = self
            .session_manager
            .get_session(&parent_session_id)
            .await?
            .ok_or_else(|| {
                crate::error::OSAgentError::ToolExecution("Parent session not found".to_string())
            })?;

        let mut subagent_session = self.storage.create_subagent_session(
            parent_session_id.clone(),
            parent_session.model.clone(),
            parent_session.provider.clone(),
            agent_type.clone(),
        )?;

        let display_name = format!(
            "{} Agent",
            agent_type.chars().next().unwrap_or('g').to_uppercase()
        );
        subagent_session.metadata["name"] = serde_json::json!(display_name);
        let _ = self.storage.update_session(&subagent_session);

        let task = SubagentTask {
            id: Uuid::new_v4().to_string(),
            session_id: subagent_session.id.clone(),
            parent_session_id: parent_session_id.clone(),
            description: description.clone(),
            prompt: prompt.clone(),
            agent_type: agent_type.clone(),
            status: "running".to_string(),
            tool_count: 0,
            result: None,
            created_at: Utc::now(),
            completed_at: None,
        };

        self.storage.create_subagent_task(&task)?;

        self.event_bus.emit(AgentEvent::SubagentCreated {
            session_id: parent_session_id.clone(),
            parent_session_id: parent_session_id.clone(),
            subagent_session_id: subagent_session.id.clone(),
            description: description.clone(),
            prompt: prompt.clone(),
            agent_type: agent_type.clone(),
            timestamp: std::time::SystemTime::now(),
        });

        let (cancel_tx, mut cancel_rx) = mpsc::channel(1);
        let storage = self.storage.clone();
        let event_bus = self.event_bus.clone();
        let session_manager = self.session_manager.clone();
        let config = self.config.clone();
        let subagent_session_id = subagent_session.id.clone();
        let task_id = task.id.clone();
        let parent_session_id_for_async = parent_session_id.clone();
        let active_subagents = self.active_subagents.clone();

        let handle = tokio::spawn(async move {
            struct CleanupGuard {
                session_id: String,
                active_subagents: Arc<DashMap<String, SubagentHandle>>,
            }

            impl Drop for CleanupGuard {
                fn drop(&mut self) {
                    self.active_subagents.remove(&self.session_id);
                }
            }

            let _cleanup = CleanupGuard {
                session_id: subagent_session_id.clone(),
                active_subagents: active_subagents.clone(),
            };

            let result = Self::run_subagent(
                subagent_session_id.clone(),
                parent_session_id_for_async.clone(),
                task_id.clone(),
                prompt,
                agent_type,
                storage.clone(),
                event_bus.clone(),
                session_manager.clone(),
                config.clone(),
                &mut cancel_rx,
            )
            .await;

            match result {
                Ok((status, result_text, tool_count)) => {
                    if let Ok(Some(mut task)) = storage.get_subagent_task(&task_id) {
                        task.status = status.clone();
                        task.result = Some(result_text.clone());
                        task.tool_count = tool_count;
                        task.completed_at = Some(Utc::now());
                        let _ = storage.update_subagent_task(&task);
                    }

                    if let Ok(Some(mut session)) = storage.get_session(&subagent_session_id) {
                        session.task_status = status.clone();
                        let _ = storage.update_session(&session);
                    }

                    event_bus.emit(AgentEvent::SubagentCompleted {
                        session_id: parent_session_id_for_async.clone(),
                        parent_session_id: parent_session_id_for_async.clone(),
                        subagent_session_id: subagent_session_id.clone(),
                        status,
                        result: result_text,
                        tool_count,
                        timestamp: std::time::SystemTime::now(),
                    });
                }
                Err(e) => {
                    error!("Subagent failed: {:?}", e);
                    if let Ok(Some(mut task)) = storage.get_subagent_task(&task_id) {
                        task.status = "failed".to_string();
                        task.result = Some(format!("Error: {}", e));
                        task.completed_at = Some(Utc::now());
                        let _ = storage.update_subagent_task(&task);
                    }

                    if let Ok(Some(mut session)) = storage.get_session(&subagent_session_id) {
                        session.task_status = "failed".to_string();
                        let _ = storage.update_session(&session);
                    }

                    event_bus.emit(AgentEvent::SubagentCompleted {
                        session_id: parent_session_id_for_async.clone(),
                        parent_session_id: parent_session_id_for_async.clone(),
                        subagent_session_id: subagent_session_id.clone(),
                        status: "failed".to_string(),
                        result: format!("Error: {}", e),
                        tool_count: 0,
                        timestamp: std::time::SystemTime::now(),
                    });
                }
            }
        });

        let subagent_handle = SubagentHandle {
            task_id: task.id.clone(),
            parent_session_id: parent_session_id.clone(),
            session_id: subagent_session.id.clone(),
            handle,
            cancel_tx,
        };

        self.active_subagents
            .insert(subagent_session.id.clone(), subagent_handle);

        Ok(subagent_session.id)
    }

    async fn run_subagent(
        session_id: String,
        parent_session_id: String,
        task_id: String,
        prompt: String,
        agent_type: String,
        storage: Arc<SqliteStorage>,
        event_bus: Arc<EventBus>,
        _session_manager: Arc<SessionManager>,
        config: Arc<tokio::sync::RwLock<Config>>,
        cancel_rx: &mut mpsc::Receiver<()>,
    ) -> Result<(String, String, i32)> {
        let cfg = config.read().await.clone();
        drop(config);

        let provider = Self::create_provider(&cfg, storage.clone()).await?;
        let tool_registry = Arc::new(ToolRegistry::new(cfg.clone(), storage.clone())?);
        let available_tool_names: HashSet<String> = tool_registry
            .get_tool_definitions()
            .into_iter()
            .map(|tool| tool.function.name)
            .collect();
        let allowed_tools: Vec<String> = Self::get_allowed_tools_for_agent_type(&agent_type)
            .into_iter()
            .filter(|tool| available_tool_names.contains(tool))
            .collect();
        let system_prompt = prompt::build_system_prompt(&allowed_tools, PromptMode::Minimal);

        if let Ok(Some(mut session)) = storage.get_session(&session_id) {
            session.messages.push(Message::system(system_prompt));
            session.messages.push(Message::user(prompt));
            let _ = storage.update_session(&session);
        }

        let max_iterations = 15;
        let mut tool_count = 0;

        for iteration in 0..max_iterations {
            let result = Self::run_iteration(
                session_id.clone(),
                parent_session_id.clone(),
                task_id.clone(),
                storage.clone(),
                event_bus.clone(),
                tool_registry.clone(),
                provider.clone(),
                allowed_tools.clone(),
                cancel_rx,
            )
            .await;

            match result {
                Ok((completed, count)) => {
                    tool_count += count;
                    if let Ok(Some(mut task)) = storage.get_subagent_task(&task_id) {
                        task.tool_count = tool_count;
                        let _ = storage.update_subagent_task(&task);
                    }

                    event_bus.emit(AgentEvent::SubagentProgress {
                        session_id: parent_session_id.clone(),
                        parent_session_id: parent_session_id.clone(),
                        subagent_session_id: session_id.clone(),
                        tool_name: format!("iteration_{}", iteration + 1),
                        tool_count,
                        status: if completed { "completed" } else { "running" }.to_string(),
                        timestamp: SystemTime::now(),
                    });

                    if completed {
                        let result_text = Self::extract_result(&storage, &session_id).await?;
                        return Ok(("completed".to_string(), result_text, tool_count));
                    }
                }
                Err(e) => {
                    error!("Subagent iteration error: {:?}", e);
                    return Err(e);
                }
            }
        }

        let result_text = Self::extract_result(&storage, &session_id).await?;
        Ok((
            "completed".to_string(),
            format!("{} (reached max iterations)", result_text),
            tool_count,
        ))
    }

    async fn create_provider(
        cfg: &Config,
        _storage: Arc<SqliteStorage>,
    ) -> Result<Arc<dyn Provider>> {
        let provider_config = if !cfg.default_provider.is_empty() {
            cfg.providers
                .iter()
                .find(|p| p.provider_type == cfg.default_provider)
                .cloned()
        } else {
            cfg.providers.first().cloned()
        };

        let provider_config = provider_config.unwrap_or_else(|| cfg.provider.clone());

        let mut config = provider_config.clone();
        if config.api_key.is_empty() {
            if let Some(key) =
                crate::agent::provider_presets::resolve_env_api_key(&config.provider_type)
            {
                config.api_key = key;
            }
        }

        let oauth_dir = PathBuf::from(shellexpand::tilde(&cfg.storage.database).to_string())
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        Ok(Arc::new(OpenAICompatibleProvider::with_catalog_and_oauth(
            config,
            None,
            Some(crate::oauth::create_oauth_storage(&oauth_dir)),
        )?))
    }

    async fn run_iteration(
        session_id: String,
        parent_session_id: String,
        _task_id: String,
        storage: Arc<SqliteStorage>,
        event_bus: Arc<EventBus>,
        tool_registry: Arc<ToolRegistry>,
        provider: Arc<dyn Provider>,
        allowed_tools: Vec<String>,
        cancel_rx: &mut mpsc::Receiver<()>,
    ) -> Result<(bool, i32)> {
        let session = storage
            .get_session(&session_id)?
            .ok_or_else(|| crate::error::OSAgentError::Session("Session not found".to_string()))?;

        let api_messages: Vec<Message> = session.messages.clone();

        let tools = tool_registry
            .get_tool_definitions()
            .into_iter()
            .filter(|t| allowed_tools.contains(&t.function.name))
            .collect::<Vec<_>>();

        let start = Instant::now();
        let response = tokio::select! {
            _ = cancel_rx.recv() => {
                return Ok((false, 0));
            }
            result = provider.complete(&api_messages, &tools) => {
                result.map_err(|e| crate::error::OSAgentError::Provider(e.to_string()))?
            }
        };

        info!(
            "Subagent LLM response in {:?}ms",
            start.elapsed().as_millis()
        );

        let mut tool_count = 0;
        let mut session = session;

        session.messages.push(Message::assistant(
            response.content.clone().unwrap_or_default(),
            response.tool_calls.clone(),
        ));

        let has_tool_calls =
            response.tool_calls.is_some() && !response.tool_calls.as_ref().unwrap().is_empty();

        if has_tool_calls {
            let tool_calls = response.tool_calls.unwrap();

            for tool_call in tool_calls {
                if !allowed_tools.contains(&tool_call.name) {
                    let error_msg = format!(
                        "Tool '{}' is not allowed for this subagent type",
                        tool_call.name
                    );
                    warn!("{}", error_msg);
                    session.messages.push(Message::tool_result(
                        tool_call.id.clone(),
                        format!("Error: {}", error_msg),
                    ));

                    let _ = storage.append_session_event(
                        &session_id,
                        "tool_start",
                        serde_json::json!({
                            "tool_call_id": tool_call.id,
                            "tool_name": tool_call.name,
                            "arguments": tool_call.arguments,
                            "message_index": 0,
                        }),
                    );
                    let _ = storage.append_session_event(
                        &session_id,
                        "tool_complete",
                        serde_json::json!({
                            "tool_call_id": tool_call.id,
                            "tool_name": tool_call.name,
                            "success": false,
                            "output": error_msg,
                        }),
                    );
                    continue;
                }

                let _ = storage.append_session_event(
                    &session_id,
                    "tool_start",
                    serde_json::json!({
                        "tool_call_id": tool_call.id,
                        "tool_name": tool_call.name,
                        "arguments": tool_call.arguments,
                        "message_index": 0,
                    }),
                );

                event_bus.emit(AgentEvent::SubagentProgress {
                    session_id: parent_session_id.clone(),
                    parent_session_id: parent_session_id.clone(),
                    subagent_session_id: session_id.clone(),
                    tool_name: tool_call.name.clone(),
                    tool_count,
                    status: "executing".to_string(),
                    timestamp: SystemTime::now(),
                });

                let start = Instant::now();
                let result = tool_registry
                    .execute(&tool_call.name, tool_call.arguments.clone())
                    .await;
                let duration_ms = start.elapsed().as_millis() as u64;

                match result {
                    Ok(output) => {
                        tool_count += 1;
                        info!(
                            "Subagent tool '{}' executed in {}ms",
                            tool_call.name, duration_ms
                        );
                        let truncated = truncate_tool_output(&tool_call.name, &output);
                        session
                            .messages
                            .push(Message::tool_result(tool_call.id.clone(), truncated));

                        let _ = storage.append_session_event(
                            &session_id,
                            "tool_complete",
                            serde_json::json!({
                                "tool_call_id": tool_call.id,
                                "tool_name": tool_call.name,
                                "success": true,
                                "output": output,
                            }),
                        );
                    }
                    Err(e) => {
                        let error_msg = format!("Error: {}", e);
                        error!("Subagent tool '{}' failed: {:?}", tool_call.name, e);
                        let truncated = truncate_tool_output(&tool_call.name, &error_msg);
                        session
                            .messages
                            .push(Message::tool_result(tool_call.id.clone(), truncated));

                        let _ = storage.append_session_event(
                            &session_id,
                            "tool_complete",
                            serde_json::json!({
                                "tool_call_id": tool_call.id,
                                "tool_name": tool_call.name,
                                "success": false,
                                "output": error_msg,
                            }),
                        );
                    }
                }

                let _ = storage.update_session(&session);
            }
        }

        let _ = storage.update_session(&session);

        let completed = !has_tool_calls || tool_count == 0;
        Ok((completed, tool_count))
    }

    async fn extract_result(storage: &Arc<SqliteStorage>, session_id: &str) -> Result<String> {
        if let Ok(Some(session)) = storage.get_session(session_id) {
            let assistant_messages: Vec<_> = session
                .messages
                .iter()
                .filter(|m| m.role == "assistant")
                .collect();

            if let Some(last) = assistant_messages.last() {
                return Ok(last.content.clone());
            }
        }
        Ok("No result available".to_string())
    }

    pub async fn cancel_subagent(&self, session_id: &str) -> Result<bool> {
        if let Some((_, handle)) = self.active_subagents.remove(session_id) {
            let _ = handle.cancel_tx.send(()).await;
            handle.handle.abort();

            if let Ok(Some(mut session)) = self.storage.get_session(session_id) {
                session.task_status = "cancelled".to_string();
                let _ = self.storage.update_session(&session);
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    pub async fn cancel_all_for_parent(&self, parent_session_id: &str) {
        let child_ids: Vec<String> = self
            .active_subagents
            .iter()
            .filter(|entry| entry.value().parent_session_id == parent_session_id)
            .map(|entry| entry.key().clone())
            .collect();

        let count = child_ids.len();
        for child_id in child_ids {
            if let Err(e) = self.cancel_subagent(&child_id).await {
                warn!(
                    "Failed to cancel child subagent {} for parent {}: {}",
                    child_id, parent_session_id, e
                );
            }
        }

        if count > 0 {
            info!(
                "Cancelled {} subagent(s) for parent session {}",
                count, parent_session_id
            );
        }
    }

    pub fn is_subagent_running(&self, session_id: &str) -> bool {
        self.active_subagents.contains_key(session_id)
    }

    pub fn is_any_running_for_parent(&self, parent_session_id: &str) -> bool {
        self.active_subagents
            .iter()
            .any(|entry| entry.value().parent_session_id == parent_session_id)
    }

    pub async fn wait_for_subagent(
        &self,
        session_id: &str,
        timeout_secs: u64,
    ) -> Result<(String, String)> {
        let start = Instant::now();
        let timeout = Duration::from_secs(timeout_secs);
        loop {
            if start.elapsed() > timeout {
                return Ok((
                    "timeout".to_string(),
                    format!("Subagent timed out after {}s", timeout_secs),
                ));
            }
            if !self.active_subagents.contains_key(session_id) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(200)).await;
        }

        if let Ok(Some(task)) = self.storage.get_subagent_task_by_session(session_id) {
            let result = task
                .result
                .unwrap_or_else(|| "No result available".to_string());
            Ok((task.status, result))
        } else {
            Ok(("unknown".to_string(), "Subagent task not found".to_string()))
        }
    }

    pub async fn cleanup_completed(&self, days: i64) -> Result<usize> {
        self.storage.cleanup_completed_subagents(days)
    }
}
