use crate::error::{OSAgentError, Result};
use crate::external::ExternalPermissionConfig;
use crate::permission::{PermissionAction, PermissionRule};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    #[serde(skip)]
    config_path: Option<PathBuf>,
    pub server: ServerConfig,
    #[serde(default)]
    pub providers: Vec<ProviderConfig>,
    pub default_provider: String,
    pub default_model: String,
    #[serde(default)]
    pub provider: ProviderConfig,
    pub agent: AgentConfig,
    pub telegram: Option<TelegramConfig>,
    pub discord: Option<DiscordConfig>,
    pub voice: Option<VoiceConfig>,
    pub lsp: LspConfig,
    pub tools: ToolsConfig,
    pub search: SearchConfig,
    pub logging: LoggingConfig,
    pub storage: StorageConfig,
    pub external: ExternalConfig,
    pub plugins: PluginConfig,
    pub update: UpdateConfig,
    #[serde(default)]
    pub experimental: ExperimentalConfig,
    #[serde(default)]
    pub scheduler: SchedulerConfig,
    #[serde(default)]
    pub mcp: McpConfig,
    #[serde(default)]
    pub spill: SpillConfig,
    #[serde(default)]
    pub compaction: CompactionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    pub password: String,
    pub password_enabled: bool,
    pub jwt_secret: String,
    #[serde(default)]
    pub cors_allowed_origins: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ProviderConfig {
    pub provider_type: String,
    pub api_key: String,
    pub base_url: String,
    pub model: String,
    pub fallbacks: Vec<String>,
    pub auth_type: Option<String>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub oauth_authorization_url: Option<String>,
    pub oauth_token_url: Option<String>,
    pub oauth_scopes: Option<Vec<String>>,
    pub custom_headers: Option<std::collections::HashMap<String, String>>,
    pub redirect_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct AgentConfig {
    pub workspace: String,
    pub workspaces: Vec<WorkspaceConfig>,
    pub active_workspace: Option<String>,
    pub max_tokens: usize,
    pub temperature: f32,
    pub thinking_level: String,
    pub checkpoint_enabled: bool,
    pub checkpoint_interval: usize,
    pub max_iterations: usize,
    pub memory_enabled: bool,
    pub memory_file: String,
    pub learning_mode: LearningMode,
    pub memory_capture_mode: CaptureMode,
    pub decision_memory_enabled: bool,
    pub decision_memory_file: String,
    pub decision_capture_mode: CaptureMode,
    #[serde(default)]
    pub permission_rules: Vec<PermissionRule>,
    /// Policy applied when the agent's `sessions` tool reads or searches a
    /// conversation other than its own (its own subagent children are
    /// exempt). Per-session overrides live in `permission_rules` with
    /// `permission = "session_access"` and glob patterns over resources like
    /// `session://<id>` or `session://*`.
    #[serde(default)]
    pub session_access_default_action: PermissionAction,
    #[serde(default)]
    pub custom_identity: Option<String>,
    #[serde(default)]
    pub custom_priorities: Option<Vec<String>>,
    #[serde(default = "default_prompt_cache_enabled")]
    pub prompt_cache_enabled: bool,
    /// Maximum number of nested subagent levels (main agent counts as 0).
    /// Spawning deeper than this fails with an explicit error.
    #[serde(default = "default_subagent_depth")]
    pub subagent_depth: usize,
    /// Per-subagent-type model overrides, e.g. `"explore" => "openrouter:deepseek/deepseek-r1"`.
    /// Values are `"provider_id:model"` or just a model name (uses the default provider).
    #[serde(default)]
    pub subagent_models: std::collections::HashMap<String, String>,
    /// When a background subagent finishes, automatically start a continuation
    /// turn on the parent session so the results reach the agent without
    /// waiting for the user's next message. Results that arrive while the
    /// parent is busy are delivered right after its current run ends.
    #[serde(default = "default_subagent_auto_resume")]
    pub subagent_auto_resume: bool,
    /// Safety cap on consecutive auto-continuation turns per session (turns
    /// started only by background-task completions, with no user message in
    /// between). Prevents runaway spawn -> complete -> wake loops.
    #[serde(default = "default_subagent_auto_resume_max_turns")]
    pub subagent_auto_resume_max_turns: usize,
    /// How many times a failed subagent run is retried at the task level when
    /// the failure looks transient (provider 5xx, rate limits, timeouts...).
    /// The retry resumes the same subagent session so completed work is kept.
    #[serde(default = "default_subagent_task_max_retries")]
    pub subagent_task_max_retries: u32,
}

fn default_prompt_cache_enabled() -> bool {
    true
}

fn default_subagent_depth() -> usize {
    1
}

fn default_subagent_auto_resume() -> bool {
    true
}

fn default_subagent_auto_resume_max_turns() -> usize {
    5
}

fn default_subagent_task_max_retries() -> u32 {
    2
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum LearningMode {
    #[default]
    Manual,
    Review,
    Auto,
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, PartialEq, Eq, Default)]
#[serde(rename_all = "snake_case")]
pub enum CaptureMode {
    Off,
    #[default]
    Review,
    Auto,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct WorkspacePath {
    pub path: String,
    pub permission: WorkspacePermission,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub paths: Vec<WorkspacePath>,
    #[serde(skip)]
    pub path: String,
    pub description: Option<String>,
    #[serde(default)]
    pub permission: WorkspacePermission,
    pub created_at: String,
    pub last_used: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum WorkspacePermission {
    ReadOnly,
    #[default]
    ReadWrite,
}

impl WorkspaceConfig {
    pub fn resolved_path(&self) -> String {
        if let Some(wp) = self.paths.iter().find(|wp| !wp.path.trim().is_empty()) {
            wp.path.clone()
        } else if !self.path.is_empty() {
            self.path.clone()
        } else {
            String::new()
        }
    }
}

impl WorkspacePermission {
    pub fn allows_writes(&self) -> bool {
        matches!(self, Self::ReadWrite)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct TelegramConfig {
    pub enabled: bool,
    pub bot_token: String,
    pub allowed_users: Vec<i64>,
}

/// Discord ids are 64-bit snowflakes, which exceed the 2^53 integer range a
/// JavaScript `Number` can hold exactly. Sending them to the web UI as JSON
/// numbers silently corrupts them (`420155234833268737` comes back as
/// `420155234833268740`), so they are written as strings and read back from
/// either form.
mod snowflake {
    #[derive(serde::Deserialize)]
    #[serde(untagged)]
    enum Raw {
        Number(u64),
        Text(String),
    }

    impl Raw {
        fn into_id(self) -> Option<u64> {
            match self {
                Raw::Number(id) => Some(id),
                Raw::Text(text) => text.trim().parse().ok(),
            }
        }
    }

    pub mod list {
        use super::Raw;
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        pub fn serialize<S: Serializer>(ids: &[u64], serializer: S) -> Result<S::Ok, S::Error> {
            ids.iter()
                .map(u64::to_string)
                .collect::<Vec<_>>()
                .serialize(serializer)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Vec<u64>, D::Error> {
            Ok(Vec::<Raw>::deserialize(deserializer)?
                .into_iter()
                .filter_map(Raw::into_id)
                .collect())
        }
    }

    pub mod optional {
        use super::Raw;
        use serde::{Deserialize, Deserializer, Serialize, Serializer};

        pub fn serialize<S: Serializer>(
            id: &Option<u64>,
            serializer: S,
        ) -> Result<S::Ok, S::Error> {
            id.map(|id| id.to_string()).serialize(serializer)
        }

        pub fn deserialize<'de, D: Deserializer<'de>>(
            deserializer: D,
        ) -> Result<Option<u64>, D::Error> {
            Ok(Option::<Raw>::deserialize(deserializer)?.and_then(Raw::into_id))
        }
    }
}

fn default_music_max_queue() -> usize {
    50
}
fn default_music_auto_leave_secs() -> u64 {
    300
}
fn default_yt_dlp_path() -> String {
    "yt-dlp".to_string()
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct DiscordConfig {
    pub enabled: bool,
    pub token: String,
    /// Enables separate restricted community and full-access trusted policies.
    /// Off preserves the original allowed_users behavior for existing installs.
    pub community_mode: bool,
    /// Allows every member in the configured community servers to use restricted
    /// chat. This is intentionally opt-in and requires at least one allowed guild.
    pub allow_community_members: bool,
    pub community_context: String,
    pub docs_url: String,
    pub github_repo: String,
    pub github_token: String,
    #[serde(with = "snowflake::optional")]
    pub github_tracking_channel: Option<u64>,
    pub github_poll_seconds: u64,
    #[serde(with = "snowflake::list")]
    pub allowed_users: Vec<u64>,
    #[serde(with = "snowflake::list")]
    pub allowed_roles: Vec<u64>,
    #[serde(with = "snowflake::list")]
    pub allowed_guilds: Vec<u64>,
    #[serde(with = "snowflake::list")]
    pub allowed_channels: Vec<u64>,
    pub allow_dms: bool,
    #[serde(with = "snowflake::list")]
    pub trusted_users: Vec<u64>,
    #[serde(with = "snowflake::list")]
    pub trusted_roles: Vec<u64>,
    #[serde(with = "snowflake::list")]
    pub trusted_guilds: Vec<u64>,
    #[serde(with = "snowflake::list")]
    pub trusted_channels: Vec<u64>,
    #[serde(with = "snowflake::optional")]
    pub last_channel_id: Option<u64>,
    // ── Music / voice playback ──────────────────────────────────────────
    /// Enable YouTube/HTTP audio playback in voice channels. Requires
    /// building with `--features discord-voice` (`yt-dlp` auto-downloads)
    /// on PATH. Off by default.
    pub music_enabled: bool,
    /// Maximum queued tracks per guild.
    #[serde(default = "default_music_max_queue")]
    pub music_max_queue: usize,
    /// Max track duration in seconds (0 = unlimited).
    #[serde(default)]
    pub music_max_duration_secs: u64,
    /// Auto-leave voice after this many seconds of empty queue (0 = never).
    #[serde(default = "default_music_auto_leave_secs")]
    pub music_auto_leave_secs: u64,
    /// Custom yt-dlp binary path (default `yt-dlp` on PATH).
    #[serde(default = "default_yt_dlp_path")]
    pub yt_dlp_path: String,
    /// Extra args appended to every yt-dlp invocation (e.g. `--cookies /data/cookies.txt`).
    #[serde(default)]
    pub yt_dlp_extra_args: String,
    /// Piped instances to try when YouTube extraction fails (fallback).
    #[serde(default)]
    pub piped_instances: Vec<String>,
}

impl Default for DiscordConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            token: String::new(),
            community_mode: false,
            allow_community_members: false,
            community_context: String::new(),
            docs_url: String::new(),
            github_repo: String::new(),
            github_token: String::new(),
            github_tracking_channel: None,
            github_poll_seconds: 0,
            allowed_users: Vec::new(),
            allowed_roles: Vec::new(),
            allowed_guilds: Vec::new(),
            allowed_channels: Vec::new(),
            allow_dms: false,
            trusted_users: Vec::new(),
            trusted_roles: Vec::new(),
            trusted_guilds: Vec::new(),
            trusted_channels: Vec::new(),
            last_channel_id: None,
            music_enabled: false,
            music_max_queue: default_music_max_queue(),
            music_max_duration_secs: 0,
            music_auto_leave_secs: default_music_auto_leave_secs(),
            yt_dlp_path: default_yt_dlp_path(),
            yt_dlp_extra_args: String::new(),
            piped_instances: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct VoiceConfig {
    pub enabled: bool,
    pub stt_provider: String,
    pub tts_provider: String,
    pub language: String,
    pub auto_send: bool,
    pub auto_speak: bool,
    /// Narrate each tool call while the agent works. Off by default: a turn with
    /// a dozen tool calls queues two dozen utterances, and the final answer
    /// interrupts the queue anyway.
    pub speak_tool_progress: bool,
    /// End the recording automatically after a pause, so a hands-free turn does
    /// not need a second click to stop. Local Whisper only: the browser's own
    /// recogniser already ends on silence.
    pub silence_auto_stop: bool,
    pub voice_speed: f32,
    pub whisper_model: Option<String>,
    pub piper_voice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct ToolsConfig {
    pub denied: Vec<String>,
    pub bash: BashToolConfig,
    pub code_python: CodeToolConfig,
    pub code_node: CodeToolConfig,
    pub code_bash: CodeToolConfig,
    pub grep: GrepToolConfig,
    pub glob: GrepToolConfig,
    pub skills: SkillsConfig,
    pub repeat_reminder: RepeatReminderConfig,
}

/// Advisory loop-breaker: escalates reminders into the conversation when
/// the same tool is called repeatedly with identical (canonicalized)
/// arguments. Unlike the hard loop guard, it never blocks — it nudges.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct RepeatReminderConfig {
    pub enabled: bool,
    /// Consecutive-identical-call counts at which a reminder is injected.
    /// Escalates: the first threshold gets a gentle nudge, later ones a
    /// detailed message with the argument preview.
    pub thresholds: Vec<u32>,
    pub arguments_preview_chars: usize,
    /// `*`-wildcard include/exclude patterns over tool names, evaluated
    /// at call time. Excluded calls are transparent: they neither count
    /// nor reset the chain.
    pub include: Vec<String>,
    pub exclude: Vec<String>,
}

impl Default for RepeatReminderConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            thresholds: vec![3, 5, 8],
            arguments_preview_chars: 500,
            include: vec!["*".to_string()],
            exclude: Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub enabled: bool,
    pub directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum BashMode {
    Allowlist,
    #[default]
    Permissive,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BashToolConfig {
    pub allowed_commands: Vec<String>,
    pub blocked_commands: Vec<String>,
    pub mode: BashMode,
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CodeToolConfig {
    pub enabled: bool,
    pub timeout_seconds: u64,
    pub max_output_bytes: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct GrepToolConfig {
    pub timeout_seconds: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SearchConfig {
    /// Enables the codesearch (quick-context) tool. Web search is unaffected.
    pub enabled: bool,
    pub max_results: usize,
    pub global_timeout_ms: u64,
    pub per_backend_timeout_ms: u64,
    pub max_parallel_backends: usize,
    pub searxng_instance_refresh_minutes: u64,
    pub searxng_max_instances: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LoggingConfig {
    pub level: String,
    pub audit_enabled: bool,
    pub audit_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct StorageConfig {
    pub database: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ExternalConfig {
    pub enabled: bool,
    pub permission: ExternalPermissionConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct PluginConfig {
    pub enabled: bool,
    pub plugins: Vec<String>,
    pub plugin_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub enabled: bool,
    pub channel: String,
    pub check_on_startup: bool,
    pub check_interval_hours: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            channel: "stable".to_string(),
            check_on_startup: true,
            check_interval_hours: 24,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct ExperimentalConfig {
    pub workflows_enabled: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SchedulerConfig {
    pub enabled: bool,
    pub max_concurrent: usize,
    pub max_retries: usize,
}

impl Default for SchedulerConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_concurrent: 3,
            max_retries: 3,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpConfig {
    pub enabled: bool,
    pub servers: Vec<McpServerConfig>,
    /// Ceiling on tools activated into a single session's tool array.
    /// Activation is what trades context for capability, so it needs a
    /// bound the model cannot talk its way past.
    pub max_activated_tools: usize,
    /// Results returned by one `tool_search` call.
    pub search_result_limit: usize,
    /// Expose the per-server manifest in the system prompt. Turning this
    /// off saves a few dozen tokens and makes MCP tools effectively
    /// undiscoverable — only sensible when every tool you want is listed
    /// in a server's `always_active`.
    pub manifest_in_prompt: bool,
}

impl Default for McpConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            servers: Vec::new(),
            max_activated_tools: 48,
            search_result_limit: 8,
            manifest_in_prompt: true,
        }
    }
}

/// Tool-result spill: oversized plain-text tool results are persisted
/// verbatim to a session-scoped file and replaced in context with a
/// bounded head/tail preview plus a retrieval hint.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SpillConfig {
    pub enabled: bool,
    /// Root directory for spill files. `~` is expanded. The default
    /// lands under `~/.osagent/spill`, grouped per session.
    pub root: String,
    /// Model-facing context cap for a single plain-text tool result, in
    /// UTF-8 bytes. Results larger than this are spilled.
    pub max_inline_bytes: usize,
}

impl Default for SpillConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            root: "~/.osagent/spill".to_string(),
            max_inline_bytes: 24_000,
        }
    }
}

/// Context-window compaction policy: when the request approaches the
/// model's context window, older history is summarized into a
/// `<compacted-summary>` frame instead of being dropped.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct CompactionConfig {
    pub enabled: bool,
    /// Fraction of the usable context window at which compaction
    /// triggers (0.8 = at 80%).
    pub threshold_ratio: f32,
    /// Model-free tool-result pruning: results over
    /// `prune_threshold_chars` are rewritten to a head/tail slice before
    /// any summarization attempt.
    pub prune_enabled: bool,
    pub prune_threshold_chars: usize,
    pub prune_head_chars: usize,
    pub prune_tail_chars: usize,
    /// Cap on the transcript fed to the summarization pass.
    pub max_transcript_chars: usize,
}

impl Default for CompactionConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            threshold_ratio: 0.8,
            prune_enabled: true,
            prune_threshold_chars: 8_192,
            prune_head_chars: 4_096,
            prune_tail_chars: 1_024,
            max_transcript_chars: 24_000,
        }
    }
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum McpTransport {
    #[default]
    Stdio,
    Http,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct McpServerConfig {
    pub name: String,
    pub enabled: bool,
    /// Explicit transport. When unset it is inferred: a `url` means
    /// http, a `command` means stdio.
    pub transport: Option<McpTransport>,
    pub command: Option<String>,
    pub args: Vec<String>,
    pub env: std::collections::HashMap<String, String>,
    pub cwd: Option<String>,
    pub url: Option<String>,
    pub headers: std::collections::HashMap<String, String>,
    pub timeout_seconds: u64,
    /// One-line capability summary for the manifest. Worth setting: it
    /// is what the model reads when deciding whether to search here.
    pub description: Option<String>,
    /// Tool names activated at startup, skipping the search round trip
    /// for paths used every session.
    pub always_active: Vec<String>,
}

impl Default for McpServerConfig {
    fn default() -> Self {
        Self {
            name: String::new(),
            // A server someone just added is meant to run; requiring a
            // second toggle to enable it is a papercut, not a safeguard.
            enabled: true,
            transport: None,
            command: None,
            args: Vec::new(),
            env: std::collections::HashMap::new(),
            cwd: None,
            url: None,
            headers: std::collections::HashMap::new(),
            timeout_seconds: 60,
            description: None,
            always_active: Vec::new(),
        }
    }
}

impl McpServerConfig {
    /// Reject a server that cannot possibly connect, before we spawn
    /// anything. Returns a message aimed at whoever typed it into the UI.
    pub fn validate(&self) -> Result<()> {
        if self.name.trim().is_empty() {
            return Err(OSAgentError::Config("MCP server name is required".into()));
        }
        if !self
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
        {
            return Err(OSAgentError::Config(format!(
                "MCP server name '{}' may only contain letters, numbers, '_' and '-'",
                self.name
            )));
        }
        match self.transport_kind() {
            McpTransport::Stdio => {
                if self.command.as_deref().unwrap_or("").trim().is_empty() {
                    return Err(OSAgentError::Config(format!(
                        "MCP server '{}': a command is required for stdio transport",
                        self.name
                    )));
                }
            }
            McpTransport::Http => {
                let url = self.url.as_deref().unwrap_or("").trim();
                if url.is_empty() {
                    return Err(OSAgentError::Config(format!(
                        "MCP server '{}': a url is required for http transport",
                        self.name
                    )));
                }
                if !url.starts_with("http://") && !url.starts_with("https://") {
                    return Err(OSAgentError::Config(format!(
                        "MCP server '{}': url must start with http:// or https://",
                        self.name
                    )));
                }
            }
        }
        Ok(())
    }

    pub fn transport_kind(&self) -> McpTransport {
        self.transport.unwrap_or({
            if self.url.is_some() {
                McpTransport::Http
            } else {
                McpTransport::Stdio
            }
        })
    }
}

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 8765,
            password: "".to_string(),
            password_enabled: false,
            jwt_secret: String::new(),
            cors_allowed_origins: vec![],
        }
    }
}

impl Default for ProviderConfig {
    fn default() -> Self {
        Self {
            provider_type: "openai-compatible".to_string(),
            api_key: "".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            model: "gpt-4.1".to_string(),
            fallbacks: vec![],
            auth_type: None,
            oauth_client_id: None,
            oauth_client_secret: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_scopes: None,
            custom_headers: None,
            redirect_url: None,
        }
    }
}

impl Default for WorkspaceConfig {
    fn default() -> Self {
        Self {
            id: "default".to_string(),
            name: "Default Workspace".to_string(),
            paths: vec![WorkspacePath {
                path: default_workspace_path(),
                permission: WorkspacePermission::ReadWrite,
                description: Some("Default working directory".to_string()),
            }],
            path: String::new(),
            description: Some("Default working directory".to_string()),
            permission: WorkspacePermission::ReadWrite,
            created_at: chrono::Utc::now().to_rfc3339(),
            last_used: Some(chrono::Utc::now().to_rfc3339()),
        }
    }
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            workspace: default_workspace_path(),
            workspaces: vec![],
            active_workspace: None,
            max_tokens: 16384,
            temperature: 0.7,
            thinking_level: "auto".to_string(),
            checkpoint_enabled: true,
            checkpoint_interval: 5,
            max_iterations: 50,
            memory_enabled: false,
            memory_file: default_memory_file(),
            learning_mode: LearningMode::Manual,
            memory_capture_mode: CaptureMode::Review,
            decision_memory_enabled: true,
            decision_memory_file: default_decision_memory_file(),
            decision_capture_mode: CaptureMode::Review,
            permission_rules: vec![],
            session_access_default_action: PermissionAction::Ask,
            custom_identity: None,
            custom_priorities: None,
            prompt_cache_enabled: default_prompt_cache_enabled(),
            subagent_depth: default_subagent_depth(),
            subagent_models: std::collections::HashMap::new(),
            subagent_auto_resume: default_subagent_auto_resume(),
            subagent_auto_resume_max_turns: default_subagent_auto_resume_max_turns(),
            subagent_task_max_retries: default_subagent_task_max_retries(),
        }
    }
}

impl Default for VoiceConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            stt_provider: "browser".to_string(),
            tts_provider: "browser".to_string(),
            language: "en".to_string(),
            auto_send: false,
            auto_speak: false,
            speak_tool_progress: false,
            silence_auto_stop: true,
            voice_speed: 1.0,
            whisper_model: None,
            piper_voice: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct LspServerConfig {
    pub command: String,
    pub args: Vec<String>,
    pub root_markers: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct LspConfig {
    pub enabled: bool,
    pub servers: std::collections::HashMap<String, LspServerConfig>,
}

impl Default for LspConfig {
    fn default() -> Self {
        let mut servers = std::collections::HashMap::new();

        servers.insert(
            "rust".to_string(),
            LspServerConfig {
                command: "rust-analyzer".to_string(),
                args: vec![],
                root_markers: vec!["Cargo.toml".to_string()],
            },
        );

        servers.insert(
            "typescript".to_string(),
            LspServerConfig {
                command: "typescript-language-server".to_string(),
                args: vec!["--stdio".to_string()],
                root_markers: vec!["package.json".to_string(), "tsconfig.json".to_string()],
            },
        );

        servers.insert(
            "python".to_string(),
            LspServerConfig {
                command: "pylsp".to_string(),
                args: vec![],
                root_markers: vec!["pyproject.toml".to_string(), "setup.py".to_string()],
            },
        );

        Self {
            enabled: false,
            servers,
        }
    }
}

impl Default for BashToolConfig {
    fn default() -> Self {
        Self {
            mode: BashMode::Permissive,
            allowed_commands: vec![
                "ls".to_string(),
                "cat".to_string(),
                "grep".to_string(),
                "head".to_string(),
                "tail".to_string(),
                "wc".to_string(),
                "find".to_string(),
                "stat".to_string(),
                "file".to_string(),
                "test".to_string(),
                "git".to_string(),
                "npm".to_string(),
                "node".to_string(),
                "cargo".to_string(),
                "rustc".to_string(),
                "python".to_string(),
                "python3".to_string(),
                "pip".to_string(),
                "mkdir".to_string(),
                "rmdir".to_string(),
                "rm".to_string(),
                "del".to_string(),
                "cp".to_string(),
                "copy".to_string(),
                "mv".to_string(),
                "move".to_string(),
                "touch".to_string(),
                "echo".to_string(),
                "pwd".to_string(),
                "dir".to_string(),
                "type".to_string(),
                "which".to_string(),
                "powershell".to_string(),
                "pwsh".to_string(),
            ],
            blocked_commands: vec![
                "format".to_string(),
                "mkfs".to_string(),
                "shutdown".to_string(),
                "reboot".to_string(),
                "halt".to_string(),
                "poweroff".to_string(),
                "sudo".to_string(),
                "doas".to_string(),
                "runas".to_string(),
                "su".to_string(),
                "dd".to_string(),
                "systemctl".to_string(),
                "service".to_string(),
                "regedit".to_string(),
                "reg".to_string(),
                "diskpart".to_string(),
                "netsh".to_string(),
                "net".to_string(),
                "taskkill".to_string(),
                "kill".to_string(),
                "killall".to_string(),
                "pkill".to_string(),
                "chown".to_string(),
                "chmod".to_string(),
                "chattr".to_string(),
                "passwd".to_string(),
                "userdel".to_string(),
                "useradd".to_string(),
                "groupdel".to_string(),
                "fdisk".to_string(),
                "parted".to_string(),
            ],
            timeout_seconds: 30,
        }
    }
}

impl Default for CodeToolConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_seconds: 60,
            max_output_bytes: 1024 * 1024,
        }
    }
}

impl Default for GrepToolConfig {
    fn default() -> Self {
        Self {
            timeout_seconds: 60,
        }
    }
}

impl Default for SearchConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            max_results: 20,
            // Public search frontends routinely take several seconds; the old
            // 2s/4.5s budget timed them out before they could ever answer.
            global_timeout_ms: 15_000,
            per_backend_timeout_ms: 8_000,
            max_parallel_backends: 5,
            searxng_instance_refresh_minutes: 30,
            searxng_max_instances: 3,
        }
    }
}

impl Default for SkillsConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            directory: "~/.osagent/skills".to_string(),
        }
    }
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: "info".to_string(),
            audit_enabled: true,
            audit_file: "~/.osagent/audit.log".to_string(),
        }
    }
}

impl Default for StorageConfig {
    fn default() -> Self {
        Self {
            database: "~/.osagent/osagent.db".to_string(),
        }
    }
}

impl Default for ExternalConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            permission: ExternalPermissionConfig::default(),
        }
    }
}

impl Default for PluginConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            plugins: vec![],
            plugin_dir: "~/.osagent/plugins".to_string(),
        }
    }
}

impl Default for Config {
    fn default() -> Self {
        Self::default_config()
    }
}

impl Config {
    pub fn default_config() -> Self {
        let mut cfg = Self {
            config_path: None,
            server: ServerConfig::default(),
            providers: vec![],
            default_provider: String::new(),
            default_model: String::new(),
            provider: ProviderConfig::default(),
            agent: AgentConfig::default(),
            telegram: None,
            discord: None,
            voice: None,
            lsp: LspConfig::default(),
            tools: ToolsConfig::default(),
            search: SearchConfig::default(),
            logging: LoggingConfig::default(),
            storage: StorageConfig::default(),
            external: ExternalConfig::default(),
            plugins: PluginConfig::default(),
            update: UpdateConfig::default(),
            experimental: ExperimentalConfig::default(),
            scheduler: SchedulerConfig::default(),
            mcp: McpConfig::default(),
            spill: SpillConfig::default(),
            compaction: CompactionConfig::default(),
        };
        cfg.ensure_server_security_defaults();
        cfg.ensure_workspace_defaults();
        cfg
    }

    /// Return the configuration shape used by the settings UI without exposing
    /// credentials to the browser.
    pub(crate) fn redacted_for_api(&self) -> Self {
        let mut redacted = self.clone();

        redacted.server.password.clear();
        redacted.server.jwt_secret.clear();
        redact_provider_config(&mut redacted.provider);
        for provider in &mut redacted.providers {
            redact_provider_config(provider);
        }

        if let Some(telegram) = &mut redacted.telegram {
            telegram.bot_token.clear();
        }
        if let Some(discord) = &mut redacted.discord {
            discord.token.clear();
            discord.github_token.clear();
        }
        for server in &mut redacted.mcp.servers {
            server.env.clear();
            server.headers.clear();
        }

        redacted
    }

    /// Restore credentials omitted by `redacted_for_api` during a settings
    /// round trip. Non-empty incoming values still replace existing values.
    pub(crate) fn preserve_secrets_from(&mut self, current: &Self) {
        if self.server.password.trim().is_empty() {
            self.server.password = current.server.password.clone();
        }
        if self.server.jwt_secret.trim().is_empty() {
            self.server.jwt_secret = current.server.jwt_secret.clone();
        }

        if provider_identity_matches(&self.provider, &current.provider) {
            preserve_provider_secrets(&mut self.provider, &current.provider);
        }
        for provider in &mut self.providers {
            if let Some(existing) = current
                .providers
                .iter()
                .find(|existing| provider_identity_matches(existing, provider))
            {
                preserve_provider_secrets(provider, existing);
            }
        }

        if let (Some(telegram), Some(existing)) = (&mut self.telegram, &current.telegram) {
            if telegram.bot_token.trim().is_empty() {
                telegram.bot_token = existing.bot_token.clone();
            }
        }
        if let (Some(discord), Some(existing)) = (&mut self.discord, &current.discord) {
            if discord.token.trim().is_empty() {
                discord.token = existing.token.clone();
            }
            if discord.github_token.trim().is_empty() {
                discord.github_token = existing.github_token.clone();
            }
        }
        for server in &mut self.mcp.servers {
            if let Some(existing) = current
                .mcp
                .servers
                .iter()
                .find(|existing| existing.name == server.name)
            {
                if server.env.is_empty() {
                    server.env = existing.env.clone();
                }
                if server.headers.is_empty() {
                    server.headers = existing.headers.clone();
                }
            }
        }
    }

    pub fn load(path: &str) -> Result<Self> {
        let expanded = shellexpand::tilde(path).to_string();
        let path_ref = Path::new(&expanded);

        if !path_ref.exists() {
            let mut cfg = Self::default_config();
            cfg.config_path = Some(path_ref.to_path_buf());
            cfg.save(path_ref)?;
            return Ok(cfg);
        }

        let raw = fs::read_to_string(path_ref).map_err(OSAgentError::Io)?;
        let mut cfg: Config = toml::from_str(&raw)
            .map_err(|e| OSAgentError::Config(format!("Failed to parse config TOML: {}", e)))?;
        cfg.config_path = Some(path_ref.to_path_buf());
        let mutated = cfg.ensure_server_security_defaults();
        cfg.ensure_workspace_defaults();
        cfg.migrate_tool_defaults();
        let migrated_max_tokens = cfg.migrate_max_tokens();
        if mutated || migrated_max_tokens {
            cfg.save(path_ref)?;
        }
        Ok(cfg)
    }

    pub fn config_dir(&self) -> PathBuf {
        self.config_path
            .as_deref()
            .and_then(Path::parent)
            .map(Path::to_path_buf)
            .unwrap_or_else(|| PathBuf::from(shellexpand::tilde("~/.osagent").to_string()))
    }

    /// Persist back to the file this config was loaded from.
    ///
    /// Callers that mutate a live config (the settings UI) need this;
    /// re-deriving the path at every call site invites writing to the
    /// wrong file when OSA was started with `--config`.
    pub fn save_to_current_path(&self) -> Result<()> {
        let path = self.config_path.clone().unwrap_or_else(|| {
            PathBuf::from(shellexpand::tilde("~/.osagent/config.toml").to_string())
        });
        self.save(path)
    }

    pub(crate) fn inherit_config_path(&mut self, existing: &Self) {
        if self.config_path.is_none() {
            self.config_path = existing.config_path.clone();
        }
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            fs::create_dir_all(parent).map_err(OSAgentError::Io)?;
        }

        let mut cloned = self.clone();
        cloned.ensure_server_security_defaults();
        cloned.ensure_workspace_defaults();
        cloned.migrate_legacy_provider();
        let data = toml::to_string_pretty(&cloned)
            .map_err(|e| OSAgentError::Config(format!("Failed to serialize config: {}", e)))?;
        fs::write(path_ref, data).map_err(OSAgentError::Io)?;
        Ok(())
    }

    pub fn ensure_server_security_defaults(&mut self) -> bool {
        let mut mutated = false;

        if self.server.jwt_secret.trim().is_empty() {
            self.server.jwt_secret = generate_jwt_secret();
            mutated = true;
        }

        let mut cleaned_origins = Vec::new();
        for origin in &self.server.cors_allowed_origins {
            let trimmed = origin.trim();
            if trimmed.is_empty() {
                continue;
            }
            if cleaned_origins.iter().any(|existing| existing == trimmed) {
                continue;
            }
            cleaned_origins.push(trimmed.to_string());
        }

        if cleaned_origins != self.server.cors_allowed_origins {
            self.server.cors_allowed_origins = cleaned_origins;
            mutated = true;
        }

        mutated
    }

    pub fn migrate_legacy_provider(&mut self) {
        if self.providers.is_empty() && !self.provider.api_key.is_empty() {
            self.providers.push(self.provider.clone());
            if self.default_provider.is_empty() {
                self.default_provider = self.provider.provider_type.clone();
            }
            if self.default_model.is_empty() {
                self.default_model = self.provider.model.clone();
            }
        }
    }

    pub fn migrate_tool_defaults(&mut self) {}

    pub fn migrate_max_tokens(&mut self) -> bool {
        if self.agent.max_tokens <= 4096 {
            self.agent.max_tokens = 16384;
            return true;
        }
        false
    }

    pub fn migrate_workspace_paths(&mut self) {
        for ws in &mut self.agent.workspaces {
            if ws.paths.is_empty() && !ws.path.is_empty() {
                ws.paths.push(WorkspacePath {
                    path: shellexpand::tilde(&ws.path).to_string(),
                    permission: ws.permission.clone(),
                    description: Some("Primary workspace directory".to_string()),
                });
            }

            ws.paths = ws
                .paths
                .iter()
                .filter_map(|wp| {
                    let expanded = shellexpand::tilde(&wp.path).to_string();
                    if expanded.trim().is_empty() {
                        None
                    } else {
                        Some(WorkspacePath {
                            path: expanded,
                            permission: wp.permission.clone(),
                            description: wp.description.clone(),
                        })
                    }
                })
                .collect();

            ws.path = ws.resolved_path();
        }
    }

    pub fn active_provider(&self) -> Option<&ProviderConfig> {
        if let Some(id) = self.default_provider.strip_prefix("env:") {
            if let Ok(key) = std::env::var(id) {
                if !key.is_empty() {
                    return self
                        .providers
                        .iter()
                        .find(|p| p.provider_type == self.default_provider);
                }
            }
        }
        self.providers
            .iter()
            .find(|p| p.provider_type == self.default_provider)
            .or(self.providers.first())
    }

    pub fn active_model(&self) -> String {
        if !self.default_model.is_empty() {
            return self.default_model.clone();
        }
        self.provider.model.clone()
    }

    pub fn set_active_provider_model(&mut self, provider_id: &str, model: &str) {
        self.default_provider = provider_id.to_string();
        self.default_model = model.to_string();
        if let Some(p) = self
            .providers
            .iter_mut()
            .find(|p| p.provider_type == provider_id)
        {
            p.model = model.to_string();
        }
    }

    pub fn ensure_workspace_defaults(&mut self) {
        self.migrate_workspace_paths();

        let fallback_path = if self.agent.workspace.trim().is_empty() {
            default_workspace_path()
        } else {
            shellexpand::tilde(&self.agent.workspace).to_string()
        };
        self.agent.workspace = fallback_path.clone();

        let mut seen = HashSet::new();
        let mut cleaned = Vec::new();
        for mut ws in self.agent.workspaces.clone() {
            if ws.id.trim().is_empty() || !seen.insert(ws.id.clone()) {
                continue;
            }

            ws.paths = ws
                .paths
                .iter()
                .filter_map(|wp| {
                    let expanded = shellexpand::tilde(&wp.path).to_string();
                    if expanded.trim().is_empty() {
                        None
                    } else {
                        Some(WorkspacePath {
                            path: expanded,
                            permission: wp.permission.clone(),
                            description: wp.description.clone(),
                        })
                    }
                })
                .collect();

            if ws.paths.is_empty() && !ws.path.trim().is_empty() {
                ws.paths.push(WorkspacePath {
                    path: shellexpand::tilde(&ws.path).to_string(),
                    permission: ws.permission.clone(),
                    description: Some("Primary workspace directory".to_string()),
                });
            }

            if ws.id == "default" && ws.paths.is_empty() {
                ws.paths.push(WorkspacePath {
                    path: fallback_path.clone(),
                    permission: WorkspacePermission::ReadWrite,
                    description: Some("Default working directory".to_string()),
                });
            }

            ws.path = ws.resolved_path();

            if ws.name.trim().is_empty() {
                ws.name = ws.id.clone();
            }
            if ws.created_at.trim().is_empty() {
                ws.created_at = chrono::Utc::now().to_rfc3339();
            }
            cleaned.push(ws);
        }

        if !cleaned.iter().any(|w| w.id == "default") {
            cleaned.push(WorkspaceConfig {
                id: "default".to_string(),
                name: "Default Workspace".to_string(),
                paths: vec![WorkspacePath {
                    path: fallback_path.clone(),
                    permission: WorkspacePermission::ReadWrite,
                    description: Some("Default working directory".to_string()),
                }],
                path: fallback_path.clone(),
                description: Some("Default working directory".to_string()),
                permission: WorkspacePermission::ReadWrite,
                created_at: chrono::Utc::now().to_rfc3339(),
                last_used: Some(chrono::Utc::now().to_rfc3339()),
            });
        }

        self.agent.workspaces = cleaned;

        let active_id = self
            .agent
            .active_workspace
            .clone()
            .filter(|id| self.agent.workspaces.iter().any(|w| &w.id == id))
            .unwrap_or_else(|| "default".to_string());
        self.agent.active_workspace = Some(active_id.clone());

        if let Some(active) = self.agent.workspaces.iter().find(|w| w.id == active_id) {
            let active_path = active.resolved_path();
            if !active_path.trim().is_empty() {
                self.agent.workspace = active_path;
            } else {
                self.agent.workspace = fallback_path;
            }
        }
    }

    pub fn get_active_workspace(&self) -> WorkspaceConfig {
        let mut cloned = self.clone();
        cloned.ensure_workspace_defaults();
        let active_id = cloned
            .agent
            .active_workspace
            .clone()
            .unwrap_or_else(|| "default".to_string());
        cloned
            .agent
            .workspaces
            .into_iter()
            .find(|w| w.id == active_id)
            .unwrap_or_else(WorkspaceConfig::default)
    }

    pub fn list_workspaces(&self) -> Vec<WorkspaceConfig> {
        let mut cloned = self.clone();
        cloned.ensure_workspace_defaults();
        cloned.agent.workspaces
    }

    pub fn get_workspace(&self, id: &str) -> Option<WorkspaceConfig> {
        self.list_workspaces().into_iter().find(|w| w.id == id)
    }

    pub fn get_workspace_by_path(&self, path: &str) -> Option<WorkspaceConfig> {
        let expanded = shellexpand::tilde(path).to_string();
        let path_canonical = Path::new(&expanded).canonicalize().ok();

        for ws in self.list_workspaces() {
            for wp in &ws.paths {
                if wp.path == expanded {
                    return Some(ws);
                }
                if let (Some(pc), Some(wpc)) =
                    (&path_canonical, Path::new(&wp.path).canonicalize().ok())
                {
                    if pc.starts_with(&wpc) {
                        return Some(ws);
                    }
                }
            }
        }
        None
    }

    pub fn get_workspace_for_path(&self, path: &str) -> Option<(WorkspaceConfig, WorkspacePath)> {
        let expanded = shellexpand::tilde(path).to_string();
        let path_canonical = Path::new(&expanded).canonicalize().ok();

        for ws in self.list_workspaces() {
            for wp in &ws.paths {
                if wp.path == expanded {
                    return Some((ws.clone(), wp.clone()));
                }
                if let (Some(pc), Some(wpc)) =
                    (&path_canonical, Path::new(&wp.path).canonicalize().ok())
                {
                    if pc.starts_with(&wpc) {
                        return Some((ws.clone(), wp.clone()));
                    }
                }
            }
        }
        None
    }

    pub fn is_path_in_workspace(&self, path: &str) -> bool {
        self.get_workspace_for_path(path).is_some()
    }

    pub fn is_workspace_writable_for_path(&self, path: &str) -> bool {
        self.get_workspace_for_path(path)
            .map(|(_, wp)| wp.permission.allows_writes())
            .unwrap_or(true)
    }

    pub fn get_path_permission(&self, path: &str) -> Option<WorkspacePermission> {
        self.get_workspace_for_path(path)
            .map(|(_, wp)| wp.permission)
    }

    pub fn add_workspace(&mut self, mut workspace: WorkspaceConfig) -> Result<()> {
        self.ensure_workspace_defaults();
        if self.agent.workspaces.iter().any(|w| w.id == workspace.id) {
            return Err(OSAgentError::Config(format!(
                "Workspace with ID '{}' already exists",
                workspace.id
            )));
        }

        for wp in &mut workspace.paths {
            wp.path = shellexpand::tilde(&wp.path).to_string();
        }
        workspace.paths.retain(|wp| !wp.path.trim().is_empty());
        workspace.path = workspace.resolved_path();
        if workspace.created_at.trim().is_empty() {
            workspace.created_at = chrono::Utc::now().to_rfc3339();
        }
        self.agent.workspaces.push(workspace);
        Ok(())
    }

    pub fn update_workspace(&mut self, mut workspace: WorkspaceConfig) -> Result<()> {
        self.ensure_workspace_defaults();
        if let Some(idx) = self
            .agent
            .workspaces
            .iter()
            .position(|w| w.id == workspace.id)
        {
            for wp in &mut workspace.paths {
                wp.path = shellexpand::tilde(&wp.path).to_string();
            }
            workspace.paths.retain(|wp| !wp.path.trim().is_empty());
            workspace.path = workspace.resolved_path();
            if workspace.created_at.trim().is_empty() {
                workspace.created_at = self.agent.workspaces[idx].created_at.clone();
            }
            self.agent.workspaces[idx] = workspace;
            return Ok(());
        }

        Err(OSAgentError::Config(format!(
            "Workspace with ID '{}' not found",
            workspace.id
        )))
    }

    pub fn add_workspace_path(
        &mut self,
        workspace_id: &str,
        mut path: WorkspacePath,
    ) -> Result<()> {
        self.ensure_workspace_defaults();
        let ws = self
            .agent
            .workspaces
            .iter_mut()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| {
                OSAgentError::Config(format!("Workspace '{}' not found", workspace_id))
            })?;

        path.path = shellexpand::tilde(&path.path).to_string();
        ws.paths.push(path);
        ws.paths.retain(|wp| !wp.path.trim().is_empty());
        ws.path = ws.resolved_path();
        Ok(())
    }

    pub fn remove_workspace_path(&mut self, workspace_id: &str, path_index: usize) -> Result<()> {
        self.ensure_workspace_defaults();
        let ws = self
            .agent
            .workspaces
            .iter_mut()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| {
                OSAgentError::Config(format!("Workspace '{}' not found", workspace_id))
            })?;

        if ws.paths.len() <= 1 {
            return Err(OSAgentError::Config(
                "Cannot remove the last path from a workspace".to_string(),
            ));
        }

        if path_index >= ws.paths.len() {
            return Err(OSAgentError::Config(format!(
                "Path index {} out of bounds",
                path_index
            )));
        }

        ws.paths.remove(path_index);
        ws.path = ws.resolved_path();
        Ok(())
    }

    pub fn update_workspace_path(
        &mut self,
        workspace_id: &str,
        path_index: usize,
        mut path: WorkspacePath,
    ) -> Result<()> {
        self.ensure_workspace_defaults();
        let ws = self
            .agent
            .workspaces
            .iter_mut()
            .find(|w| w.id == workspace_id)
            .ok_or_else(|| {
                OSAgentError::Config(format!("Workspace '{}' not found", workspace_id))
            })?;

        if path_index >= ws.paths.len() {
            return Err(OSAgentError::Config(format!(
                "Path index {} out of bounds",
                path_index
            )));
        }

        path.path = shellexpand::tilde(&path.path).to_string();
        ws.paths[path_index] = path;
        ws.paths.retain(|wp| !wp.path.trim().is_empty());
        ws.path = ws.resolved_path();
        Ok(())
    }

    pub fn get_workspace_paths(&self, workspace_id: &str) -> Option<Vec<WorkspacePath>> {
        self.list_workspaces()
            .into_iter()
            .find(|w| w.id == workspace_id)
            .map(|w| w.paths)
    }

    pub fn remove_workspace(&mut self, id: &str) -> Result<()> {
        self.ensure_workspace_defaults();
        if id == "default" {
            return Err(OSAgentError::Config(
                "Cannot remove the default workspace".to_string(),
            ));
        }

        let before = self.agent.workspaces.len();
        self.agent.workspaces.retain(|w| w.id != id);
        if self.agent.workspaces.len() == before {
            return Err(OSAgentError::Config(format!(
                "Workspace '{}' was not found",
                id
            )));
        }

        if self.agent.active_workspace.as_deref() == Some(id) {
            self.agent.active_workspace = Some("default".to_string());
        }
        Ok(())
    }

    pub fn add_permission_rule(&mut self, mut rule: PermissionRule) -> Result<()> {
        if rule.id.is_empty() {
            rule.id = uuid::Uuid::new_v4().to_string();
        }
        self.agent.permission_rules.push(rule);
        Ok(())
    }

    pub fn remove_permission_rule(&mut self, rule_id: &str) -> Result<()> {
        let before = self.agent.permission_rules.len();
        self.agent.permission_rules.retain(|r| r.id != rule_id);
        if self.agent.permission_rules.len() == before {
            return Err(OSAgentError::Config(format!(
                "Permission rule '{}' not found",
                rule_id
            )));
        }
        Ok(())
    }

    pub fn get_permission_rules(&self) -> Vec<PermissionRule> {
        self.agent.permission_rules.clone()
    }

    pub fn evaluate_permission_rule(
        &self,
        tool_name: &str,
        path: &str,
    ) -> Option<PermissionAction> {
        let path = shellexpand::tilde(path).to_string();
        for rule in &self.agent.permission_rules {
            let matches_tool = rule.permission == "all" || rule.permission == tool_name;
            if !matches_tool {
                continue;
            }
            let matches_path = if let Ok(matcher) = globset::Glob::new(&rule.pattern) {
                matcher.compile_matcher().is_match(&path)
            } else {
                false
            };
            if matches_path {
                return Some(rule.action.clone());
            }
        }
        None
    }
}

fn redact_provider_config(provider: &mut ProviderConfig) {
    provider.api_key.clear();
    provider.oauth_client_secret = None;
    provider.custom_headers = None;
}

fn provider_identity_matches(left: &ProviderConfig, right: &ProviderConfig) -> bool {
    left.provider_type == right.provider_type && left.base_url == right.base_url
}

fn preserve_provider_secrets(incoming: &mut ProviderConfig, current: &ProviderConfig) {
    if incoming.api_key.trim().is_empty() {
        incoming.api_key = current.api_key.clone();
    }
    if incoming.oauth_client_secret.is_none() {
        incoming.oauth_client_secret = current.oauth_client_secret.clone();
    }
    if incoming.custom_headers.is_none() {
        incoming.custom_headers = current.custom_headers.clone();
    }
}

pub fn setup_wizard(path: &str) -> Result<()> {
    let expanded = shellexpand::tilde(path).to_string();
    let path_ref = Path::new(&expanded);
    if path_ref.exists() {
        eprintln!(
            "Config already exists at {}. Please delete it first or edit it manually.",
            expanded
        );
        return Ok(());
    }

    println!("\n=== OSA Setup Wizard ===\n");
    println!("This wizard will help you configure OSA (Open Source Agent).");
    println!("Press Ctrl+C at any time to abort.\n");

    let password =
        prompt_password("Enter a password for the web UI (leave empty to generate random): ")?;
    let password_hash = if password.is_empty() {
        let random_pw = generate_random_password(16);
        println!("Generated password: {}\n", random_pw);
        bcrypt::hash(&random_pw, bcrypt::DEFAULT_COST)
            .map_err(|e| OSAgentError::Config(format!("Failed to hash password: {}", e)))?
    } else {
        bcrypt::hash(&password, bcrypt::DEFAULT_COST)
            .map_err(|e| OSAgentError::Config(format!("Failed to hash password: {}", e)))?
    };

    println!("\nSelect a provider:");
    println!("  1. OpenRouter (recommended - 200+ models including Claude, GPT, Gemini)");
    println!("  2. OpenAI (GPT-4.1, GPT-4o)");
    println!("  3. Anthropic (Claude Sonnet 4, Claude 3.5)");
    println!("  4. Google (Gemini 2.5 Pro, Gemini Flash)");
    println!("  5. Ollama (local models)");
    println!("  6. Groq (fast free models)");
    println!("  7. DeepSeek (DeepSeek R1, V3)");
    println!("  8. xAI (Grok 3)");

    let provider_choice = prompt_input("Enter choice [1-8] (default: 1): ")?;
    let provider_choice = provider_choice.trim().chars().next().unwrap_or('1');

    let (provider_type, api_key_prompt, base_url, default_model) = match provider_choice {
        '2' => (
            "openai",
            "OpenAI API key: ",
            "https://api.openai.com/v1",
            "gpt-4.1",
        ),
        '3' => (
            "anthropic",
            "Anthropic API key: ",
            "https://api.anthropic.com/v1",
            "claude-sonnet-4-20250514",
        ),
        '4' => (
            "google",
            "Google AI API key: ",
            "https://generativelanguage.googleapis.com/v1beta/openai",
            "gemini-2.0-flash",
        ),
        '5' => (
            "ollama",
            "Ollama API key (or press Enter for none): ",
            "http://localhost:11434/v1",
            "llama3.2",
        ),
        '6' => (
            "groq",
            "Groq API key: ",
            "https://api.groq.com/openai/v1",
            "llama-3.3-70b-versatile",
        ),
        '7' => (
            "deepseek",
            "DeepSeek API key: ",
            "https://api.deepseek.com",
            "deepseek-chat",
        ),
        '8' => ("xai", "xAI API key: ", "https://api.x.ai/v1", "grok-3"),
        _ => (
            "openrouter",
            "OpenRouter API key: ",
            "https://openrouter.ai/api/v1",
            "anthropic/claude-sonnet-4",
        ),
    };

    let api_key = prompt_password(api_key_prompt)?;
    if api_key.is_empty() && provider_type != "ollama" {
        eprintln!(
            "API key is required for {}. Setup cancelled.",
            provider_type
        );
        return Err(OSAgentError::Config("API key required".to_string()));
    }

    let mut cfg = Config::default_config();
    cfg.server.password = password_hash;
    cfg.server.password_enabled = true;
    cfg.providers.push(ProviderConfig {
        provider_type: provider_type.to_string(),
        api_key,
        base_url: base_url.to_string(),
        model: default_model.to_string(),
        fallbacks: vec![],
        auth_type: None,
        oauth_client_id: None,
        oauth_client_secret: None,
        oauth_authorization_url: None,
        oauth_token_url: None,
        oauth_scopes: None,
        custom_headers: None,
        redirect_url: None,
    });
    cfg.default_provider = provider_type.to_string();
    cfg.default_model = default_model.to_string();

    println!("\nCreating config at {}...", expanded);
    cfg.save(path_ref)?;
    println!("\n✓ Configuration saved!");
    println!("\nNext steps:");
    println!("  1. Run 'osagent start' to start the server");
    println!("  2. Open http://localhost:8765 in your browser");
    println!("  3. Log in with your password\n");

    Ok(())
}

fn prompt_input(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    std::io::Write::flush(&mut std::io::stdout())
        .map_err(|_| OSAgentError::Io(std::io::Error::other("flush error")))?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(OSAgentError::Io)?;
    Ok(input)
}

fn prompt_password(prompt: &str) -> Result<String> {
    print!("{}", prompt);
    std::io::Write::flush(&mut std::io::stdout())
        .map_err(|_| OSAgentError::Io(std::io::Error::other("flush error")))?;
    let mut input = String::new();
    std::io::stdin()
        .read_line(&mut input)
        .map_err(OSAgentError::Io)?;
    Ok(input.trim().to_string())
}

fn generate_random_password(length: usize) -> String {
    use rand::Rng;
    const CHARSET: &[u8] =
        b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789!@#$%^&*";
    let mut rng = rand::thread_rng();
    (0..length)
        .map(|_| {
            let idx = rng.gen_range(0..CHARSET.len());
            CHARSET[idx] as char
        })
        .collect()
}

fn default_workspace_path() -> String {
    "~/.osagent/workspace".to_string()
}

fn default_memory_file() -> String {
    "~/.osagent/memories.json".to_string()
}

fn default_decision_memory_file() -> String {
    "~/.osagent/decision_memories.json".to_string()
}

fn generate_jwt_secret() -> String {
    use base64::engine::general_purpose::URL_SAFE_NO_PAD;
    use base64::Engine;
    use rand::RngCore;

    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    URL_SAFE_NO_PAD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::{Config, DiscordConfig, McpServerConfig, ProviderConfig, TelegramConfig};
    use std::collections::HashMap;
    use std::fs;
    use tempfile::tempdir;

    /// A snowflake must survive the trip through the web UI unchanged — as a
    /// JSON number it would come back rounded to the nearest multiple of 64.
    #[test]
    fn discord_ids_survive_a_json_round_trip() {
        let discord: DiscordConfig = toml::from_str(
            r#"
enabled = true
token = "t"
allowed_users = [420155234833268737]
last_channel_id = 1478327393205882900
"#,
        )
        .expect("legacy integer ids must still load");

        assert_eq!(discord.allowed_users, vec![420155234833268737]);
        assert_eq!(discord.last_channel_id, Some(1478327393205882900));

        let json = serde_json::to_string(&discord).unwrap();
        assert!(
            json.contains("\"420155234833268737\""),
            "ids must be sent as strings, got {json}"
        );

        let round_tripped: DiscordConfig = serde_json::from_str(&json).unwrap();
        assert_eq!(round_tripped.allowed_users, discord.allowed_users);
        assert_eq!(round_tripped.last_channel_id, discord.last_channel_id);
    }

    #[test]
    fn discord_ids_persist_to_toml_as_strings() {
        let discord = DiscordConfig {
            enabled: true,
            token: "t".to_string(),
            community_mode: false,
            allow_community_members: false,
            community_context: String::new(),
            docs_url: String::new(),
            github_repo: String::new(),
            github_token: String::new(),
            github_tracking_channel: None,
            github_poll_seconds: 300,
            allowed_users: vec![420155234833268737],
            allowed_roles: vec![],
            allowed_guilds: vec![],
            allowed_channels: vec![],
            allow_dms: false,
            trusted_users: vec![],
            trusted_roles: vec![],
            trusted_guilds: vec![],
            trusted_channels: vec![],
            last_channel_id: None,
            music_enabled: false,
            music_max_queue: 50,
            music_max_duration_secs: 0,
            music_auto_leave_secs: 300,
            yt_dlp_path: "yt-dlp".to_string(),
            yt_dlp_extra_args: String::new(),
            piped_instances: Vec::new(),
        };

        let text = toml::to_string(&discord).unwrap();
        let reloaded: DiscordConfig = toml::from_str(&text).unwrap();

        assert_eq!(reloaded.allowed_users, vec![420155234833268737]);
        assert_eq!(reloaded.last_channel_id, None);
    }

    #[test]
    fn api_config_redaction_removes_credentials() {
        let mut config = Config::default_config();
        config.server.password = "password-hash-secret".to_string();
        config.server.jwt_secret = "jwt-secret".to_string();

        let provider = ProviderConfig {
            provider_type: "test-provider".to_string(),
            api_key: "provider-api-key".to_string(),
            oauth_client_secret: Some("oauth-client-secret".to_string()),
            custom_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "provider-header-secret".to_string(),
            )])),
            ..ProviderConfig::default()
        };
        config.provider = provider.clone();
        config.providers = vec![provider];

        let telegram = TelegramConfig {
            bot_token: "telegram-bot-token".to_string(),
            ..TelegramConfig::default()
        };
        config.telegram = Some(telegram);

        let discord = DiscordConfig {
            token: "discord-bot-token".to_string(),
            github_token: "github-token".to_string(),
            ..DiscordConfig::default()
        };
        config.discord = Some(discord);

        let mut mcp = McpServerConfig {
            name: "secret-server".to_string(),
            ..McpServerConfig::default()
        };
        mcp.env
            .insert("API_TOKEN".to_string(), "mcp-env-secret".to_string());
        mcp.headers
            .insert("Authorization".to_string(), "mcp-header-secret".to_string());
        config.mcp.servers = vec![mcp];

        let redacted = config.redacted_for_api();
        let serialized = serde_json::to_string(&redacted).unwrap();

        for secret in [
            "password-hash-secret",
            "jwt-secret",
            "provider-api-key",
            "oauth-client-secret",
            "provider-header-secret",
            "telegram-bot-token",
            "discord-bot-token",
            "github-token",
            "mcp-env-secret",
            "mcp-header-secret",
        ] {
            assert!(
                !serialized.contains(secret),
                "redacted config contains {secret}"
            );
        }

        assert_eq!(redacted.provider.provider_type, "test-provider");
        assert_eq!(redacted.mcp.servers[0].name, "secret-server");
    }

    #[test]
    fn redacted_config_round_trip_preserves_credentials() {
        let mut current = Config::default_config();
        current.server.password = "password-hash-secret".to_string();
        current.server.jwt_secret = "jwt-secret".to_string();

        let provider = ProviderConfig {
            provider_type: "test-provider".to_string(),
            api_key: "provider-api-key".to_string(),
            oauth_client_secret: Some("oauth-client-secret".to_string()),
            custom_headers: Some(HashMap::from([(
                "Authorization".to_string(),
                "provider-header-secret".to_string(),
            )])),
            ..ProviderConfig::default()
        };
        current.provider = provider.clone();
        current.providers = vec![provider];

        let telegram = TelegramConfig {
            bot_token: "telegram-bot-token".to_string(),
            ..TelegramConfig::default()
        };
        current.telegram = Some(telegram);

        let discord = DiscordConfig {
            token: "discord-bot-token".to_string(),
            github_token: "github-token".to_string(),
            ..DiscordConfig::default()
        };
        current.discord = Some(discord);

        let mut mcp = McpServerConfig {
            name: "secret-server".to_string(),
            ..McpServerConfig::default()
        };
        mcp.env
            .insert("API_TOKEN".to_string(), "mcp-env-secret".to_string());
        mcp.headers
            .insert("Authorization".to_string(), "mcp-header-secret".to_string());
        current.mcp.servers = vec![mcp];

        let mut incoming = current.redacted_for_api();
        incoming.agent.temperature = 0.25;
        incoming.preserve_secrets_from(&current);

        assert_eq!(incoming.server.password, current.server.password);
        assert_eq!(incoming.server.jwt_secret, current.server.jwt_secret);
        assert_eq!(incoming.provider.api_key, current.provider.api_key);
        assert_eq!(incoming.providers[0].api_key, current.providers[0].api_key);
        assert_eq!(
            incoming.providers[0].oauth_client_secret,
            current.providers[0].oauth_client_secret
        );
        assert_eq!(
            incoming.telegram.as_ref().unwrap().bot_token,
            current.telegram.as_ref().unwrap().bot_token
        );
        assert_eq!(
            incoming.discord.as_ref().unwrap().github_token,
            current.discord.as_ref().unwrap().github_token
        );
        assert_eq!(incoming.mcp.servers[0].env, current.mcp.servers[0].env);
        assert_eq!(
            incoming.mcp.servers[0].headers,
            current.mcp.servers[0].headers
        );
        assert_eq!(incoming.agent.temperature, 0.25);
    }

    #[test]
    fn load_generates_and_persists_jwt_secret() {
        let temp_dir = tempdir().unwrap();
        let config_path = temp_dir.path().join("config.toml");

        fs::write(
            &config_path,
            r#"
[server]
bind = "127.0.0.1"
port = 8765
password = ""
password_enabled = false
"#,
        )
        .unwrap();

        let first = Config::load(config_path.to_str().unwrap()).unwrap();
        assert!(!first.server.jwt_secret.is_empty());
        assert_eq!(first.config_dir(), temp_dir.path());

        let second = Config::load(config_path.to_str().unwrap()).unwrap();
        assert_eq!(first.server.jwt_secret, second.server.jwt_secret);
    }
}
