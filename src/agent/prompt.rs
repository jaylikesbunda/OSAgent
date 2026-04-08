#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PromptMode {
    Full,
    Minimal,
    Explore,
    Verify,
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
    match name {
        "glob" => Some("- glob: find files by path or name patterns"),
        "grep" => Some("- grep: search file contents"),
        "codesearch" => Some("- codesearch: semantic code search"),
        "list_files" => Some("- list_files: inspect directories"),
        "read_file" => Some("- read_file: read a file"),
        "edit_file" => Some("- edit_file: smart text replacement"),
        "write_file" => Some("- write_file: create or rewrite a file"),
        "delete_file" => Some("- delete_file: remove files"),
        "apply_patch" => Some("- apply_patch: precise multi-hunk edits"),
        "batch" => Some("- batch: run multiple calls in parallel"),
        "bash" => Some("- bash: run commands (build, test, etc)"),
        "process" => Some("- process: inspect or kill processes"),
        "calendar" => Some("- calendar: manage calendar events"),
        "schedule" => Some("- schedule: set reminders or recurring tasks"),
        "weather" => Some("- weather: fetch weather forecast"),
        "system_status" => Some("- system_status: machine OS, CPU, memory, disk"),
        "code_python" => Some("- code_python: run Python code"),
        "code_node" => Some("- code_node: run JavaScript/TypeScript"),
        "code_bash" => Some("- code_bash: shell transformations"),
        "web_fetch" => Some("- web_fetch: fetch a URL"),
        "web_search" => Some("- web_search: search the web"),
        "task" => Some("- task: track multi-step work"),
        "todowrite" => Some("- todowrite: manage todo list"),
        "todoread" => Some("- todoread: read todo list"),
        "persona" => Some("- persona: change assistant style"),
        "record_memory" => Some("- record_memory: save persistent facts"),
        "question" => Some("- question: ask user for clarification"),
        "skill" => Some("- skill: inspect loaded skill metadata"),
        "skill_list" => Some("- skill_list: list available skills"),
        "skill_action" => Some("- skill_action: execute skill action"),
        "lsp" => Some("- lsp: query language server"),
        "subagent" => Some("- subagent: delegate to worker"),
        "coordinator" => Some("- coordinator: complex task delegation"),
        "plan_exit" => Some("- plan_exit: signal planning complete"),
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
