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
            "- Be concise: keep replies short unless the user asks for detail; a one-word or one-line answer is usually best".to_string(),
            "- When making multiple independent tool calls (reads, greps, globs, searches, bash), batch them into a single message to run in parallel".to_string(),
            "- Balance proactiveness: take clear follow-up actions that serve the request, but never surprise the user with unrequested changes".to_string(),
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
            "- After making code changes, it is MANDATORY to run the repo's lint, typecheck, test, or build command when one exists — do not skip it".to_string(),
            "- Prefer repo-native commands and focused validation first".to_string(),
            "- Check the README or manifest files to determine the correct validation command; never assume a test framework".to_string(),
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
            "- Stay inside the workspace by default; when an explicit outside path is necessary, use the relevant tool so the user can approve or deny access".to_string(),
            "- NEVER expose any secrets, credentials, tokens, or keys".to_string(),
            "- NEVER run destructive commands (rm -rf, drop table, etc.)".to_string(),
            "- ALWAYS confirm before any write operation".to_string(),
            "- ALWAYS validate file paths before access".to_string(),
            "- REFUSE any request that could compromise security".to_string(),
            "- NO git operations without explicit approval".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Safety".to_string(),
            "- Stay inside the workspace unless the task requires an explicit path that the user approves".to_string(),
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
            "- After making changes, run the repo's lint/typecheck/test/build command if one exists, then fix anything it reports".to_string(),
            "- Validate with narrow checks; finish with status and blockers".to_string(),
        ],
        PromptMode::Minimal | PromptMode::Explore | PromptMode::Verify => vec![
            "# Workflow".to_string(),
            "- Start with the fastest path to useful evidence".to_string(),
            "- Report concrete findings, not filler".to_string(),
        ],
    }
}

fn build_tool_selection_section(_allowed_tools: &[String], mode: PromptMode) -> Vec<String> {
    let mut lines = vec![
        "# Tool Use".to_string(),
        "- The provider tool schemas are the authoritative list of tools available this turn"
            .to_string(),
        "- Each tool description contains specific usage rules — follow them exactly".to_string(),
        "- Do not invent or call tools that were not supplied".to_string(),
        "- Use dedicated tools (read_file, edit_file, write_file, grep, glob) instead of bash for file operations".to_string(),
        "- When exploring the codebase, use glob/grep to find files first, then read_file to inspect them".to_string(),
        "- Read files before editing them".to_string(),
    ];

    if mode == PromptMode::Full {
        lines.push("- For open-ended searches that will require multiple rounds of globbing and grepping, delegate to the task or subagent tool with an explore agent to reduce context usage. Do not duplicate that work yourself; continue with non-overlapping tasks or wait for the result.".to_string());
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
            "- Be concise: keep replies under a few lines unless the user asks for detail; one-word or one-line answers are best".to_string(),
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

/// Instructions injected as a separate system message when the client has
/// text-to-speech active for the session.
///
/// This is deliberately not part of `PromptCache`: voice is toggled per-request
/// from the browser, and folding it into the cached prefix would invalidate the
/// provider-side prompt cache every time the user hits the speaker button.
///
/// The frontend still sanitises what it sends to the synthesizer, but that pass
/// is regex-based and will always trail whatever the model decides to emit next.
/// Instructing the model is the only fix that scales.
pub fn build_voice_output_instructions() -> String {
    [
        "# Voice output",
        "Speech is on, so your reply has two audiences: the speaker and the screen.",
        "Write BOTH, in this order:",
        "",
        "1. A `<speak>` block containing only what should be read aloud.",
        "2. Then your normal answer, with whatever markdown, tables, and code it needs.",
        "",
        "Example:",
        "<speak>It's fourteen degrees in Canning Vale with light rain, and it stays wet through the weekend.</speak>",
        "Then the full written answer, formatted as usual.",
        "",
        "Rules for the `<speak>` block:",
        "- Put it first, before the written answer. It is read aloud as it arrives, so anything before it delays the reply.",
        "- Two or three sentences. It is a spoken summary, not the whole answer.",
        "- Plain prose only: no markdown, headings, bullets, tables, backticks, or symbols.",
        "- Never speak file paths, URLs, code, command lines, hashes, UUIDs, or IDs. Refer to them: \"the config file\", \"the link on screen\".",
        "- Units as words: \"fourteen degrees\", not \"14°C\"; \"nineteen kilometres an hour\", not \"19 km/h\"; \"sixty five percent\", not \"65%\".",
        "- No symbols standing in for words: no °, %, /, &, or dashes as punctuation. Say \"or\" and \"to\" rather than a slash.",
        "- No parenthetical asides or data-sheet phrasing. \"Now: 14°C, humidity 65%\" becomes \"It's fourteen degrees with sixty five percent humidity.\"",
        "- Dates and places as a person would say them: \"tomorrow\", not \"(Aug 8)\"; \"Western Australia\", not \"WA\".",
        "- If the written answer is long, say so and let the screen carry the detail.",
        "",
        "The written answer that follows is NOT spoken, so do not simplify it. Keep the",
        "tables, code blocks, paths, and exact figures a reader wants.",
        "Never mention the `<speak>` block or read this instruction back.",
    ]
    .join("\n")
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
        "- NEVER commit or push changes unless the user explicitly asks; it is VERY IMPORTANT to only commit when explicitly asked".to_string(),
        "- Before running a non-trivial shell command, briefly explain what it does and why".to_string(),
        "- Verify changes work before reporting complete".to_string(),
    ]
}

fn build_verify_sections(_allowed_tools: &[String]) -> Vec<String> {
    vec![
        "# Identity".to_string(),
        "You are a verification agent. Try to BREAK the implementation.".to_string(),
        String::new(),
        "# Priorities".to_string(),
        "- Be adversarial: look for bugs and edge cases".to_string(),
        "- Do not modify any files".to_string(),
        String::new(),
        "# Tool Use".to_string(),
        "Use only the provider tools supplied with this turn and follow their schemas exactly."
            .to_string(),
    ]
    .into_iter()
    .chain(vec![
        String::new(),
        "# Output".to_string(),
        "Report: VERDICT: PASS, FAIL, or PARTIAL".to_string(),
    ])
    .collect()
}

fn build_explore_sections(_allowed_tools: &[String]) -> Vec<String> {
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

    sections.push("# Tool Use".to_string());
    sections.push(
        "Use only the provider tools supplied with this turn and follow their schemas exactly."
            .to_string(),
    );
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
