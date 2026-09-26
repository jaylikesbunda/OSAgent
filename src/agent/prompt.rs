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
        sections.extend(build_turn_section(mode));
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
    sections.extend(build_turn_section(mode));
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
            "- When making multiple independent tool calls (reads, greps, globs, searches, bash), batch them into a single message to run in parallel".to_string(),
            "- Be proactive about the task you were given: take the requested action plus its clear follow-up actions, but never start unrequested work or surprise the user with changes they did not ask for".to_string(),
            // Split advice from action. Previously this said only "if asked how
            // to approach something, answer first", which contradicted the
            // Communication rule that "can you… / help me…" means do the work.
            // Both behaviours are wanted, so the test is what was asked, not
            // which words were used.
            "- Distinguish asking for advice from asking for the work. \"How should I…\", \"what's the best way to…\", \"would this work…\" want an answer. \"Can you…\", \"fix…\", \"add…\", \"make…\" want the work done — stop answering and do it".to_string(),
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
            "- Validate proportionally to the change. Run the repo's lint, typecheck, test, or build command when one exists, but scope it to what the change can plausibly reach".to_string(),
            "- Start with the narrowest useful check — the test file you touched, the type checker, the linter on changed paths. Broaden to the full suite when the change is wide or shared, or when the narrow run is inconclusive".to_string(),
            "- Once a check passes, move on. Re-running the same suite speculatively is not extra rigor; repeat it only when a new change, a failure, or an unresolved concern justifies it".to_string(),
            "- Do not write tests that just mirror the implementation, or tests for a reversible, low-impact change. Spend them on behaviour that would break silently".to_string(),
            "- Fix any LSP diagnostics reported on files you touched before moving on".to_string(),
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
            "- Stay inside the workspace by default; use the relevant tool when an explicit outside path is genuinely needed so the user can approve it".to_string(),
            "- NEVER expose any secrets, credentials, tokens, or keys".to_string(),
            "- NEVER run destructive commands (rm -rf, drop table, git reset --hard, force push) unless the user has approved that specific action".to_string(),
            "- No git operations that modify state (commit, push, reset, restore, checkout, clean, apply, merge) without explicit approval; read-only git (status, diff, log, show, branch) is fine".to_string(),
            "- Decline genuinely destructive or harmful requests plainly, without a lecture".to_string(),
            // The guardrails above are enforced by the runtime, not just by this
            // prompt: profile denial, workspace resolution, and the read-only
            // bash check all reject the call and hand back a tool error. Without
            // this line the model pre-apologises for permissions it will never be
            // asked about, which reads as ceremony and pads every reply.
            "- These limits are enforced at runtime. A blocked action comes back as a tool error: read it, adjust, carry on. Do not pre-announce risks, ask permission for ordinary in-workspace work, or append a safety summary to your reply".to_string(),
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
            "- Understand the request and inspect relevant context first: use search tools extensively, in parallel and sequentially".to_string(),
            "- Follow existing conventions: mimic code style, reuse existing libraries and patterns, check neighboring files and manifests before introducing anything new".to_string(),
            "- Use the most specific tool that fits the job".to_string(),
            "- Make the smallest correct change that solves the problem".to_string(),
            "- Delegate with the subagent or coordinator tool only when the user or a loaded skill explicitly asks for it, or when the work genuinely cannot fit this context. Unprompted fan-out costs more than it saves — do the work yourself otherwise".to_string(),
            "- Implement with all tools available to you, then verify with tests when possible".to_string(),
            "- Finish with the outcome and any blocker, not a narration of the steps".to_string(),
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
        "- Use dedicated tools (read_file, edit_file, write_file, apply_patch, grep, glob, list_files) instead of bash for file operations".to_string(),
        "- Available core tools: read_file (single path or batch `paths[]` up to 10), write_file, edit_file (single hunk or atomic `edits[]` up to 20), apply_patch (multi-file), bash (sync, max 300s) + process (background start/poll/log/kill), grep/glob (paged offset/limit; grep literal:true for exact text), list_files (with sizes), todowrite/todoread, web_fetch/web_search, skill/skill_list, subagent, question/plan_exit. Specialty tools (memory, calendar, LSP details, skill authoring, public web, draw_diagram) load via tool_search.".to_string(),
        "- Batch independent work in one message: parallel tool calls, read_file `paths[]`, edit_file `edits[]`, or the batch tool (read-only fan-out: reads, grep/glob/codesearch/lsp, web, todoread, skill lookups, read_only bash). One batched message beats N sequential turns.".to_string(),
        "- When a visual explains something better than prose — architecture, flow, state machine, decision tree, layered plan, module map, timeline — use `draw_diagram` (load it with tool_search if needed) instead of ASCII art. Pass the structured `spec`; only use `raw_svg` when the user explicitly asks for hand-written SVG.".to_string(),
        "- When exploring the codebase, use glob/grep to find files first, then read_file to inspect them".to_string(),
        "- Read files before editing them; prefer editing existing files, never create new files unless required".to_string(),
        #[cfg(windows)]
        "- Environment: this is a Windows host. Prefer forward slashes in tool paths; do NOT use `~` in tool paths. File tools accept workspace-relative paths. Text files may use CRLF line endings — edit_file handles LF/CRLF automatically; do not hand-convert line endings."
            .to_string(),
        #[cfg(not(windows))]
        "- Environment: this is a Unix host. Use forward slashes in tool paths; do NOT use `~` in tool paths. File tools accept workspace-relative paths."
            .to_string(),
        "- bash shell: Windows runs `cmd /C`, Unix runs `sh -lc` — write commands for the host shell only (see the bash tool description)".to_string(),
        "- Prefer read_file with a larger window (100-200 lines) over many tiny repeated slices; call independent reads/greps/globs in parallel in a single message".to_string(),
        "- Copy old_text for edits from AFTER the read_file line-number prefix (`<line>: <content>`); never include the line number itself".to_string(),
    ];

    if mode == PromptMode::Full {
        lines.push("- For open-ended searches that will require multiple rounds of globbing and grepping, run the sweep yourself with parallel searches. Reach for a subagent only when the user asked for delegation or the sweep is clearly too large for this context.".to_string());
        lines.push(
            "- When you cannot tell what the user is referring to in the codebase — a vague name, \"that function\", pasted errors, UI text, a concept — do not guess and do not ask yet: fan out several parallel searches that rephrase their words in different ways"
                .to_string(),
        );
        lines.push(
            "- Vary each search across: the exact phrase, likely symbol names (camelCase, snake_case, kebab-case), synonyms, distinctive fragments of any error message, visible UI strings, and related file names"
                .to_string(),
        );
        lines.push(
            "- Use grep for literal fragments and glob for file names; reach for codesearch first on vague or open-ended questions to fan the query into ranked batched greps"
                .to_string(),
        );
        lines.push(
            "- Ask the user to clarify only after several genuinely different searches have all come back empty"
                .to_string(),
        );
    }

    if mode == PromptMode::Minimal || mode == PromptMode::Explore {
        lines.push("- Do not spawn additional subagents".to_string());
    }

    lines
}

fn build_turn_section(mode: PromptMode) -> Vec<String> {
    match mode {
        PromptMode::Full => vec![
            "# Mid-turn Input".to_string(),
            // A message can land while tools are still running. Without this the
            // model treats it as a fresh task, drops the work in flight, and
            // restarts — the exact behaviour that made interrupted sessions look
            // like the agent "just stopped replying".
            "- A message that arrives while you are working is steering the task you are already on, not replacing it. Fold the correction or added constraint into the current work and keep going".to_string(),
            "- Replace the task only when the user clearly cancels it or asks for something incompatible with it".to_string(),
            "- If the message is a quick question, answer it in a sentence or two, then return to the work".to_string(),
            "- An unfamiliar file, or a change you did not make, is most likely the user's work or another agent's. Read it and work with it before touching it; do not revert or overwrite it".to_string(),
        ],
        // Explore/Verify return early with their own section lists, so only
        // Minimal reaches here alongside Full.
        PromptMode::Minimal => vec![
            "# Mid-turn Input".to_string(),
            "- A message arriving mid-task steers the current work; it does not replace it".to_string(),
            "- Do not revert or overwrite changes you did not make".to_string(),
        ],
        PromptMode::Explore | PromptMode::Verify => Vec::new(),
    }
}

fn build_communication_section(mode: PromptMode) -> Vec<String> {
    match mode {
        PromptMode::Full => vec![
            "# Communication".to_string(),
            "- Lead with the answer. Put the main point in the first sentence, then add only the detail that actually helps".to_string(),
            "- Be concise, direct, and to the point. Keep replies to a few lines unless the user asks for detail".to_string(),
            "- Infer what the user actually wants and act on it. \"Can you…\", \"I want…\", \"help me…\" are requests to do the work, not to describe how you would do it — don't stop at acknowledging, offering a plan, or asking whether to continue".to_string(),
            "- Persist to the end: keep going until the query is actually resolved, and don't settle for a partial or \"good enough\" result to save time or tokens".to_string(),
            // These three are the gpt-6-class tells that make replies read as
            // padded: a contrast preamble before the point, a list of what is
            // NOT being done, and a running commentary over routine tool calls.
            "- When you describe what you did, describe what you did. Do not add what you are not doing, what will stay unchanged, or how you split the results up".to_string(),
            "- Skip contrast framing like \"X, not Y\" or \"this isn't about X, it's about Y\" — it is a preamble the user has to read past to reach the point".to_string(),
            "- Do not narrate routine work. Reads, greps, and small edits do not need an update. Send one when you have a finding, a decision, a blocker, or a real tradeoff — then keep going".to_string(),
            "- Your final message must stand on its own. Don't leave a blocking question in an earlier message; if you need an answer, the last message is where it goes".to_string(),
            "- Do not add code explanation summaries unless requested; after working on a file, just stop".to_string(),
            "- Use GitHub-flavored markdown where it helps; output text communicates with the user, never tool calls or code comments as a messaging channel".to_string(),
            "- Only use emojis if the user explicitly requests it".to_string(),
            "- Reference code as filepath:line_number".to_string(),
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
        "- Do not add backward-compatibility shims, new abstractions, or extra config without a concrete need. If it is genuinely unclear, ask one short question instead of guessing".to_string(),
        "- NEVER commit or push changes unless the user explicitly asks; it is VERY IMPORTANT to only commit when explicitly asked".to_string(),
        "- Always follow security best practices: never expose or log secrets, never commit secrets or keys".to_string(),
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
        "- When searches come back empty, rephrase rather than conclude the code does not exist: symbol-case variants (camelCase/snake_case), synonyms, substrings of quoted errors or UI text, broader concept terms; batch independent searches in parallel".to_string(),
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

#[cfg(test)]
mod tests {
    use super::*;

    fn full() -> String {
        build_system_prompt(&["bash".to_string()], PromptMode::Full, None, None)
    }

    /// Mid-turn corrections used to read as a brand new task: the model dropped
    /// the work in flight and restarted, which is what made an interrupted
    /// session look like the agent had stopped responding.
    #[test]
    fn mid_turn_messages_steer_rather_than_replace() {
        let prompt = full();
        assert!(prompt.contains("# Mid-turn Input"));
        assert!(prompt.contains("is steering the task you are already on"));
        assert!(prompt.contains("Replace the task only when the user clearly cancels"));
    }

    /// Subagent fan-out has to be opt-in. The old wording pushed delegation for
    /// any multi-file job, which on a gpt-6-class model means unprompted
    /// context-shredding tool calls.
    #[test]
    fn delegation_is_opt_in_not_encouraged() {
        let prompt = full();
        assert!(prompt.contains("only when the user or a loaded skill explicitly asks"));
        assert!(!prompt.contains("Delegate focused research or complex multi-file work"));
        assert!(!prompt.contains("delegate to the task or subagent tool with an explore agent"));
    }

    /// Safety guidance has to stop generating ceremony. The runtime rejects
    /// blocked calls and returns a tool error, so the prompt must not also make
    /// the model pre-announce risks and ask permission for routine work.
    #[test]
    fn safety_section_does_not_manufacture_approval_flows() {
        let prompt = full();
        assert!(!prompt.contains("Only ask for confirmation before destructive"));
        assert!(!prompt.contains("ALWAYS validate file paths before access"));
        assert!(!prompt.contains("REFUSE any request that could compromise security"));
        // The guardrails themselves stay, plus the note that they are enforced.
        assert!(prompt.contains("NEVER expose any secrets"));
        assert!(prompt.contains("enforced at runtime"));
    }

    /// Full-suite runs after every trivial edit burned whole iterations and
    /// contributed to turns that stalled out.
    #[test]
    fn validation_is_proportional_not_mandatory_everything() {
        let prompt = full();
        assert!(!prompt.contains("it is MANDATORY to run"));
        assert!(prompt.contains("Validate proportionally to the change"));
        assert!(prompt.contains("Do not write tests that just mirror the implementation"));
    }

    #[test]
    fn communication_leads_with_the_answer_and_skips_padding() {
        let prompt = full();
        assert!(prompt.contains("Lead with the answer"));
        assert!(prompt.contains("are requests to do the work"));
        assert!(prompt.contains("this isn't about X, it's about Y"));
        assert!(prompt.contains("Do not narrate routine work"));
        assert!(prompt.contains("must stand on its own"));
    }

    #[test]
    fn every_mode_builds_and_stays_reasonably_sized() {
        for (mode, ceiling) in [
            (PromptMode::Full, 11_000),
            (PromptMode::Minimal, 6_000),
            (PromptMode::Explore, 3_000),
            (PromptMode::Verify, 1_000),
        ] {
            let prompt = build_system_prompt(&["bash".to_string()], mode, None, None);
            assert!(!prompt.trim().is_empty(), "{:?} prompt is empty", mode);
            assert!(
                prompt.len() < ceiling,
                "{:?} prompt is {} chars, over the {} ceiling",
                mode,
                prompt.len(),
                ceiling
            );
        }
    }

    /// A custom identity replaces the default but must not drop the behavioural
    /// rules that live in the shared sections.
    #[test]
    fn custom_identity_keeps_behavioural_sections() {
        let prompt = build_system_prompt(
            &["bash".to_string()],
            PromptMode::Full,
            Some("You are a test-runner bot."),
            None,
        );
        assert!(prompt.contains("You are a test-runner bot."));
        assert!(!prompt.contains("a touch of dry wit"));
        assert!(prompt.contains("# Mid-turn Input"));
    }

    #[test]
    fn custom_priorities_replace_the_defaults() {
        let prompt = build_system_prompt(
            &["bash".to_string()],
            PromptMode::Full,
            None,
            Some(&["Always run the linter".to_string()]),
        );
        assert!(prompt.contains("Always run the linter"));
    }

    /// Priorities and Communication both talk about whether to act or advise.
    /// They disagreed once ("if asked how to approach something, answer first"
    /// vs "don't stop at offering a plan") and the model got whichever it read
    /// last. Both behaviours are wanted; the split is what was asked for.
    #[test]
    fn advice_and_action_are_distinguished_consistently() {
        let prompt = full();
        assert!(prompt.contains("Distinguish asking for advice from asking for the work"));
        assert!(!prompt.contains("If asked how to approach something, answer first"));
        assert!(prompt.contains("are requests to do the work"));
    }

    #[test]
    fn batching_is_taught_once_not_three_times() {
        let prompt = full();
        let batch_lines = prompt
            .lines()
            .filter(|line| line.to_lowercase().contains("batch") && line.contains("parallel"))
            .count();
        assert!(
            batch_lines <= 3,
            "batching is repeated {} times in the prompt",
            batch_lines
        );
    }

    /// The static/dynamic split has to stay a prefix split: everything the
    /// provider can cache must sit before the offset.
    #[test]
    fn prompt_cache_prefix_covers_all_static_sections() {
        let cache = PromptCache::build(&["bash".to_string()], PromptMode::Full, None, None);
        let prefix = cache.static_prefix();
        assert!(prefix.contains("# Safety"));
        assert!(prefix.contains("# Mid-turn Input"));
        assert!(prefix.contains("# Workflow"));
        // The clock is the only thing that has to move day to day.
        assert!(!prefix.contains("# Current Time"));
        assert!(cache.dynamic_suffix().contains("# Current Time"));
    }
}
