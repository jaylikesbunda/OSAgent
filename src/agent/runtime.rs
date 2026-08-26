use crate::agent::checkpoint::CheckpointManager;
use crate::agent::coordinator::Coordinator;
use crate::agent::decision_memory::{DecisionCaptureOutcome, DecisionMemory, DecisionSuggestion};
use crate::agent::events::{AgentEvent, EventBus, EventTokenUsage, ToolStatus};
use crate::agent::instruction::{
    format_system_reminder, git_workspace_context, global_instruction_blocks,
    workspace_instruction_blocks,
};
use crate::agent::memory::{
    MemoryCategory, MemoryEntry, MemoryStatus, MemoryStore, MemorySuggestion,
};
use crate::agent::model_catalog::ModelCatalog;
use crate::agent::persona::{self, ActivePersona};
use crate::agent::prompt::{self, PromptMode};
use crate::agent::provider::{
    OpenAICompatibleProvider, Provider, RetryListener, StreamEvent, MAX_RETRIES,
};
use crate::agent::provider_presets;
use crate::agent::session::SessionManager;
use crate::agent::subagent_manager::SubagentManager;
use crate::config::{AgentConfig, Config, WorkspaceConfig, WorkspacePath};
use crate::error::{OSAgentError, Result};
use crate::external::{ExternalDirectoryManager, PermissionAction, PermissionPrompt};
use crate::plugin::PluginManager;
use crate::scheduler::Scheduler;
use crate::skills::{get_skills_base_dir, SkillLoader};
use crate::storage::{
    AuditEntry, Message, MessageAttachment, MessageImage, MessageTokens, QueuedMessage, Session,
    SessionEventRecord, SessionSummary, SqliteStorage, ToolCall,
};
use crate::tools::bash::BashTool;
use crate::tools::file_cache::FileReadCache;
use crate::tools::guard::ensure_relative_path_not_backups;
use crate::tools::output::path_touches_tool_outputs;
use crate::tools::registry::{ToolOutcome, ToolProfile, ToolRegistry, ToolResult};
use crate::tools::truncation::{self, TruncationOptions};
use chrono::Utc;
use dashmap::DashMap;
use futures::{future::join_all, StreamExt};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::{watch, Mutex, Notify};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

const COMPACTION_PROMPT: &str = r#"Provide a continuation summary of the earlier conversation below so that another pass can continue the work.

Follow this structure exactly:
## Primary Request and Intent
- What the user wants, in their own words when possible

## Key Technical Concepts
- Technologies, constraints, and conventions in play

## Errors and Fixes
- Failures encountered and how they were resolved (or why unresolved)

## Files and Code Sections
- Files or directory areas that matter most, and what is known about them

## Problem Solving
- What was tried, what is known not to work, and why

## Next Step
- The concrete next action for the continuation pass

Rules:
- Be concrete and repo-specific; optimize for continuing the task, not retelling it.
- If the transcript already contains earlier compacted summaries, merge them into your output instead of repeating them.
- Wrap your output in a single <compacted-summary> block. Do not acknowledge this summary explicitly in your reply."#;

pub struct AgentRuntime {
    config: Arc<tokio::sync::RwLock<Config>>,
    agent_settings: Arc<tokio::sync::RwLock<AgentConfig>>,
    provider: Arc<tokio::sync::RwLock<Arc<dyn Provider>>>,
    providers: Arc<tokio::sync::RwLock<Vec<(String, Arc<dyn Provider>)>>>,
    catalog: Arc<ModelCatalog>,
    session_manager: Arc<SessionManager>,
    checkpoint_manager: Arc<CheckpointManager>,
    tool_registry: Arc<ToolRegistry>,
    storage: Arc<SqliteStorage>,
    memory_store: Arc<MemoryStore>,
    decision_memory: Arc<DecisionMemory>,
    external_manager: Arc<ExternalDirectoryManager>,
    plugin_manager: Arc<PluginManager>,
    subagent_manager: Arc<SubagentManager>,
    prompt_cache: Arc<std::sync::RwLock<prompt::PromptCache>>,
    last_prompt_refresh: Arc<std::sync::Mutex<chrono::NaiveDate>>,
    event_bus: EventBus,
    session_locks: DashMap<String, Arc<Mutex<()>>>,
    session_cancellation: DashMap<String, Arc<Notify>>,
    session_cancel_flags: DashMap<String, Arc<std::sync::atomic::AtomicBool>>,
    active_runs: Arc<DashMap<String, ActiveRunInfo>>,
    /// Per-session advisory repeat-tool-call chains. A user message
    /// resets the chain; identical (tool, canonical args) calls count
    /// toward the `[tools].repeat_reminder.thresholds` nudge ladder.
    repeat_reminders:
        DashMap<String, std::sync::Mutex<crate::tools::repeat_reminder::RepeatReminderState>>,
    /// Session-scoped storage for oversized tool results. See
    /// `crate::tools::spill`.
    spill_store: Arc<crate::tools::spill::SpillStore>,
    /// Durable goal state + in-memory arming. See `crate::agent::goal`.
    goal_store: Arc<crate::agent::goal::GoalStore>,
    /// Weak handle back to this runtime, set during construction so
    /// `&self` methods can dispatch follow-up queue runs.
    self_arc: std::sync::OnceLock<std::sync::Weak<AgentRuntime>>,
    /// Parent sessions where a background subagent finished while a run was
    /// already active; drained into a continuation turn when the run ends.
    pending_wakes: Arc<DashMap<String, ()>>,
    /// Consecutive auto-continuation turns per session (started by background
    /// task completions with no real user message in between). Reset whenever
    /// a genuine user turn runs; guards against runaway wake loops.
    auto_wake_turns: Arc<DashMap<String, usize>>,
    shutdown_tx: Arc<watch::Sender<bool>>,
    restart_tx: Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<()>>>>,
    scheduler: Arc<Scheduler>,
    run_prompt_rx: tokio::sync::Mutex<
        Option<tokio::sync::mpsc::UnboundedReceiver<crate::scheduler::executor::RunPromptRequest>>,
    >,
}

#[derive(Debug, Clone)]
struct ActiveRunInfo {
    started_at: SystemTime,
    user: String,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
}

struct RunGuard {
    session_id: String,
    active_runs: Arc<DashMap<String, ActiveRunInfo>>,
}

impl Drop for RunGuard {
    fn drop(&mut self) {
        self.active_runs.remove(&self.session_id);
    }
}

struct TaskStatusGuard {
    session_id: String,
    session_manager: Arc<SessionManager>,
}

impl TaskStatusGuard {
    fn new(session_id: String, session_manager: Arc<SessionManager>) -> Self {
        Self {
            session_id,
            session_manager,
        }
    }
}

impl Drop for TaskStatusGuard {
    fn drop(&mut self) {
        let sm = self.session_manager.clone();
        let sid = self.session_id.clone();
        tokio::spawn(async move {
            if let Ok(Some(mut session)) = sm.get_session(&sid).await {
                if session.task_status == "running" {
                    session.task_status = "active".to_string();
                    if let Err(e) = sm.update_session(&session).await {
                        warn!("TaskStatusGuard: failed to reset task_status: {}", e);
                    }
                }
            }
        });
    }
}

fn is_repo_exploration_request(message: &str) -> bool {
    let lower = message.trim().to_lowercase();
    if lower.is_empty() {
        return false;
    }

    [
        "checkout the codebase",
        "check out the codebase",
        "explore the codebase",
        "explore codebase",
        "look through the codebase",
        "understand the codebase",
        "inspect the codebase",
        "review the codebase",
        "audit the codebase",
        "familiarize yourself with the codebase",
        "search around",
        "how is this structured",
        "how does this work",
        "how is this organized",
        "what does this codebase",
        "give me an overview",
        "walk me through",
        "show me around",
        "what's the architecture",
        "where is the",
        "how does the",
        "find where",
        "which file",
        "where are the",
        "where is the implementation",
        "look at the code",
        "check the code",
        "look into",
        "dive into",
    ]
    .iter()
    .any(|phrase| lower.contains(phrase))
}

fn should_continue_queue_after_run(result: &Result<String>) -> bool {
    match result {
        Ok(_) => true,
        Err(OSAgentError::Session(message)) if message == "Operation cancelled" => true,
        _ => false,
    }
}

impl AgentRuntime {
    pub async fn active_provider(&self) -> Arc<dyn Provider> {
        self.provider.read().await.clone()
    }

    pub fn new(config: Config) -> Result<Arc<Self>> {
        let startup_profile = std::env::var("OSAGENT_STARTUP_PROFILE")
            .map(|v| {
                let s = v.trim().to_ascii_lowercase();
                s == "1" || s == "true" || s == "yes" || s == "on"
            })
            .unwrap_or(false);
        let startup_total = Instant::now();
        let mut phase_start = Instant::now();
        let log_phase = |name: &str, phase_start: &mut Instant| {
            if startup_profile {
                info!(
                    target: "osagent::startup",
                    "phase={} elapsed_ms={:.2}",
                    name,
                    phase_start.elapsed().as_secs_f64() * 1000.0
                );
            }
            *phase_start = Instant::now();
        };

        let mut config = config;
        config.ensure_workspace_defaults();
        config.migrate_legacy_provider();
        let agent_settings = Arc::new(tokio::sync::RwLock::new(config.agent.clone()));
        log_phase("config_init", &mut phase_start);

        let catalog = Arc::new(ModelCatalog::new());
        let oauth_dir = PathBuf::from(shellexpand::tilde(&config.storage.database).to_string())
            .parent()
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));

        let storage = Arc::new(SqliteStorage::new(&config.storage.database)?);
        storage
            .clear_all_tasks()
            .map_err(|e| warn!("Failed to clear tasks on startup: {}", e))
            .ok();
        log_phase("storage_init", &mut phase_start);

        let event_bus = EventBus::new_with_storage(storage.clone());

        let mut provider_instances: Vec<(String, Arc<dyn Provider>)> = Vec::new();

        for provider_cfg in &config.providers {
            let mut cfg = provider_cfg.clone();

            if cfg.api_key.is_empty() {
                if let Some(key) = provider_presets::resolve_env_api_key(&cfg.provider_type) {
                    info!(
                        "Auto-detected API key for provider '{}' from environment variable",
                        cfg.provider_type
                    );
                    cfg.api_key = key;
                }
            }

            let provider = Arc::new(
                OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
                    cfg,
                    Some(catalog.clone()),
                    Some(crate::oauth::create_oauth_storage(&oauth_dir)),
                    agent_settings.clone(),
                )?,
            );
            provider.set_retry_listener(Some(Self::provider_retry_listener(
                Arc::new(event_bus.clone()),
                storage.clone(),
            )));
            provider_instances.push((provider_cfg.provider_type.clone(), provider));
        }

        if provider_instances.is_empty() {
            warn!("No providers configured - using legacy single provider config");
            let provider = Arc::new(
                OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
                    config.provider.clone(),
                    Some(catalog.clone()),
                    Some(crate::oauth::create_oauth_storage(&oauth_dir)),
                    agent_settings.clone(),
                )?,
            );
            provider.set_retry_listener(Some(Self::provider_retry_listener(
                Arc::new(event_bus.clone()),
                storage.clone(),
            )));
            provider_instances.push((config.provider.provider_type.clone(), provider));
        }
        log_phase("providers_init", &mut phase_start);

        let active_provider_id = if !config.default_provider.is_empty() {
            config.default_provider.clone()
        } else if let Some((id, _)) = provider_instances.first() {
            id.clone()
        } else {
            "openai-compatible".to_string()
        };

        let provider = provider_instances
            .iter()
            .find(|(id, _)| id == &active_provider_id)
            .map(|(_, p)| p.clone())
            .or_else(|| provider_instances.first().map(|(_, p)| p.clone()))
            .ok_or_else(|| OSAgentError::Config("No provider available".to_string()))?;

        if provider_instances.len() == 1 {
            if let Some((id, _)) = provider_instances.first() {
                if config.default_provider.is_empty() {
                    config.default_provider = id.clone();
                }
            }
            if config.default_model.is_empty() {
                config.default_model = config.provider.model.clone();
            }
        }

        let session_manager = Arc::new(SessionManager::new((*storage).clone()));
        let checkpoint_manager = Arc::new(CheckpointManager::new((*storage).clone()));
        let memory_store = Arc::new(MemoryStore::new(
            config.agent.memory_enabled,
            config.agent.memory_file.clone(),
            config.agent.learning_mode,
        )?);
        let decision_memory = Arc::new(DecisionMemory::new(
            config.agent.decision_memory_enabled,
            config.agent.decision_memory_file.clone(),
            config.agent.learning_mode,
            config.agent.decision_capture_mode,
        )?);
        log_phase("session_and_memory_init", &mut phase_start);

        let external_manager = Arc::new(ExternalDirectoryManager::new(
            config.external.permission.clone(),
        ));

        let plugin_dir = PathBuf::from(shellexpand::tilde(&config.plugins.plugin_dir).to_string());
        let plugin_manager = Arc::new(PluginManager::new(plugin_dir, config.plugins.enabled));
        if config.plugins.enabled {
            if let Err(e) = tokio::task::block_in_place(|| {
                tokio::runtime::Handle::current()
                    .block_on(async { plugin_manager.load_all().await })
            }) {
                warn!("Failed to load plugins: {}", e);
            }
        }
        log_phase("plugin_init", &mut phase_start);

        let skill_loader = if config.tools.skills.enabled {
            let skills_dir =
                PathBuf::from(shellexpand::tilde(&config.tools.skills.directory).to_string());
            let legacy_skills_dir = get_skills_base_dir();

            let loader = if legacy_skills_dir != skills_dir {
                SkillLoader::new(skills_dir).with_additional_skills_dir(legacy_skills_dir)
            } else {
                SkillLoader::new(skills_dir)
            };

            if let Err(e) = loader.load_all() {
                warn!("Failed to load skills: {}", e);
            }
            Some(Arc::new(loader))
        } else {
            None
        };
        log_phase("skills_init", &mut phase_start);

        let mut subagent_manager = SubagentManager::new(
            storage.clone(),
            Arc::new(event_bus.clone()),
            session_manager.clone(),
            Arc::new(tokio::sync::RwLock::new(config.clone())),
            PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string()),
        );
        subagent_manager.set_shared_provider(provider.clone());
        let subagent_manager = Arc::new(subagent_manager);

        let mut tool_registry_instance = ToolRegistry::with_full(
            config.clone(),
            storage.clone(),
            Some(Arc::new(event_bus.clone())),
            skill_loader,
            Some(subagent_manager.clone()),
            Some(memory_store.clone()),
            Some(decision_memory.clone()),
            Arc::new(FileReadCache::with_default_capacity()),
        )?;
        log_phase("tool_registry_init", &mut phase_start);

        let workspace_root = PathBuf::from(
            shellexpand::tilde(&config.get_active_workspace().resolved_path()).to_string(),
        );
        let coordinator = Arc::new(Coordinator::new(
            Arc::new(event_bus.clone()),
            subagent_manager.clone(),
            workspace_root,
        ));

        tool_registry_instance.register_coordinator(coordinator.clone());

        let goal_store = Arc::new(crate::agent::goal::GoalStore::new(storage.clone()));
        tool_registry_instance.register_goals(goal_store.clone());

        let mut scheduler = Scheduler::new(storage.clone(), event_bus.clone());

        let (run_prompt_tx, mut run_prompt_rx) = tokio::sync::mpsc::unbounded_channel();
        scheduler.set_prompt_sender(run_prompt_tx);

        let scheduler = Arc::new(scheduler);

        tool_registry_instance.register_scheduler(Arc::clone(&scheduler));
        let tool_registry = Arc::new(tool_registry_instance);
        log_phase("coordinator_and_scheduler_init", &mut phase_start);

        // MCP servers connect in the background: spawning child
        // processes and waiting on their handshakes would otherwise add
        // seconds to startup for every configured server. Until they
        // land, the agent runs with its native tools and an empty
        // manifest.
        if config.mcp.enabled && !config.mcp.servers.is_empty() {
            let registry = tool_registry.clone();
            let mcp_config = config.mcp.clone();
            tokio::spawn(async move {
                let manager = crate::mcp::McpManager::connect(&mcp_config).await;
                info!(
                    "MCP catalog ready: {} tools across {} server(s)",
                    manager.catalog_size(),
                    manager.server_summaries().len()
                );
                if let Some(previous) = registry.register_mcp(manager) {
                    previous.shutdown().await;
                }
            });
        }

        let custom_identity = config.agent.custom_identity.as_deref();
        let custom_priorities = config.agent.custom_priorities.as_deref();
        let use_cache = config.agent.prompt_cache_enabled;
        let available_tools = tool_registry
            .get_tool_definitions("")
            .into_iter()
            .map(|tool| tool.function.name)
            .collect::<Vec<_>>();
        let system_prompt = prompt::build_system_prompt(
            &available_tools,
            PromptMode::Full,
            custom_identity,
            custom_priorities,
        );
        let prompt_cache = if use_cache {
            prompt::PromptCache::build(
                &available_tools,
                PromptMode::Full,
                custom_identity,
                custom_priorities,
            )
        } else {
            // Fallback: wrap the plain prompt, entire content is treated as dynamic
            prompt::PromptCache {
                prompt: system_prompt.clone(),
                dynamic_offset: 0,
                mode: PromptMode::Full,
                cache_version: 0,
            }
        };
        let today = chrono::Local::now().date_naive();
        log_phase("system_prompt_build", &mut phase_start);

        if startup_profile {
            info!(
                target: "osagent::startup",
                "phase=agent_runtime_new_total elapsed_ms={:.2}",
                startup_total.elapsed().as_secs_f64() * 1000.0
            );
        }

        let spill_store = Arc::new(crate::tools::spill::SpillStore::new(
            crate::tools::spill::resolve_spill_root(&config.spill.root)?,
        ));

        // Refresh the models.dev catalog in the background so the model list
        // and reasoning levels are current at boot instead of waiting for the
        // first picker open to trigger a fetch.
        {
            let startup_catalog = catalog.clone();
            tokio::spawn(async move {
                startup_catalog.refresh_catalog().await;
            });
        }

        let runtime = Arc::new(Self {
            config: Arc::new(tokio::sync::RwLock::new(config)),
            agent_settings,
            provider: Arc::new(tokio::sync::RwLock::new(provider)),
            providers: Arc::new(tokio::sync::RwLock::new(provider_instances)),
            catalog,
            session_manager,
            checkpoint_manager,
            tool_registry,
            storage,
            memory_store,
            decision_memory,
            external_manager,
            plugin_manager,
            subagent_manager,
            prompt_cache: Arc::new(std::sync::RwLock::new(prompt_cache)),
            last_prompt_refresh: Arc::new(std::sync::Mutex::new(today)),
            event_bus,
            session_locks: DashMap::new(),
            session_cancellation: DashMap::new(),
            session_cancel_flags: DashMap::new(),
            active_runs: Arc::new(DashMap::new()),
            repeat_reminders: DashMap::new(),
            spill_store,
            goal_store,
            self_arc: std::sync::OnceLock::new(),
            pending_wakes: Arc::new(DashMap::new()),
            auto_wake_turns: Arc::new(DashMap::new()),
            shutdown_tx: Arc::new(watch::channel(false).0),
            restart_tx: Arc::new(std::sync::Mutex::new(None)),
            scheduler,
            run_prompt_rx: tokio::sync::Mutex::new(Some(run_prompt_rx)),
        });
        let _ = runtime.self_arc.set(std::sync::Arc::downgrade(&runtime));

        // Crash recovery: tasks still marked `running` belong to a previous
        // process. Fail them (leaving notified_at NULL) so the parent is told
        // about the interruption on its next turn / wake-up.
        match runtime.storage.fail_stale_running_subagent_tasks() {
            Ok(0) => {}
            Ok(count) => info!(
                "Startup recovery: marked {} orphaned subagent task(s) as failed",
                count
            ),
            Err(e) => warn!("Startup recovery sweep failed: {}", e),
        }

        // Background subagent completions wake the parent for a continuation
        // turn instead of waiting for the user's next message.
        let weak_for_wake = Arc::downgrade(&runtime);
        runtime
            .subagent_manager
            .set_wake_callback(Arc::new(move |parent_session_id: String| {
                if let Some(runtime) = weak_for_wake.upgrade() {
                    runtime.notify_background_task_finished(parent_session_id);
                }
            }));

        Ok(runtime)
    }

    pub async fn create_session(&self) -> Result<Session> {
        let cfg = self.config.read().await;
        let model = cfg.active_model();
        let provider = if cfg.default_provider.is_empty() {
            "openai-compatible".to_string()
        } else {
            cfg.default_provider.clone()
        };
        let active_workspace_id = cfg
            .agent
            .active_workspace
            .clone()
            .unwrap_or_else(|| "default".to_string());
        drop(cfg);

        let count = self.session_manager.get_session_count().await.unwrap_or(0);
        let name = Some(format!("Session {}", count + 1));

        let mut session = self
            .session_manager
            .create_session(model, provider, name)
            .await?;

        Self::set_session_workspace_id(&mut session, &active_workspace_id)?;
        self.session_manager.update_session(&session).await?;

        info!("Created session: {}", session.id);
        Ok(session)
    }

    pub fn subscribe_to_events(&self) -> tokio::sync::broadcast::Receiver<AgentEvent> {
        self.event_bus.subscribe()
    }

    pub fn subscribe_to_events_from(
        &self,
        session_id: &str,
        from_sequence: u64,
    ) -> Result<(
        Vec<AgentEvent>,
        tokio::sync::broadcast::Receiver<AgentEvent>,
    )> {
        self.event_bus.subscribe_from(session_id, from_sequence)
    }

    pub async fn answer_question(&self, question_id: &str, answers: Vec<Vec<String>>) -> bool {
        self.event_bus.answer_question(question_id, answers).await
    }

    /// Get or create a lock for a specific session
    fn get_session_lock(&self, session_id: &str) -> Arc<Mutex<()>> {
        self.session_locks
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Mutex::new(())))
            .clone()
    }

    /// Get or create a cancellation notifier for a specific session
    fn get_cancellation_notify(&self, session_id: &str) -> Arc<Notify> {
        self.session_cancellation
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(Notify::new()))
            .clone()
    }

    fn get_cancel_flag(&self, session_id: &str) -> Arc<std::sync::atomic::AtomicBool> {
        self.session_cancel_flags
            .entry(session_id.to_string())
            .or_insert_with(|| Arc::new(std::sync::atomic::AtomicBool::new(false)))
            .clone()
    }

    fn clear_cancel_flag(&self, session_id: &str) {
        self.session_cancel_flags.remove(session_id);
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
    }

    /// Cancel all subagents belonging to a parent session
    pub async fn cancel_subagents_for_parent(&self, session_id: &str) {
        self.subagent_manager
            .cancel_all_for_parent(session_id)
            .await;
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
    async fn wake_for_subagent_results(self: Arc<Self>, session_id: String) {
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
    async fn start_wake_turn(self: Arc<Self>, session_id: String) {
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
                let is_cancelled =
                    matches!(error, OSAgentError::Session(msg) if msg == "Operation cancelled");
                if is_cancelled {
                    runtime.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.clone(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                } else {
                    runtime.event_bus.emit(AgentEvent::Error {
                        session_id: session_id.clone(),
                        sequence: 0,
                        error: error.to_string(),
                        recoverable: false,
                        timestamp: SystemTime::now(),
                    });
                }
            }

            drop(run_guard);
            runtime.dispatch_queued_messages(&session_id, "system");
        });
    }

    /// Called at run end: if a background subagent finished while this run was
    /// active, start the deferred continuation turn.
    pub fn check_pending_wake(&self, session_id: &str) {
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
    fn merge_pending_subagent_results(&self, session: &mut Session, session_id: &str) -> usize {
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

    fn try_start_run(&self, session_id: &str, user: &str) -> Result<RunGuard> {
        if self.active_runs.contains_key(session_id) {
            return Err(OSAgentError::Session(
                "A run is already in progress for this session".to_string(),
            ));
        }

        self.active_runs.insert(
            session_id.to_string(),
            ActiveRunInfo {
                started_at: SystemTime::now(),
                user: user.to_string(),
            },
        );

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
                let is_cancelled =
                    matches!(error, OSAgentError::Session(msg) if msg == "Operation cancelled");
                if is_cancelled {
                    runtime.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.clone(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                } else {
                    runtime.event_bus.emit(AgentEvent::Error {
                        session_id: session_id.clone(),
                        sequence: 0,
                        error: error.to_string(),
                        recoverable: false,
                        timestamp: SystemTime::now(),
                    });
                }
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

        let queued_message = match self.storage.claim_next_queued_message(&session_id)? {
            Some(item) => item,
            None => {
                drop(run_guard);
                // Queue fully drained: honor any deferred background-completion
                // wake before the session goes idle.
                self.check_pending_wake(&session_id);
                return Ok(None);
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
                let is_cancelled =
                    matches!(error, OSAgentError::Session(msg) if msg == "Operation cancelled");
                if is_cancelled {
                    runtime.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.clone(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                } else {
                    runtime.event_bus.emit(AgentEvent::Error {
                        session_id: session_id.clone(),
                        sequence: 0,
                        error: error.to_string(),
                        recoverable: false,
                        timestamp: SystemTime::now(),
                    });
                }
            }

            let should_continue = should_continue_queue_after_run(&result);
            drop(run_guard);

            if should_continue {
                let next_runtime = runtime.clone();
                let next_session_id = session_id.clone();
                let next_user = user.clone();
                tokio::spawn(async move {
                    if let Err(error) =
                        next_runtime.spawn_next_queued_message_run(next_session_id, next_user)
                    {
                        error!(
                            "Failed to continue queued runs for session {}: {}",
                            session_id, error
                        );
                    }
                });
            } else {
                // Terminal error stops the queue chain; still honor any
                // background-completion wake that piled up behind this run.
                runtime.check_pending_wake(&session_id);
            }
        });

        Ok(Some(started_queue_id))
    }

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

    async fn process_message_internal(
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
        self.clear_cancel_flag(session_id);
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
        let max_iterations = agent_settings.max_iterations;
        drop(agent_settings);
        let mut pending_tool_followup = false;
        let mut response_truncated = false;
        let mut max_iterations_reached = false;
        let mut response_complete_emitted = false;
        let mut tool_success_count = 0usize;
        let mut tool_failure_count = 0usize;
        let mut last_iteration_only_meta_tools = false;
        let mut meta_only_followup_attempts = 0usize;
        let mut recent_tool_intents: Vec<String> = Vec::new();
        let mut recent_tool_signatures: Vec<String> = Vec::new();
        let mut recent_tool_outcomes: Vec<(String, bool, String)> = Vec::new();

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

        loop {
            iteration += 1;
            info!(
                "process_message: Iteration {} of {}",
                iteration, max_iterations
            );

            if pending_tool_followup {
                self.event_bus.emit(AgentEvent::Thinking {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    message: format!("Processing tool results... iteration {}", iteration),
                    timestamp: SystemTime::now(),
                });
            }

            // Check for cancellation at the start of each iteration
            if cancel_flag.load(std::sync::atomic::Ordering::Acquire) {
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
            let cancel_fut = cancel_notify.notified();
            tokio::select! {
                _ = cancel_fut => {
                    warn!("Operation cancelled for session {} at iteration {}", session_id, iteration);
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

            if iteration > max_iterations {
                warn!("Max iterations reached for session {}", session_id);
                max_iterations_reached = true;
                break;
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
                if let Some(decision_block) = self.decision_memory.prompt_block().await? {
                    api_messages.push(Message::system(decision_block));
                }
                if let Some(memory_block) = self.memory_store.prompt_block().await? {
                    api_messages.push(Message::system(memory_block));
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
            }

            api_messages.reserve(session.messages.len());
            for msg in &session.messages {
                api_messages.push(msg.clone());
            }
            if pending_tool_followup && !is_roleplay {
                api_messages.push(Message::user(
                    "The last tool calls completed. If the request is addressed, give a concise final summary and stop. Do not start tangential work or unsolicited improvements; only continue if a concrete step directly remains.".to_string(),
                ));
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
            let context_window = provider.model_context_window().await;
            if let Some(window) = context_window {
                let model_limit = self
                    .catalog
                    .lookup_model_limit(provider.provider_type(), &provider.current_model().await);
                let input_limit = model_limit.as_ref().and_then(|l| l.input);
                let output_limit = model_limit.as_ref().map(|l| l.output).unwrap_or(8192);

                let estimated_pre_tokens: usize =
                    api_messages.iter().map(Self::message_tokens).sum();
                let actual_pre_tokens = Self::session_actual_tokens(&api_messages);
                let tool_schema_tokens = serde_json::to_string(&tools)
                    .map(|schemas| schemas.chars().count().div_ceil(4))
                    .unwrap_or(0);
                let message_pre_tokens = if actual_pre_tokens > 0 {
                    actual_pre_tokens
                } else {
                    estimated_pre_tokens
                };
                let pre_tokens = message_pre_tokens.saturating_add(tool_schema_tokens);

                let reserved_output = std::cmp::min(output_limit, 8192);
                let usable = input_limit.unwrap_or(window.saturating_sub(reserved_output));
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

                    if let Some((pruned, compacted, replayed)) = self
                        .compact_session_history(
                            &mut session,
                            &active_workspace,
                            &runtime_config.compaction,
                        )
                        .await?
                    {
                        if let Some(ref mut cs) = session.context_state {
                            cs.compaction_stats.total_compactions += 1;
                            cs.compaction_stats.total_pruned_messages += pruned;
                            cs.compaction_stats.total_compacted_messages += compacted;
                        }
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
                    }),
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
                _ = cancel_notify.notified() => {
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
                    match self
                        .consume_provider_stream(
                            session_id,
                            &mut session,
                            stream,
                            cancel_notify.clone(),
                        )
                        .await
                    {
                        Ok(response) => (response, true),
                        Err(e) if Self::is_streaming_fallback_error(&e) => {
                            warn!(
                                "Stream consumption failed with fallback-eligible error in session {}: {}. Falling back to non-streaming.",
                                session_id, e
                            );
                            // Remove the empty assistant message pushed by consume_provider_stream
                            if let Some(last) = session.messages.last() {
                                if last.role == "assistant"
                                    && last.content.is_empty()
                                    && last.tool_calls.is_none()
                                {
                                    session.messages.pop();
                                }
                            }
                            let response = tokio::select! {
                                _ = cancel_notify.notified() => {
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
                                            error: e.to_string(),
                                            recoverable: e.is_recoverable(),
                                            timestamp: SystemTime::now(),
                                        });
                                        e
                                    })?
                                }
                            };
                            (response, false)
                        }
                        Err(e) => {
                            error!("Provider stream error in session {}: {}", session_id, e);
                            self.event_bus.emit(AgentEvent::Error {
                                session_id: session_id.to_string(),
                                sequence: 0,
                                error: e.to_string(),
                                recoverable: e.is_recoverable(),
                                timestamp: SystemTime::now(),
                            });
                            return Err(e);
                        }
                    }
                }
                Err(error) if Self::is_streaming_fallback_error(&error) => {
                    let response = tokio::select! {
                        _ = cancel_notify.notified() => {
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
                                    error: e.to_string(),
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
                    self.event_bus.emit(AgentEvent::Error {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        error: error.to_string(),
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
                    _ = cancel_notify.notified() => {
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
                                error: e.to_string(),
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
                    let provider_compaction =
                        Self::apply_provider_compaction(&mut session, &summary);
                    if let Some((compacted_count, dropped)) = provider_compaction {
                        if let Err(error) = self.storage.archive_messages(&session.id, &dropped) {
                            warn!(
                                "Failed to archive provider-compacted messages for {}: {}",
                                session_id, error
                            );
                        }
                        info!(
                            "Persisting provider-side context compression for session {}: replaced {} messages with summary",
                            session_id, compacted_count
                        );
                        if let Some(ref mut cs) = session.context_state {
                            cs.compaction_stats.total_compactions += 1;
                            cs.compaction_stats.total_compacted_messages += compacted_count;
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

            let has_tool_calls = response
                .tool_calls
                .as_ref()
                .map(|calls| !calls.is_empty())
                .unwrap_or(false);

            if used_streaming {
                if let Some(last_message) = session.messages.last_mut() {
                    last_message.tool_calls = response.tool_calls.clone();
                    if has_tool_calls {
                        last_message.metadata = serde_json::json!({
                            "synthetic": true,
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
                        });
                        total_input_tokens += usage.input;
                        total_output_tokens += usage.output;
                        total_tokens += usage.total;
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
                        "synthetic": true,
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
                    });
                    total_input_tokens += usage.input;
                    total_output_tokens += usage.output;
                    total_tokens += usage.total;
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
                    let message_index = (session.messages.len() as i32) - 1;
                    let parallel_results = self
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
                        .await?;

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
                            let message_index = (session.messages.len() as i32) - 1;
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
                        let cancel_fut = cancel_notify.notified();
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
                        if !tool_profile.allows(&tool_call.name) {
                            return Err(OSAgentError::ToolExecution(format!(
                                "Tool '{}' is not allowed by the {:?} profile",
                                tool_call.name, tool_profile
                            )));
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
                        let result: Result<ToolResult> = if tool_call.name == "persona" {
                            self.handle_persona_tool_call(&mut session, &tool_call.arguments)
                                .map(ToolResult::new)
                        } else if tool_call.name == "batch" {
                            let batch_message_index = (session.messages.len() as i32) - 1;
                            self.handle_batch_tool_call(
                                &mut session,
                                &active_workspace,
                                &tool_call.arguments,
                                batch_message_index,
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
                            // Reading other conversations through the
                            // `sessions` tool is gated by the
                            // `session_access` policy (popup, rules) before
                            // anything executes.
                            let session_access = if tool_call.name == "sessions" {
                                self.authorize_session_access(session_id, &tool_call.id, &tool_args)
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
                                            tool_args[key] = serde_json::Value::String(resolved);
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

                        if tool_call.name != "batch" {
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
                                session.messages.push(Message::synthetic_user(
                                    reminder,
                                    "repeat_tool_reminder",
                                ));
                            }
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
                let finish = response.finish_reason.to_lowercase();
                let looks_like_completion =
                    finish == "stop" || finish == "end_turn" || finish == "completed";
                let is_truncated = finish == "length";
                if !looks_like_completion && !is_truncated {
                    pending_tool_followup = true;
                } else if is_truncated {
                    warn!(
                        "Response with tool calls truncated (finish_reason=length) for session {} - treating as pending continuation",
                        session_id
                    );
                    pending_tool_followup = true;
                } else {
                    info!("process_message: Model stopped naturally (finish_reason={}), not forcing continuation", finish);
                }
            } else {
                info!("process_message: No tool calls, response complete");

                let finish = response.finish_reason.to_lowercase();
                if finish == "length" && iteration < max_iterations {
                    warn!(
                        "Response truncated (finish_reason=length) for session {} - requesting continuation",
                        session_id
                    );
                    response_truncated = true;
                    continue;
                }

                if pending_tool_followup
                    && last_iteration_only_meta_tools
                    && meta_only_followup_attempts < 2
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
                    pending_tool_followup = false;
                    continue;
                }

                if pending_tool_followup
                    && response
                        .content
                        .as_ref()
                        .map(|c| c.trim().is_empty())
                        .unwrap_or(true)
                {
                    warn!(
                        "Empty post-tool response for session {} - continuing",
                        session_id
                    );
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
                self.event_bus.emit(AgentEvent::ResponseComplete {
                    session_id: session_id.to_string(),
                    sequence: 0,
                    timestamp: SystemTime::now(),
                    usage: if total_tokens > 0 {
                        Some(EventTokenUsage {
                            input: total_input_tokens,
                            output: total_output_tokens,
                            total: total_tokens,
                            cached_read: None,
                            cached_write: None,
                            reasoning: None,
                        })
                    } else {
                        None
                    },
                });
                response_complete_emitted = true;
                break;
            }
        }

        info!("process_message: Loop complete, finalizing session");
        self.session_manager.update_session(&session).await?;

        if runtime_config.agent.checkpoint_enabled {
            if let Err(error) = self
                .checkpoint_manager
                .create_checkpoint(
                    &session,
                    Some("assistant_turn_complete".to_string()),
                    None,
                    Some(active_workspace.resolved_path()),
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
                && !Self::is_synthetic_message(m)
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
                "Status: partial. I finished running tools, but hit the iteration limit before finalizing.".to_string()
            } else {
                "Status: partial. I hit the iteration limit before completing the task.".to_string()
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
                    let mut snippet = line.replace(['\n', '\r'], " ");
                    if snippet.len() > 180 {
                        snippet.truncate(180);
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

        let tool_messages: Vec<String> = session
            .messages
            .iter()
            .filter(|m| m.role == "tool")
            .map(|m| m.content.clone())
            .collect();

        if !tool_messages.is_empty() && result.trim().is_empty() {
            let mut tool_summary = String::from("Tool Results:\n");
            let mut recent: Vec<String> = tool_messages.into_iter().rev().take(3).collect();
            recent.reverse();

            for message in recent {
                let mut snippet = message;
                if snippet.len() > 1500 {
                    snippet.truncate(1500);
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

        if !response_complete_emitted {
            self.event_bus.emit(AgentEvent::ResponseComplete {
                session_id: session_id.to_string(),
                sequence: 0,
                timestamp: SystemTime::now(),
                usage: if total_tokens > 0 {
                    Some(EventTokenUsage {
                        input: total_input_tokens,
                        output: total_output_tokens,
                        total: total_tokens,
                        cached_read: None,
                        cached_write: None,
                        reasoning: None,
                    })
                } else {
                    None
                },
            });
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
    async fn maybe_queue_goal_round(&self, session: &Session) -> Result<()> {
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
            &format!("goal-round-{}", round),
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
    fn dispatch_queued_messages(&self, session_id: &str, user: &str) {
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

    fn emit_chunked_text<F>(&self, text: &str, chunk_size: usize, mut emit: F)
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

    fn emit_thinking_chunks(&self, session_id: &str, thinking: &str) {
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

    fn emit_response_chunks(&self, session_id: &str, content: &str) {
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
    fn provider_retry_listener(
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

    fn is_streaming_fallback_error(error: &OSAgentError) -> bool {
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

    async fn maybe_persist_streaming_message(
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

    async fn consume_provider_stream(
        &self,
        session_id: &str,
        session: &mut Session,
        mut stream: futures::stream::BoxStream<'static, Result<StreamEvent>>,
        cancel_notify: Arc<Notify>,
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

        loop {
            let next_event = tokio::select! {
                _ = cancel_notify.notified() => {
                    self.event_bus.emit(AgentEvent::Cancelled {
                        session_id: session_id.to_string(),
                        sequence: 0,
                        timestamp: SystemTime::now(),
                    });
                    return Err(OSAgentError::Session("Operation cancelled".to_string()));
                }
                event = stream.next() => event,
            };

            let Some(event) = next_event else {
                break;
            };

            let event = event?;

            if let Some(reason) = event.finish_reason.as_ref() {
                finish_reason = reason.clone();
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

    fn active_persona_from_session(session: &Session) -> Option<ActivePersona> {
        session
            .metadata
            .get("active_persona")
            .and_then(|v| serde_json::from_value(v.clone()).ok())
    }

    fn session_metadata_mut(
        session: &mut Session,
    ) -> Result<&mut serde_json::Map<String, serde_json::Value>> {
        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }

        session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Parse("Session metadata must be a JSON object".to_string())
        })
    }

    fn push_session_event(
        session: &mut Session,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        let metadata = Self::session_metadata_mut(session)?;
        let events = metadata
            .entry("session_events".to_string())
            .or_insert_with(|| serde_json::json!([]));
        let array = events.as_array_mut().ok_or_else(|| {
            OSAgentError::Parse("Session events metadata must be an array".to_string())
        })?;

        array.push(
            serde_json::to_value(SessionEventRecord {
                event_type: event_type.to_string(),
                timestamp: Utc::now(),
                data,
            })
            .map_err(|e| OSAgentError::Parse(format!("Failed to encode session event: {}", e)))?,
        );

        if array.len() > 200 {
            let drop_count = array.len() - 200;
            array.drain(0..drop_count);
        }

        Ok(())
    }

    fn message_kind(message: &Message) -> Option<&str> {
        message
            .metadata
            .get("kind")
            .and_then(|value| value.as_str())
    }

    fn is_synthetic_message(message: &Message) -> bool {
        message
            .metadata
            .get("synthetic")
            .and_then(|value| value.as_bool())
            .unwrap_or(false)
    }

    fn is_real_user_message(message: &Message) -> bool {
        message.role == "user" && !Self::is_synthetic_message(message)
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

    fn is_visible_assistant_message(message: &Message) -> bool {
        message.role == "assistant"
            && !Self::is_synthetic_message(message)
            && !message.content.trim().is_empty()
            && message
                .tool_calls
                .as_ref()
                .map(|calls| calls.is_empty())
                .unwrap_or(true)
            && !Self::looks_like_internal_tool_dump(&message.content)
    }

    fn replay_start_index(messages: &[Message]) -> usize {
        for (index, message) in messages.iter().enumerate().rev() {
            if Self::is_real_user_message(message) {
                return index;
            }
        }

        for (index, message) in messages.iter().enumerate().rev() {
            if message.role == "user" {
                return index;
            }
        }

        messages.len().saturating_sub(6)
    }

    fn compactable_message_content(message: &Message) -> Option<String> {
        if message.role == "system" || Self::is_synthetic_message(message) {
            return None;
        }

        let mut content = message.content.replace('\n', " ").trim().to_string();
        if content.is_empty() {
            return None;
        }

        if content.chars().count() > 500 {
            content = content.chars().take(500).collect::<String>();
            content.push_str("...");
        }

        let label = match (message.role.as_str(), Self::message_kind(message)) {
            ("assistant", Some("compaction_summary")) => "assistant_summary",
            (role, Some(kind)) => return Some(format!("{}({}): {}", role, kind, content)),
            (role, None) => role,
        };

        Some(format!("{}: {}", label, content))
    }

    fn transcript_for_compaction(messages: &[Message], max_chars: usize) -> String {
        let mut transcript = String::new();

        for line in messages
            .iter()
            .filter_map(Self::compactable_message_content)
        {
            let extra = if transcript.is_empty() { 0 } else { 1 };
            if transcript.chars().count() + line.chars().count() + extra > max_chars {
                break;
            }
            if !transcript.is_empty() {
                transcript.push('\n');
            }
            transcript.push_str(&line);
        }

        if transcript.is_empty() {
            "No earlier transcript available.".to_string()
        } else {
            transcript
        }
    }

    /// Model-free pre-compaction pass: rewrite over-budget tool results
    /// to a bounded head/tail slice with a fixed middle marker. No LLM
    /// call; a second pass over already-pruned content is provably a
    /// no-op because `head + marker + tail <= threshold` is clamped.
    /// Ported from DSH's `compaction-tool-result-pruner`.
    fn prune_old_tool_messages(
        session: &mut Session,
        preserve_from: usize,
        compaction: &crate::config::CompactionConfig,
    ) -> usize {
        const MIDDLE_MARKER: &str = "\n[... tool result middle pruned ...]\n";

        let mut pruned = 0usize;
        let threshold = compaction.prune_threshold_chars;
        let head = compaction.prune_head_chars;
        let tail = compaction
            .prune_tail_chars
            .min(threshold.saturating_sub(head + MIDDLE_MARKER.len()));

        for message in session.messages.iter_mut().take(preserve_from) {
            if message.role != "tool" {
                continue;
            }
            if message
                .metadata
                .get("pruned_for_context")
                .and_then(|value| value.as_bool())
                .unwrap_or(false)
            {
                continue;
            }
            let chars = message.content.chars().count();
            if chars <= threshold {
                continue;
            }

            let head_text: String = message.content.chars().take(head).collect();
            let tail_text: String = message
                .content
                .chars()
                .rev()
                .take(tail)
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            message.content = format!("{}{}{}", head_text, MIDDLE_MARKER, tail_text);
            message.metadata["pruned_for_context"] = serde_json::json!(true);
            pruned += 1;
        }

        pruned
    }

    /// Persist a provider-side context compression into the session: the middle
    /// of the history is replaced with the summary the provider produced, so the
    /// model sees it on later turns instead of the compression being lost after
    /// the single request.
    ///
    /// Returns the removed span so the caller can archive it before
    /// persisting; `None` when there was nothing to compact.
    fn apply_provider_compaction(
        session: &mut Session,
        summary: &str,
    ) -> Option<(usize, Vec<Message>)> {
        let original_len = session.messages.len();
        if original_len < 8 {
            return None;
        }
        let keep_tail = 6usize.min(original_len.saturating_sub(4));
        let keep_head = 3usize.min(original_len.saturating_sub(keep_tail));
        if original_len <= keep_head + keep_tail + 1 {
            return None;
        }

        let head = session.messages[..keep_head].to_vec();
        let dropped = session.messages[keep_head..original_len - keep_tail].to_vec();
        let tail = session.messages[original_len - keep_tail..].to_vec();
        let mut compacted = head;
        compacted.push(Message::synthetic_assistant(
            format!("Compaction summary:\n{}", summary.trim()),
            "compaction_summary",
        ));
        compacted.extend(tail);
        let compacted_count = original_len - compacted.len();
        session.messages = compacted;
        Some((compacted_count, dropped))
    }

    /// Frame a compaction summary the way DSH does: a single
    /// `<compacted-summary>` block plus a preamble telling the model not
    /// to acknowledge it, so the replacement reads as a continuation
    /// checkpoint rather than a fresh message to respond to.
    fn frame_compacted_summary(summary: &str) -> String {
        format!(
            "<compacted-summary>\n{}\n</compacted-summary>\n\nDo not acknowledge the compacted summary explicitly in your reply.",
            summary.trim()
        )
    }

    async fn compact_session_history(
        &self,
        session: &mut Session,
        active_workspace: &WorkspaceConfig,
        compaction: &crate::config::CompactionConfig,
    ) -> Result<Option<(usize, usize, bool)>> {
        if session.messages.len() < 8 {
            return Ok(None);
        }

        let replay_start = Self::replay_start_index(&session.messages);
        let pruned = if compaction.prune_enabled {
            Self::prune_old_tool_messages(session, replay_start, compaction)
        } else {
            0
        };
        let compact_end = replay_start.max(session.messages.len().saturating_sub(6));
        if compact_end == 0 {
            return Ok(if pruned > 0 {
                Some((pruned, 0, false))
            } else {
                None
            });
        }

        let prefix = session.messages[..compact_end].to_vec();
        let tail = session.messages[compact_end..].to_vec();
        if prefix.is_empty() {
            return Ok(if pruned > 0 {
                Some((pruned, 0, false))
            } else {
                None
            });
        }

        let workspace_path = std::path::PathBuf::from(
            shellexpand::tilde(&active_workspace.resolved_path()).to_string(),
        );
        let mut compact_messages = vec![Message::system(COMPACTION_PROMPT.to_string())];
        let config_dir = self.config.read().await.config_dir();
        if let Some(reminder) = format_system_reminder(&global_instruction_blocks(&config_dir)) {
            compact_messages.push(Message::system(reminder));
        }
        if let Some(reminder) =
            format_system_reminder(&workspace_instruction_blocks(&workspace_path))
        {
            compact_messages.push(Message::system(reminder));
        }
        compact_messages.push(Message::user(format!(
            "Earlier conversation transcript:\n{}",
            Self::transcript_for_compaction(&prefix, compaction.max_transcript_chars)
        )));

        let provider = self.active_provider().await;
        let summary = match provider
            .complete(Some(&session.id), &compact_messages, &[])
            .await
        {
            Ok(response) => response
                .content
                .unwrap_or_else(|| Self::summarize_for_context(&prefix, 2_500)),
            Err(error) => {
                warn!("Compaction model pass failed: {}", error);
                Self::summarize_for_context(&prefix, 2_500)
            }
        };

        // Shrink validation: a summary must cost fewer tokens than the
        // region it shadows, or the compaction is a loss. Fall back to
        // the deterministic summarizer, which is always smaller.
        let framed = Self::frame_compacted_summary(&summary);
        let framed_tokens = framed.chars().count().div_ceil(4).max(1);
        let prefix_tokens: usize = prefix.iter().map(Self::message_tokens).sum();
        let final_summary = if framed_tokens >= prefix_tokens {
            warn!(
                "Compaction summary did not shrink ({} est. tokens vs {}): using deterministic fallback",
                framed_tokens, prefix_tokens
            );
            Self::frame_compacted_summary(&Self::summarize_for_context(&prefix, 2_500))
        } else {
            framed
        };

        let mut compacted_messages = vec![Message::synthetic_assistant(
            final_summary,
            "compaction_summary",
        )];
        compacted_messages.extend(tail);

        // Preserve what is about to be summarized away so the `sessions`
        // tool can still search and read pre-compaction content.
        if let Err(error) = self.storage.archive_messages(&session.id, &prefix) {
            warn!(
                "Failed to archive compacted messages for {}: {}",
                session.id, error
            );
        }

        session.messages = compacted_messages;

        Ok(Some((pruned, prefix.len(), true)))
    }

    fn emit_reasoning_event(&self, session_id: &str, summary: impl Into<String>) {
        self.event_bus.emit(AgentEvent::Reasoning {
            session_id: session_id.to_string(),
            sequence: 0,
            summary: summary.into(),
            timestamp: SystemTime::now(),
        });
    }

    fn record_session_event(
        &self,
        session: &mut Session,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<()> {
        Self::push_session_event(session, event_type, data.clone())?;
        self.storage
            .append_session_event(&session.id, event_type, data)?;
        Ok(())
    }

    fn snapshot_candidate_paths(tool_name: &str, args: &serde_json::Value) -> Vec<String> {
        match tool_name {
            "write_file" | "edit_file" | "delete_file" => args["path"]
                .as_str()
                .map(|path| vec![path.to_string()])
                .unwrap_or_default(),
            "apply_patch" => args["patch"]
                .as_str()
                .map(Self::patch_paths_for_snapshot)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    fn patch_paths_for_snapshot(patch: &str) -> Vec<String> {
        patch
            .lines()
            .filter_map(|line| {
                [
                    "*** Add File: ",
                    "*** Delete File: ",
                    "*** Update File: ",
                    "*** Move to: ",
                ]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .map(|path| path.trim().to_string())
            })
            .collect()
    }

    fn workspace_root(workspace: &WorkspaceConfig) -> std::path::PathBuf {
        std::path::PathBuf::from(shellexpand::tilde(&workspace.resolved_path()).to_string())
    }

    fn snapshot_allowed_path(path: &str) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        if ensure_relative_path_not_backups(path).is_err() {
            return false;
        }
        let joined = std::path::Path::new(path);
        !path_touches_tool_outputs(joined)
    }

    fn truncate_diff_text(input: &str, limit_chars: usize) -> String {
        if input.chars().count() <= limit_chars {
            return input.to_string();
        }
        let mut out = String::new();
        for ch in input.chars().take(limit_chars) {
            out.push(ch);
        }
        out
    }

    fn build_file_diff_metadata(
        &self,
        session_id: &str,
        snapshot_id: &str,
        workspace: &WorkspaceConfig,
    ) -> Result<serde_json::Value> {
        let records = self
            .storage
            .list_file_snapshot_records(session_id, snapshot_id)?;
        let root = Self::workspace_root(workspace);

        let mut files = Vec::new();
        for record in records {
            let abs_path = root.join(&record.path);
            let new_content_bytes = std::fs::read(&abs_path).ok();

            let old_content = record
                .content
                .as_ref()
                .map(|bytes| Self::truncate_diff_text(&String::from_utf8_lossy(bytes), 50_000));
            let new_content = new_content_bytes
                .as_ref()
                .map(|bytes| Self::truncate_diff_text(&String::from_utf8_lossy(bytes), 50_000));

            let status = if !record.existed && new_content.is_some() {
                "added"
            } else if record.existed && new_content.is_none() {
                "deleted"
            } else {
                "modified"
            };

            files.push(serde_json::json!({
                "path": record.path,
                "status": status,
                "old_content": old_content,
                "new_content": new_content,
            }));
        }

        Ok(serde_json::json!({ "diff_files": files }))
    }

    fn capture_file_snapshots(
        &self,
        session_id: &str,
        workspace: &WorkspaceConfig,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<Option<String>> {
        let mut paths = Self::snapshot_candidate_paths(tool_name, args)
            .into_iter()
            .filter(|path| Self::snapshot_allowed_path(path))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();

        if paths.is_empty() {
            return Ok(None);
        }

        let root = Self::workspace_root(workspace);
        let snapshot_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let records = paths
            .into_iter()
            .map(|path| {
                let full_path = root.join(&path);
                let content = std::fs::read(&full_path).ok();
                crate::storage::FileSnapshotRecord {
                    id: Uuid::new_v4().to_string(),
                    snapshot_id: snapshot_id.clone(),
                    session_id: session_id.to_string(),
                    tool_name: tool_name.to_string(),
                    path,
                    existed: full_path.exists(),
                    content,
                    created_at: now,
                }
            })
            .collect::<Vec<_>>();

        self.storage
            .create_file_snapshot_records(session_id, &snapshot_id, tool_name, &records)?;
        Ok(Some(snapshot_id))
    }

    async fn revert_snapshot_for_session(
        &self,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<crate::storage::FileSnapshotSummary> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;
        let cfg = self.config.read().await;
        let workspace = Self::resolve_workspace_for_session(&session, &cfg)
            .unwrap_or_else(|| cfg.get_active_workspace());
        drop(cfg);

        let records = self
            .storage
            .list_file_snapshot_records(session_id, snapshot_id)?;
        if records.is_empty() {
            return Err(OSAgentError::Session(format!(
                "Snapshot '{}' not found for session {}",
                snapshot_id, session_id
            )));
        }

        let root = Self::workspace_root(&workspace);
        for record in &records {
            let path = root.join(&record.path);
            if record.existed {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, record.content.clone().unwrap_or_default())?;
            } else if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }

        let summary = self
            .storage
            .list_file_snapshot_summaries(session_id)?
            .into_iter()
            .find(|item| item.snapshot_id == snapshot_id)
            .ok_or_else(|| {
                OSAgentError::Session("Snapshot summary missing after restore".to_string())
            })?;
        Ok(summary)
    }

    /// `tool_script` runs outside the normal tool path for the same
    /// reason `batch` does: it needs the registry itself, which a tool
    /// owned by that registry cannot hold.
    async fn handle_tool_script_call(
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
            event_bus: Some(self.event_bus.clone()),
            session_id: session_id.to_string(),
        };
        crate::tools::tool_script::run_script(context, args).await
    }

    async fn handle_batch_tool_call(
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

    async fn execute_parallel_tool_calls(
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

        let cancel_notify = self.get_cancellation_notify(&session_id);
        let cancel_fut = cancel_notify.notified();
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
            let cancel_notify = self.get_cancellation_notify(&session_id);
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

                let cancel_fut = cancel_notify.notified();
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

                let result = registry
                    .execute_in_workspace_result(&tool_name, args, Some(workspace_path))
                    .await;

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

    fn emit_tool_progress(
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

    fn emit_tool_complete(
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

    fn non_empty_tool_metadata(metadata: &serde_json::Value) -> Option<serde_json::Value> {
        match metadata {
            serde_json::Value::Object(map) if map.is_empty() => None,
            _ => Some(metadata.clone()),
        }
    }

    fn session_workspace_id(session: &Session) -> Option<String> {
        session
            .metadata
            .get("workspace_id")
            .and_then(|v| v.as_str())
            .map(|v| v.to_string())
    }

    fn session_archived(session: &Session) -> bool {
        session
            .metadata
            .get("archived")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
    }

    fn set_session_workspace_id(session: &mut Session, workspace_id: &str) -> Result<()> {
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

    fn resolve_workspace_for_session(
        session: &Session,
        config: &Config,
    ) -> Option<WorkspaceConfig> {
        let workspace_id = Self::session_workspace_id(session)?;
        config.get_workspace(&workspace_id)
    }

    fn write_active_persona_to_session(
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

    fn persona_status_text(session: &Session) -> String {
        if let Some(persona) = Self::active_persona_from_session(session) {
            format!(
                "Active persona: {} ({}) - {}",
                persona.id, persona.name, persona.summary
            )
        } else {
            "Active persona: default (none selected)".to_string()
        }
    }

    fn handle_persona_tool_call(
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

    fn estimate_tokens(text: &str) -> usize {
        let chars = text.chars().count();
        (chars / 4).max(1)
    }

    fn summarize_tool_output_for_context(
        tool_name: &str,
        outcome: ToolOutcome,
        output: &str,
    ) -> String {
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

        let rendered = match tool_name {
            "read_file" => format!(
                "Tool: {}\nOutput summary (trimmed file content for context):\n{}",
                tool_name, compact
            ),
            "list_files" | "glob" | "grep" => format!(
                "Tool: {}\nOutput summary (trimmed search results for context):\n{}",
                tool_name, compact
            ),
            _ => format!("Tool: {}\nOutput:\n{}", tool_name, compact),
        };

        format!("[tool outcome: {}]\n{}", outcome.as_str(), rendered)
    }

    fn stable_json_string(value: &serde_json::Value) -> String {
        match value {
            serde_json::Value::Null => "null".to_string(),
            serde_json::Value::Bool(value) => value.to_string(),
            serde_json::Value::Number(value) => value.to_string(),
            serde_json::Value::String(value) => format!("{:?}", value),
            serde_json::Value::Array(values) => format!(
                "[{}]",
                values
                    .iter()
                    .map(Self::stable_json_string)
                    .collect::<Vec<_>>()
                    .join(",")
            ),
            serde_json::Value::Object(map) => {
                let mut entries = map.iter().collect::<Vec<_>>();
                entries.sort_by(|a, b| a.0.cmp(b.0));
                format!(
                    "{{{}}}",
                    entries
                        .into_iter()
                        .map(|(key, value)| {
                            format!("{}:{}", key, Self::stable_json_string(value))
                        })
                        .collect::<Vec<_>>()
                        .join(",")
                )
            }
        }
    }

    fn tool_call_signature(name: &str, args: &serde_json::Value) -> String {
        format!("{}:{}", name, Self::stable_json_string(args))
    }

    fn normalize_loop_text(value: &str) -> String {
        value
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ")
            .to_ascii_lowercase()
    }

    fn normalized_arg(args: &serde_json::Value, key: &str, default: &str) -> String {
        args.get(key)
            .and_then(|value| value.as_str())
            .map(Self::normalize_loop_text)
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| default.to_string())
    }

    fn tool_intent_signature(name: &str, args: &serde_json::Value) -> Option<String> {
        match name {
            "grep" => Some(format!(
                "grep:{}:{}",
                Self::normalized_arg(args, "pattern", "*"),
                Self::normalized_arg(args, "file_pattern", "*"),
            )),
            "glob" => Some(format!(
                "glob:{}:{}",
                Self::normalized_arg(args, "pattern", "*"),
                Self::normalized_arg(args, "path", "."),
            )),
            "list_files" => Some(format!(
                "list_files:{}:{}",
                Self::normalized_arg(args, "path", "."),
                args.get("recursive")
                    .and_then(|value| value.as_bool())
                    .unwrap_or(false)
            )),
            "read_file" => Some(format!(
                "read_file:{}:{}:{}",
                Self::normalized_arg(args, "path", ""),
                args.get("start_line")
                    .and_then(|value| value.as_u64())
                    .unwrap_or(1),
                args.get("end_line")
                    .and_then(|value| value.as_u64())
                    .map(|value| value.to_string())
                    .unwrap_or_else(|| "end".to_string())
            )),
            "bash" => Some(format!(
                "bash:{}:{}",
                Self::normalized_arg(args, "command", ""),
                Self::normalized_arg(args, "workdir", "."),
            )),
            "code_python" | "code_node" | "code_bash" => Some(format!(
                "{}:{}",
                name,
                args.get("code")
                    .and_then(|value| value.as_str())
                    .map(|code| Self::normalize_loop_text(
                        &code.chars().take(160).collect::<String>()
                    ))
                    .unwrap_or_default()
            )),
            _ => None,
        }
    }

    fn consecutive_repeat_count(history: &[String], signature: &str) -> usize {
        let mut count = 0usize;
        for previous in history.iter().rev() {
            if previous == signature {
                count += 1;
            } else {
                break;
            }
        }
        count
    }

    fn tool_loop_guidance(tool_name: &str) -> &'static str {
        match tool_name {
            "grep" | "glob" | "list_files" => {
                "Change the path or pattern, or inspect one of the files you already found instead of repeating the same search."
            }
            "read_file" => {
                "Read a different file or a narrower line range instead of rereading the same section unchanged."
            }
            "bash" | "code_python" | "code_node" | "code_bash" => {
                "Adjust the command or script, inspect the prior output, or summarize the blocker instead of rerunning it unchanged."
            }
            _ => {
                "Use the previous result, change strategy, or explain the blocker instead of repeating the same call."
            }
        }
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

    fn message_actual_tokens(message: &Message) -> Option<usize> {
        message.tokens.as_ref().map(|t| t.total)
    }

    fn session_actual_tokens(messages: &[Message]) -> usize {
        messages
            .iter()
            .filter_map(Self::message_actual_tokens)
            .sum()
    }

    fn condense_messages(messages: &[Message], context_window: usize) -> Vec<Message> {
        let budget = ((context_window as f32) * 0.8) as usize;
        let mut total: usize = messages.iter().map(Self::message_tokens).sum();
        if total <= budget {
            return messages.to_vec();
        }

        let mut system_message: Option<Message> = None;
        let mut user_message: Option<Message> = None;
        let mut other: Vec<Message> = Vec::new();

        for message in messages {
            if message.role == "system" && system_message.is_none() {
                system_message = Some(message.clone());
            } else if message.role == "user" {
                user_message = Some(message.clone());
                other.push(message.clone());
            } else {
                other.push(message.clone());
            }
        }

        let summary = Self::summarize_for_context(&other, budget / 2);
        let summary_message = Message::assistant(format!("Context summary:\n{}", summary), None);

        let mut condensed: Vec<Message> = Vec::new();
        if let Some(system) = system_message {
            condensed.push(system);
        }
        condensed.push(summary_message);
        if let Some(user) = user_message {
            condensed.push(user);
        }

        total = condensed.iter().map(Self::message_tokens).sum();
        if total > budget {
            return condensed;
        }

        let mut remaining: Vec<Message> = Vec::new();
        for message in messages.iter().rev() {
            if message.role == "system" {
                continue;
            }
            let msg_tokens = Self::message_tokens(message);
            if total + msg_tokens > budget {
                continue;
            }
            total += msg_tokens;
            remaining.push(message.clone());
        }

        remaining.reverse();
        condensed.extend(remaining);
        condensed
    }

    fn summarize_for_context(messages: &[Message], token_budget: usize) -> String {
        let mut lines: Vec<String> = Vec::new();
        let mut used = 0;

        for message in messages.iter().rev() {
            if message.role == "system"
                || (Self::is_synthetic_message(message)
                    && Self::message_kind(message) != Some("compaction_summary"))
            {
                continue;
            }

            let Some(line) = Self::compactable_message_content(message) else {
                continue;
            };
            let line = format!("- {}", line);
            let line_tokens = Self::estimate_tokens(&line);
            if used + line_tokens > token_budget {
                break;
            }

            used += line_tokens;
            lines.push(line);
        }

        if lines.is_empty() {
            return "No prior context retained.".to_string();
        }

        lines.reverse();
        lines.join("\n")
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.session_manager.get_session(id).await
    }

    pub async fn get_child_sessions(&self, parent_id: &str) -> Result<Vec<Session>> {
        self.storage.get_child_sessions(parent_id)
    }

    pub async fn get_parent_session(&self, session_id: &str) -> Result<Option<Session>> {
        let session = self.storage.get_session(session_id)?;
        if let Some(s) = session {
            if let Some(parent_id) = &s.parent_id {
                return self.storage.get_session(parent_id);
            }
        }
        Ok(None)
    }

    pub async fn get_subagent_status(
        &self,
        session_id: &str,
    ) -> Result<(Session, crate::storage::SubagentTask, bool)> {
        let session = self
            .storage
            .get_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        let task = self
            .storage
            .get_subagent_task_by_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Subagent task not found".to_string()))?;

        let is_running = self.subagent_manager.is_subagent_running(session_id);

        Ok((session, task, is_running))
    }

    pub async fn get_subagent_result(
        &self,
        session_id: &str,
    ) -> Result<crate::storage::SubagentTask> {
        self.storage
            .get_subagent_task_by_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Subagent task not found".to_string()))
    }

    pub async fn cancel_subagent(&self, session_id: &str) -> Result<bool> {
        self.subagent_manager.cancel_subagent(session_id).await
    }

    pub async fn cleanup_completed_subagents(&self, days: i64) -> Result<usize> {
        self.subagent_manager.cleanup_completed(days).await
    }

    pub async fn list_subagent_tasks(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<crate::storage::SubagentTask>> {
        self.storage.list_subagent_tasks(parent_session_id)
    }

    pub fn is_any_subagent_running(&self, parent_session_id: &str) -> bool {
        self.subagent_manager
            .is_any_running_for_parent(parent_session_id)
    }

    pub fn list_personas(&self) -> Vec<persona::PersonaOption> {
        persona::persona_options()
    }

    pub async fn get_session_persona(&self, session_id: &str) -> Result<Option<ActivePersona>> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        Ok(Self::active_persona_from_session(&session))
    }

    pub async fn set_session_persona(
        &self,
        session_id: &str,
        persona_id: String,
        roleplay_character: Option<String>,
    ) -> Result<ActivePersona> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        let active = persona::resolve_active_persona(&persona_id, roleplay_character)
            .map_err(OSAgentError::ToolExecution)?;
        Self::write_active_persona_to_session(&mut session, Some(active.clone()))?;
        self.session_manager.update_session(&session).await?;

        Ok(active)
    }

    pub async fn reset_session_persona(&self, session_id: &str) -> Result<()> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        Self::write_active_persona_to_session(&mut session, None)?;
        self.session_manager.update_session(&session).await?;
        Ok(())
    }

    pub async fn get_current_model(&self) -> String {
        self.active_provider().await.current_model().await
    }

    pub async fn set_current_model(&self, model: String) {
        self.active_provider().await.set_model(model).await;
    }

    pub async fn switch_provider_model(&self, provider_id: String, model: String) -> Result<()> {
        let providers = self.providers.read().await;
        let new_provider = providers
            .iter()
            .find(|(id, _)| id == &provider_id)
            .map(|(_, p)| p.clone())
            .ok_or_else(|| OSAgentError::Config(format!("Provider '{}' not found", provider_id)))?;
        drop(providers);

        new_provider.set_model(model.clone()).await;

        let mut provider_lock = self.providers.write().await;
        *provider_lock = provider_lock
            .iter()
            .map(|(id, p)| {
                if id == &provider_id {
                    (id.clone(), new_provider.clone())
                } else {
                    (id.clone(), p.clone())
                }
            })
            .collect();
        drop(provider_lock);

        let mut cfg = self.config.write().await;
        cfg.set_active_provider_model(&provider_id, &model);
        drop(cfg);

        let mut active_provider = self.provider.write().await;
        *active_provider = new_provider;
        drop(active_provider);

        info!("Switched to provider={}, model={}", provider_id, model);
        Ok(())
    }

    pub async fn get_catalog_state(&self) -> crate::agent::model_catalog::CatalogState {
        let cfg = self.config.read().await;
        let catalog = self.catalog.clone();
        tokio::spawn(async move {
            catalog.refresh_catalog().await;
        });
        self.catalog.get_state(&cfg.providers)
    }

    pub async fn get_provider_models(
        &self,
        provider_id: String,
    ) -> Vec<crate::agent::model_catalog::ModelInfo> {
        self.catalog.get_models_for_provider(&provider_id)
    }

    pub async fn search_catalog_models(
        &self,
        query: String,
    ) -> Vec<crate::agent::model_catalog::ModelInfo> {
        self.catalog.search_models(&query)
    }

    pub async fn add_provider(&self, provider_config: crate::config::ProviderConfig) -> Result<()> {
        let catalog = self.catalog.clone();
        let provider_id = provider_config.provider_type.clone();
        let oauth_dir = PathBuf::from(
            shellexpand::tilde(&self.config.read().await.storage.database).to_string(),
        )
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
        let provider = Arc::new(
            OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
                provider_config.clone(),
                Some(catalog),
                Some(crate::oauth::create_oauth_storage(&oauth_dir)),
                self.agent_settings.clone(),
            )?,
        );
        provider.set_retry_listener(Some(Self::provider_retry_listener(
            Arc::new(self.event_bus.clone()),
            self.storage.clone(),
        )));

        let default_provider = self.config.read().await.default_provider.clone();
        let should_activate = default_provider.is_empty() || default_provider == provider_id;

        let mut providers = self.providers.write().await;
        providers.retain(|(id, _)| id != &provider_config.provider_type);
        providers.push((provider_config.provider_type.clone(), provider));
        drop(providers);

        let mut cfg = self.config.write().await;
        cfg.providers
            .retain(|p| p.provider_type != provider_config.provider_type);
        cfg.providers.push(provider_config.clone());
        if cfg.default_provider.is_empty() {
            cfg.default_provider = provider_config.provider_type.clone();
        }
        if cfg.default_provider == provider_id {
            cfg.default_model = provider_config.model.clone();
        }
        drop(cfg);

        if should_activate {
            let active = self
                .providers
                .read()
                .await
                .iter()
                .find(|(id, _)| id == &provider_id)
                .map(|(_, p)| p.clone())
                .ok_or_else(|| {
                    OSAgentError::Config(format!("Provider '{}' not found", provider_id))
                })?;
            let mut active_provider = self.provider.write().await;
            *active_provider = active;
        }

        Ok(())
    }

    pub async fn remove_provider(&self, provider_id: String) -> Result<()> {
        let mut providers = self.providers.write().await;
        let was_active = providers.iter().any(|(id, _)| id == &provider_id);
        providers.retain(|(id, _)| id != &provider_id);
        if providers.is_empty() {
            drop(providers);
            return Err(OSAgentError::Config(
                "Cannot remove the last provider".to_string(),
            ));
        }
        let first_provider_id = providers.first().map(|(id, _)| id.clone());
        drop(providers);

        let mut cfg = self.config.write().await;
        cfg.providers.retain(|p| p.provider_type != provider_id);
        if cfg.default_provider == provider_id {
            if let Some(ref first_id) = first_provider_id {
                cfg.default_provider = first_id.clone();
            }
            if let Some(first) = cfg.providers.first() {
                cfg.default_model = first.model.clone();
            }
        }
        drop(cfg);

        if was_active {
            let provider_guard = self.providers.read().await;
            if let Some((_, first)) = provider_guard.first() {
                let mut active_provider = self.provider.write().await;
                *active_provider = first.clone();
            }
        }

        Ok(())
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.session_manager.list_sessions().await
    }

    pub async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>> {
        self.session_manager.list_session_summaries().await
    }

    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::storage::SessionSearchHit>> {
        self.storage.search_messages(query, limit)
    }

    /// Drops messages from `from` onward so the next turn re-runs from that
    /// point. Refuses while a turn is in flight, since the running turn holds
    /// its own copy of the transcript and would write it straight back.
    pub async fn truncate_session_messages(&self, session_id: &str, from: usize) -> Result<()> {
        if self.active_runs.contains_key(session_id) {
            return Err(OSAgentError::Session(
                "Cannot edit history while the agent is running. Stop it first.".to_string(),
            ));
        }

        let session_lock = self.get_session_lock(session_id);
        let _lock_guard = session_lock.lock().await;
        self.storage.truncate_session_messages(session_id, from)
    }

    pub async fn enqueue_message(
        &self,
        session_id: &str,
        client_message_id: &str,
        content: &str,
        images: &[MessageImage],
        attachment_context: Option<&str>,
        attachments: &[MessageAttachment],
    ) -> Result<(QueuedMessage, bool)> {
        if self.get_session(session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }
        self.storage.enqueue_message(
            session_id,
            client_message_id,
            content,
            images,
            attachment_context,
            attachments,
        )
    }

    pub async fn list_queued_messages(&self, session_id: &str) -> Result<Vec<QueuedMessage>> {
        self.storage.list_queued_messages(session_id)
    }

    pub async fn list_session_history(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::StoredSessionEvent>> {
        self.storage.list_session_events(session_id)
    }

    pub async fn append_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<crate::storage::StoredSessionEvent> {
        self.storage
            .append_session_event(session_id, event_type, data)
    }

    pub async fn list_file_snapshots(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::FileSnapshotSummary>> {
        self.storage.list_file_snapshot_summaries(session_id)
    }

    pub async fn list_todo_items(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::tools::todo::TodoItem>> {
        self.storage.list_todo_items(session_id)
    }

    pub async fn list_message_feedback(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::MessageFeedback>> {
        self.storage.list_message_feedback(session_id)
    }

    pub async fn put_message_feedback(
        &self,
        session_id: &str,
        seq: i64,
        rating: &str,
        note: Option<&str>,
        if_version: Option<&str>,
    ) -> Result<crate::storage::FeedbackPutOutcome> {
        self.storage
            .put_message_feedback(session_id, seq, rating, note, if_version)
    }

    pub async fn delete_message_feedback(&self, session_id: &str, seq: i64) -> Result<bool> {
        self.storage.delete_message_feedback(session_id, seq)
    }

    pub async fn get_session_goal(&self, session_id: &str) -> Result<Option<crate::storage::Goal>> {
        self.goal_store.get(session_id)
    }

    pub async fn clear_session_goal(&self, session_id: &str) -> Result<bool> {
        self.goal_store.clear(session_id)
    }

    pub async fn revert_file_snapshot(
        &self,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<crate::storage::FileSnapshotSummary> {
        let summary = self
            .revert_snapshot_for_session(session_id, snapshot_id)
            .await?;

        if let Some(mut session) = self.session_manager.get_session(session_id).await? {
            self.record_session_event(
                &mut session,
                "snapshot_revert",
                serde_json::json!({
                    "snapshot_id": snapshot_id,
                    "paths": summary.paths.clone(),
                }),
            )?;
            self.session_manager.update_session(&session).await?;
        }

        Ok(summary)
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        // Cancel any in-progress operation
        self.cancel_session(id);
        // Cancel any child subagents
        self.subagent_manager.cancel_all_for_parent(id).await;
        // Clean up session locks and cancellation notifiers
        self.session_locks.remove(id);
        self.session_cancellation.remove(id);
        // Drop the session's tool activation so its loaded tools do not
        // linger in memory or leak into future sessions.
        self.tool_registry.prune_session_tools(id);
        self.session_manager.delete_session(id).await
    }

    pub async fn update_session(&self, session: &Session) -> Result<()> {
        self.session_manager.update_session(session).await
    }

    pub async fn set_discord_community_profile(
        &self,
        session_id: &str,
        context: String,
    ) -> Result<()> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session(format!("Session not found: {session_id}")))?;
        let metadata = session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Session("Session metadata is not an object".to_string())
        })?;
        metadata.insert(
            "discord_community".to_string(),
            serde_json::Value::Bool(true),
        );
        metadata.insert(
            "discord_community_context".to_string(),
            serde_json::Value::String(context),
        );
        self.session_manager.update_session(&session).await
    }

    pub async fn delete_all_sessions(&self) -> Result<()> {
        self.session_manager.delete_all_sessions().await
    }

    pub async fn list_checkpoints(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::Checkpoint>> {
        self.checkpoint_manager.list_checkpoints(session_id).await
    }

    pub async fn create_checkpoint(&self, session_id: &str) -> Result<crate::storage::Checkpoint> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;
        let cfg = self.config.read().await;
        let workspace = Self::resolve_workspace_for_session(&session, &cfg)
            .unwrap_or_else(|| cfg.get_active_workspace());
        drop(cfg);

        self.checkpoint_manager
            .create_checkpoint(
                &session,
                Some("manual".to_string()),
                None,
                Some(workspace.resolved_path()),
            )
            .await
    }

    pub async fn get_checkpoint_diffs(
        &self,
        checkpoint_id: &str,
    ) -> Result<Vec<crate::storage::CheckpointDiff>> {
        self.checkpoint_manager
            .checkpoint_diffs(checkpoint_id)
            .await
    }

    pub async fn list_checkpoint_changed_files(
        &self,
        checkpoint_id: &str,
    ) -> Result<Vec<std::path::PathBuf>> {
        self.checkpoint_manager
            .list_changed_files(checkpoint_id)
            .await
    }

    pub async fn compute_checkpoint_diff(
        &self,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
    ) -> Result<String> {
        self.checkpoint_manager
            .compute_diff(from_checkpoint_id, to_checkpoint_id)
            .await
    }

    pub async fn rollback(&self, checkpoint_id: &str) -> Result<Session> {
        self.checkpoint_manager.rollback(checkpoint_id).await
    }

    pub async fn get_database_path(&self) -> String {
        self.config.read().await.storage.database.clone()
    }

    pub fn get_subagent_manager(&self) -> Arc<SubagentManager> {
        self.subagent_manager.clone()
    }

    pub async fn get_config(&self) -> Config {
        self.config.read().await.clone()
    }

    pub async fn get_reasoning_state(
        &self,
        provider_id: &str,
        model: &str,
        selected: &str,
    ) -> crate::agent::reasoning::ThinkingOptionsState {
        let meta = self.catalog.lookup_reasoning_metadata(provider_id, model);
        crate::agent::reasoning::state_for(provider_id, model, meta.as_ref(), selected)
    }

    pub async fn get_workspaces(&self) -> Vec<WorkspaceConfig> {
        self.config.read().await.list_workspaces()
    }

    pub async fn get_active_workspace(&self) -> WorkspaceConfig {
        self.config.read().await.get_active_workspace()
    }

    pub async fn set_active_workspace(&self, workspace_id: &str) -> Result<WorkspaceConfig> {
        let mut cfg = self.config.write().await;
        cfg.ensure_workspace_defaults();

        let workspace = cfg
            .agent
            .workspaces
            .iter_mut()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| {
                OSAgentError::Config(format!("Workspace '{}' not found", workspace_id))
            })?;

        let workspace_id_owned = workspace.id.clone();
        let workspace_path_owned = workspace.resolved_path();
        workspace.last_used = Some(chrono::Utc::now().to_rfc3339());
        workspace.path = workspace_path_owned.clone();
        let workspace_out = workspace.clone();

        cfg.agent.active_workspace = Some(workspace_id_owned);
        cfg.agent.workspace = workspace_path_owned;
        Ok(workspace_out)
    }

    pub async fn set_provider_model_in_config(&self, model: String) {
        let mut cfg = self.config.write().await;
        cfg.default_model = model.clone();
        let default_provider = cfg.default_provider.clone();
        if let Some(p) = cfg
            .providers
            .iter_mut()
            .find(|p| p.provider_type == default_provider)
        {
            p.model = model;
        } else {
            cfg.provider.model = model;
        }
    }

    pub async fn replace_config(&self, mut config: Config) {
        {
            let existing = self.config.read().await;
            config.inherit_config_path(&existing);
        }
        config.ensure_workspace_defaults();
        {
            let mut agent_settings = self.agent_settings.write().await;
            *agent_settings = config.agent.clone();
        }
        let mut cfg = self.config.write().await;
        *cfg = config;
        if let Err(e) = self.memory_store.set_config(
            cfg.agent.memory_enabled,
            cfg.agent.memory_file.clone(),
            cfg.agent.learning_mode,
        ) {
            warn!("Failed to update memory state: {}", e);
        }
        if let Err(e) = self.decision_memory.set_config(
            cfg.agent.decision_memory_enabled,
            cfg.agent.decision_memory_file.clone(),
            cfg.agent.learning_mode,
            cfg.agent.decision_capture_mode,
        ) {
            warn!("Failed to update decision memory state: {}", e);
        }
    }

    pub fn signal_shutdown(&self) {
        let _ = self.shutdown_tx.send(true);
    }

    pub fn is_shutdown_signaled(&self) -> bool {
        *self.shutdown_tx.borrow()
    }

    pub fn subscribe_shutdown(&self) -> tokio::sync::watch::Receiver<bool> {
        self.shutdown_tx.subscribe()
    }

    pub fn get_restart_sender(&self) -> Option<tokio::sync::oneshot::Sender<()>> {
        self.restart_tx.lock().unwrap().take()
    }

    pub fn set_restart_sender(&self, sender: tokio::sync::oneshot::Sender<()>) {
        *self.restart_tx.lock().unwrap() = Some(sender);
    }

    pub async fn discord_config(&self) -> Option<crate::config::DiscordConfig> {
        self.config.read().await.discord.clone()
    }

    pub async fn add_workspace(&self, workspace: WorkspaceConfig) -> Result<WorkspaceConfig> {
        let mut cfg = self.config.write().await;
        cfg.add_workspace(workspace.clone())?;
        Ok(cfg.get_workspace(&workspace.id).unwrap_or(workspace))
    }

    pub async fn update_workspace(&self, workspace: WorkspaceConfig) -> Result<WorkspaceConfig> {
        let mut cfg = self.config.write().await;
        cfg.update_workspace(workspace.clone())?;
        Ok(cfg.get_workspace(&workspace.id).unwrap_or(workspace))
    }

    pub async fn remove_workspace(&self, workspace_id: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.remove_workspace(workspace_id)
    }

    pub async fn add_workspace_path(&self, workspace_id: &str, path: WorkspacePath) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.add_workspace_path(workspace_id, path)
    }

    pub async fn update_workspace_path(
        &self,
        workspace_id: &str,
        path_index: usize,
        path: WorkspacePath,
    ) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.update_workspace_path(workspace_id, path_index, path)
    }

    pub async fn remove_workspace_path(&self, workspace_id: &str, path_index: usize) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.remove_workspace_path(workspace_id, path_index)
    }

    pub async fn get_workspace(&self, workspace_id: &str) -> Result<WorkspaceConfig> {
        let cfg = self.config.read().await;
        cfg.get_workspace(workspace_id)
            .ok_or_else(|| OSAgentError::Config(format!("Workspace '{}' not found", workspace_id)))
    }

    pub async fn get_session_workspace(&self, session_id: &str) -> Result<WorkspaceConfig> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;
        let cfg = self.config.read().await;
        Ok(Self::resolve_workspace_for_session(&session, &cfg)
            .unwrap_or_else(|| cfg.get_active_workspace()))
    }

    pub async fn set_session_workspace(
        &self,
        session_id: &str,
        workspace_id: &str,
    ) -> Result<WorkspaceConfig> {
        info!(
            "set_session_workspace: Setting workspace {} for session {}",
            workspace_id, session_id
        );

        if self.active_runs.contains_key(session_id) {
            return Err(OSAgentError::Session(
                "Cannot switch workspace while this session is running. Stop it or wait for the current turn to finish.".to_string(),
            ));
        }

        let workspace = {
            let cfg = self.config.read().await;
            cfg.get_workspace(workspace_id).ok_or_else(|| {
                OSAgentError::Config(format!("Workspace '{}' not found", workspace_id))
            })?
        };

        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        info!(
            "set_session_workspace: Session loaded, current metadata: {:?}",
            session.metadata
        );

        Self::set_session_workspace_id(&mut session, workspace_id)?;

        info!(
            "set_session_workspace: Session metadata after update: {:?}",
            session.metadata
        );

        self.session_manager.update_session(&session).await?;

        info!("set_session_workspace: Session updated successfully");

        Ok(workspace)
    }

    pub async fn get_session_id_for_user(&self, user_key: &str) -> Result<Option<String>> {
        let sessions = self.session_manager.list_sessions().await?;
        for session in sessions {
            if session
                .metadata
                .get("owner")
                .and_then(|v| v.as_str())
                .map(|v| v == user_key)
                .unwrap_or(false)
                && !Self::session_archived(&session)
            {
                return Ok(Some(session.id));
            }
        }
        Ok(None)
    }

    pub async fn get_or_create_session_for_user(&self, user_key: &str) -> Result<Session> {
        if let Some(existing) = self.get_session_id_for_user(user_key).await? {
            if let Some(session) = self.get_session(&existing).await? {
                return Ok(session);
            }
        }

        self.create_session_for_user(user_key, "discord").await
    }

    pub async fn create_session_for_user(&self, user_key: &str, source: &str) -> Result<Session> {
        let mut session = self.create_session().await?;
        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }
        if let Some(meta) = session.metadata.as_object_mut() {
            meta.insert(
                "owner".to_string(),
                serde_json::Value::String(user_key.to_string()),
            );
            meta.insert(
                "source".to_string(),
                serde_json::Value::String(source.to_string()),
            );
            meta.insert("archived".to_string(), serde_json::Value::Bool(false));
            meta.remove("archived_at");
        }
        self.session_manager.update_session(&session).await?;
        Ok(session)
    }

    pub async fn archive_session(&self, session_id: &str) -> Result<()> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        if !session.metadata.is_object() {
            session.metadata = serde_json::json!({});
        }

        let metadata = session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Parse("Session metadata must be a JSON object".to_string())
        })?;

        metadata.insert("archived".to_string(), serde_json::Value::Bool(true));
        metadata.insert(
            "archived_at".to_string(),
            serde_json::Value::String(Utc::now().to_rfc3339()),
        );

        self.session_manager.update_session(&session).await
    }

    pub async fn save_config(&self, config_path: &std::path::Path) -> Result<()> {
        let cfg = self.config.read().await.clone();
        cfg.save(config_path)
    }

    pub fn memory_status(&self) -> MemoryStatus {
        self.memory_store.status()
    }

    pub async fn list_memories(&self) -> Result<Vec<MemoryEntry>> {
        self.memory_store.list().await
    }

    pub async fn add_memory(
        &self,
        title: String,
        content: String,
        tags: Vec<String>,
        category: Option<MemoryCategory>,
        confirmed: bool,
        source: String,
    ) -> Result<MemoryEntry> {
        self.memory_store
            .add(title, content, tags, category, confirmed, source)
            .await
    }

    pub async fn update_memory(
        &self,
        id: &str,
        title: Option<String>,
        content: Option<String>,
        tags: Option<Vec<String>>,
        category: Option<MemoryCategory>,
        confirmed: Option<bool>,
    ) -> Result<MemoryEntry> {
        self.memory_store
            .update(id, title, content, tags, category, confirmed)
            .await
    }

    pub async fn delete_memory(&self, id: &str) -> Result<bool> {
        self.memory_store.delete(id).await
    }

    pub async fn list_memory_suggestions(&self) -> Result<Vec<MemorySuggestion>> {
        self.memory_store.list_suggestions().await
    }

    pub async fn approve_memory_suggestion(&self, id: &str, actor: String) -> Result<MemoryEntry> {
        self.memory_store.approve_suggestion(id, actor).await
    }

    pub async fn reject_memory_suggestion(
        &self,
        id: &str,
        actor: String,
        note: Option<String>,
    ) -> Result<bool> {
        self.memory_store.reject_suggestion(id, actor, note).await
    }

    pub async fn list_decision_suggestions(&self) -> Result<Vec<DecisionSuggestion>> {
        self.decision_memory.list_suggestions().await
    }

    pub async fn approve_decision_suggestion(
        &self,
        id: &str,
        actor: String,
    ) -> Result<crate::agent::decision_memory::DecisionEntry> {
        self.decision_memory.approve_suggestion(id, actor).await
    }

    pub async fn reject_decision_suggestion(
        &self,
        id: &str,
        actor: String,
        note: Option<String>,
    ) -> Result<bool> {
        self.decision_memory
            .reject_suggestion(id, actor, note)
            .await
    }

    fn requested_absolute_path(
        tool_name: &str,
        args: &serde_json::Value,
        workspace_root: Option<&Path>,
    ) -> Option<String> {
        if tool_name == "bash" || tool_name == "process" {
            if let Some(workdir) = args.get("workdir").and_then(|value| value.as_str()) {
                if let Some(resolved) = Self::resolve_key_path(workdir, workspace_root) {
                    return Some(resolved);
                }
            }

            let command = args.get("command").and_then(|value| value.as_str())?;
            for token in Self::extract_command_absolute_paths(command) {
                let is_inside = workspace_root
                    .map(|root| Path::new(&token).starts_with(root))
                    .unwrap_or(false);
                if !is_inside {
                    return Some(token);
                }
            }
            return None;
        }

        let key = match tool_name {
            "read_file" => args
                .get("filePath")
                .and_then(|value| value.as_str())
                .or_else(|| args.get("path").and_then(|value| value.as_str())),
            "write_file" | "edit_file" | "delete_file" | "list_files" | "grep" | "glob" => {
                args.get("path").and_then(|value| value.as_str())
            }
            _ => None,
        }?;

        Self::resolve_key_path(key, workspace_root)
    }

    fn resolve_key_path(key: &str, workspace_root: Option<&Path>) -> Option<String> {
        let expanded = shellexpand::tilde(key).to_string();
        if Path::new(&expanded).is_absolute() {
            return Some(expanded);
        }

        if let Some(root) = workspace_root {
            if expanded.contains("..") {
                let resolved = root.join(&expanded);
                let canonical = resolved.canonicalize().unwrap_or(resolved);
                if canonical.is_absolute() {
                    return Some(canonical.to_string_lossy().to_string());
                }
            }
        }

        None
    }

    /// Expands the environment-variable forms a shell would resolve *after*
    /// this check runs (`%VAR%`, `$env:VAR`, `$VAR`, `~`), so a path like
    /// `%USERPROFILE%\Documents` is recognised as the absolute path it will
    /// actually become. Unknown variables are left untouched.
    fn expand_path_vars(token: &str) -> String {
        // %VAR% (cmd)
        let mut out = String::new();
        let mut rest = token;
        while let Some(start) = rest.find('%') {
            let after = &rest[start + 1..];
            let Some(end) = after.find('%') else { break };
            let name = &after[..end];
            out.push_str(&rest[..start]);
            match std::env::var(name) {
                Ok(value) if !name.is_empty() => out.push_str(&value),
                _ => {
                    out.push('%');
                    out.push_str(name);
                    out.push('%');
                }
            }
            rest = &after[end + 1..];
        }
        out.push_str(rest);

        // $env:VAR (PowerShell) and $VAR (POSIX)
        let mut expanded = String::new();
        let mut rest = out.as_str();
        while let Some(start) = rest.find('$') {
            expanded.push_str(&rest[..start]);
            let after = &rest[start + 1..];
            let (body, prefix_len) = match after.strip_prefix("env:") {
                Some(stripped) => (stripped, "env:".len()),
                None => (after, 0),
            };
            let name_len = body
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(body.len());
            let name = &body[..name_len];
            match std::env::var(name) {
                Ok(value) if !name.is_empty() => expanded.push_str(&value),
                _ => expanded.push_str(&rest[start..start + 1 + prefix_len + name_len]),
            }
            rest = &body[name_len..];
        }
        expanded.push_str(rest);

        shellexpand::tilde(&expanded).to_string()
    }

    /// Heuristically scans a shell command string for tokens that look like
    /// absolute filesystem paths (Windows drive-letter, UNC, or POSIX-style),
    /// so outside-workspace approval can trigger even when the path is
    /// embedded in the command text rather than passed via `workdir`.
    /// Environment-variable forms are expanded first. Still best-effort: it
    /// cannot resolve globs or paths built up dynamically inside the command.
    fn extract_command_absolute_paths(command: &str) -> Vec<String> {
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;

        for ch in command.chars() {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                c if c.is_whitespace() && !in_single && !in_double => {
                    if !current.is_empty() {
                        tokens.push(std::mem::take(&mut current));
                    }
                }
                c => current.push(c),
            }
        }
        if !current.is_empty() {
            tokens.push(current);
        }

        tokens
            .into_iter()
            .map(|token| Self::expand_path_vars(&token))
            .filter(|token| Self::looks_like_absolute_path(token))
            .collect()
    }

    fn looks_like_absolute_path(token: &str) -> bool {
        let bytes = token.as_bytes();
        if bytes.len() >= 3
            && bytes[0].is_ascii_alphabetic()
            && bytes[1] == b':'
            && (bytes[2] == b'\\' || bytes[2] == b'/')
        {
            return true;
        }
        if token.starts_with("\\\\") && token.len() > 2 {
            return true;
        }
        if cfg!(not(windows)) && token.starts_with('/') && token.len() > 1 {
            return true;
        }
        false
    }

    fn path_is_inside_workspace(path: &Path, workspace: &WorkspaceConfig) -> bool {
        workspace.paths.iter().any(|entry| {
            let root = PathBuf::from(shellexpand::tilde(&entry.path).to_string());
            match (path.canonicalize(), root.canonicalize()) {
                (Ok(path), Ok(root)) => path.starts_with(root),
                _ => path.starts_with(root),
            }
        })
    }

    fn external_execution_root(tool_name: &str, path: &str) -> String {
        let path = Path::new(path);
        if matches!(
            tool_name,
            "list_files" | "grep" | "glob" | "bash" | "process"
        ) || path.is_dir()
        {
            return path.to_string_lossy().to_string();
        }
        path.parent().unwrap_or(path).to_string_lossy().to_string()
    }

    /// Policy gate for the `sessions` tool. Reading or searching
    /// conversations other than the caller's own (its own subagent children
    /// are exempt) follows `session_access` permission rules first, then
    /// `[agent] session_access_default_action`; Ask raises the same approval
    /// popup used for outside-workspace path access, addressed as
    /// `session://<id>` (or `session://*` for broad search).
    async fn authorize_session_access(
        &self,
        caller_session_id: &str,
        tool_call_id: &str,
        args: &serde_json::Value,
    ) -> Result<()> {
        let action = args
            .get("action")
            .and_then(|value| value.as_str())
            .unwrap_or("read");
        let target = args
            .get("target_session_id")
            .and_then(|value| value.as_str());
        let scope = crate::tools::sessions::resolve_access_scope(caller_session_id, action, target);
        let resource = match scope {
            crate::tools::sessions::SessionAccessScope::Ungated
            | crate::tools::sessions::SessionAccessScope::Exempt => return Ok(()),
            crate::tools::sessions::SessionAccessScope::Resource(resource) => resource,
        };

        // A read of a session that does not exist needs no approval — the
        // tool answers with a clean not-found error instead of a popup.
        if let Some(id) = resource.strip_prefix("session://") {
            if id != "*" {
                match self.storage.get_session(id)? {
                    None => return Ok(()),
                    Some(other) => {
                        // The caller's own subagent children are part of
                        // its task tree and need no prompt.
                        if other.parent_id.as_deref() == Some(caller_session_id) {
                            return Ok(());
                        }
                    }
                }
            }
        }

        if self
            .external_manager
            .has_granted_permission(&resource)
            .await
        {
            return Ok(());
        }

        let effective = {
            let config = self.config.read().await;
            config
                .evaluate_permission_rule("session_access", &resource)
                .unwrap_or_else(|| config.agent.session_access_default_action.clone())
        };
        // Note: `evaluate_permission_rule` returns the `permission` module's
        // action enum; the external-path flow uses the identical-looking
        // `external` module one.
        match effective {
            crate::permission::PermissionAction::Allow => return Ok(()),
            crate::permission::PermissionAction::Deny => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Access to conversation '{}' is denied by policy",
                    resource
                )))
            }
            crate::permission::PermissionAction::Ask => {}
        }

        let (prompt, response) = self
            .external_manager
            .create_waiting_prompt(
                caller_session_id.to_string(),
                format!("sessions:{}", tool_call_id),
                resource.clone(),
                "session_read".to_string(),
                Vec::new(),
            )
            .await;

        let timeout =
            std::time::Duration::from_secs(self.external_manager.prompt_timeout_seconds().max(1));
        let cancel_notify = self.get_cancellation_notify(caller_session_id);
        let response = tokio::select! {
            response = tokio::time::timeout(timeout, response) => response,
            _ = cancel_notify.notified() => {
                self.external_manager.expire_prompt(&prompt.id).await;
                return Err(OSAgentError::Session("Operation cancelled".to_string()));
            }
        };
        match response {
            Ok(Ok(true)) => Ok(()),
            Ok(Ok(false)) => Err(OSAgentError::ToolExecution(format!(
                "User denied access to conversation '{}'",
                resource
            ))),
            Ok(Err(_)) => Err(OSAgentError::ToolExecution(
                "Session access permission request was cancelled".to_string(),
            )),
            Err(_) => {
                self.external_manager.expire_prompt(&prompt.id).await;
                Err(OSAgentError::ToolExecution(format!(
                    "Permission request for '{}' timed out",
                    resource
                )))
            }
        }
    }

    async fn authorize_external_paths(
        &self,
        session_id: &str,
        tool_name: &str,
        tool_call_id: &str,
        args: &serde_json::Value,
        workspace: &WorkspaceConfig,
    ) -> Result<(Vec<String>, Option<String>)> {
        let workspace_root =
            PathBuf::from(shellexpand::tilde(&workspace.resolved_path()).to_string());
        let Some(path) = Self::requested_absolute_path(tool_name, args, Some(&workspace_root))
        else {
            return Ok((Vec::new(), None));
        };
        if Self::path_is_inside_workspace(Path::new(&path), workspace) {
            return Ok((Vec::new(), None));
        }
        let execution_root = Self::external_execution_root(tool_name, &path);
        if self.external_manager.has_granted_permission(&path).await {
            return Ok((vec![execution_root], Some(path)));
        }

        let action = self
            .external_manager
            .evaluate(&path, &workspace.resolved_path());
        match action {
            PermissionAction::Allow => return Ok((vec![execution_root], Some(path))),
            PermissionAction::Deny => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Access to external path '{}' is denied by policy",
                    path
                )))
            }
            PermissionAction::Ask => {}
        }

        let path_type = if matches!(tool_name, "write_file" | "edit_file" | "delete_file") {
            "write"
        } else if matches!(tool_name, "bash" | "process") {
            "execute"
        } else {
            "read"
        };
        let parent_pattern = Path::new(&path)
            .parent()
            .map(|parent| format!("{}{}**", parent.display(), std::path::MAIN_SEPARATOR))
            .unwrap_or_else(|| path.clone());
        let (prompt, response) = self
            .external_manager
            .create_waiting_prompt(
                session_id.to_string(),
                format!("{}:{}", tool_name, tool_call_id),
                path.clone(),
                path_type.to_string(),
                vec![parent_pattern],
            )
            .await;

        let timeout =
            std::time::Duration::from_secs(self.external_manager.prompt_timeout_seconds().max(1));
        let cancel_notify = self.get_cancellation_notify(session_id);
        let response = tokio::select! {
            response = tokio::time::timeout(timeout, response) => response,
            _ = cancel_notify.notified() => {
                self.external_manager.expire_prompt(&prompt.id).await;
                return Err(OSAgentError::Session("Operation cancelled".to_string()));
            }
        };
        match response {
            Ok(Ok(true)) => Ok((vec![execution_root], Some(path))),
            Ok(Ok(false)) => Err(OSAgentError::ToolExecution(format!(
                "User denied access to external path '{}'",
                path
            ))),
            Ok(Err(_)) => Err(OSAgentError::ToolExecution(
                "External path permission request was cancelled".to_string(),
            )),
            Err(_) => {
                self.external_manager.expire_prompt(&prompt.id).await;
                Err(OSAgentError::ToolExecution(format!(
                    "Permission request for '{}' timed out",
                    path
                )))
            }
        }
    }

    pub async fn check_external_directory_permission(&self, path: &str) -> PermissionAction {
        let cfg = self.config.read().await;
        let workspace = cfg.get_active_workspace();
        drop(cfg);
        self.external_manager
            .evaluate(path, &workspace.resolved_path())
    }

    pub async fn create_permission_prompt(
        &self,
        session_id: String,
        source: String,
        path: String,
        path_type: String,
        patterns: Vec<String>,
    ) -> PermissionPrompt {
        self.external_manager
            .create_prompt(session_id, source, path, path_type, patterns)
            .await
    }

    pub async fn respond_to_permission_prompt(
        &self,
        prompt_id: &str,
        allowed: bool,
        always: bool,
    ) -> Option<PermissionPrompt> {
        self.external_manager
            .respond_to_prompt(prompt_id, allowed, always)
            .await
    }

    pub async fn get_pending_permission_prompts(&self) -> Vec<PermissionPrompt> {
        self.external_manager.get_pending_prompts().await
    }

    pub async fn get_session_permission_prompts(&self, session_id: &str) -> Vec<PermissionPrompt> {
        self.external_manager.get_session_prompts(session_id).await
    }

    pub async fn has_granted_external_permission(&self, path: &str) -> bool {
        self.external_manager.has_granted_permission(path).await
    }

    /// Configured servers joined with their live connection state, for
    /// the settings UI. Servers that failed to start appear here with
    /// their error rather than silently missing.
    pub async fn mcp_status(&self) -> serde_json::Value {
        let config = self.config.read().await.clone();
        let manager = self.tool_registry.mcp_manager();
        let summaries = manager
            .as_ref()
            .map(|manager| manager.server_summaries())
            .unwrap_or_default();

        let servers: Vec<serde_json::Value> = config
            .mcp
            .servers
            .iter()
            .map(|server| {
                let live = summaries.iter().find(|summary| summary.name == server.name);
                serde_json::json!({
                    "name": server.name,
                    "enabled": server.enabled,
                    "transport": match server.transport_kind() {
                        crate::config::McpTransport::Http => "http",
                        crate::config::McpTransport::Stdio => "stdio",
                    },
                    "command": server.command,
                    "args": server.args,
                    "env": server.env,
                    "cwd": server.cwd,
                    "url": server.url,
                    "headers_keys": server.headers.keys().collect::<Vec<_>>(),
                    "timeout_seconds": server.timeout_seconds,
                    "description": server.description,
                    "always_active": server.always_active,
                    "connected": live.map(|summary| summary.connected).unwrap_or(false),
                    "tool_count": live.map(|summary| summary.tool_count).unwrap_or(0),
                    "blurb": live.map(|summary| summary.blurb.clone()),
                    "error": live.and_then(|summary| summary.error.clone()),
                })
            })
            .collect();

        serde_json::json!({
            "enabled": config.mcp.enabled,
            "connecting": manager.is_none() && !config.mcp.servers.is_empty(),
            "catalog_size": manager.as_ref().map(|m| m.catalog_size()).unwrap_or(0),
            "activated": manager.as_ref().map(|m| m.all_activated_names()).unwrap_or_default(),
            "max_activated_tools": config.mcp.max_activated_tools,
            "servers": servers,
        })
    }

    /// The catalog, for browsing in the UI. Never enters the model's
    /// context — that is the entire point of the deferred design.
    pub async fn mcp_catalog(&self, server: Option<String>) -> Vec<serde_json::Value> {
        let Some(manager) = self.tool_registry.mcp_manager() else {
            return Vec::new();
        };
        let activated = manager.all_activated_names();
        manager
            .entries()
            .into_iter()
            .filter(|entry| {
                server
                    .as_ref()
                    .map(|filter| &entry.server == filter)
                    .unwrap_or(true)
            })
            .map(|entry| {
                serde_json::json!({
                    "name": entry.qualified_name,
                    "server": entry.server,
                    "tool": entry.tool,
                    "description": entry.description,
                    "read_only": entry.read_only,
                    "activated": activated.contains(&entry.qualified_name),
                })
            })
            .collect()
    }

    /// Add or replace a server by name, persist it, and reconnect.
    ///
    /// Validation runs before anything is written so a typo in the UI
    /// surfaces as a message rather than a dead server in the config.
    pub async fn upsert_mcp_server(&self, server: crate::config::McpServerConfig) -> Result<()> {
        server.validate()?;

        {
            let mut config = self.config.write().await;
            match config
                .mcp
                .servers
                .iter_mut()
                .find(|existing| existing.name == server.name)
            {
                Some(existing) => *existing = server,
                None => config.mcp.servers.push(server),
            }
            config.save_to_current_path()?;
        }

        self.reload_mcp_servers().await
    }

    pub async fn remove_mcp_server(&self, name: &str) -> Result<()> {
        {
            let mut config = self.config.write().await;
            let before = config.mcp.servers.len();
            config.mcp.servers.retain(|server| server.name != name);
            if config.mcp.servers.len() == before {
                return Err(OSAgentError::Config(format!(
                    "No MCP server named '{}'",
                    name
                )));
            }
            config.save_to_current_path()?;
        }

        self.reload_mcp_servers().await
    }

    pub async fn set_mcp_server_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        {
            let mut config = self.config.write().await;
            let server = config
                .mcp
                .servers
                .iter_mut()
                .find(|server| server.name == name)
                .ok_or_else(|| OSAgentError::Config(format!("No MCP server named '{}'", name)))?;
            server.enabled = enabled;
            config.save_to_current_path()?;
        }

        self.reload_mcp_servers().await
    }

    /// Tear down the current MCP connections and rebuild from config.
    ///
    /// Synchronous by design: the UI needs the connection result to show
    /// whether the server the user just added actually works.
    pub async fn reload_mcp_servers(&self) -> Result<()> {
        let mcp_config = self.config.read().await.mcp.clone();
        let manager = crate::mcp::McpManager::connect(&mcp_config).await;
        if let Some(previous) = self.tool_registry.register_mcp(manager) {
            previous.shutdown().await;
        }
        Ok(())
    }

    /// Connect to a candidate server without saving it, so the UI can
    /// offer a "test connection" button before the user commits.
    pub async fn test_mcp_server(
        &self,
        server: crate::config::McpServerConfig,
    ) -> Result<serde_json::Value> {
        server.validate()?;

        let probe = crate::config::McpConfig {
            enabled: true,
            servers: vec![server.clone()],
            ..Default::default()
        };
        let manager = crate::mcp::McpManager::connect(&probe).await;
        let summary = manager
            .server_summaries()
            .into_iter()
            .next()
            .ok_or_else(|| OSAgentError::Config("Server did not report a status".to_string()))?;
        let sample: Vec<String> = manager
            .search(&server.name, 8)
            .into_iter()
            .map(|entry| entry.tool)
            .collect();
        manager.shutdown().await;

        Ok(serde_json::json!({
            "connected": summary.connected,
            "tool_count": summary.tool_count,
            "blurb": summary.blurb,
            "error": summary.error,
            "sample_tools": sample,
        }))
    }

    pub async fn get_permission_rules(&self) -> Vec<crate::permission::PermissionRule> {
        let cfg = self.config.read().await;
        cfg.get_permission_rules()
    }

    pub async fn add_permission_rule(&self, rule: crate::permission::PermissionRule) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.add_permission_rule(rule)
    }

    pub async fn remove_permission_rule(&self, rule_id: &str) -> Result<()> {
        let mut cfg = self.config.write().await;
        cfg.remove_permission_rule(rule_id)
    }

    pub async fn get_plugins(&self) -> Vec<crate::plugin::LoadedPlugin> {
        self.plugin_manager.list_plugins().await
    }

    pub async fn list_plugins(&self) -> Vec<crate::plugin::LoadedPlugin> {
        self.plugin_manager.list_plugins().await
    }

    pub async fn enable_plugin(&self, name: &str) -> std::result::Result<(), String> {
        self.plugin_manager.enable_plugin(name).await
    }

    pub async fn disable_plugin(&self, name: &str) -> std::result::Result<(), String> {
        self.plugin_manager.disable_plugin(name).await
    }

    pub async fn reload_plugins(&self) -> std::result::Result<(), String> {
        self.plugin_manager.load_all().await
    }

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
                    let session_id = match req.session_id {
                        Some(sid) => sid,
                        None => {
                            let mut session = match this.create_session().await {
                                Ok(s) => s,
                                Err(e) => {
                                    warn!("Scheduler run_prompt: failed to create session: {}", e);
                                    if let Some(tx) = req.response_tx {
                                        let _ = tx.send(format!("Failed to create session: {}", e));
                                    }
                                    continue;
                                }
                            };
                            if let Some(source) = &req.source {
                                if !session.metadata.is_object() {
                                    session.metadata = serde_json::json!({});
                                }
                                if let Some(meta) = session.metadata.as_object_mut() {
                                    meta.insert(
                                        "source".to_string(),
                                        serde_json::Value::String(source.clone()),
                                    );
                                }
                                if let Err(e) = this.session_manager.update_session(&session).await
                                {
                                    warn!("Failed to set session source: {}", e);
                                }
                            }
                            info!("Scheduler run_prompt: created session {}", session.id);
                            session.id
                        }
                    };

                    info!("Scheduler run_prompt: executing in session {}", session_id);
                    let result = this
                        .process_message(&session_id, req.prompt, "scheduler".to_string())
                        .await;

                    if let Some(tx) = req.response_tx {
                        let response = match &result {
                            Ok(r) => r.clone(),
                            Err(e) => format!("Error: {}", e),
                        };
                        let _ = tx.send(response);
                    }

                    if let Err(e) = result {
                        error!(
                            "Scheduler run_prompt failed for session {}: {}",
                            session_id, e
                        );
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

#[cfg(test)]
mod external_path_scan_tests {
    use super::{AgentRuntime, ToolOutcome};
    use std::path::PathBuf;

    fn ws() -> PathBuf {
        PathBuf::from(r"I:\GhostESP2\Research-Projects\Pwn-Power")
    }

    fn scan(cmd: &str) -> Option<String> {
        let args = serde_json::json!({ "command": cmd });
        AgentRuntime::requested_absolute_path("bash", &args, Some(&ws()))
    }

    #[test]
    fn tool_context_starts_with_model_visible_outcome() {
        let rendered = AgentRuntime::summarize_tool_output_for_context(
            "bash",
            ToolOutcome::Failure,
            "Exit code: 1",
        );
        assert!(rendered.starts_with("[tool outcome: failure]\nTool: bash"));
        assert!(rendered.contains("Exit code: 1"));
    }

    #[test]
    fn detects_env_var_paths() {
        // This test is Windows-specific (USERPROFILE, backslashes). Skip on non-Windows
        // where the scan logic treats those paths differently and env var is absent.
        if cfg!(not(windows)) {
            return;
        }
        let profile = std::env::var("USERPROFILE").expect("USERPROFILE set on Windows");
        for cmd in [
            r#"dir "%USERPROFILE%\Documents""#,
            r#"dir %USERPROFILE%\Documents"#,
            r#"powershell -Command Get-ChildItem $env:USERPROFILE\Documents"#,
        ] {
            let got = scan(cmd).unwrap_or_else(|| panic!("no external path found in: {cmd}"));
            assert!(
                got.starts_with(&profile),
                "expected expansion under {profile}, got {got} for: {cmd}"
            );
        }
    }

    #[test]
    fn detects_literal_absolute_paths() {
        assert_eq!(
            scan(r#"dir "C:\Users\deki\Documents""#),
            Some(r"C:\Users\deki\Documents".to_string())
        );
    }

    #[test]
    fn ignores_workspace_internal_and_plain_commands() {
        assert_eq!(scan("cargo test --lib"), None);
        assert_eq!(scan("git status"), None);
        // Workspace-internal Windows path check is only meaningful on Windows where
        // backslashes are path separators. On Unix the same string is parsed as
        // an external absolute path, so only test it on Windows.
        if cfg!(windows) {
            assert_eq!(
                scan(r#"dir "I:\GhostESP2\Research-Projects\Pwn-Power\src""#),
                None
            );
        }
    }
}
