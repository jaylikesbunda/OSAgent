use crate::error::{OSAgentError, Result};
use crate::external::ExternalPermissionConfig;
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::fs;
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
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
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ServerConfig {
    pub bind: String,
    pub port: u16,
    pub password: String,
    pub password_enabled: bool,
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
    pub checkpoint_enabled: bool,
    pub checkpoint_interval: usize,
    pub memory_enabled: bool,
    pub memory_file: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct WorkspaceConfig {
    pub id: String,
    pub name: String,
    pub path: String,
    pub description: Option<String>,
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

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
#[derive(Default)]
pub struct DiscordConfig {
    pub enabled: bool,
    pub token: String,
    pub allowed_users: Vec<u64>,
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
    pub voice_speed: f32,
    pub whisper_model: Option<String>,
    pub piper_voice: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct ToolsConfig {
    pub allowed: Vec<String>,
    pub bash: BashToolConfig,
    pub code_python: CodeToolConfig,
    pub code_node: CodeToolConfig,
    pub code_bash: CodeToolConfig,
    pub grep: GrepToolConfig,
    pub glob: GrepToolConfig,
    pub skills: SkillsConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct SkillsConfig {
    pub enabled: bool,
    pub directory: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct BashToolConfig {
    pub allowed_commands: Vec<String>,
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
    pub enabled: bool,
    pub index_on_startup: bool,
    pub max_results: usize,
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
    pub repo: String,
    pub check_on_startup: bool,
    pub check_interval_hours: u64,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            channel: "stable".to_string(),
            repo: "owner/osagent".to_string(),
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

impl Default for ServerConfig {
    fn default() -> Self {
        Self {
            bind: "127.0.0.1".to_string(),
            port: 8765,
            password: "".to_string(),
            password_enabled: false,
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
            path: default_workspace_path(),
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
            max_tokens: 4096,
            temperature: 0.7,
            checkpoint_enabled: true,
            checkpoint_interval: 5,
            memory_enabled: false,
            memory_file: default_memory_file(),
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
            index_on_startup: true,
            max_results: 20,
        }
    }
}

impl Default for ToolsConfig {
    fn default() -> Self {
        Self {
            allowed: vec![
                "batch".to_string(),
                "bash".to_string(),
                "read_file".to_string(),
                "write_file".to_string(),
                "edit_file".to_string(),
                "apply_patch".to_string(),
                "list_files".to_string(),
                "delete_file".to_string(),
                "grep".to_string(),
                "glob".to_string(),
                "web_fetch".to_string(),
                "web_search".to_string(),
                "code_python".to_string(),
                "code_node".to_string(),
                "code_bash".to_string(),
                "task".to_string(),
                "persona".to_string(),
                "todowrite".to_string(),
                "todoread".to_string(),
                "question".to_string(),
                "skill".to_string(),
                "lsp".to_string(),
                "plan_exit".to_string(),
                "subagent".to_string(),
                "process".to_string(),
                "codesearch".to_string(),
                "record_memory".to_string(),
            ],
            bash: BashToolConfig::default(),
            code_python: CodeToolConfig::default(),
            code_node: CodeToolConfig::default(),
            code_bash: CodeToolConfig::default(),
            grep: GrepToolConfig::default(),
            glob: GrepToolConfig::default(),
            skills: SkillsConfig::default(),
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
        };
        cfg.ensure_workspace_defaults();
        cfg
    }

    pub fn load(path: &str) -> Result<Self> {
        let expanded = shellexpand::tilde(path).to_string();
        let path_ref = Path::new(&expanded);

        if !path_ref.exists() {
            let cfg = Self::default_config();
            cfg.save(path_ref)?;
            return Ok(cfg);
        }

        let raw = fs::read_to_string(path_ref).map_err(OSAgentError::Io)?;
        let mut cfg: Config = toml::from_str(&raw)
            .map_err(|e| OSAgentError::Config(format!("Failed to parse config TOML: {}", e)))?;
        cfg.ensure_workspace_defaults();
        cfg.migrate_tool_defaults();
        Ok(cfg)
    }

    pub fn save<P: AsRef<Path>>(&self, path: P) -> Result<()> {
        let path_ref = path.as_ref();
        if let Some(parent) = path_ref.parent() {
            fs::create_dir_all(parent).map_err(OSAgentError::Io)?;
        }

        let mut cloned = self.clone();
        cloned.ensure_workspace_defaults();
        cloned.migrate_legacy_provider();
        let data = toml::to_string_pretty(&cloned)
            .map_err(|e| OSAgentError::Config(format!("Failed to serialize config: {}", e)))?;
        fs::write(path_ref, data).map_err(OSAgentError::Io)?;
        Ok(())
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

    pub fn migrate_tool_defaults(&mut self) {
        let default_allowed = ToolsConfig::default().allowed;
        for tool in default_allowed {
            if !self.tools.allowed.contains(&tool) {
                self.tools.allowed.push(tool);
            }
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
            ws.path = shellexpand::tilde(&ws.path).to_string();
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
            self.agent.workspace = active.path.clone();
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
        self.list_workspaces()
            .into_iter()
            .find(|w| w.path == expanded)
    }

    pub fn is_workspace_writable_for_path(&self, path: &str) -> bool {
        self.get_workspace_by_path(path)
            .map(|workspace| workspace.permission.allows_writes())
            .unwrap_or(true)
    }

    pub fn add_workspace(&mut self, mut workspace: WorkspaceConfig) -> Result<()> {
        self.ensure_workspace_defaults();
        if self.agent.workspaces.iter().any(|w| w.id == workspace.id) {
            return Err(OSAgentError::Config(format!(
                "Workspace with ID '{}' already exists",
                workspace.id
            )));
        }

        workspace.path = shellexpand::tilde(&workspace.path).to_string();
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
            workspace.path = shellexpand::tilde(&workspace.path).to_string();
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
