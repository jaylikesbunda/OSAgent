use crate::agent::events::{AgentEvent, EventBus};
use crate::agent::instruction::{
    format_system_reminder, git_workspace_context, global_instruction_blocks,
    workspace_instruction_blocks,
};
use crate::agent::prompt::{self, PromptMode};
use crate::agent::provider::{OpenAICompatibleProvider, Provider};
use crate::agent::session::SessionManager;
use crate::config::{Config, WorkspaceConfig};
use crate::error::Result;
use crate::storage::{
    CompactionStats, Message, MessageImage, MessageTokens, Session, SessionContextState,
    SqliteStorage, SubagentTask, ToolUsageStats,
};
use crate::tools::registry::ToolRegistry;
use chrono::Utc;
use dashmap::DashMap;
use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::sync::{mpsc, oneshot};
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

/// (status, result text, tool count)
pub type SubagentOutcome = (String, String, i32);

pub struct SubagentManager {
    storage: Arc<SqliteStorage>,
    event_bus: Arc<EventBus>,
    session_manager: Arc<SessionManager>,
    active_subagents: Arc<DashMap<String, SubagentHandle>>,
    /// Delivers each subagent's outcome straight to its waiter, so results do
    /// not have to be discovered by polling and re-read from storage.
    pending_results: Arc<DashMap<String, oneshot::Receiver<SubagentOutcome>>>,
    config: Arc<tokio::sync::RwLock<Config>>,
    shared_provider: Option<Arc<dyn Provider>>,
    workspace_root: PathBuf,
    /// Invoked with the parent session id whenever a *background* subagent
    /// reaches a terminal state. Set by the runtime after construction so the
    /// parent can be woken for a continuation turn without waiting for the
    /// user's next message. Kept in a OnceLock to avoid a constructor cycle.
    wake_callback: std::sync::OnceLock<Arc<dyn Fn(String) + Send + Sync>>,
}

struct SubagentHandle {
    task_id: String,
    parent_session_id: String,
    handle: JoinHandle<()>,
    cancel_tx: mpsc::Sender<()>,
}

impl SubagentManager {
    pub fn new(
        storage: Arc<SqliteStorage>,
        event_bus: Arc<EventBus>,
        session_manager: Arc<SessionManager>,
        config: Arc<tokio::sync::RwLock<Config>>,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            storage,
            event_bus,
            session_manager,
            active_subagents: Arc::new(DashMap::new()),
            pending_results: Arc::new(DashMap::new()),
            config,
            shared_provider: None,
            workspace_root,
            wake_callback: std::sync::OnceLock::new(),
        }
    }

    pub fn set_shared_provider(&mut self, provider: Arc<dyn Provider>) {
        self.shared_provider = Some(provider);
    }

    /// Register the callback fired when a background subagent reaches a
    /// terminal state (see [`SubagentManager::wake_callback`]).
    pub fn set_wake_callback(&self, callback: Arc<dyn Fn(String) + Send + Sync>) {
        let _ = self.wake_callback.set(callback);
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
            "skill_action",
            "lsp",
            "persona",
            "process",
            "todowrite",
            "todoread",
            "tool_search",
        ];

        let general_tools: HashSet<String> = all_tools.iter().map(|s| s.to_string()).collect();

        let explore_tools: HashSet<String> = [
            "read_file",
            "list_files",
            "grep",
            "glob",
            // Explore agents need shell access for read-only repository
            // inspection such as git status, diff, log, and show. Bash still
            // applies its command safety checks and blocks mutations.
            "bash",
            "web_fetch",
            "web_search",
            "reflect",
            "skill",
            "skill_list",
            "skill_action",
            "lsp",
            "tool_search",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let verify_tools: HashSet<String> = [
            "read_file",
            "list_files",
            "grep",
            "glob",
            "bash",
            "web_fetch",
            "web_search",
            "reflect",
            "lsp",
            "tool_search",
        ]
        .iter()
        .map(|s| s.to_string())
        .collect();

        let allowed = match agent_type {
            "general" => general_tools,
            "explore" => explore_tools,
            "verify" => verify_tools,
            _ => general_tools,
        };

        allowed.into_iter().collect()
    }
    /// Count how many subagent levels the given session sits below the main
    /// agent: 0 for a top-level session, 1 for a direct subagent, etc.
    fn subagent_depth_of(storage: &SqliteStorage, session_id: &str) -> usize {
        let mut depth = 0usize;
        let mut current = Some(session_id.to_string());
        while let Some(id) = current {
            match storage.get_session(&id) {
                Ok(Some(session)) => {
                    current = session.parent_id.clone();
                    if session.parent_id.is_some() {
                        depth += 1;
                    }
                }
                _ => break,
            }
        }
        depth
    }

    /// Wall-clock time the subagent task has been running, in milliseconds.
    fn task_duration_ms(storage: &Arc<SqliteStorage>, task_id: &str) -> u64 {
        storage
            .get_subagent_task(task_id)
            .ok()
            .flatten()
            .map(|task| (Utc::now() - task.created_at).num_milliseconds().max(0) as u64)
            .unwrap_or(0)
    }

    /// Exponential backoff for task-level retries: 30s, 60s, 120s... capped
    /// at 10 minutes.
    fn task_retry_delay_secs(attempt: u32) -> u64 {
        const BASE_SECS: u64 = 30;
        const CAP_SECS: u64 = 600;
        BASE_SECS
            .saturating_mul(2u64.saturating_pow(attempt.saturating_sub(1).min(8)))
            .min(CAP_SECS)
    }

    /// Spawn a brand-new subagent session and run it.
    ///
    /// When `background` is true the call returns as soon as the subagent is
    /// launched; its result is injected into the parent session as a
    /// synthetic message when it finishes, so the parent can react on its
    /// next turn. Otherwise the caller is expected to follow up with
    /// [`Self::wait_for_subagent`].
    pub async fn spawn_subagent(
        &self,
        parent_session_id: String,
        description: String,
        prompt: String,
        agent_type: String,
        background: bool,
    ) -> Result<String> {
        self.check_depth_limit(&parent_session_id).await?;

        let parent_session = self
            .session_manager
            .get_session(&parent_session_id)
            .await?
            .ok_or_else(|| {
                crate::error::OSAgentError::ToolExecution("Parent session not found".to_string())
            })?;

        let parent_workspace = {
            let cfg = self.config.read().await.clone();
            parent_session
                .metadata
                .get("workspace_id")
                .and_then(|value| value.as_str())
                .and_then(|workspace_id| cfg.get_workspace(workspace_id))
                .unwrap_or_else(|| cfg.get_active_workspace())
        };

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
        subagent_session.metadata["workspace_id"] = serde_json::json!(parent_workspace.id.clone());
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
            notified_at: None,
            background,
        };

        self.storage.create_subagent_task(&task)?;

        self.event_bus.emit(AgentEvent::SubagentCreated {
            session_id: parent_session_id.clone(),
            sequence: 0,
            parent_session_id: parent_session_id.clone(),
            subagent_session_id: subagent_session.id.clone(),
            description: description.clone(),
            prompt: prompt.clone(),
            agent_type: agent_type.clone(),
            timestamp: std::time::SystemTime::now(),
        });

        self.launch_run(
            parent_session_id,
            subagent_session.id.clone(),
            task,
            description,
            prompt,
            agent_type,
            background,
            true,
            parent_workspace,
        )?;

        Ok(subagent_session.id)
    }

    /// Resume a previously-completed subagent session by re-using its session
    /// (and accumulated message history) for a fresh prompt. Returns the
    /// session id. Works in foreground or background mode.
    pub async fn resume_subagent(
        &self,
        parent_session_id: String,
        session_id: String,
        description: String,
        prompt: String,
        agent_type: String,
        background: bool,
    ) -> Result<String> {
        self.check_depth_limit(&parent_session_id).await?;

        let subagent_session = self
            .session_manager
            .get_session(&session_id)
            .await?
            .ok_or_else(|| {
                crate::error::OSAgentError::ToolExecution(
                    "Subagent session to resume not found".to_string(),
                )
            })?;

        if subagent_session.parent_id.as_deref() != Some(parent_session_id.as_str()) {
            return Err(crate::error::OSAgentError::ToolExecution(
                "Cannot resume subagent: session does not belong to this parent".to_string(),
            ));
        }

        if self.active_subagents.contains_key(&session_id) {
            return Err(crate::error::OSAgentError::ToolExecution(
                "Cannot resume subagent: session is already running".to_string(),
            ));
        }

        let parent_workspace = {
            let cfg = self.config.read().await.clone();
            subagent_session
                .metadata
                .get("workspace_id")
                .and_then(|value| value.as_str())
                .and_then(|workspace_id| cfg.get_workspace(workspace_id))
                .unwrap_or_else(|| cfg.get_active_workspace())
        };

        let task = SubagentTask {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.clone(),
            parent_session_id: parent_session_id.clone(),
            description: description.clone(),
            prompt: prompt.clone(),
            agent_type: agent_type.clone(),
            status: "running".to_string(),
            tool_count: 0,
            result: None,
            created_at: Utc::now(),
            completed_at: None,
            notified_at: None,
            background,
        };

        self.storage.create_subagent_task(&task)?;

        self.event_bus.emit(AgentEvent::SubagentCreated {
            session_id: parent_session_id.clone(),
            sequence: 0,
            parent_session_id: parent_session_id.clone(),
            subagent_session_id: session_id.clone(),
            description: description.clone(),
            prompt: prompt.clone(),
            agent_type: agent_type.clone(),
            timestamp: std::time::SystemTime::now(),
        });

        self.launch_run(
            parent_session_id,
            session_id.clone(),
            task,
            description,
            prompt,
            agent_type,
            background,
            false,
            parent_workspace,
        )?;

        Ok(session_id)
    }

    async fn check_depth_limit(&self, parent_session_id: &str) -> Result<()> {
        let depth_limit = self.config.read().await.agent.subagent_depth;
        let current_depth = Self::subagent_depth_of(&self.storage, parent_session_id);
        if current_depth >= depth_limit {
            return Err(crate::error::OSAgentError::ToolExecution(format!(
                "Subagent depth limit reached ({}). Increase 'subagent_depth' in the agent config to allow nested subagents.",
                depth_limit
            )));
        }
        Ok(())
    }

    /// Spawn the async runner for a subagent (fresh or resumed) and register
    /// it in the active-subagent table. Returns the task id.
    #[allow(clippy::too_many_arguments)]
    fn launch_run(
        &self,
        parent_session_id: String,
        subagent_session_id: String,
        task: SubagentTask,
        description: String,
        prompt: String,
        agent_type: String,
        background: bool,
        fresh_session: bool,
        parent_workspace: WorkspaceConfig,
    ) -> Result<String> {
        let task_id = task.id.clone();

        let (cancel_tx, mut cancel_rx) = mpsc::channel(1);
        let storage = self.storage.clone();
        let event_bus = self.event_bus.clone();
        let session_manager = self.session_manager.clone();
        let config = self.config.clone();
        let subagent_session_id = subagent_session_id.clone();
        let task_id = task.id.clone();
        let subagent_session_id_for_insert = subagent_session_id.clone();
        let task_id_for_return = task.id.clone();
        let parent_session_id_for_async = parent_session_id.clone();
        let active_subagents = self.active_subagents.clone();
        let shared_provider = self.shared_provider.clone();
        let workspace_root = self.workspace_root.clone();
        let parent_workspace_for_async = parent_workspace.clone();

        // Background runs have no waiter, so no oneshot channel is needed.
        let (result_tx, result_rx) = if background {
            (None, None)
        } else {
            let (tx, rx) = oneshot::channel::<SubagentOutcome>();
            (Some(tx), Some(rx))
        };
        if let Some(rx) = result_rx {
            self.pending_results.insert(subagent_session_id.clone(), rx);
        }
        let storage_for_guard = self.storage.clone();
        let guard_task_id = task.id.clone();
        let background_for_notify = background;
        let description_for_notify = description.clone();
        let wake_callback = self.wake_callback.get().cloned();

        let handle = tokio::spawn(async move {
            struct CleanupGuard {
                session_id: String,
                task_id: String,
                active_subagents: Arc<DashMap<String, SubagentHandle>>,
                storage: Arc<SqliteStorage>,
                result_tx: Option<oneshot::Sender<SubagentOutcome>>,
            }

            impl CleanupGuard {
                /// Hand the outcome to the waiter once the normal path has
                /// already persisted it.
                fn deliver(&mut self, outcome: SubagentOutcome) {
                    if let Some(tx) = self.result_tx.take() {
                        let _ = tx.send(outcome);
                    }
                }
            }

            impl Drop for CleanupGuard {
                fn drop(&mut self) {
                    // If the run was aborted (cancel/timeout) the normal path
                    // never delivered, so the waiter would hang until its own
                    // timeout. Fall back to whatever storage knows.
                    if self.result_tx.is_some() {
                        let outcome = match self.storage.get_subagent_task(&self.task_id) {
                            Ok(Some(task)) if task.completed_at.is_some() => (
                                task.status.clone(),
                                task.result.clone().unwrap_or_default(),
                                task.tool_count,
                            ),
                            Ok(Some(mut task)) => {
                                task.status = "failed".to_string();
                                task.result = Some(
                                    "Subagent terminated before returning a result".to_string(),
                                );
                                task.completed_at = Some(Utc::now());
                                let _ = self.storage.update_subagent_task(&task);
                                (
                                    "failed".to_string(),
                                    "Subagent terminated before returning a result".to_string(),
                                    task.tool_count,
                                )
                            }
                            _ => (
                                "failed".to_string(),
                                "Subagent terminated before returning a result".to_string(),
                                0,
                            ),
                        };
                        self.deliver(outcome);
                    }
                    self.active_subagents.remove(&self.session_id);
                }
            }

            let mut _cleanup = CleanupGuard {
                session_id: subagent_session_id.clone(),
                task_id: guard_task_id,
                active_subagents: active_subagents.clone(),
                storage: storage_for_guard,
                result_tx,
            };

            // Task-level retry: when a run dies on what looks like a transient
            // failure (provider outage, rate limit, timeout...), relaunch it
            // instead of discarding every completed iteration. Retries resume
            // the same child session, so prior work stays in context.
            // User-initiated cancellation is never retried.
            let max_task_retries = config.read().await.agent.subagent_task_max_retries;
            let mut task_attempt: u32 = 0;
            let result = loop {
                let resumed_run = task_attempt > 0;
                let outcome = Self::run_subagent(
                    subagent_session_id.clone(),
                    parent_session_id_for_async.clone(),
                    task_id.clone(),
                    prompt.clone(),
                    agent_type.clone(),
                    storage.clone(),
                    event_bus.clone(),
                    session_manager.clone(),
                    config.clone(),
                    shared_provider.clone(),
                    parent_workspace_for_async.clone(),
                    workspace_root.clone(),
                    fresh_session,
                    resumed_run,
                    &mut cancel_rx,
                )
                .await;

                match outcome {
                    Ok(value) => break Ok(value),
                    Err(e) => {
                        task_attempt += 1;
                        if e.is_retryable() && task_attempt <= max_task_retries {
                            let delay =
                                Duration::from_secs(Self::task_retry_delay_secs(task_attempt));
                            warn!(
                                "Subagent {} hit a retryable error ({}/{}): {} — resuming in {}s",
                                subagent_session_id,
                                task_attempt,
                                max_task_retries,
                                e,
                                delay.as_secs()
                            );
                            event_bus.emit(AgentEvent::SubagentRetrying {
                                session_id: parent_session_id_for_async.clone(),
                                sequence: 0,
                                parent_session_id: parent_session_id_for_async.clone(),
                                subagent_session_id: subagent_session_id.clone(),
                                attempt_count: task_attempt,
                                max_attempts: max_task_retries,
                                next_retry_in_ms: delay.as_millis() as u64,
                                reason: e.to_string(),
                                timestamp: std::time::SystemTime::now(),
                            });
                            tokio::select! {
                                _ = tokio::time::sleep(delay) => continue,
                                _ = cancel_rx.recv() => {
                                    let tools = storage
                                        .get_subagent_task(&task_id)
                                        .ok()
                                        .flatten()
                                        .map(|task| task.tool_count)
                                        .unwrap_or(0);
                                    break Ok((
                                        "cancelled".to_string(),
                                        "Subagent cancelled while waiting to retry.".to_string(),
                                        tools,
                                    ));
                                }
                            }
                        }
                        break Err(e);
                    }
                }
            };

            match result {
                Ok((status, result_text, tool_count)) => {
                    let duration_ms = Self::task_duration_ms(&storage, &task_id);
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
                        sequence: 0,
                        parent_session_id: parent_session_id_for_async.clone(),
                        subagent_session_id: subagent_session_id.clone(),
                        status: status.clone(),
                        result: result_text.clone(),
                        tool_count,
                        duration_ms,
                        background: background_for_notify,
                        description: description_for_notify.clone(),
                        timestamp: std::time::SystemTime::now(),
                    });

                    _cleanup.deliver((status, result_text, tool_count));

                    // Background tasks have no waiter: poke the runtime so the
                    // parent session can be woken for a continuation turn.
                    if background_for_notify {
                        if let Some(callback) = &wake_callback {
                            callback(parent_session_id_for_async.clone());
                        }
                    }
                }
                Err(e) => {
                    error!("Subagent failed: {:?}", e);
                    let duration_ms = Self::task_duration_ms(&storage, &task_id);
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
                        sequence: 0,
                        parent_session_id: parent_session_id_for_async.clone(),
                        subagent_session_id: subagent_session_id.clone(),
                        status: "failed".to_string(),
                        result: format!("Error: {}", e),
                        tool_count: 0,
                        duration_ms,
                        background: background_for_notify,
                        description: description_for_notify.clone(),
                        timestamp: std::time::SystemTime::now(),
                    });

                    _cleanup.deliver(("failed".to_string(), format!("Error: {}", e), 0));

                    if background_for_notify {
                        if let Some(callback) = &wake_callback {
                            callback(parent_session_id_for_async.clone());
                        }
                    }
                }
            }
        });

        let subagent_handle = SubagentHandle {
            task_id: task.id.clone(),
            parent_session_id: parent_session_id.clone(),
            handle,
            cancel_tx,
        };

        self.active_subagents
            .insert(subagent_session_id_for_insert, subagent_handle);

        Ok(task_id_for_return)
    }

    #[allow(clippy::too_many_arguments)]
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
        shared_provider: Option<Arc<dyn Provider>>,
        parent_workspace: WorkspaceConfig,
        workspace_root: PathBuf,
        fresh_session: bool,
        resumed_run: bool,
        cancel_rx: &mut mpsc::Receiver<()>,
    ) -> Result<(String, String, i32)> {
        let mut cfg = config.read().await.clone();
        drop(config);

        if !parent_workspace.resolved_path().trim().is_empty() {
            let parent_workspace_id = parent_workspace.id.clone();
            cfg.agent.active_workspace = Some(parent_workspace_id.clone());
            cfg.agent.workspace = parent_workspace.resolved_path();
            if let Some(existing) = cfg
                .agent
                .workspaces
                .iter_mut()
                .find(|workspace| workspace.id == parent_workspace_id)
            {
                *existing = parent_workspace.clone();
            } else {
                cfg.agent.workspaces.push(parent_workspace.clone());
            }
            cfg.ensure_workspace_defaults();
        } else if cfg.agent.workspace.trim().is_empty() {
            cfg.agent.workspace = workspace_root.to_string_lossy().to_string();
            cfg.ensure_workspace_defaults();
        }

        // Subagents can pin their own model per agent type via
        // `agent.subagent_models` (e.g. "explore" => "openrouter:deepseek/deepseek-r1").
        // Overrides always use a dedicated provider instance so the shared
        // provider used by the main agent is never mutated.
        let model_override = cfg.agent.subagent_models.get(&agent_type).cloned();
        let provider = if let Some(override_spec) = model_override {
            Self::create_provider_with_override(&cfg, storage.clone(), override_spec).await?
        } else if let Some(shared) = shared_provider {
            shared
        } else {
            Self::create_provider(&cfg, storage.clone()).await?
        };
        let tool_registry = Arc::new(ToolRegistry::with_deps(
            cfg.clone(),
            storage.clone(),
            Some(event_bus.clone()),
            None,
            None,
        )?);
        // Keep deferred built-ins (code_python, lsp, skill_action, ...) in
        // the allowed set even though they are not loaded yet: the subagent
        // can activate them via tool_search, and the shared registry's
        // activation makes them appear in later iterations. Filter only
        // against tools that actually exist in this build.
        let allowed_tools: Vec<String> = Self::get_allowed_tools_for_agent_type(&agent_type)
            .into_iter()
            .filter(|tool| tool_registry.has_tool(tool))
            .collect();
        // Subagents use default identity and priorities (no custom sections)
        let prompt_mode = if agent_type == "explore" {
            PromptMode::Explore
        } else {
            PromptMode::Minimal
        };
        let system_prompt = prompt::build_system_prompt(&allowed_tools, prompt_mode, None, None);
        let active_root =
            PathBuf::from(shellexpand::tilde(&parent_workspace.resolved_path()).to_string());
        let global_instructions =
            format_system_reminder(&global_instruction_blocks(&cfg.config_dir()));
        let workspace_instructions =
            format_system_reminder(&workspace_instruction_blocks(&active_root));
        let git_context = git_workspace_context(&active_root).await;
        let is_git_repo = active_root.join(".git").is_dir() || git_context.is_some();
        let skill_summary = tool_registry.skill_summary_prompt();
        let native_manifest = tool_registry.native_manifest_prompt();
        let mcp_manifest = if cfg.mcp.manifest_in_prompt {
            tool_registry.mcp_manifest_prompt()
        } else {
            None
        };
        let provider_type = provider.provider_type().to_string();
        let model = provider.current_model().await;
        let workspace_paths = parent_workspace
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
        let environment_prompt = format!(
            "# Runtime Environment\n- Model: {} / {}\n- Workspace: {} ({})\n- Working directory: {}\n- Is directory a git repo: {}\n- Platform: {}\n# Workspace Roots\n{}",
            provider_type,
            model,
            parent_workspace.name,
            parent_workspace.id,
            parent_workspace.resolved_path(),
            if is_git_repo { "yes" } else { "no" },
            std::env::consts::OS,
            workspace_paths
        );

        if let Ok(Some(mut session)) = storage.get_session(&session_id) {
            if fresh_session {
                session.messages.push(Message::system(system_prompt));
                session.messages.push(Message::system(environment_prompt));
                if let Some(git_context) = git_context {
                    session.messages.push(Message::system(git_context));
                }
                if let Some(global_instructions) = global_instructions {
                    session.messages.push(Message::system(global_instructions));
                }
                if let Some(workspace_instructions) = workspace_instructions {
                    session
                        .messages
                        .push(Message::system(workspace_instructions));
                }
                if let Some(skill_summary) = skill_summary {
                    session.messages.push(Message::system(skill_summary));
                }
                if let Some(manifest) = native_manifest {
                    session.messages.push(Message::system(manifest));
                }
                if let Some(manifest) = mcp_manifest {
                    session.messages.push(Message::system(manifest));
                }
            }
            // A resumed run (task-level retry) already has the original task
            // and all prior work in context; nudge it to continue instead of
            // re-sending the full prompt.
            let effective_prompt = if resumed_run {
                format!(
                    "Your previous run was interrupted by a temporary provider error before you could finish. Continue from where you left off and produce your final answer.\n\nOriginal task:\n{}",
                    prompt
                )
            } else {
                prompt
            };
            session.messages.push(Message::user(effective_prompt));
            session.model = model.clone();
            session.provider = provider_type.clone();
            let _ = storage.update_session(&session);
        }

        let max_iterations = if agent_type == "explore" { 100 } else { 50 };
        let budget_warning_at = (max_iterations as f64 * 0.75) as usize;
        let mut budget_warned = false;
        let mut tool_count = 0;

        for iteration in 0..max_iterations {
            if !budget_warned && iteration >= budget_warning_at {
                budget_warned = true;
                if let Ok(Some(mut session)) = storage.get_session(&session_id) {
                    session.messages.push(Message::synthetic_user(
                        "You are approaching your iteration limit. Stop exploring now and synthesize all your findings into a comprehensive final summary. Do not make any more tool calls.".to_string(),
                        "budget_warning",
                    ));
                    let _ = storage.update_session(&session);
                }
            }

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
                        sequence: 0,
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
                    if e.to_string().contains("Subagent cancelled") {
                        let result_text = Self::extract_result(&storage, &session_id).await?;
                        return Ok(("cancelled".to_string(), result_text, tool_count));
                    }
                    error!("Subagent iteration error: {:?}", e);
                    return Err(e);
                }
            }
        }

        // The iteration budget ran out without a final no-tool-call response:
        // report an honest `partial` status (with the best available text) so
        // the parent knows the task can be resumed via `task_id`.
        let result_text = Self::extract_result(&storage, &session_id).await?;
        Ok((
            "partial".to_string(),
            format!(
                "{}\n\n(Note: the subagent hit its iteration budget before finishing; this is a partial result. Resume it by passing task_id=\"{}\".)",
                result_text, task_id
            ),
            tool_count,
        ))
    }

    /// Build a dedicated provider for a subagent from `agent.subagent_models`,
    /// which maps an agent type to either `"provider_id:model"` or `"model"` (the
    /// latter uses the default provider). A fresh instance is always created so
    /// the shared provider used by the main agent is never mutated.
    async fn create_provider_with_override(
        cfg: &Config,
        storage: Arc<SqliteStorage>,
        spec: String,
    ) -> Result<Arc<dyn Provider>> {
        let (provider_id, model) = match spec.split_once(':') {
            Some((id, model)) => {
                let id = id.trim();
                let model = model.trim();
                if id.is_empty() {
                    (None, model.to_string())
                } else if model.is_empty() {
                    return Err(crate::error::OSAgentError::ToolExecution(
                        "subagent_models entry with a provider id must also specify a model: \"provider_id:model\"".to_string(),
                    ));
                } else {
                    (Some(id.to_string()), model.to_string())
                }
            }
            None => (None, spec.trim().to_string()),
        };

        let selected = if let Some(id) = &provider_id {
            cfg.providers
                .iter()
                .find(|p| p.provider_type == *id)
                .cloned()
        } else if !cfg.default_provider.is_empty() {
            cfg.providers
                .iter()
                .find(|p| p.provider_type == cfg.default_provider)
                .cloned()
        } else {
            cfg.providers.first().cloned()
        };
        let selected = selected.unwrap_or_else(|| cfg.provider.clone());

        let mut config = selected.clone();
        if model.is_empty() {
            return Err(crate::error::OSAgentError::ToolExecution(
                "subagent_models entry must be \"provider_id:model\" or a model name".to_string(),
            ));
        }
        config.model = model;
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

        Ok(Arc::new(
            OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
                config,
                None,
                Some(crate::oauth::create_oauth_storage(&oauth_dir)),
                Arc::new(tokio::sync::RwLock::new(cfg.agent.clone())),
            )?,
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

        Ok(Arc::new(
            OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
                config,
                None,
                Some(crate::oauth::create_oauth_storage(&oauth_dir)),
                Arc::new(tokio::sync::RwLock::new(cfg.agent.clone())),
            )?,
        ))
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
        let mut session = storage
            .get_session(&session_id)?
            .ok_or_else(|| crate::error::OSAgentError::Session("Session not found".to_string()))?;

        let api_messages: Vec<Message> = session.messages.clone();

        let tools = tool_registry
            .get_tool_definitions(&session_id)
            .into_iter()
            .filter(|t| allowed_tools.contains(&t.function.name))
            .collect::<Vec<_>>();

        let tool_schema_tokens = serde_json::to_string(&tools)
            .map(|schemas| schemas.chars().count().div_ceil(4))
            .unwrap_or(0);
        Self::update_context_tracking(
            &mut session,
            &session_id,
            &parent_session_id,
            &provider,
            &event_bus,
            &storage,
            tool_schema_tokens,
        )
        .await?;

        let start = Instant::now();
        let response = tokio::select! {
            _ = cancel_rx.recv() => {
                return Err(crate::error::OSAgentError::ToolExecution("Subagent cancelled".to_string()));
            }
            result = provider.complete(Some(&session_id), &api_messages, &tools) => {
                result.map_err(|e| crate::error::OSAgentError::Provider(e.to_string()))?
            }
        };

        info!(
            "Subagent LLM response in {:?}ms",
            start.elapsed().as_millis()
        );

        let mut tool_count = 0;

        session.messages.push(Message::assistant(
            response.content.clone().unwrap_or_default(),
            response.tool_calls.clone(),
        ));

        if let Some(ref usage) = response.usage {
            if let Some(ref mut cs) = session.context_state {
                cs.actual_usage = Some(MessageTokens {
                    input: usage.input,
                    output: usage.output,
                    total: usage.total,
                    cached_read: usage.cached_read,
                    cached_write: usage.cached_write,
                    reasoning: usage.reasoning,
                    cache_reason: None,
                });
            }
        }

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
                    sequence: 0,
                    parent_session_id: parent_session_id.clone(),
                    subagent_session_id: session_id.clone(),
                    tool_name: tool_call.name.clone(),
                    tool_count,
                    status: "executing".to_string(),
                    timestamp: SystemTime::now(),
                });

                let start = Instant::now();
                let mut tool_args = tool_call.arguments.clone();
                // Every tool call carries the subagent's session id so
                // session-scoped machinery (tool_search activation, MCP
                // auto-activation, todos) finds its session.
                tool_args["session_id"] = serde_json::json!(session_id.clone());
                let result = tool_registry
                    .execute_result(&tool_call.name, tool_args)
                    .await;
                let duration_ms = start.elapsed().as_millis() as u64;

                match result {
                    Ok(output) => {
                        tool_count += 1;
                        info!(
                            "Subagent tool '{}' executed in {}ms",
                            tool_call.name, duration_ms
                        );
                        let truncated = truncate_tool_output(&tool_call.name, &output.output);
                        session
                            .messages
                            .push(Message::tool_result(tool_call.id.clone(), truncated));
                        if let Some(last) = session.messages.last_mut() {
                            for attachment in &output.attachments {
                                last.images.push(MessageImage {
                                    filename: attachment.filename.clone(),
                                    mime: attachment.mime.clone(),
                                    data_url: attachment.data_url.clone(),
                                });
                            }
                        }

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

                        event_bus.emit(AgentEvent::SubagentProgress {
                            session_id: parent_session_id.clone(),
                            sequence: 0,
                            parent_session_id: parent_session_id.clone(),
                            subagent_session_id: session_id.clone(),
                            tool_name: tool_call.name.clone(),
                            tool_count,
                            status: "completed".to_string(),
                            timestamp: SystemTime::now(),
                        });
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

                        event_bus.emit(AgentEvent::SubagentProgress {
                            session_id: parent_session_id.clone(),
                            sequence: 0,
                            parent_session_id: parent_session_id.clone(),
                            subagent_session_id: session_id.clone(),
                            tool_name: tool_call.name.clone(),
                            tool_count,
                            status: "failed".to_string(),
                            timestamp: SystemTime::now(),
                        });
                    }
                }

                let _ = storage.update_session(&session);
            }
        }

        let _ = storage.update_session(&session);

        let completed = !has_tool_calls;
        Ok((completed, tool_count))
    }

    async fn extract_result(storage: &Arc<SqliteStorage>, session_id: &str) -> Result<String> {
        if let Ok(Some(session)) = storage.get_session(session_id) {
            // First: find the last assistant message with no tool calls and non-empty content
            let final_message = session.messages.iter().rev().find(|message| {
                message.role == "assistant"
                    && !message.content.trim().is_empty()
                    && message
                        .tool_calls
                        .as_ref()
                        .map(|calls| calls.is_empty())
                        .unwrap_or(true)
                    && !Self::looks_like_internal_tool_dump(&message.content)
            });

            if let Some(message) = final_message {
                return Ok(message.content.clone());
            }

            // Second: find the last assistant message with non-empty content even if it has tool calls
            let fallback_with_tools = session.messages.iter().rev().find(|message| {
                message.role == "assistant"
                    && !message.content.trim().is_empty()
                    && !Self::looks_like_internal_tool_dump(&message.content)
            });

            if let Some(message) = fallback_with_tools {
                return Ok(message.content.clone());
            }

            // Third: synthesize from tool results — collect the last few non-empty tool outputs
            let tool_results: Vec<String> = session
                .messages
                .iter()
                .rev()
                .filter(|m| m.role == "tool" && !m.content.trim().is_empty())
                .take(5)
                .map(|m| m.content.clone())
                .collect();

            if !tool_results.is_empty() {
                return Ok(format!(
                    "Subagent completed with tool results but no final summary.\n\nRecent findings:\n{}",
                    tool_results.join("\n---\n")
                ));
            }
        }
        Ok("No result available".to_string())
    }

    fn is_synthetic_message(message: &Message) -> bool {
        message
            .metadata
            .get("synthetic")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    fn looks_like_internal_tool_dump(content: &str) -> bool {
        let trimmed = content.trim();
        let lower = trimmed.to_lowercase();

        trimmed.starts_with("OLCALL>")
            || lower.starts_with("tool_calls")
            || (trimmed.starts_with('[')
                && lower.contains("\"name\"")
                && lower.contains("\"arguments\""))
            || (trimmed.starts_with('{')
                && lower.contains("\"name\"")
                && lower.contains("\"arguments\""))
    }

    pub async fn cancel_subagent(&self, session_id: &str) -> Result<bool> {
        self.stop_subagent(session_id, "cancelled", "Subagent cancelled".to_string())
            .await
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

    /// Return the persisted task record and live-running state for a child
    /// session. This is intentionally read-only so the parent agent can check
    /// on a background task before deciding whether to resume it.
    pub fn get_subagent_status(&self, session_id: &str) -> Result<Option<(SubagentTask, bool)>> {
        let task = self.storage.get_subagent_task_by_session(session_id)?;
        Ok(task.map(|task| {
            let running = self.active_subagents.contains_key(session_id);
            (task, running)
        }))
    }

    pub async fn wait_for_subagent(
        &self,
        session_id: &str,
        timeout_secs: u64,
    ) -> Result<(String, String, i32)> {
        // Wait on the subagent's own completion signal rather than polling for
        // its disappearance and re-reading storage, which added latency and
        // could observe a not-yet-written result.
        if let Some((_, receiver)) = self.pending_results.remove(session_id) {
            match tokio::time::timeout(Duration::from_secs(timeout_secs), receiver).await {
                Ok(Ok(outcome)) => return Ok(outcome),
                // Sender dropped without delivering: fall through to storage.
                Ok(Err(_)) => {}
                Err(_) => {
                    // Preserve the work already written to the child session.
                    // A timeout is an interruption, not a reason to replace
                    // the useful partial result with a bare error string.
                    let partial_result = Self::extract_result(&self.storage, session_id)
                        .await
                        .unwrap_or_else(|_| "No partial result available".to_string());
                    let tool_count = self
                        .storage
                        .get_subagent_task_by_session(session_id)
                        .ok()
                        .flatten()
                        .map(|task| task.tool_count)
                        .unwrap_or(0);
                    let timeout_message = format!(
                        "Subagent timed out after {}s. The session is resumable with task_id=\"{}\".\n\nPartial result:\n{}",
                        timeout_secs, session_id, partial_result
                    );
                    let _ = self
                        .stop_subagent(session_id, "timeout", timeout_message.clone())
                        .await;
                    return Ok(("timeout".to_string(), timeout_message, tool_count));
                }
            }
        }

        if let Ok(Some(task)) = self.storage.get_subagent_task_by_session(session_id) {
            let result = task
                .result
                .unwrap_or_else(|| "No result available".to_string());
            Ok((task.status, result, task.tool_count))
        } else {
            Ok((
                "unknown".to_string(),
                "Subagent task not found".to_string(),
                0,
            ))
        }
    }

    async fn stop_subagent(&self, session_id: &str, status: &str, result: String) -> Result<bool> {
        if let Some((_, handle)) = self.active_subagents.remove(session_id) {
            let SubagentHandle {
                task_id,
                handle,
                cancel_tx,
                ..
            } = handle;

            let _ = cancel_tx.send(()).await;
            handle.abort();
            let _ = handle.await;

            if let Ok(Some(mut task)) = self.storage.get_subagent_task(&task_id) {
                task.status = status.to_string();
                task.result = Some(result.clone());
                task.completed_at = Some(Utc::now());
                let _ = self.storage.update_subagent_task(&task);
            }

            if let Ok(Some(mut session)) = self.storage.get_session(session_id) {
                session.task_status = status.to_string();
                let _ = self.storage.update_session(&session);
            }

            Ok(true)
        } else {
            Ok(false)
        }
    }

    fn estimate_tokens(text: &str) -> usize {
        let chars = text.chars().count();
        (chars / 4).max(1)
    }

    fn message_tokens(message: &Message) -> usize {
        let mut total = Self::estimate_tokens(&message.content) + 8;
        if let Some(thinking) = &message.thinking {
            total += Self::estimate_tokens(thinking);
        }
        if let Some(tool_calls) = &message.tool_calls {
            for call in tool_calls {
                total += Self::estimate_tokens(&call.name);
                total += Self::estimate_tokens(&call.arguments.to_string());
            }
        }
        if let Some(tool_call_id) = &message.tool_call_id {
            total += Self::estimate_tokens(tool_call_id);
        }
        total
    }

    async fn update_context_tracking(
        session: &mut Session,
        session_id: &str,
        parent_session_id: &str,
        provider: &Arc<dyn Provider>,
        event_bus: &Arc<EventBus>,
        storage: &Arc<SqliteStorage>,
        tool_schema_tokens: usize,
    ) -> Result<()> {
        let context_window = provider.model_context_window().await;
        if let Some(window) = context_window {
            let estimated_tokens: usize = session
                .messages
                .iter()
                .map(Self::message_tokens)
                .sum::<usize>()
                .saturating_add(tool_schema_tokens);
            let output_limit = 8192usize;
            let reserved_output = std::cmp::min(output_limit, 8192);
            let usable = window.saturating_sub(reserved_output);
            let budget = ((usable as f32) * 0.8) as usize;

            session.context_state = Some(SessionContextState {
                estimated_tokens,
                context_window: window,
                budget_tokens: budget,
                actual_usage: None,
                cache_provider: None,
                cache_model: None,
                cache_tools_fingerprint: None,
                tool_usage: Vec::new(),
                compaction_stats: CompactionStats::default(),
            });
            let _ = storage.update_session(session);

            event_bus.emit(AgentEvent::ContextUpdate {
                // Subagent lifecycle events are published on the parent
                // stream. Do the same for context updates so the parent's
                // subagent card receives live ring updates.
                session_id: parent_session_id.to_string(),
                sequence: 0,
                context_window: window,
                estimated_tokens,
                budget_tokens: budget,
                tool_schema_tokens,
                condensed: false,
                actual_usage: None,
                subagent_session_id: Some(session_id.to_string()),
                timestamp: SystemTime::now(),
            });
        }
        Ok(())
    }

    pub async fn cleanup_completed(&self, days: i64) -> Result<usize> {
        self.storage.cleanup_completed_subagents(days)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_retry_backoff_is_exponential_and_capped() {
        assert_eq!(SubagentManager::task_retry_delay_secs(1), 30);
        assert_eq!(SubagentManager::task_retry_delay_secs(2), 60);
        assert_eq!(SubagentManager::task_retry_delay_secs(3), 120);
        // Very large attempts saturate at the 10 minute cap instead of
        // overflowing or growing without bound.
        assert_eq!(SubagentManager::task_retry_delay_secs(12), 600);
        assert_eq!(SubagentManager::task_retry_delay_secs(u32::MAX), 600);
    }

    #[test]
    fn explore_agents_can_use_read_only_bash_for_repository_inspection() {
        let tools = SubagentManager::get_allowed_tools_for_agent_type("explore");
        assert!(tools.iter().any(|tool| tool == "bash"));
    }
}
