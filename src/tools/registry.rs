use crate::agent::coordinator::Coordinator;
use crate::agent::decision_memory::DecisionMemory;
use crate::agent::events::EventBus;
use crate::agent::memory::MemoryStore;
use crate::agent::provider::ToolDefinition;
use crate::agent::subagent_manager::SubagentManager;
use crate::config::{Config, WorkspacePath, WorkspacePermission};
use crate::error::{OSAgentError, Result};
use crate::indexer::CodeIndexer;
use crate::mcp::{McpHandle, McpManager, MCP_TOOL_PREFIX};
use crate::skills::SkillLoader;
use crate::tools::file_cache::FileReadCache;
use crate::tools::{
    bash, batch, calendar, code, codesearch, coordinator, decision_memory, files, lsp, memory,
    native_catalog::NativeToolCatalog, news, patch, persona, plan, process, question, scheduler,
    search, sessions, skill, subagent, system_status, task, todo, tool_script, tool_search,
    weather, web,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

/// Appended alongside MCP tools rather than sorted into the native
/// block; see `get_tool_definitions`.
const TOOL_SEARCH_NAME: &str = "tool_search";

#[derive(Debug, Clone)]
pub struct ToolExample {
    pub description: String,
    pub input: Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolAttachment {
    pub filename: String,
    pub mime: String,
    pub data_url: String,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolResult {
    pub output: String,
    #[serde(default)]
    pub outcome: ToolOutcome,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default = "default_tool_result_metadata")]
    pub metadata: Value,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub attachments: Vec<ToolAttachment>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolOutcome {
    #[default]
    Success,
    Failure,
    Retryable,
}

impl ToolOutcome {
    pub fn is_success(self) -> bool {
        matches!(self, Self::Success)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Self::Success => "success",
            Self::Failure => "failure",
            Self::Retryable => "retryable",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProfile {
    Default,
    Code,
    Plan,
    Creative,
    Custom,
    /// Public Discord support: public web reads only, with no host capabilities.
    Community,
}

impl ToolProfile {
    pub fn from_persona_id(persona_id: Option<&str>) -> Self {
        match persona_id.unwrap_or("default") {
            "code" => Self::Code,
            "plan" => Self::Plan,
            "creative" => Self::Creative,
            "custom" => Self::Custom,
            _ => Self::Default,
        }
    }

    pub(crate) fn allows(self, tool_name: &str) -> bool {
        // Discovery and the tools it yields travel together. A profile
        // that can call `tool_search` but not the tools it activates
        // sends the agent down a dead end: it searches, is told the tool
        // loaded, then cannot call it. Custom is the roleplay persona
        // and is deliberately near-toolless, so it gets neither.
        //
        // Plan additionally drops non-read-only MCP tools; that needs
        // the catalog, so it happens in the registry.
        if tool_name == "tool_search" || tool_name.starts_with(MCP_TOOL_PREFIX) {
            return !matches!(self, Self::Custom);
        }
        if tool_name == "tool_script" {
            return matches!(self, Self::Default | Self::Code | Self::Creative);
        }
        match self {
            Self::Default => !matches!(tool_name, "public_web_fetch"),
            Self::Code => !matches!(
                tool_name,
                "calendar" | "weather" | "news" | "public_web_fetch"
            ),
            Self::Plan => matches!(
                tool_name,
                "read_file"
                    | "list_files"
                    | "grep"
                    | "glob"
                    | "web_fetch"
                    | "web_search"
                    | "question"
                    | "skill"
                    | "skill_list"
                    | "lsp"
                    | "codesearch"
                    | "task"
                    | "todowrite"
                    | "todoread"
                    | "subagent"
                    | "system_status"
                    | "sessions"
                    | "plan_exit"
                    | "persona"
            ),
            Self::Creative => !matches!(
                tool_name,
                "bash"
                    | "code_bash"
                    | "delete_file"
                    | "process"
                    | "coordinator"
                    | "calendar"
                    | "weather"
                    | "news"
                    | "public_web_fetch"
            ),
            Self::Custom => matches!(
                tool_name,
                "web_fetch" | "web_search" | "question" | "skill" | "skill_list" | "persona"
            ),
            Self::Community => matches!(tool_name, "web_search" | "public_web_fetch"),
        }
    }
}

fn default_tool_result_metadata() -> Value {
    json!({})
}

impl ToolResult {
    pub fn new(output: impl Into<String>) -> Self {
        Self {
            output: output.into(),
            outcome: ToolOutcome::Success,
            title: None,
            metadata: default_tool_result_metadata(),
            attachments: Vec::new(),
        }
    }

    pub fn failure(output: impl Into<String>) -> Self {
        Self::new(output).with_outcome(ToolOutcome::Failure)
    }

    pub fn retryable(output: impl Into<String>) -> Self {
        Self::new(output).with_outcome(ToolOutcome::Retryable)
    }

    pub fn with_outcome(mut self, outcome: ToolOutcome) -> Self {
        self.outcome = outcome;
        self
    }
}

impl From<String> for ToolResult {
    fn from(output: String) -> Self {
        ToolResult::new(output)
    }
}

impl From<&str> for ToolResult {
    fn from(output: &str) -> Self {
        ToolResult::new(output)
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn name(&self) -> &str;
    fn description(&self) -> &str;
    fn parameters(&self) -> Value;
    async fn execute(&self, args: Value) -> Result<String>;

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        self.execute(args).await.map(ToolResult::from)
    }

    /// Per-tool execution budget in milliseconds. `None` means no
    /// registry-enforced deadline; the tool owns its own timing.
    /// When set, the registry wraps every dispatch site (sequential,
    /// parallel, batch sub-calls, per-workspace rebuilds) in a fused
    /// timeout and reports a structured `ToolTimeout` error instead of
    /// whatever the tool was doing when the deadline hit.
    fn timeout_ms(&self) -> Option<u64> {
        None
    }

    fn when_to_use(&self) -> &str {
        "See tool description"
    }

    fn when_not_to_use(&self) -> &str {
        "See tool description"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![]
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    allowed: HashSet<String>,
    base_config: Config,
    storage: Arc<crate::storage::SqliteStorage>,
    skill_loader: Option<Arc<SkillLoader>>,
    file_cache: Arc<FileReadCache>,
    coordinator: Option<Arc<Coordinator>>,
    scheduler: Option<Arc<crate::scheduler::Scheduler>>,
    mcp: McpHandle,
    /// Deferred low-frequency built-in tools, searched and activated via
    /// `tool_search`. These stay out of the always-loaded native block.
    native_catalog: Arc<NativeToolCatalog>,
    /// Native tool definitions only. MCP definitions are appended fresh
    /// on every read so that activating one never reorders or rewrites
    /// this block — the provider's cached prompt prefix survives.
    cached_tool_definitions: std::sync::RwLock<Option<Vec<ToolDefinition>>>,
}

/// Render the model-facing description for a tool: description,
/// when-to-use, when-not-to-use, and up to two examples. Shared by the
/// always-loaded block, the deferred built-in catalog, and `tool_search`
/// results so a tool reads identically wherever the model meets it.
pub(crate) fn tool_prompt_description(tool: &Arc<dyn Tool>) -> String {
    let mut sections = vec![tool.description().trim().to_string()];

    let when_to_use = tool.when_to_use().trim();
    if !when_to_use.is_empty() && when_to_use != "See tool description" {
        sections.push(format!("Use when: {}", when_to_use));
    }

    let when_not_to_use = tool.when_not_to_use().trim();
    if !when_not_to_use.is_empty() && when_not_to_use != "See tool description" {
        sections.push(format!("Avoid when: {}", when_not_to_use));
    }

    let examples = tool.examples();
    if !examples.is_empty() {
        let rendered_examples = examples
            .iter()
            .take(2)
            .map(|example| {
                let payload = serde_json::to_string(&example.input).unwrap_or_default();
                format!("{} => {}", example.description.trim(), payload)
            })
            .collect::<Vec<_>>()
            .join(" | ");
        if !rendered_examples.is_empty() {
            sections.push(format!("Examples: {}", rendered_examples));
        }
    }

    sections.join(" ")
}

impl ToolRegistry {
    pub fn new(config: Config, storage: Arc<crate::storage::SqliteStorage>) -> Result<Self> {
        let cache = Arc::new(FileReadCache::with_default_capacity());
        Self::with_deps_and_cache(config, storage, None, None, None, cache)
    }

    pub fn with_event_bus(
        config: Config,
        storage: Arc<crate::storage::SqliteStorage>,
        event_bus: Option<Arc<EventBus>>,
    ) -> Result<Self> {
        let cache = Arc::new(FileReadCache::with_default_capacity());
        Self::with_deps_and_cache(config, storage, event_bus, None, None, cache)
    }

    pub fn with_event_bus_and_skills(
        config: Config,
        storage: Arc<crate::storage::SqliteStorage>,
        event_bus: Option<Arc<EventBus>>,
        skill_loader: Option<Arc<SkillLoader>>,
    ) -> Result<Self> {
        let cache = Arc::new(FileReadCache::with_default_capacity());
        Self::with_indexer(
            config,
            storage,
            event_bus,
            skill_loader,
            None,
            None,
            None,
            None,
            cache,
        )
    }

    pub fn with_deps(
        config: Config,
        storage: Arc<crate::storage::SqliteStorage>,
        event_bus: Option<Arc<EventBus>>,
        skill_loader: Option<Arc<SkillLoader>>,
        subagent_manager: Option<Arc<SubagentManager>>,
    ) -> Result<Self> {
        let cache = Arc::new(FileReadCache::with_default_capacity());
        Self::with_indexer(
            config,
            storage,
            event_bus,
            skill_loader,
            subagent_manager,
            None,
            None,
            None,
            cache,
        )
    }

    pub fn with_deps_and_cache(
        config: Config,
        storage: Arc<crate::storage::SqliteStorage>,
        event_bus: Option<Arc<EventBus>>,
        skill_loader: Option<Arc<SkillLoader>>,
        subagent_manager: Option<Arc<SubagentManager>>,
        file_cache: Arc<FileReadCache>,
    ) -> Result<Self> {
        Self::with_indexer(
            config,
            storage,
            event_bus,
            skill_loader,
            subagent_manager,
            None,
            None,
            None,
            file_cache,
        )
    }

    pub fn with_indexer(
        config: Config,
        storage: Arc<crate::storage::SqliteStorage>,
        event_bus: Option<Arc<EventBus>>,
        skill_loader: Option<Arc<SkillLoader>>,
        subagent_manager: Option<Arc<SubagentManager>>,
        indexer: Option<Arc<CodeIndexer>>,
        memory_store: Option<Arc<MemoryStore>>,
        decision_memory: Option<Arc<DecisionMemory>>,
        file_cache: Arc<FileReadCache>,
    ) -> Result<Self> {
        let mut tools: HashMap<String, Arc<dyn Tool>> = HashMap::new();
        let mcp = McpHandle::new();
        let native_catalog = Arc::new(NativeToolCatalog::new());

        tools.insert("batch".to_string(), Arc::new(batch::BatchTool::new()));

        // Registered unconditionally: MCP servers connect after the
        // registry is built, and both tools read the catalog through
        // `mcp` at call time. They self-describe as unavailable when no
        // server is connected.
        tools.insert(
            "tool_search".to_string(),
            Arc::new(tool_search::ToolSearchTool::new(
                mcp.clone(),
                native_catalog.clone(),
                config.mcp.search_result_limit,
            )),
        );
        tools.insert(
            "tool_script".to_string(),
            Arc::new(tool_script::ToolScriptTool::new()),
        );

        tools.insert(
            "bash".to_string(),
            Arc::new(bash::BashTool::new(config.clone())),
        );

        tools.insert(
            "read_file".to_string(),
            Arc::new(files::ReadFileTool::new(config.clone(), file_cache.clone())),
        );
        tools.insert(
            "write_file".to_string(),
            Arc::new(files::WriteFileTool::new(
                config.clone(),
                file_cache.clone(),
            )),
        );
        tools.insert(
            "edit_file".to_string(),
            Arc::new(files::EditFileTool::new(config.clone(), file_cache.clone())),
        );
        tools.insert(
            "apply_patch".to_string(),
            Arc::new(patch::ApplyPatchTool::new(
                config.clone(),
                file_cache.clone(),
            )),
        );
        tools.insert(
            "list_files".to_string(),
            Arc::new(files::ListFilesTool::new(config.clone())),
        );
        tools.insert(
            "delete_file".to_string(),
            Arc::new(files::DeleteFileTool::new(
                config.clone(),
                file_cache.clone(),
            )),
        );

        let code_python_tool: Arc<dyn Tool> =
            Arc::new(code::CodeInterpreterTool::python(config.clone()));
        tools.insert("code_python".to_string(), code_python_tool.clone());
        native_catalog.register(code_python_tool);
        let code_node_tool: Arc<dyn Tool> =
            Arc::new(code::CodeInterpreterTool::node(config.clone()));
        tools.insert("code_node".to_string(), code_node_tool.clone());
        native_catalog.register(code_node_tool);
        let code_bash_tool: Arc<dyn Tool> =
            Arc::new(code::CodeInterpreterTool::bash(config.clone()));
        tools.insert("code_bash".to_string(), code_bash_tool.clone());
        native_catalog.register(code_bash_tool);

        let task_tool: Arc<dyn Tool> = Arc::new(task::TaskTool::new(storage.clone()));
        tools.insert("task".to_string(), task_tool.clone());
        native_catalog.register(task_tool);
        let persona_tool: Arc<dyn Tool> = Arc::new(persona::PersonaTool::new());
        tools.insert("persona".to_string(), persona_tool.clone());
        native_catalog.register(persona_tool);

        tools.insert(
            "todowrite".to_string(),
            Arc::new(todo::TodoWriteTool::new(storage.clone())),
        );
        tools.insert(
            "todoread".to_string(),
            Arc::new(todo::TodoReadTool::new(storage.clone())),
        );

        tools.insert(
            "grep".to_string(),
            Arc::new(search::GrepTool::new(config.clone())),
        );
        tools.insert(
            "glob".to_string(),
            Arc::new(search::GlobTool::new(config.clone())),
        );

        tools.insert(
            "web_fetch".to_string(),
            Arc::new(web::WebFetchTool::new(config.clone())),
        );
        tools.insert(
            "web_search".to_string(),
            Arc::new(web::WebSearchTool::new(config.clone())),
        );
        tools.insert(
            "public_web_fetch".to_string(),
            Arc::new(web::PublicWebFetchTool::new()),
        );

        if let Some(ref eb) = event_bus {
            tools.insert(
                "question".to_string(),
                Arc::new(question::QuestionTool::new(eb.clone())),
            );
        }

        if let Some(ref sl) = skill_loader {
            tools.insert(
                "skill".to_string(),
                Arc::new(skill::SkillTool::new(sl.clone())),
            );
            tools.insert(
                "skill_list".to_string(),
                Arc::new(skill::SkillListTool::new(sl.clone())),
            );
            let skill_action_tool: Arc<dyn Tool> =
                Arc::new(skill::SkillActionTool::new(sl.clone()));
            tools.insert("skill_action".to_string(), skill_action_tool.clone());
            native_catalog.register(skill_action_tool);
        }

        let lsp_tool: Arc<dyn Tool> = Arc::new(lsp::LspTool::new(config.clone()));
        tools.insert("lsp".to_string(), lsp_tool.clone());
        native_catalog.register(lsp_tool);

        if let Some(ref sm) = subagent_manager {
            tools.insert(
                "subagent".to_string(),
                Arc::new(subagent::SubagentTool::with_manager(sm.clone())),
            );
        } else {
            tools.insert(
                "subagent".to_string(),
                Arc::new(subagent::SubagentTool::new()),
            );
        }

        tools.insert("plan_exit".to_string(), Arc::new(plan::PlanExitTool::new()));

        let process_tool: Arc<dyn Tool> = Arc::new(process::ProcessTool::new(config.clone()));
        tools.insert("process".to_string(), process_tool.clone());
        native_catalog.register(process_tool);
        let calendar_tool: Arc<dyn Tool> = Arc::new(calendar::CalendarTool::new(config.clone()));
        tools.insert("calendar".to_string(), calendar_tool.clone());
        native_catalog.register(calendar_tool);
        let weather_tool: Arc<dyn Tool> = Arc::new(weather::WeatherTool::new(config.clone()));
        tools.insert("weather".to_string(), weather_tool.clone());
        native_catalog.register(weather_tool);
        let news_tool: Arc<dyn Tool> = Arc::new(news::NewsTool::new(config.clone()));
        tools.insert("news".to_string(), news_tool.clone());
        native_catalog.register(news_tool);
        let system_status_tool: Arc<dyn Tool> =
            Arc::new(system_status::SystemStatusTool::new(config.clone()));
        tools.insert("system_status".to_string(), system_status_tool.clone());
        native_catalog.register(system_status_tool);
        let sessions_tool: Arc<dyn Tool> = Arc::new(sessions::SessionsTool::new(storage.clone()));
        tools.insert("sessions".to_string(), sessions_tool.clone());
        native_catalog.register(sessions_tool);

        if let Some(ref idx) = indexer {
            let codesearch_tool: Arc<dyn Tool> =
                Arc::new(codesearch::CodeSearchTool::new(idx.clone()));
            tools.insert("codesearch".to_string(), codesearch_tool.clone());
            native_catalog.register(codesearch_tool);
        }

        if let Some(ref ms) = memory_store {
            let memory_tools: Vec<Arc<dyn Tool>> = vec![
                Arc::new(memory::RecordMemoryTool::new(ms.clone())),
                Arc::new(memory::ListMemorySuggestionsTool::new(ms.clone())),
                Arc::new(memory::ApproveMemorySuggestionTool::new(ms.clone())),
                Arc::new(memory::RejectMemorySuggestionTool::new(ms.clone())),
            ];
            for tool in memory_tools {
                let name = tool.name().to_string();
                tools.insert(name, tool.clone());
                native_catalog.register(tool);
            }
        }

        if let Some(ref dm) = decision_memory {
            let decision_tools: Vec<Arc<dyn Tool>> = vec![
                Arc::new(decision_memory::RecordDecisionTool::new(dm.clone())),
                Arc::new(decision_memory::ListDecisionSuggestionsTool::new(
                    dm.clone(),
                )),
                Arc::new(decision_memory::ApproveDecisionSuggestionTool::new(
                    dm.clone(),
                )),
                Arc::new(decision_memory::RejectDecisionSuggestionTool::new(
                    dm.clone(),
                )),
            ];
            for tool in decision_tools {
                let name = tool.name().to_string();
                tools.insert(name, tool.clone());
                native_catalog.register(tool);
            }
        }

        Ok(Self {
            tools,
            allowed: config.tools.denied.iter().cloned().collect(),
            base_config: config,
            storage,
            skill_loader,
            file_cache,
            coordinator: None,
            scheduler: None,
            mcp,
            native_catalog,
            cached_tool_definitions: std::sync::RwLock::new(None),
        })
    }

    fn build_tool(
        tool_name: &str,
        config: Config,
        storage: Arc<crate::storage::SqliteStorage>,
        file_cache: &Arc<FileReadCache>,
    ) -> Option<Arc<dyn Tool>> {
        match tool_name {
            "bash" => Some(Arc::new(bash::BashTool::new(config))),
            "batch" => Some(Arc::new(batch::BatchTool::new())),
            "read_file" => Some(Arc::new(files::ReadFileTool::new(
                config,
                file_cache.clone(),
            ))),
            "write_file" => Some(Arc::new(files::WriteFileTool::new(
                config,
                file_cache.clone(),
            ))),
            "edit_file" => Some(Arc::new(files::EditFileTool::new(
                config,
                file_cache.clone(),
            ))),
            "apply_patch" => Some(Arc::new(patch::ApplyPatchTool::new(
                config,
                file_cache.clone(),
            ))),
            "list_files" => Some(Arc::new(files::ListFilesTool::new(config))),
            "delete_file" => Some(Arc::new(files::DeleteFileTool::new(
                config,
                file_cache.clone(),
            ))),
            "code_python" => Some(Arc::new(code::CodeInterpreterTool::python(config))),
            "code_node" => Some(Arc::new(code::CodeInterpreterTool::node(config))),
            "code_bash" => Some(Arc::new(code::CodeInterpreterTool::bash(config))),
            "task" => Some(Arc::new(task::TaskTool::new(storage.clone()))),
            "persona" => Some(Arc::new(persona::PersonaTool::new())),
            "todowrite" => Some(Arc::new(todo::TodoWriteTool::new(storage.clone()))),
            "todoread" => Some(Arc::new(todo::TodoReadTool::new(storage.clone()))),
            "grep" => Some(Arc::new(search::GrepTool::new(config))),
            "glob" => Some(Arc::new(search::GlobTool::new(config))),
            "web_fetch" => Some(Arc::new(web::WebFetchTool::new(config))),
            "web_search" => Some(Arc::new(web::WebSearchTool::new(config))),
            "public_web_fetch" => Some(Arc::new(web::PublicWebFetchTool::new())),
            "process" => Some(Arc::new(process::ProcessTool::new(config))),
            "calendar" => Some(Arc::new(calendar::CalendarTool::new(config.clone()))),
            "weather" => Some(Arc::new(weather::WeatherTool::new(config.clone()))),
            "news" => Some(Arc::new(news::NewsTool::new(config.clone()))),
            "system_status" => Some(Arc::new(system_status::SystemStatusTool::new(config))),
            "sessions" => Some(Arc::new(sessions::SessionsTool::new(storage.clone()))),
            "lsp" => Some(Arc::new(lsp::LspTool::new(config))),
            "plan_exit" => Some(Arc::new(plan::PlanExitTool::new())),
            _ => None,
        }
    }

    pub fn has_tool(&self, tool_name: &str) -> bool {
        self.tools.contains_key(tool_name)
    }

    pub fn is_allowed(&self, tool_name: &str) -> bool {
        self.allowed.is_empty() || !self.allowed.contains(tool_name)
    }

    pub fn is_parallel_safe(&self, tool_name: &str) -> bool {
        // An MCP tool is only safe to fan out when the server itself
        // claims it is read-only; absent the hint, assume it mutates.
        if Self::is_mcp_tool(tool_name) {
            return self
                .mcp_manager()
                .and_then(|manager| manager.entry(tool_name))
                .map(|entry| entry.read_only)
                .unwrap_or(false);
        }

        matches!(
            tool_name,
            "read_file"
                | "list_files"
                | "grep"
                | "glob"
                | "web_fetch"
                | "web_search"
                | "reflect"
                | "codesearch"
                | "todoread"
                | "process"
                | "weather"
                | "news"
                | "system_status"
                | "sessions"
                | "lsp"
        )
    }

    /// Always-loaded native tools, then `tool_search`, then every
    /// activated deferred built-in and MCP tool.
    ///
    /// Order is load-bearing. The native block is sorted and byte-stable
    /// for the life of the process; everything appended after it comes
    /// from activation, so discovering a tool invalidates only the tail
    /// of the provider's cached prompt prefix instead of the whole block.
    ///
    /// `tool_search` rides in that tail rather than in the sorted native
    /// block because it is useless without a catalog: with nothing
    /// deferred and no MCP server connected it would otherwise cost
    /// tokens in every request just to tell the model there is nothing
    /// to search.
    pub fn get_tool_definitions(&self, session: &str) -> Vec<ToolDefinition> {
        let mut definitions = self.native_tool_definitions();

        let has_catalog = !self.native_catalog.is_empty()
            || self
                .mcp_manager()
                .map(|manager| !manager.is_empty())
                .unwrap_or(false);
        if !has_catalog {
            return definitions;
        }

        if let Some(search) = self.tools.get(TOOL_SEARCH_NAME) {
            if self.is_allowed(TOOL_SEARCH_NAME) {
                definitions.push(ToolDefinition {
                    tool_type: "function".to_string(),
                    function: crate::agent::provider::ToolFunction {
                        name: search.name().to_string(),
                        description: tool_prompt_description(search),
                        parameters: search.parameters(),
                    },
                });
            }
        }

        definitions.extend(self.native_catalog.activated_definitions(session));
        if let Some(manager) = self.mcp_manager() {
            definitions.extend(manager.activated_definitions(session));
        }
        definitions
    }

    fn native_tool_definitions(&self) -> Vec<ToolDefinition> {
        {
            let cache = self.cached_tool_definitions.read().unwrap();
            if let Some(ref defs) = *cache {
                return defs.clone();
            }
        }

        let mut definitions: Vec<ToolDefinition> = self
            .tools
            .values()
            .filter(|tool| tool.name() != TOOL_SEARCH_NAME)
            .filter(|tool| !self.native_catalog.contains(tool.name()))
            .filter(|tool| self.allowed.is_empty() || !self.allowed.contains(tool.name()))
            .map(|tool| ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::agent::provider::ToolFunction {
                    name: tool.name().to_string(),
                    description: tool_prompt_description(tool),
                    parameters: tool.parameters(),
                },
            })
            .collect();
        definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));

        let mut cache = self.cached_tool_definitions.write().unwrap();
        *cache = Some(definitions.clone());
        definitions
    }

    /// Compact manifest for installed skills. The full list — names,
    /// descriptions, runtime actions — lives behind `skill_list`; this is
    /// just the table of contents so the model knows the capability
    /// exists, in the same spirit as the MCP and deferred built-in
    /// manifests. It keeps prompt cost flat no matter how many skills are
    /// installed.
    pub fn skill_summary_prompt(&self) -> Option<String> {
        let loader = self.skill_loader.as_ref()?;
        let skills = loader.list();
        if skills.is_empty() {
            return None;
        }
        Some(format!(
            "# Available Skills\n{} skill(s) installed. Call `skill_list` to browse them by name and description, then `skill` to run one.\n",
            skills.len()
        ))
    }

    pub fn get_tool_definitions_for_profile(
        &self,
        profile: ToolProfile,
        session: &str,
    ) -> Vec<ToolDefinition> {
        let manager = self.mcp_manager();
        self.get_tool_definitions(session)
            .into_iter()
            .filter(|tool| profile.allows(&tool.function.name))
            .filter(|tool| {
                // Plan mode is read-only by contract; an MCP tool that
                // does not claim `readOnlyHint` has to be assumed to
                // mutate something.
                if profile != ToolProfile::Plan || !tool.function.name.starts_with(MCP_TOOL_PREFIX)
                {
                    return true;
                }
                manager
                    .as_ref()
                    .and_then(|manager| manager.entry(&tool.function.name))
                    .map(|entry| entry.read_only)
                    .unwrap_or(false)
            })
            .collect()
    }

    pub fn mcp_manager(&self) -> Option<Arc<McpManager>> {
        self.mcp.get()
    }

    pub fn mcp_handle(&self) -> McpHandle {
        self.mcp.clone()
    }

    /// Attach a connected MCP catalog. Takes `&self` because servers
    /// connect after the registry is already shared behind an `Arc`, and
    /// because the UI can reconnect them mid-session. Returns the
    /// displaced manager so the caller can shut it down.
    pub fn register_mcp(&self, manager: Arc<McpManager>) -> Option<Arc<McpManager>> {
        self.mcp.set(Some(manager))
    }

    /// The always-in-context server manifest, if any servers connected.
    pub fn mcp_manifest_prompt(&self) -> Option<String> {
        let manager = self.mcp_manager()?;
        if manager.is_empty() {
            return None;
        }
        manager.manifest_prompt()
    }

    /// The always-in-context manifest of deferred built-in tools. Without
    /// it the model cannot know a specialty tool exists and never thinks
    /// to search for it.
    pub fn native_manifest_prompt(&self) -> Option<String> {
        self.native_catalog.manifest()
    }

    /// The deferred built-in catalog, for callers that need activation
    /// state or the catalog itself (e.g. tool_search).
    pub fn native_catalog(&self) -> Arc<NativeToolCatalog> {
        self.native_catalog.clone()
    }

    /// Drop per-session tool activation (native and MCP) when a session
    /// is deleted, so its loaded tools do not linger in memory.
    pub fn prune_session_tools(&self, session: &str) {
        self.native_catalog.prune_session(session);
        if let Some(manager) = self.mcp_manager() {
            manager.prune_session(session);
        }
    }

    pub fn is_mcp_tool(tool_name: &str) -> bool {
        tool_name.starts_with(MCP_TOOL_PREFIX)
    }

    async fn execute_mcp(&self, tool_name: &str, args: Value) -> Result<ToolResult> {
        if !self.is_allowed(tool_name) {
            return Err(OSAgentError::ToolNotAllowed(tool_name.to_string()));
        }

        let manager = self.mcp_manager().ok_or_else(|| {
            OSAgentError::ToolExecution("No MCP servers are connected".to_string())
        })?;

        // The runtime injects session_id into every tool call; activation
        // is session-scoped so a direct call auto-activates only for the
        // session that made it.
        let session = args
            .get("session_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();
        let (output, is_error) = manager.call(&session, tool_name, args).await?;
        if is_error {
            // The server reported a tool-level failure. Surface it as an
            // error so the runtime's retry and loop-detection logic sees
            // it, rather than as a successful result containing prose.
            return Err(OSAgentError::ToolExecution(output));
        }

        let entry = manager.entry(tool_name);
        Ok(ToolResult {
            output,
            outcome: ToolOutcome::Success,
            title: entry
                .as_ref()
                .map(|entry| format!("{} · {}", entry.server, entry.tool)),
            metadata: json!({
                "mcp_server": entry.as_ref().map(|entry| entry.server.clone()),
                "mcp_tool": entry.as_ref().map(|entry| entry.tool.clone()),
            }),
            attachments: Vec::new(),
        })
    }

    pub async fn execute(&self, tool_name: &str, args: Value) -> Result<String> {
        let result = self.execute_result(tool_name, args).await?;
        Ok(result.output)
    }

    /// Execute a native tool under its declared `timeout_ms` budget, if
    /// any. The deadline is fused with the tool's own timing: whichever
    /// fires first wins, and a registry timeout replaces the result with
    /// a structured `ToolTimeout` error so the runtime's retry and
    /// loop-detection logic treats it as a failure.
    async fn run_tool_with_timeout(tool: &Arc<dyn Tool>, args: Value) -> Result<ToolResult> {
        match tool.timeout_ms() {
            Some(ms) if ms > 0 => {
                match tokio::time::timeout(std::time::Duration::from_millis(ms), async {
                    tool.execute_result(args).await
                })
                .await
                {
                    Ok(result) => result,
                    Err(_) => Err(OSAgentError::ToolTimeout {
                        seconds: ms.div_ceil(1_000).max(1),
                    }),
                }
            }
            _ => tool.execute_result(args).await,
        }
    }

    pub async fn execute_result(&self, tool_name: &str, args: Value) -> Result<ToolResult> {
        if Self::is_mcp_tool(tool_name) {
            return self.execute_mcp(tool_name, args).await;
        }

        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| OSAgentError::ToolExecution(format!("Tool not found: {}", tool_name)))?;

        if !self.is_allowed(tool_name) {
            return Err(OSAgentError::ToolNotAllowed(tool_name.to_string()));
        }

        Self::run_tool_with_timeout(tool, args)
            .await
            .map_err(Self::with_rewrite_guidance)
    }

    /// Append explicit "rewrite the input" guidance to argument/schema
    /// validation errors so the model knows how to recover, instead of
    /// receiving a bare error string it may not understand.
    fn with_rewrite_guidance(error: OSAgentError) -> OSAgentError {
        const GUIDANCE: &str = " Please rewrite the input so it satisfies the expected schema.";
        if let OSAgentError::ToolExecution(message) = &error {
            if !message.contains(GUIDANCE) && Self::is_argument_error(message) {
                return OSAgentError::ToolExecution(format!("{}{}", message, GUIDANCE));
            }
        }
        error
    }

    fn is_argument_error(message: &str) -> bool {
        let lower = message.to_lowercase();
        lower.contains("missing")
            || lower.contains("required")
            || lower.contains("expected")
            || lower.contains("must be provided")
            || lower.contains("parameter")
            || lower.contains("argument")
            || lower.contains("schema")
    }

    pub async fn execute_in_workspace(
        &self,
        tool_name: &str,
        args: Value,
        workspace_path: Option<String>,
    ) -> Result<String> {
        let result = self
            .execute_in_workspace_result(tool_name, args, workspace_path)
            .await?;
        Ok(result.output)
    }

    pub async fn execute_in_workspace_result(
        &self,
        tool_name: &str,
        args: Value,
        workspace_path: Option<String>,
    ) -> Result<ToolResult> {
        self.execute_in_workspace_with_external_result(tool_name, args, workspace_path, &[])
            .await
    }

    pub async fn execute_in_workspace_with_external_result(
        &self,
        tool_name: &str,
        args: Value,
        workspace_path: Option<String>,
        external_paths: &[String],
    ) -> Result<ToolResult> {
        if !self.is_allowed(tool_name) {
            return Err(OSAgentError::ToolNotAllowed(tool_name.to_string()));
        }

        // MCP tools have no workspace binding — the server owns its own
        // scope — so they skip the per-workspace rebuild entirely.
        if Self::is_mcp_tool(tool_name) {
            return self.execute_mcp(tool_name, args).await;
        }

        if let Some(path) = workspace_path {
            let mut config = self.base_config.clone();
            if let Some(workspace) = config.get_workspace_by_path(&path) {
                config.agent.active_workspace = Some(workspace.id.clone());
                config.agent.workspace = workspace.resolved_path();
            } else {
                let active_id = config
                    .agent
                    .active_workspace
                    .clone()
                    .unwrap_or_else(|| "default".to_string());
                if let Some(workspace) = config
                    .agent
                    .workspaces
                    .iter_mut()
                    .find(|workspace| workspace.id == active_id)
                {
                    workspace.paths = vec![WorkspacePath {
                        path: path.clone(),
                        permission: WorkspacePermission::ReadWrite,
                        description: Some("Session workspace".to_string()),
                    }];
                    workspace.path = path.clone();
                }
                config.agent.active_workspace = Some(active_id);
                config.agent.workspace = path;
            }
            config.ensure_workspace_defaults();
            if !external_paths.is_empty() {
                let active_id = config.agent.active_workspace.clone();
                if let Some(workspace) = config
                    .agent
                    .workspaces
                    .iter_mut()
                    .find(|workspace| Some(&workspace.id) == active_id.as_ref())
                {
                    for path in external_paths {
                        if !workspace.paths.iter().any(|entry| entry.path == *path) {
                            workspace.paths.push(WorkspacePath {
                                path: path.clone(),
                                permission: WorkspacePermission::ReadWrite,
                                description: Some("User-approved external path".to_string()),
                            });
                        }
                    }
                }
            }
            if let Some(tool) =
                Self::build_tool(tool_name, config, self.storage.clone(), &self.file_cache)
            {
                return Self::run_tool_with_timeout(&tool, args)
                    .await
                    .map_err(Self::with_rewrite_guidance);
            }
            if let Some(tool) = self.tools.get(tool_name) {
                return Self::run_tool_with_timeout(tool, args)
                    .await
                    .map_err(Self::with_rewrite_guidance);
            }
            return Err(OSAgentError::ToolExecution(format!(
                "Tool not found: {}",
                tool_name
            )));
        }

        self.execute_result(tool_name, args).await
    }

    pub fn file_cache(&self) -> &Arc<FileReadCache> {
        &self.file_cache
    }

    pub fn invalidate_file_cache_all(&self) {
        self.file_cache.invalidate_all();
    }

    pub fn register_coordinator(&mut self, coordinator: Arc<Coordinator>) {
        if self.allowed.is_empty() || !self.allowed.contains("coordinator") {
            let tool: Arc<dyn Tool> =
                Arc::new(coordinator::CoordinatorTool::new(coordinator.clone()));
            self.tools.insert("coordinator".to_string(), tool.clone());
            self.native_catalog.register(tool);
            self.coordinator = Some(coordinator);
            *self.cached_tool_definitions.write().unwrap() = None;
        }
    }

    pub fn register_scheduler(&mut self, scheduler: Arc<crate::scheduler::Scheduler>) {
        if self.allowed.is_empty() || !self.allowed.contains("schedule") {
            let tool: Arc<dyn Tool> = Arc::new(scheduler::ScheduleTool::new(scheduler.clone()));
            self.tools.insert("schedule".to_string(), tool.clone());
            self.native_catalog.register(tool);
            self.scheduler = Some(scheduler);
            *self.cached_tool_definitions.write().unwrap() = None;
        }
    }

    pub fn register_goals(&mut self, goals: Arc<crate::agent::goal::GoalStore>) {
        let goal_tools: Vec<Arc<dyn Tool>> = vec![
            Arc::new(crate::tools::goal::GetGoalTool::new(goals.clone())),
            Arc::new(crate::tools::goal::CreateGoalTool::new(goals.clone())),
            Arc::new(crate::tools::goal::UpdateGoalTool::new(goals)),
        ];
        for tool in goal_tools {
            let name = tool.name().to_string();
            self.tools.insert(name, tool.clone());
            self.native_catalog.register(tool);
        }
        *self.cached_tool_definitions.write().unwrap() = None;
    }
}

#[cfg(test)]
mod tests {
    use super::{ToolOutcome, ToolProfile, ToolResult};
    use crate::agent::provider::{ToolDefinition, ToolFunction};
    use serde_json::json;

    fn definitions() -> Vec<ToolDefinition> {
        [
            "read_file",
            "list_files",
            "grep",
            "glob",
            "bash",
            "write_file",
            "edit_file",
            "apply_patch",
            "question",
            "todowrite",
            "subagent",
            "weather",
            "calendar",
            "news",
            "web_search",
            "web_fetch",
            "public_web_fetch",
            "process",
            "system_status",
            "persona",
            "coordinator",
        ]
        .into_iter()
        .map(|name| ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: name.to_string(),
                description: format!("{name} tool"),
                parameters: json!({ "type": "object" }),
            },
        })
        .collect()
    }

    fn selected_names(profile: ToolProfile) -> Vec<String> {
        definitions()
            .into_iter()
            .filter(|tool| profile.allows(&tool.function.name))
            .map(|tool| tool.function.name)
            .collect()
    }

    #[test]
    fn tool_results_default_to_success_for_compatibility() {
        let result = ToolResult::new("ok");
        assert_eq!(result.outcome, ToolOutcome::Success);

        let legacy: ToolResult = serde_json::from_value(json!({
            "output": "legacy",
            "metadata": {},
            "attachments": []
        }))
        .expect("legacy result should deserialize");
        assert_eq!(legacy.outcome, ToolOutcome::Success);
    }

    #[test]
    fn tool_result_failure_and_retryable_are_not_success() {
        assert!(!ToolResult::failure("failed").outcome.is_success());
        assert!(!ToolResult::retryable("later").outcome.is_success());
    }

    #[test]
    fn default_profile_keeps_every_configured_tool_except_community_web_fetch() {
        let names = selected_names(ToolProfile::Default);
        assert_eq!(names.len(), definitions().len() - 1);
        assert!(!names.contains(&"public_web_fetch".to_string()));
    }

    #[test]
    fn code_profile_keeps_all_coding_tools() {
        let selected = selected_names(ToolProfile::Code);
        let names = selected.iter().map(String::as_str).collect::<Vec<_>>();

        for core in [
            "read_file",
            "list_files",
            "grep",
            "glob",
            "bash",
            "write_file",
            "edit_file",
            "apply_patch",
            "question",
            "todowrite",
            "subagent",
        ] {
            assert!(names.contains(&core), "missing coding tool: {core}");
        }
        assert!(!names.contains(&"weather"));
        assert!(!names.contains(&"calendar"));
        assert!(!names.contains(&"news"));
        assert!(!names.contains(&"public_web_fetch"));
    }

    #[test]
    fn plan_profile_excludes_mutating_tools() {
        let names = selected_names(ToolProfile::Plan);
        assert!(names.iter().any(|name| name == "read_file"));
        assert!(names.iter().any(|name| name == "subagent"));
        assert!(!names.iter().any(|name| name == "write_file"));
        assert!(!names.iter().any(|name| name == "bash"));
    }

    #[test]
    fn custom_profile_is_minimal() {
        let names = selected_names(ToolProfile::Custom);
        assert!(names.iter().any(|name| name == "question"));
        assert!(names.iter().any(|name| name == "web_search"));
        assert!(!names.iter().any(|name| name == "bash"));
        assert!(!names.iter().any(|name| name == "write_file"));
    }

    #[test]
    fn community_profile_cannot_access_the_host() {
        let names = selected_names(ToolProfile::Community);
        assert_eq!(names, vec!["web_search", "public_web_fetch"]);
    }
}
