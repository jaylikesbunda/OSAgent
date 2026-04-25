use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PromptMode {
    Full,
    Minimal,
    Explore,
    Verify,
}

/// A versioned system prompt cache that separates static sections (reusable
/// across turns when the tool set or config hasn't changed) from dynamic
/// sections (date/time, which change daily or per-request).
///
/// The static prefix is suitable for LLM prompt caching (e.g. Anthropic's
/// cache_control or OpenAI's automatic caching) because it remains identical
/// across many API calls within a session.
#[derive(Debug, Clone)]
pub struct PromptCache {
    /// The full assembled system prompt text
    pub prompt: String,
    /// Byte offset of the dynamic boundary — everything before this offset
    /// is the static prefix that never changes per session
    pub dynamic_offset: usize,
    /// Mode this cache was built for
    pub mode: PromptMode,
    /// Hash of inputs that determine cache validity
    pub cache_version: u64,
}

impl PromptCache {
    /// Build a fresh prompt cache for the given parameters.
    /// The static prefix goes up to (and including) the Tools section.
    /// The dynamic suffix starts after the Tools section.
    pub fn build(
        allowed_tools: &[String],
        mode: PromptMode,
        custom_identity: Option<&str>,
        custom_priorities: Option<&[String]>,
    ) -> Self {
        let mut sorted_tools = allowed_tools.to_vec();
        sorted_tools.sort();

        let mut hasher = DefaultHasher::new();
        mode.hash(&mut hasher);
        for tool in &sorted_tools {
            tool.hash(&mut hasher);
        }
        if let Some(id) = custom_identity {
            id.hash(&mut hasher);
        }
        if let Some(prios) = custom_priorities {
            for p in prios {
                p.hash(&mut hasher);
            }
        }
        let cache_version = hasher.finish();

        // Build full prompt from sections, tracking the static prefix
        let mut sections = Vec::new();

        if mode == PromptMode::Verify {
            let verify_sections = build_verify_sections(allowed_tools);
            sections = verify_sections;
            let prompt = sections.join("\n");
            return Self {
                prompt: prompt.clone(),
                dynamic_offset: prompt.len(),
                mode,
                cache_version,
            };
        }

        if mode == PromptMode::Explore {
            let explore_sections = build_explore_sections(allowed_tools);
            sections = explore_sections;
            let prompt = sections.join("\n");
            return Self {
                prompt: prompt.clone(),
                dynamic_offset: prompt.len(),
                mode,
                cache_version,
            };
        }

        // Static prefix accumulates here
        sections.extend(build_priorities_section(mode, custom_priorities));
        sections.push(String::new());
        sections.extend(build_validation_section(mode));
        sections.push(String::new());
        sections.extend(build_tool_selection_section(allowed_tools, mode));
        sections.push(String::new());

        if mode == PromptMode::Full {
            sections.extend(build_editing_rules_section());
            sections.push(String::new());
            sections.extend(build_constraints_section());
            sections.push(String::new());
        }

        sections.extend(build_workflow_section(mode));
        sections.push(String::new());
        sections.extend(build_safety_section(mode));
        sections.push(String::new());

        // Mark dynamic boundary here — everything after is dynamic
        let static_prefix = sections.join("\n");
        let dynamic_offset = static_prefix.len();

        // Dynamic suffix
        sections.extend(build_datetime_section());
        sections.push(String::new());
        sections.extend(build_identity_section(mode, custom_identity));
        sections.push(String::new());
        sections.extend(build_communication_section(mode));

        let prompt = sections.join("\n");

        Self {
            prompt,
            dynamic_offset,
            mode,
            cache_version,
        }
    }

    /// Returns the static prefix (cacheable portion) of the system prompt.
    /// This is safe to use with Anthropic cache_control breakpoints.
    pub fn static_prefix(&self) -> &str {
        &self.prompt[..self.dynamic_offset.min(self.prompt.len())]
    }

    /// Returns the dynamic suffix (non-cacheable portion) of the system prompt.
    pub fn dynamic_suffix(&self) -> &str {
        &self.prompt[self.dynamic_offset.min(self.prompt.len())..]
    }

    /// Check whether the cache is still valid given current parameters.
    pub fn is_valid(
        &self,
        allowed_tools: &[String],
        mode: PromptMode,
        custom_identity: Option<&str>,
        custom_priorities: Option<&[String]>,
    ) -> bool {
        if mode != self.mode {
            return false;
        }

        let mut sorted_tools = allowed_tools.to_vec();
        sorted_tools.sort();

        let mut hasher = DefaultHasher::new();
        mode.hash(&mut hasher);
        for tool in &sorted_tools {
            tool.hash(&mut hasher);
        }
        if let Some(id) = custom_identity {
            id.hash(&mut hasher);
        }
        if let Some(prios) = custom_priorities {
            for p in prios {
                p.hash(&mut hasher);
            }
        }

        hasher.finish() == self.cache_version
    }

    /// Rebuild only the dynamic portion (date/time, etc.) on top of the
    /// cached static prefix. Returns the updated full prompt.
    pub fn refresh_dynamic(&mut self, custom_identity: Option<&str>) {
        let mut dynamic = Vec::new();
        dynamic.extend(build_datetime_section());
        dynamic.push(String::new());
        dynamic.extend(build_identity_section(self.mode, custom_identity));
        dynamic.push(String::new());
        dynamic.extend(build_communication_section(self.mode));

        let new_prefix = self.static_prefix().to_string();
        let new_suffix = dynamic.join("\n");
        self.prompt = format!("{}\n{}", new_prefix, new_suffix);
        self.dynamic_offset = new_prefix.len();
    }
}

pub fn build_system_prompt(
    allowed_tools: &[String],
    mode: PromptMode,
    custom_identity: Option<&str>,
    custom_priorities: Option<&[String]>,
) -> String {
    let mut sections = Vec::new();

    if mode == PromptMode::Verify {
        sections.extend(build_verify_sections(allowed_tools));
        return sections.join("\n");
    }

    if mode == PromptMode::Explore {
        sections.extend(build_explore_sections(allowed_tools));
        return sections.join("\n");
    }

    sections.extend(build_priorities_section(mode, custom_priorities));
    sections.push(String::new());
    sections.extend(build_datetime_section());
    sections.push(String::new());
    sections.extend(build_validation_section(mode));
    sections.push(String::new());
    sections.extend(build_tool_selection_section(allowed_tools, mode));
    sections.push(String::new());

    if mode == PromptMode::Full {
        sections.extend(build_editing_rules_section());
        sections.push(String::new());
        sections.extend(build_constraints_section());
        sections.push(String::new());
    }

    sections.extend(build_workflow_section(mode));
    sections.push(String::new());
    sections.extend(build_safety_section(mode));
    sections.push(String::new());
    sections.extend(build_identity_section(mode, custom_identity));
    sections.push(String::new());
    sections.extend(build_communication_section(mode));

    sections.join("\n")
}

fn build_datetime_section() -> Vec<String> {
    use std::time::{SystemTime, UNIX_EPOCH};

    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();

    let datetime = chrono::DateTime::from_timestamp(now as i64, 0).unwrap_or_else(chrono::Utc::now);

    let local: chrono::DateTime<chrono::Local> = chrono::DateTime::from(datetime);
    let date_str = local.format("%Y-%m-%d").to_string();
    let time_str = local.format("%H:%M:%S").to_string();
    let tz_str = local.format("%z").to_string();
    let weekday = local.format("%A").to_string();

    let tz_display = if tz_str.starts_with('+') || tz_str.starts_with('-') {
        let sign = &tz_str[..1];
        let rest = &tz_str[1..];
        if rest.len() >= 4 {
            format!("{}{}:{}", sign, &rest[..2], &rest[2..4])
        } else {
            tz_str.clone()
        }
    } else {
        tz_str.clone()
    };

    vec![
        "# Current Time".to_string(),
        format!("- Date: {} ({})", date_str, weekday),
        format!("- Time: {}", time_str),
        format!("- Timezone: {}", tz_display),
    ]
}

fn build_priorities_section(mode: PromptMode, custom_priorities: Option<&[String]>) -> Vec<String> {
    // Use custom priorities if provided
    if let Some(priorities) = custom_priorities {
        if !priorities.is_empty() {
            let mut lines = vec!["# Priorities".to_string()];
            for priority in priorities {
                lines.push(priority.clone());
            }
            return lines;
        }
    }

    // Fall back to default priorities
    match mode {
        PromptMode::Full => vec![
            "# Priorities".to_string(),
            "- Answer directly from knowledge when confident".to_string(),
            "- For repo-specific work, inspect local context and use tools proactively".to_string(),
            "- Arithmetic: work step by step, don't rely on memory".to_string(),
            "- Prefer the most specific tool; parallelize independent search/read steps"
                .to_string(),
            "- Use todowrite for multi-step work that is easy to lose track of".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Priorities".to_string(),
            "- Complete the assigned task efficiently".to_string(),
            "- Stay tightly scoped to the request".to_string(),
            "- Use only the tools available to you".to_string(),
            "- Stop once you have the answer or result".to_string(),
        ],
    }
}

fn build_validation_section(mode: PromptMode) -> Vec<String> {
    match mode {
        PromptMode::Full => vec![
            "# Validation".to_string(),
            "- Run lint, typecheck, tests, or build steps when they exist and are relevant"
                .to_string(),
            "- Prefer repo-native commands and focused validation first".to_string(),
            "- Report whether validation passed, failed, or was unavailable".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Validation".to_string(),
            "- Validate results when feasible".to_string(),
            "- Report findings directly".to_string(),
        ],
    }
}

fn build_safety_section(mode: PromptMode) -> Vec<String> {
    match mode {
        PromptMode::Full => vec![
            "# Safety".to_string(),
            "- NEVER access anything outside the workspace".to_string(),
            "- NEVER expose any secrets, credentials, tokens, or keys".to_string(),
            "- NEVER run destructive commands (rm -rf, drop table, etc.)".to_string(),
            "- ALWAYS confirm before any write operation".to_string(),
            "- ALWAYS validate file paths before access".to_string(),
            "- REFUSE any request that could compromise security".to_string(),
            "- NO git operations without explicit approval".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Safety".to_string(),
            "- Stay inside the workspace".to_string(),
            "- Never expose secrets or credentials".to_string(),
            "- Refuse destructive or policy-violating requests".to_string(),
        ],
    }
}

fn build_workflow_section(mode: PromptMode) -> Vec<String> {
    match mode {
        PromptMode::Full => vec![
            "# Workflow".to_string(),
            "- Understand the request and inspect relevant context first".to_string(),
            "- Use the most specific tool that fits the job".to_string(),
            "- Make the smallest correct change that solves the problem".to_string(),
            "- Delegate focused research or complex multi-file work with subagent or coordinator when it reduces context load or risk".to_string(),
            "- Validate with narrow checks; finish with status and blockers".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Workflow".to_string(),
            "- Start with the fastest path to useful evidence".to_string(),
            "- Report concrete findings, not filler".to_string(),
        ],
    }
}

fn build_tool_selection_section(allowed_tools: &[String], mode: PromptMode) -> Vec<String> {
    let mut lines = vec!["# Tools".to_string()];

    for tool in allowed_tools {
        if let Some(desc) = tool_line(tool) {
            lines.push(desc.to_string());
        }
    }

    if mode == PromptMode::Minimal || mode == PromptMode::Explore {
        lines.push("- Do not spawn additional subagents".to_string());
    }

    lines
}

fn build_communication_section(mode: PromptMode) -> Vec<String> {
    match mode {
        PromptMode::Full => vec![
            "# Communication".to_string(),
            "- Be precise and technical".to_string(),
            "- Include relevant code snippets and line numbers".to_string(),
            "- Explain the why, not just the what".to_string(),
            "- Use standard technical terminology".to_string(),
            "- Reference: filepath:line_number format".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Communication".to_string(),
            "- Report results concisely".to_string(),
            "- Use filepath:line_number for code references".to_string(),
        ],
    }
}

fn build_identity_section(mode: PromptMode, custom_identity: Option<&str>) -> Vec<String> {
    // Use custom identity if provided
    if let Some(identity) = custom_identity {
        if !identity.trim().is_empty() {
            return vec!["# Identity".to_string(), identity.trim().to_string()];
        }
    }

    // Fall back to default identity
    match mode {
        PromptMode::Full => vec![
            "# Identity".to_string(),
            "You are OSA, a workspace-aware general assistant with a calm, capable voice and a touch of dry wit. Help with software work, research, organization, system tasks, and practical day-to-day requests with precise, actionable assistance.".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Identity".to_string(),
            "You are OSA, a specialized worker agent.".to_string(),
        ],
    }
}

fn build_constraints_section() -> Vec<String> {
    vec![
        "# Constraints".to_string(),
        "- Do not add features or refactor beyond what was asked".to_string(),
        "- Do not add comments/TODOs unless explicitly asked".to_string(),
        "- Verify changes work before reporting complete".to_string(),
    ]
}

fn build_editing_rules_section() -> Vec<String> {
    vec![
        "# Editing Rules".to_string(),
        "- Read before edit".to_string(),
        "- Prefer apply_patch for precise multi-hunk changes".to_string(),
        "- Preserve formatting and surrounding conventions".to_string(),
        "- Do not overwrite unrelated user changes".to_string(),
        "- If no file change is needed, say so clearly".to_string(),
    ]
}

fn tool_line(name: &str) -> Option<&'static str> {
    // Rich tool descriptions with usage guidance, following the pattern:
    // - tool_name: brief summary. Use for X. Prefer over Y when Z. Avoid for A.
    match name {
        // ── File discovery & search ──
        "glob" => Some(
            "- glob: find files by name patterns (e.g. `**/*.rs`, `src/**/*.ts`). Use to locate files by path; prefer over grep when you know the filename pattern. Results sorted by modification time.",
        ),
        "grep" => Some(
            "- grep: search file contents with regex patterns. Use to find symbols, function definitions, or error messages. Supports include filters (e.g. `*.rs`). Performs exact regex matching.",
        ),
        "codesearch" => Some(
            "- codesearch: semantic code search via MeiliSearch index. Use when grep is too literal and you need meaning-based results. Requires the codebase to be indexed.",
        ),
        "list_files" => Some(
            "- list_files: list directory contents. Use to inspect project structure quickly. Skips common noise directories like node_modules by default.",
        ),

        // ── Reading & inspection ──
        "read_file" => Some(
            "- read_file: read a file or directory with offset/limit paging. Use to inspect file contents line-by-line. Supports reading images, PDFs, and binary files. Paths are cached for fast subsequent access. Always read before editing.",
        ),

        // ── File modification ──
        "edit_file" => Some(
            "- edit_file: exact string replacement in files. Use for targeted inline changes. Requires the old string to match exactly in the file — copy from read_file output to ensure precision. For multi-hunk changes across a file, prefer apply_patch.",
        ),
        "write_file" => Some(
            "- write_file: create or completely overwrite a file. Use for new files or full rewrites. Will overwrite existing content without warning — prefer edit_file or apply_patch for partial changes.",
        ),
        "delete_file" => Some(
            "- delete_file: remove files from the workspace. Irreversible — confirm before using.",
        ),
        "apply_patch" => Some(
            "- apply_patch: apply unified diff patches for precise multi-hunk edits. Use when you need to change multiple locations in a file atomically. Preferred over edit_file for complex changes spanning several sections. Produces minimal diff output.",
        ),

        // ── Execution ──
        "bash" => Some(
            "- bash: execute shell commands with optional timeout. Use for builds, tests, linting, git operations (staging, diff, log — never push), package management, file operations (mkdir, cp, mv), and system queries. Commands are workspace-scoped. Avoid for simple file reads (use read_file) or content searches (use grep/glob).",
        ),
        "batch" => Some(
            "- batch: run multiple independent tool calls in a single step. Use to parallelize independent reads, searches, or status checks. Do NOT use for operations that depend on each other's results.",
        ),
        "code_python" => Some(
            "- code_python: execute Python scripts in an isolated runtime. Use for data processing, file transformations, or quick calculations. Has a configurable timeout.",
        ),
        "code_node" => Some(
            "- code_node: execute JavaScript/TypeScript scripts. Use for Node.js ecosystem tasks, JSON processing, or quick scripting in workspace context.",
        ),
        "code_bash" => Some(
            "- code_bash: execute shell scripts in the workspace. Use for multi-step shell transformations that are cleaner as a script than as individual bash calls.",
        ),

        // ── Web ──
        "web_fetch" => Some(
            "- web_fetch: fetch and parse a URL into text, markdown, or HTML. Use for reading documentation, API references, or issue trackers. Not for arbitrary browsing — fetch specific URLs with clear intent.",
        ),
        "web_search" => Some(
            "- web_search: search the web for current information. Use for finding documentation, examples, or answers beyond your training data. Prefer when grep/glob/codesearch don't have the answer.",
        ),

        // ── Task planning & tracking ──
        "task" => Some(
            "- task: launch a sub-agent for complex multi-step work. Use to delegate focused research, large-scale refactoring, or exploration that would bloat the main context. Sub-agents return a final summary. Prefer over coordinator for single-focus delegation.",
        ),
        "todowrite" => Some(
            "- todowrite: create or update a structured task list for the current session. Use for multi-step work that is easy to lose track of. Mark items in_progress before working on them, completed when done. Helps maintain focus across iterations.",
        ),
        "todoread" => Some(
            "- todoread: read the current todo list state. Use to review progress before continuing work.",
        ),
        "plan_exit" => Some(
            "- plan_exit: signal that planning is complete and execution should begin. Use at the end of a planning phase to transition to implementation.",
        ),

        // ── Delegation & coordination ──
        "subagent" => Some(
            "- subagent: delegate a self-contained task to a specialized worker sub-agent. Use for parallel exploration, independent research threads, or tasks that need a fresh context window. Returns a concise result summary.",
        ),
        "coordinator" => Some(
            "- coordinator: delegate complex multi-file work with planning and verification. Use when a task requires changes across many files with interdependent steps. The coordinator plans, executes, and verifies each step.",
        ),

        // ── System & OS control ──
        "process" => Some(
            "- process: list or manage running OS processes. Use to find PIDs, check resource usage, or terminate hung processes. Read-only listing is safe; killing requires confirmation.",
        ),
        "system_status" => Some(
            "- system_status: query machine OS, CPU usage, memory consumption, disk space, and uptime. Use for diagnostics, capacity checks, or environment verification.",
        ),
        "calendar" => Some(
            "- calendar: manage calendar events. Use to create, list, or modify scheduled events. Integrates with system calendar where configured.",
        ),
        "schedule" => Some(
            "- schedule: set reminders or recurring tasks. Use to schedule background jobs or future notifications.",
        ),
        "weather" => Some(
            "- weather: fetch current weather or forecast for a location. Use for environment-aware planning or informational queries.",
        ),

        // ── Interaction & state ──
        "question" => Some(
            "- question: ask the user clarifying questions with structured multiple-choice or free-text options. Use when requirements are ambiguous, when you need to choose between approaches, or when a decision has security/safety implications.",
        ),
        "persona" => Some(
            "- persona: change the assistant's behavior style. Use to switch between coding, teaching, or roleplay modes.",
        ),
        "record_memory" => Some(
            "- record_memory: save persistent facts, preferences, or project context for future sessions. Use to remember user preferences, project conventions, or important discoveries. Stored across sessions.",
        ),
        "record_decision" => Some(
            "- record_decision: store durable approved decisions that must be applied consistently. Use when the user explicitly approves a specific approach or constraint. Enforced across future turns and sessions.",
        ),

        // ── Skills & LSP ──
        "skill" => Some(
            "- skill: inspect metadata for a loaded skill. Use to understand what a skill provides before invoking it.",
        ),
        "skill_list" => Some(
            "- skill_list: list all available skills with their descriptions. Use to discover what capabilities are available through the skills system.",
        ),
        "skill_action" => Some(
            "- skill_action: execute a named action within a loaded skill. Use to invoke specific skill capabilities like API calls or script executions.",
        ),
        "lsp" => Some(
            "- lsp: query a Language Server Protocol server for code intelligence. Use for go-to-definition, find-references, hover info, or document symbols. Requires an LSP server to be running for the file's language.",
        ),

        // ── Memory management tools (internal, for review workflows) ──
        "list_memory_suggestions" => Some(
            "- list_memory_suggestions: review pending memory suggestions from the learning system. Use during review workflows.",
        ),
        "approve_memory_suggestion" => Some(
            "- approve_memory_suggestion: approve a pending memory suggestion. Use to confirm learned facts or preferences.",
        ),
        "reject_memory_suggestion" => Some(
            "- reject_memory_suggestion: reject a pending memory suggestion that is incorrect or unwanted.",
        ),
        "list_decision_suggestions" => Some(
            "- list_decision_suggestions: review pending decision suggestions.",
        ),
        "approve_decision_suggestion" => Some(
            "- approve_decision_suggestion: approve a pending decision suggestion for consistent enforcement.",
        ),
        "reject_decision_suggestion" => Some(
            "- reject_decision_suggestion: reject a pending decision suggestion.",
        ),

        _ => None,
    }
}

fn build_verify_sections(allowed_tools: &[String]) -> Vec<String> {
    vec![
        "# Identity".to_string(),
        "You are a verification agent. Try to BREAK the implementation.".to_string(),
        String::new(),
        "# Priorities".to_string(),
        "- Be adversarial: look for bugs and edge cases".to_string(),
        "- Do not modify any files".to_string(),
        String::new(),
        "# Tools".to_string(),
    ]
    .into_iter()
    .chain(
        allowed_tools
            .iter()
            .filter_map(|tool| tool_line(tool).map(String::from)),
    )
    .chain(vec![
        String::new(),
        "# Output".to_string(),
        "Report: VERDICT: PASS, FAIL, or PARTIAL".to_string(),
    ])
    .collect()
}

fn build_explore_sections(allowed_tools: &[String]) -> Vec<String> {
    let mut sections = vec![
        "# Identity".to_string(),
        "You are a codebase exploration specialist. You excel at rapidly navigating codebases, finding relevant files, understanding architecture, and synthesizing findings into clear reports.".to_string(),
        String::new(),
        "# Priorities".to_string(),
        "- Read files thoroughly to understand the full picture".to_string(),
        "- Stay tightly scoped to the request".to_string(),
        "- Use only the tools available to you".to_string(),
        String::new(),
    ];

    sections.extend(build_datetime_section());
    sections.push(String::new());

    sections.extend(build_validation_section(PromptMode::Minimal));
    sections.push(String::new());

    sections.push("# Tools".to_string());
    for tool in allowed_tools {
        if let Some(desc) = tool_line(tool) {
            sections.push(desc.to_string());
        }
    }
    sections.push("- Do not spawn additional subagents".to_string());
    sections.push(String::new());

    sections.extend(vec![
        "# Workflow".to_string(),
        "- Start with the fastest path to useful evidence: use glob/grep to find relevant files, then read them".to_string(),
        "- Adapt your search approach based on the thoroughness level specified by the caller".to_string(),
        "- Return file paths as absolute paths".to_string(),
        "- Do not create any files, or run commands that modify the system".to_string(),
        String::new(),
        "# Output".to_string(),
        "When you have gathered enough information, you MUST produce a comprehensive summary of your findings as your final response.".to_string(),
        "- Structure your findings clearly with headers and file references".to_string(),
        "- Include specific file paths and line numbers for all references".to_string(),
        "- If the task is too large to complete fully, summarize what you found and note what remains unexplored".to_string(),
        "- NEVER end with only tool outputs — always provide a synthesized written summary".to_string(),
        String::new(),
        "# Safety".to_string(),
        "- Stay inside the workspace".to_string(),
        "- Never expose secrets or credentials".to_string(),
        "- Refuse destructive or policy-violating requests".to_string(),
    ]);

    sections
}
