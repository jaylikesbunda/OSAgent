use crate::agent::checkpoint::CheckpointManager;
use crate::agent::coordinator::Coordinator;
use crate::agent::decision_memory::{DecisionCaptureOutcome, DecisionMemory, DecisionSuggestion};
use crate::agent::events::{AgentEvent, EventBus, EventTokenUsage, ToolStatus};
use crate::agent::instruction::{
    format_system_reminder, git_workspace_context, global_instruction_blocks,
    workspace_instruction_blocks,
};
use crate::agent::memory::{
    MemoryCategory, MemoryEntry, MemoryScope, MemoryStatus, MemoryStore, MemorySuggestion,
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
use crate::scheduler::executor::RunPromptResponse;
use crate::scheduler::Scheduler;
use crate::skills::{get_skills_base_dir, SkillLoader};
use crate::storage::{
    AuditEntry, Message, MessageAttachment, MessageImage, MessageTokens, QueuedMessage, Session,
    SessionEventRecord, SessionSummary, SqliteStorage, ToolCall,
};
use crate::tools::bash::BashTool;
use crate::tools::file_cache::FileReadCache;
use crate::tools::guard::ensure_relative_path_not_backups;
use crate::tools::loop_detect::{
    LoopDetectionConfig, LoopDetectionResult, LoopSeverity, ToolLoopDetector,
};
use crate::tools::output::path_touches_tool_outputs;
use crate::tools::registry::{ToolOutcome, ToolProfile, ToolRegistry, ToolResult};
use crate::tools::truncation::{self, TruncationOptions};
use chrono::Utc;
use dashmap::DashMap;
use futures::{future::join_all, StreamExt};
use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Instant, SystemTime};
use tokio::sync::{watch, Mutex, Notify};
use tracing::{debug, error, info, warn};
use uuid::Uuid;

async fn wait_for_cancel_signal(notify: &Notify, flag: &std::sync::atomic::AtomicBool) {
    let notified = notify.notified();
    tokio::pin!(notified);
    // Register before checking the persistent flag, closing the lost-wake gap.
    notified.as_mut().enable();
    if flag.load(std::sync::atomic::Ordering::Acquire) {
        return;
    }
    notified.await;
}

#[cfg(test)]
mod cancellation_signal_tests {
    use super::*;

    #[tokio::test]
    async fn cancellation_before_waiter_registration_is_not_lost() {
        let notify = Notify::new();
        let flag = std::sync::atomic::AtomicBool::new(true);
        notify.notify_waiters();
        tokio::time::timeout(
            std::time::Duration::from_millis(100),
            wait_for_cancel_signal(&notify, &flag),
        )
        .await
        .unwrap();
    }

    #[tokio::test]
    async fn cancellation_releases_all_registered_tool_waiters() {
        let notify = Arc::new(Notify::new());
        let flag = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut tasks = Vec::new();
        for _ in 0..3 {
            let notify = notify.clone();
            let flag = flag.clone();
            tasks.push(tokio::spawn(async move {
                wait_for_cancel_signal(&notify, &flag).await
            }));
        }
        tokio::task::yield_now().await;
        flag.store(true, std::sync::atomic::Ordering::Release);
        notify.notify_waiters();
        for task in tasks {
            tokio::time::timeout(std::time::Duration::from_millis(100), task)
                .await
                .unwrap()
                .unwrap();
        }
    }
}

// A finish_reason completes the model output, but usage can arrive in a
// trailing event. Bound that drain independently of the normal idle timeout.
async fn next_provider_stream_event(
    stream: &mut futures::stream::BoxStream<'static, Result<StreamEvent>>,
    finish_deadline: Option<tokio::time::Instant>,
    idle_timeout: std::time::Duration,
) -> Option<Result<StreamEvent>> {
    let deadline = finish_deadline.unwrap_or_else(|| tokio::time::Instant::now() + idle_timeout);
    match tokio::time::timeout_at(deadline, stream.next()).await {
        Ok(event) => event,
        Err(_) if finish_deadline.is_some() => None,
        Err(_) => Some(Err(OSAgentError::Provider(
            "Provider stream stalled: no response events received before the idle timeout"
                .to_string(),
        ))),
    }
}

/// Metadata key holding the ids of other chats added as reference context.
const CONTEXT_SESSIONS_KEY: &str = "context_session_ids";
/// How many other chats one chat can reference at once.
const MAX_CONTEXT_SESSIONS: usize = 5;
/// Character budget for all referenced chats together, per turn.
const CONTEXT_BUDGET_CHARS: usize = 12_000;
/// Per-message cap inside a referenced chat.
const CONTEXT_MESSAGE_CHARS: usize = 800;

/// A chat referenced as context, as shown to the user.
#[derive(Debug, Clone, serde::Serialize)]
pub struct SessionContextEntry {
    pub id: String,
    pub name: String,
    pub message_count: usize,
}

fn session_display_name(session: &Session) -> String {
    session
        .metadata
        .get("name")
        .and_then(|value| value.as_str())
        .map(str::trim)
        .filter(|name| !name.is_empty())
        .map(str::to_string)
        .unwrap_or_else(|| "Untitled chat".to_string())
}

fn clip_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_string();
    }
    let cut: String = text.chars().take(limit).collect();
    format!("{cut}…")
}

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
- If the transcript already contains earlier compacted summaries, merge them into your output instead of repeating them. A prior summary is discarded after this pass: carry forward anything still needed (objectives, constraints, decisions, parallel workstreams) and drop only what is finished. Where the newer transcript contradicts a prior summary, the transcript wins.
- Preserve exact file paths, symbols, commands, and error strings when known.
- Keep every section, using "(none)" when empty. Use terse bullets, not prose.
- Wrap your output in a single <compacted-summary> block. Do not acknowledge this summary explicitly in your reply."#;

const NOTES_VERIFY_PROMPT: &str = r#"Review the session's rolling working notes against the earlier conversation transcript below and produce corrected notes.

The notes are a short handoff a fresh agent reads to continue instantly. Fix anything stale, wrong, or missing: goals, key decisions, files touched, what was tried, errors and fixes, and the concrete next step. Keep it under 2000 characters. Be concrete and repo-specific.

Rules:
- Preserve what is still accurate; correct what the transcript contradicts.
- Fold in important transcript facts the notes missed.
- Drop trivia that does not help continue the work.
- Output only the corrected notes text, no preamble."#;

const WORKING_NOTES_INSTRUCTIONS: &str = r#"# Working Notes
Maintain a short rolling handoff note so a fresh agent can continue instantly after compaction. After each meaningful step, update the note with: current goal, key decisions, files touched, what was tried, errors/fixes, and the concrete next step. Keep it under 2000 characters, concrete and repo-specific. Use the `update_notes` tool. Do not acknowledge the notes in your reply."#;

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
    stopped_sessions: DashMap<String, ()>,
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

fn is_operation_cancelled(error: &OSAgentError) -> bool {
    matches!(error, OSAgentError::Session(message) if message == "Operation cancelled")
}

impl AgentRuntime {
    pub async fn active_provider(&self) -> Arc<dyn Provider> {
        self.provider.read().await.clone()
    }

    /// Cheap-model slot for background chores (compaction summaries,
    /// titles). Resolves `small_provider`/`small_model` from config,
    /// falling back to the active provider. Builds an ephemeral provider
    /// when the slot names a different model than the shared entry carries,
    /// so the main agent's model is never mutated.
    pub async fn small_slot_provider(&self) -> Arc<dyn Provider> {
        let (provider_id, model) = self.config.read().await.small_model_ref();
        let active = self.active_provider().await;
        let cfg = self.config.read().await;
        let wanted_id = if provider_id.trim().is_empty() {
            cfg.default_provider.clone()
        } else {
            provider_id
        };
        let wanted_model = model.trim().to_string();
        // Same entry, same model → the active provider already is the slot.
        if wanted_id == cfg.default_provider
            && (wanted_model.is_empty() || wanted_model == cfg.default_model)
        {
            return active;
        }
        let Some(entry) = cfg
            .providers
            .iter()
            .find(|p| p.effective_id() == wanted_id.as_str() || p.provider_type == wanted_id)
            .cloned()
        else {
            return active;
        };
        drop(cfg);
        let mut entry = entry;
        if !wanted_model.is_empty() {
            entry.model = wanted_model;
        }
        let oauth_dir = PathBuf::from(
            shellexpand::tilde(&self.config.read().await.storage.database).to_string(),
        )
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
        match OpenAICompatibleProvider::with_catalog_oauth_and_agent_settings(
            entry,
            Some(self.catalog.clone()),
            Some(crate::oauth::create_oauth_storage(&oauth_dir)),
            self.agent_settings.clone(),
        ) {
            Ok(provider) => Arc::new(provider),
            Err(e) => {
                tracing::warn!("small-model provider failed to build ({}); using active", e);
                active
            }
        }
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
            if !config.provider_enabled(provider_cfg.effective_id()) {
                info!(
                    "Provider '{}' is disabled by enabled/disabled_providers; skipping",
                    provider_cfg.effective_id()
                );
                continue;
            }
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
            provider_instances.push((provider_cfg.effective_id().to_string(), provider));
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
            config.agent.memory_capture_mode,
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

        let mut scheduler =
            Scheduler::new(storage.clone(), event_bus.clone(), config.scheduler.clone());

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
            stopped_sessions: DashMap::new(),
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
}

mod checkpoints;
mod compaction;
mod goals;
mod lifecycle;
mod mcp;
mod memory;
mod permissions;
mod personas;
mod plugins;
mod providers;
mod queue;
mod scheduler_facade;
mod sessions;
mod snapshots;
mod subagents;
mod tool_loop;
mod tools_exec;
mod turn;
mod workspaces;

impl AgentRuntime {
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
            cfg.agent.memory_capture_mode,
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
}

#[cfg(test)]
mod external_path_scan_tests {
    use super::{AgentRuntime, ToolOutcome};
    use crate::storage::models::Message;
    use std::path::PathBuf;

    fn test_message(role: &str, content: &str) -> Message {
        match role {
            "user" => Message::user(content.to_string()),
            "assistant" => Message::assistant(content.to_string(), None),
            "tool" => Message::tool_result("call-1".to_string(), content.to_string()),
            _ => Message::system(content.to_string()),
        }
    }

    #[test]
    fn compaction_split_walks_back_whole_turns() {
        let compaction = crate::config::CompactionConfig {
            preserve_recent_tokens: 2_000,
            ..crate::config::CompactionConfig::default()
        };
        let messages = vec![
            test_message("user", "first question"),
            test_message("assistant", "first answer"),
            test_message("user", "second question"),
            test_message("assistant", "second answer"),
        ];
        let split = AgentRuntime::compaction_split_index(&messages, &compaction);
        assert!(split > 0 && split < messages.len());
        assert_eq!(messages[split].role, "user");
    }

    #[test]
    fn compaction_split_never_orphans_tool_pairs() {
        let compaction = crate::config::CompactionConfig {
            preserve_recent_tokens: 2_000,
            ..crate::config::CompactionConfig::default()
        };
        let mut call = test_message("assistant", "");
        call.tool_calls = Some(vec![crate::storage::models::ToolCall {
            id: "call-9".to_string(),
            name: "read_file".to_string(),
            arguments: serde_json::json!({}),
        }]);
        let mut result = test_message("tool", "file contents");
        result.tool_call_id = Some("call-9".to_string());
        let messages = vec![
            test_message("user", "read the file"),
            call,
            result,
            test_message("assistant", "done"),
        ];
        let split = AgentRuntime::compaction_split_index(&messages, &compaction);
        assert!(
            split <= 1,
            "split must not strand the tool pair, got {split}"
        );
    }

    #[test]
    fn compaction_split_compacts_single_turn_sessions() {
        let compaction = crate::config::CompactionConfig {
            preserve_recent_tokens: 2_000,
            ..crate::config::CompactionConfig::default()
        };
        let messages: Vec<Message> = (0..10)
            .map(|i| {
                if i % 2 == 0 {
                    test_message("user", &format!("q{i}"))
                } else {
                    test_message("assistant", &format!("a{i}"))
                }
            })
            .collect();
        let split = AgentRuntime::compaction_split_index(&messages, &compaction);
        assert!(split > 0 && split < messages.len());
    }

    #[test]
    fn compactable_content_strips_tool_call_markup() {
        let msg = test_message(
            "assistant",
            "Let me read it:<tool_call>read_file<arg_key>path</arg_key><arg_value>x.h</arg_value></tool_call> done",
        );
        let content = AgentRuntime::compactable_message_content(&msg).unwrap();
        assert!(!content.to_lowercase().contains("tool_call"), "{content}");
        assert!(content.contains("Let me read it"), "{content}");
    }

    #[test]
    fn compactable_content_unwraps_prior_summaries() {
        let msg = Message::synthetic_assistant(
            "<compacted-summary>\n## Next Step\nDo X\n</compacted-summary>\n\nDo not acknowledge."
                .to_string(),
            "compaction_summary",
        );
        let content = AgentRuntime::compactable_message_content(&msg).unwrap();
        assert!(
            !content.to_lowercase().contains("compacted-summary"),
            "{content}"
        );
        assert!(content.contains("Do X"), "{content}");
    }

    #[tokio::test]
    async fn provider_stream_finished_without_done_releases_waiter() {
        use futures::StreamExt;
        let mut stream = futures::stream::pending().boxed();
        let result = super::next_provider_stream_event(
            &mut stream,
            Some(tokio::time::Instant::now()),
            std::time::Duration::from_secs(120),
        )
        .await;
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn provider_stream_unfinished_stall_is_an_error() {
        use futures::StreamExt;
        let mut stream = futures::stream::pending().boxed();
        let result = super::next_provider_stream_event(
            &mut stream,
            None,
            std::time::Duration::from_millis(1),
        )
        .await;
        assert!(
            matches!(result, Some(Err(super::OSAgentError::Provider(message))) if message.contains("stalled"))
        );
    }

    #[tokio::test]
    async fn provider_stream_preserves_trailing_usage() {
        use futures::StreamExt;
        let mut stream = futures::stream::iter(vec![Ok(super::StreamEvent {
            event_type: "usage".to_string(),
            content: None,
            thinking: None,
            tool_call_deltas: vec![],
            finish_reason: None,
            usage: Some(crate::agent::provider::TokenUsage {
                input: 10,
                output: 5,
                total: 15,
                cached_read: None,
                cached_write: None,
                reasoning: None,
            }),
            done: false,
        })])
        .chain(futures::stream::pending())
        .boxed();
        let result = super::next_provider_stream_event(
            &mut stream,
            Some(tokio::time::Instant::now() + std::time::Duration::from_secs(2)),
            std::time::Duration::from_secs(120),
        )
        .await
        .unwrap()
        .unwrap();
        assert_eq!(result.usage.unwrap().total, 15);
    }

    fn ws() -> PathBuf {
        PathBuf::from(r"C:\example-workspace\Research-Projects\firmware-demo")
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
    fn read_file_preview_never_splices_middle() {
        let lines: Vec<String> = (1..=350).map(|i| format!("{i}: body {i}")).collect();
        let rendered = AgentRuntime::summarize_tool_output_for_context(
            "read_file",
            ToolOutcome::Success,
            &lines.join("\n"),
        );
        assert!(rendered.contains("1: body 1"));
        assert!(!rendered.contains("middle lines omitted"));
        assert!(!rendered.contains("tail kept below"));
        assert!(rendered.contains("of 350 lines"));
    }

    #[test]
    fn read_file_intent_key_covers_real_and_alias_params() {
        let direct = AgentRuntime::tool_intent_signature(
            "read_file",
            &serde_json::json!({"filePath": "a.c", "offset": 200, "limit": 350}),
        )
        .expect("intent");
        let alias = AgentRuntime::tool_intent_signature(
            "read_file",
            &serde_json::json!({"path": "a.c", "start_line": 200, "end_line": 549}),
        )
        .expect("intent");
        assert_eq!(direct, alias);
        let next_page = AgentRuntime::tool_intent_signature(
            "read_file",
            &serde_json::json!({"filePath": "a.c", "offset": 550, "limit": 350}),
        )
        .expect("intent");
        assert_ne!(direct, next_page);
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
                scan(r#"dir "C:\example-workspace\Research-Projects\firmware-demo\src""#),
                None
            );
        }
    }
}

#[cfg(test)]
mod workspace_lock_tests {
    use super::AgentRuntime;
    use crate::config::{Config, WorkspaceConfig, WorkspacePath, WorkspacePermission};
    use crate::storage::models::Message;
    use tempfile::tempdir;

    #[tokio::test]
    async fn chat_workspace_locks_once_it_has_messages() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let workspace = root.join("workspace");
        let other = root.join("other");
        std::fs::create_dir_all(&workspace).unwrap();
        std::fs::create_dir_all(&other).unwrap();

        let mut config = Config::default_config();
        config.search.enabled = false;
        config.storage.database = root.join("osagent.db").to_string_lossy().to_string();
        config.agent.workspace = workspace.to_string_lossy().to_string();
        config.agent.active_workspace = Some("default".to_string());
        if let Some(default_workspace) = config.agent.workspaces.get_mut(0) {
            default_workspace.path = workspace.to_string_lossy().to_string();
        }
        let other_path = other.to_string_lossy().to_string();
        config.agent.workspaces.push(WorkspaceConfig {
            id: "other".to_string(),
            name: "Other".to_string(),
            paths: vec![WorkspacePath {
                path: other_path.clone(),
                permission: WorkspacePermission::ReadWrite,
                description: None,
            }],
            path: other_path,
            description: None,
            permission: WorkspacePermission::ReadWrite,
            created_at: "2026-01-01T00:00:00Z".to_string(),
            last_used: None,
        });

        let agent = AgentRuntime::new(config).unwrap();
        let session = agent.create_session().await.unwrap();

        // An empty chat can still move to another workspace.
        agent
            .set_session_workspace(&session.id, "other")
            .await
            .expect("an empty chat may change workspace");

        // Once it has a message, its workspace is fixed.
        let mut stored = agent
            .session_manager
            .get_session(&session.id)
            .await
            .unwrap()
            .unwrap();
        stored.messages.push(Message::user("hello".to_string()));
        agent.session_manager.update_session(&stored).await.unwrap();

        let summary = agent
            .list_session_summaries()
            .await
            .unwrap()
            .into_iter()
            .find(|s| s.id == session.id)
            .expect("summary for the session");
        assert_eq!(summary.message_count, 1);

        let err = agent
            .set_session_workspace(&session.id, "default")
            .await
            .expect_err("a chat with messages must not change workspace");
        assert!(
            err.to_string().contains("locked"),
            "unexpected error: {err}"
        );

        // Re-selecting the workspace the chat already uses is not a change.
        agent
            .set_session_workspace(&session.id, "other")
            .await
            .expect("re-selecting the current workspace stays allowed");
    }
}

#[cfg(test)]
mod session_context_tests {
    use super::{AgentRuntime, MAX_CONTEXT_SESSIONS};
    use crate::config::Config;
    use crate::storage::models::Message;
    use tempfile::tempdir;

    #[tokio::test]
    async fn referenced_chats_feed_each_turn_without_entering_history() {
        let temp_dir = tempdir().unwrap();
        let root = temp_dir.path();
        let workspace = root.join("workspace");
        std::fs::create_dir_all(&workspace).unwrap();

        let mut config = Config::default_config();
        config.search.enabled = false;
        config.storage.database = root.join("osagent.db").to_string_lossy().to_string();
        config.agent.workspace = workspace.to_string_lossy().to_string();
        config.agent.active_workspace = Some("default".to_string());
        if let Some(default_workspace) = config.agent.workspaces.get_mut(0) {
            default_workspace.path = workspace.to_string_lossy().to_string();
        }
        let agent = AgentRuntime::new(config).unwrap();

        // A source chat with a named title and two turns.
        let source = agent.create_session().await.unwrap();
        let mut stored = agent
            .session_manager
            .get_session(&source.id)
            .await
            .unwrap()
            .unwrap();
        stored.metadata["name"] = serde_json::json!("Deploy notes");
        stored
            .messages
            .push(Message::user("Deploy checklist is in step 3".to_string()));
        stored
            .messages
            .push(Message::assistant("Use the staging key.".to_string(), None));
        agent.session_manager.update_session(&stored).await.unwrap();

        let target = agent.create_session().await.unwrap();

        let added = agent
            .add_session_context(&target.id, &source.id)
            .await
            .expect("another chat can be added as context");
        assert_eq!(added.name, "Deploy notes");
        assert_eq!(added.message_count, 2);

        let self_reference = agent.add_session_context(&target.id, &target.id).await;
        assert!(self_reference.is_err(), "a chat must not reference itself");

        // Adding the same chat twice keeps one reference.
        agent
            .add_session_context(&target.id, &source.id)
            .await
            .unwrap();
        assert_eq!(
            agent.list_session_context(&target.id).await.unwrap().len(),
            1
        );

        // The turn block carries the source's text.
        let target_session = agent
            .session_manager
            .get_session(&target.id)
            .await
            .unwrap()
            .unwrap();
        let block = agent
            .referenced_session_context(&target_session)
            .await
            .unwrap()
            .expect("a block for referenced chats");
        assert!(block.contains("## Deploy notes"), "{block}");
        assert!(block.contains("Deploy checklist is in step 3"), "{block}");
        assert!(block.contains("assistant: Use the staging key."), "{block}");

        // The reference is metadata: the target's transcript stays empty.
        assert!(target_session.messages.is_empty());

        // Limit: the target already references one chat.
        for _ in 1..MAX_CONTEXT_SESSIONS {
            let extra = agent.create_session().await.unwrap();
            agent
                .add_session_context(&target.id, &extra.id)
                .await
                .unwrap();
        }
        let overflow = agent.create_session().await.unwrap();
        assert!(agent
            .add_session_context(&target.id, &overflow.id)
            .await
            .is_err());

        agent
            .remove_session_context(&target.id, &source.id)
            .await
            .unwrap();
        let remaining = agent.list_session_context(&target.id).await.unwrap();
        assert!(remaining.iter().all(|entry| entry.id != source.id));
    }
}
