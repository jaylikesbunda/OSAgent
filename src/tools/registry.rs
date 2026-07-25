use crate::agent::coordinator::Coordinator;
use crate::agent::decision_memory::DecisionMemory;
use crate::agent::events::EventBus;
use crate::agent::memory::MemoryStore;
use crate::agent::provider::ToolDefinition;
use crate::agent::subagent_manager::SubagentManager;
use crate::config::{Config, WorkspacePath, WorkspacePermission};
use crate::error::{OSAgentError, Result};
use crate::indexer::CodeIndexer;
use crate::skills::SkillLoader;
use crate::tools::file_cache::FileReadCache;
use crate::tools::{
    bash, batch, calendar, code, codesearch, coordinator, decision_memory, files, lsp, memory,
    news, patch, persona, plan, process, question, scheduler, search, skill, subagent,
    system_status, task, todo, weather, web,
};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;

#[allow(dead_code)]
#[derive(Debug, Clone)]
pub struct ToolExample {
    pub description: String,
    pub input: Value,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ToolResult {
    pub output: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(default = "default_tool_result_metadata")]
    pub metadata: Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ToolProfile {
    Default,
    Code,
    Plan,
    Creative,
    Custom,
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

    fn allows(self, tool_name: &str) -> bool {
        match self {
            Self::Default => true,
            Self::Code => !matches!(tool_name, "calendar" | "weather" | "news"),
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
            ),
            Self::Custom => matches!(
                tool_name,
                "web_fetch" | "web_search" | "question" | "skill" | "skill_list" | "persona"
            ),
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
            title: None,
            metadata: default_tool_result_metadata(),
        }
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

    #[allow(dead_code)]
    fn when_to_use(&self) -> &str {
        "See tool description"
    }

    #[allow(dead_code)]
    fn when_not_to_use(&self) -> &str {
        "See tool description"
    }

    #[allow(dead_code)]
    fn examples(&self) -> Vec<ToolExample> {
        vec![]
    }
}

pub struct ToolRegistry {
    tools: HashMap<String, Arc<dyn Tool>>,
    allowed: HashSet<String>,
    base_config: Config,
    storage: Arc<crate::storage::SqliteStorage>,
    event_bus: Option<Arc<EventBus>>,
    skill_loader: Option<Arc<SkillLoader>>,
    subagent_manager: Option<Arc<SubagentManager>>,
    indexer: Option<Arc<CodeIndexer>>,
    memory_store: Option<Arc<MemoryStore>>,
    decision_memory: Option<Arc<DecisionMemory>>,
    file_cache: Arc<FileReadCache>,
    coordinator: Option<Arc<Coordinator>>,
    scheduler: Option<Arc<crate::scheduler::Scheduler>>,
    cached_tool_definitions: std::sync::RwLock<Option<Vec<ToolDefinition>>>,
}

impl ToolRegistry {
    fn tool_prompt_description(tool: &Arc<dyn Tool>) -> String {
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

        tools.insert("batch".to_string(), Arc::new(batch::BatchTool::new()));

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

        tools.insert(
            "code_python".to_string(),
            Arc::new(code::CodeInterpreterTool::python(config.clone())),
        );
        tools.insert(
            "code_node".to_string(),
            Arc::new(code::CodeInterpreterTool::node(config.clone())),
        );
        tools.insert(
            "code_bash".to_string(),
            Arc::new(code::CodeInterpreterTool::bash(config.clone())),
        );

        tools.insert(
            "task".to_string(),
            Arc::new(task::TaskTool::new(storage.clone())),
        );
        tools.insert("persona".to_string(), Arc::new(persona::PersonaTool::new()));

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
            tools.insert(
                "skill_action".to_string(),
                Arc::new(skill::SkillActionTool::new(sl.clone())),
            );
        }

        tools.insert(
            "lsp".to_string(),
            Arc::new(lsp::LspTool::new(config.clone())),
        );

        if let Some(ref sm) = subagent_manager {
            tools.insert(
                "subagent".to_string(),
                Arc::new(subagent::SubagentTool::with_manager(
                    storage.clone(),
                    sm.clone(),
                )),
            );
        } else {
            tools.insert(
                "subagent".to_string(),
                Arc::new(subagent::SubagentTool::new(storage.clone())),
            );
        }

        tools.insert("plan_exit".to_string(), Arc::new(plan::PlanExitTool::new()));

        tools.insert(
            "process".to_string(),
            Arc::new(process::ProcessTool::new(config.clone())),
        );
        tools.insert(
            "calendar".to_string(),
            Arc::new(calendar::CalendarTool::new(config.clone())),
        );
        tools.insert(
            "weather".to_string(),
            Arc::new(weather::WeatherTool::new(config.clone())),
        );
        tools.insert(
            "news".to_string(),
            Arc::new(news::NewsTool::new(config.clone())),
        );
        tools.insert(
            "system_status".to_string(),
            Arc::new(system_status::SystemStatusTool::new(config.clone())),
        );

        if let Some(ref idx) = indexer {
            tools.insert(
                "codesearch".to_string(),
                Arc::new(codesearch::CodeSearchTool::new(idx.clone())),
            );
        }

        if let Some(ref ms) = memory_store {
            tools.insert(
                "record_memory".to_string(),
                Arc::new(memory::RecordMemoryTool::new(ms.clone())),
            );
            tools.insert(
                "list_memory_suggestions".to_string(),
                Arc::new(memory::ListMemorySuggestionsTool::new(ms.clone())),
            );
            tools.insert(
                "approve_memory_suggestion".to_string(),
                Arc::new(memory::ApproveMemorySuggestionTool::new(ms.clone())),
            );
            tools.insert(
                "reject_memory_suggestion".to_string(),
                Arc::new(memory::RejectMemorySuggestionTool::new(ms.clone())),
            );
        }

        if let Some(ref dm) = decision_memory {
            tools.insert(
                "record_decision".to_string(),
                Arc::new(decision_memory::RecordDecisionTool::new(dm.clone())),
            );
            tools.insert(
                "list_decision_suggestions".to_string(),
                Arc::new(decision_memory::ListDecisionSuggestionsTool::new(
                    dm.clone(),
                )),
            );
            tools.insert(
                "approve_decision_suggestion".to_string(),
                Arc::new(decision_memory::ApproveDecisionSuggestionTool::new(
                    dm.clone(),
                )),
            );
            tools.insert(
                "reject_decision_suggestion".to_string(),
                Arc::new(decision_memory::RejectDecisionSuggestionTool::new(
                    dm.clone(),
                )),
            );
        }

        Ok(Self {
            tools,
            allowed: config.tools.denied.iter().cloned().collect(),
            base_config: config,
            storage,
            event_bus,
            skill_loader,
            subagent_manager,
            indexer,
            memory_store,
            decision_memory,
            file_cache,
            coordinator: None,
            scheduler: None,
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
            "process" => Some(Arc::new(process::ProcessTool::new(config))),
            "calendar" => Some(Arc::new(calendar::CalendarTool::new(config.clone()))),
            "weather" => Some(Arc::new(weather::WeatherTool::new(config.clone()))),
            "news" => Some(Arc::new(news::NewsTool::new(config.clone()))),
            "system_status" => Some(Arc::new(system_status::SystemStatusTool::new(config))),
            "lsp" => Some(Arc::new(lsp::LspTool::new(config))),
            "plan_exit" => Some(Arc::new(plan::PlanExitTool::new())),
            _ => None,
        }
    }

    pub fn is_allowed(&self, tool_name: &str) -> bool {
        self.allowed.is_empty() || !self.allowed.contains(tool_name)
    }

    pub fn is_parallel_safe(&self, tool_name: &str) -> bool {
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
                | "lsp"
        )
    }

    pub fn get_tool_definitions(&self) -> Vec<ToolDefinition> {
        {
            let cache = self.cached_tool_definitions.read().unwrap();
            if let Some(ref defs) = *cache {
                return defs.clone();
            }
        }

        let mut definitions: Vec<ToolDefinition> = self
            .tools
            .values()
            .filter(|tool| self.allowed.is_empty() || !self.allowed.contains(tool.name()))
            .map(|tool| ToolDefinition {
                tool_type: "function".to_string(),
                function: crate::agent::provider::ToolFunction {
                    name: tool.name().to_string(),
                    description: Self::tool_prompt_description(tool),
                    parameters: tool.parameters(),
                },
            })
            .collect();
        definitions.sort_by(|left, right| left.function.name.cmp(&right.function.name));

        let mut cache = self.cached_tool_definitions.write().unwrap();
        *cache = Some(definitions.clone());
        definitions
    }

    pub fn skill_summary_prompt(&self) -> Option<String> {
        let loader = self.skill_loader.as_ref()?;
        let mut skills = loader.list();
        skills.sort_by(|left, right| left.name.cmp(&right.name));
        if skills.is_empty() {
            return None;
        }
        let lines = skills
            .into_iter()
            .take(30)
            .map(|skill| {
                let normalized = skill
                    .description
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ");
                let description = normalized.chars().take(240).collect::<String>();
                format!("- {}: {}", skill.name, description)
            })
            .collect::<Vec<_>>()
            .join("\n");
        Some(format!(
            "# Available Skills\nUse the skill tool when one of these specialized workflows clearly matches the task.\n{}",
            lines
        ))
    }

    pub fn get_tool_definitions_for_message(&self, user_message: &str) -> Vec<ToolDefinition> {
        Self::filter_tool_definitions(self.get_tool_definitions(), user_message)
    }

    pub fn get_tool_definitions_for_profile(&self, profile: ToolProfile) -> Vec<ToolDefinition> {
        self.get_tool_definitions()
            .into_iter()
            .filter(|tool| profile.allows(&tool.function.name))
            .collect()
    }

    fn filter_tool_definitions(
        all_tools: Vec<ToolDefinition>,
        user_message: &str,
    ) -> Vec<ToolDefinition> {
        if all_tools.len() <= 15 {
            return all_tools;
        }

        let message_lower = user_message.to_lowercase();
        let coding_request = Self::is_coding_repository_request(&message_lower);
        let personal_request = Self::is_personal_assistant_request(&message_lower);
        let mut scored: Vec<(ToolDefinition, usize)> = all_tools
            .into_iter()
            .map(|tool| {
                let score = Self::score_tool_relevance(
                    &tool.function.name,
                    &tool.function.description,
                    &message_lower,
                );
                (tool, score)
            })
            .collect();

        scored.sort_by(|left, right| {
            right
                .1
                .cmp(&left.1)
                .then_with(|| left.0.function.name.cmp(&right.0.function.name))
        });

        let limit = if coding_request { 18 } else { 12 };
        let mut result: Vec<ToolDefinition> = Vec::with_capacity(limit);
        if coding_request {
            const CODING_CORE: &[&str] = &[
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
            ];
            for name in CODING_CORE {
                if let Some((tool, _)) = scored.iter().find(|(tool, _)| tool.function.name == *name)
                {
                    result.push(tool.clone());
                }
            }
        }

        for (tool, score) in &scored {
            if result.len() >= limit {
                break;
            }
            if (coding_request || personal_request) && *score == 0 {
                continue;
            }
            if !result
                .iter()
                .any(|selected| selected.function.name == tool.function.name)
            {
                result.push(tool.clone());
            }
        }

        if personal_request
            && !result.iter().any(|tool| tool.function.name == "question")
            && result.len() < limit
        {
            if let Some((question, _)) = scored
                .iter()
                .find(|(tool, _)| tool.function.name == "question")
            {
                result.push(question.clone());
            }
        }

        result
    }

    fn is_coding_repository_request(message: &str) -> bool {
        let strong_context = [
            "repository",
            "repo",
            "codebase",
            "code",
            "source code",
            "git diff",
        ];
        let personal_context = [
            "weather",
            "forecast",
            "calendar",
            "appointment",
            "headlines",
            "latest news",
        ];
        if Self::message_has_any(message, &personal_context)
            && !Self::message_has_any(message, &strong_context)
        {
            return false;
        }

        let coding_context = [
            "code",
            "repository",
            "repo",
            "source",
            "file",
            "function",
            "class",
            "symbol",
            "module",
            "crate",
            "package",
            "test",
            "compile",
            "diff",
            "patch",
            "bug",
            "api",
            "frontend",
            "backend",
            "database",
            "cargo",
            "npm",
            "pytest",
        ];
        let coding_action = [
            "inspect",
            "read",
            "find",
            "search",
            "edit",
            "change",
            "implement",
            "fix",
            "add",
            "remove",
            "refactor",
            "test",
            "build",
            "run",
            "debug",
            "review",
            "trace",
            "create",
            "write",
        ];

        Self::message_has_any(message, &coding_context)
            && Self::message_has_any(message, &coding_action)
    }

    fn is_personal_assistant_request(message: &str) -> bool {
        Self::message_has_any(
            message,
            &[
                "weather",
                "forecast",
                "temperature",
                "calendar",
                "appointment",
                "meeting",
                "headlines",
                "latest news",
                "current events",
            ],
        )
    }

    fn message_has_any(message: &str, terms: &[&str]) -> bool {
        let words = message
            .split(|character: char| !character.is_alphanumeric() && character != '_')
            .filter(|word| !word.is_empty())
            .collect::<Vec<_>>();
        terms.iter().any(|term| {
            if term.contains(' ') {
                message.contains(term)
            } else {
                words.iter().any(|word| word == term)
            }
        })
    }

    fn score_tool_relevance(tool_name: &str, tool_description: &str, message: &str) -> usize {
        let mut score = 0usize;

        let keywords: &[(&str, &[&str])] = &[
            (
                "read_file",
                &[
                    "read",
                    "view",
                    "show",
                    "file",
                    "content",
                    "code",
                    "look at",
                    "check file",
                    "see",
                ],
            ),
            (
                "write_file",
                &[
                    "write",
                    "create",
                    "new file",
                    "make file",
                    "save",
                    "generate",
                ],
            ),
            (
                "edit_file",
                &[
                    "edit", "change", "modify", "update", "replace", "fix", "add to",
                ],
            ),
            (
                "bash",
                &[
                    "run", "execute", "command", "shell", "terminal", "build", "test", "compile",
                    "npm", "cargo", "git",
                ],
            ),
            (
                "grep",
                &[
                    "search", "find", "grep", "pattern", "look for", "where is", "contains",
                ],
            ),
            (
                "glob",
                &[
                    "find",
                    "glob",
                    "list",
                    "files",
                    "directory",
                    "path",
                    "pattern",
                ],
            ),
            (
                "web_search",
                &[
                    "search",
                    "web",
                    "internet",
                    "google",
                    "online",
                    "latest",
                    "find online",
                ],
            ),
            (
                "web_fetch",
                &["fetch", "url", "website", "page", "html", "download"],
            ),
            (
                "todowrite",
                &[
                    "todo",
                    "checklist",
                    "task list",
                    "track",
                    "progress",
                    "plan",
                    "steps",
                ],
            ),
            (
                "todoread",
                &[
                    "todo",
                    "checklist",
                    "track",
                    "progress",
                    "continue",
                    "resume",
                ],
            ),
            (
                "task",
                &["subtask", "parent task", "task record", "hierarchy"],
            ),
            (
                "subagent",
                &[
                    "delegate",
                    "subagent",
                    "worker",
                    "parallel",
                    "split",
                    "research",
                    "investigate",
                ],
            ),
            (
                "coordinator",
                &[
                    "coordinate",
                    "orchestrate",
                    "multi-file",
                    "complex change",
                    "implement",
                    "verify",
                ],
            ),
            (
                "code_python",
                &["python", "code", "script", "compute", "calculate"],
            ),
            (
                "code_node",
                &["javascript", "js", "node", "typescript", "ts"],
            ),
            (
                "calendar",
                &["calendar", "event", "schedule", "meeting", "appointment"],
            ),
            (
                "weather",
                &["weather", "temperature", "forecast", "rain", "climate"],
            ),
            (
                "news",
                &[
                    "news",
                    "headlines",
                    "current events",
                    "breaking",
                    "what's happening",
                    "latest news",
                ],
            ),
            (
                "process",
                &["process", "running", "kill", "ps", "memory", "cpu"],
            ),
            (
                "system_status",
                &["system", "status", "os", "disk", "uptime", "machine"],
            ),
            ("skill", &["skill", "plugin", "extension"]),
            (
                "question",
                &["question", "ask", "confirm", "clarify", "approve"],
            ),
            ("persona", &["persona", "style", "tone", "personality"]),
            ("plan_exit", &["plan", "planning", "exit"]),
        ];

        if let Some((_, keyword_list)) = keywords.iter().find(|(name, _)| *name == tool_name) {
            for keyword in *keyword_list {
                if message.contains(keyword) {
                    score += 10;
                }
            }
        }

        if tool_description.to_lowercase().contains("file") && message.contains("file") {
            score += 3;
        }
        if tool_description.to_lowercase().contains("search")
            && (message.contains("search") || message.contains("find"))
        {
            score += 3;
        }
        if tool_description.to_lowercase().contains("web")
            && (message.contains("web") || message.contains("online") || message.contains("search"))
        {
            score += 3;
        }

        score
    }

    pub async fn execute(&self, tool_name: &str, args: Value) -> Result<String> {
        let result = self.execute_result(tool_name, args).await?;
        Ok(result.output)
    }

    pub async fn execute_result(&self, tool_name: &str, args: Value) -> Result<ToolResult> {
        let tool = self
            .tools
            .get(tool_name)
            .ok_or_else(|| OSAgentError::ToolExecution(format!("Tool not found: {}", tool_name)))?;

        if !self.is_allowed(tool_name) {
            return Err(OSAgentError::ToolNotAllowed(tool_name.to_string()));
        }

        tool.execute_result(args).await
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
                return tool.execute_result(args).await;
            }
            if let Some(tool) = self.tools.get(tool_name) {
                return tool.execute_result(args).await;
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
            self.tools.insert(
                "coordinator".to_string(),
                Arc::new(coordinator::CoordinatorTool::new(
                    self.storage.clone(),
                    coordinator.clone(),
                )),
            );
            self.coordinator = Some(coordinator);
            *self.cached_tool_definitions.write().unwrap() = None;
        }
    }

    pub fn register_scheduler(&mut self, scheduler: Arc<crate::scheduler::Scheduler>) {
        if self.allowed.is_empty() || !self.allowed.contains("schedule") {
            self.tools.insert(
                "schedule".to_string(),
                Arc::new(scheduler::ScheduleTool::new(scheduler.clone())),
            );
            self.scheduler = Some(scheduler);
            *self.cached_tool_definitions.write().unwrap() = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::ToolProfile;
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
    fn default_profile_keeps_every_configured_tool() {
        assert_eq!(
            selected_names(ToolProfile::Default).len(),
            definitions().len()
        );
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
}
