use crate::agent::events::{AgentEvent, EventBus};
use crate::agent::prompt::{self, PromptMode};
use crate::agent::subagent_manager::SubagentManager;
use crate::config::Config;
use crate::error::Result;
use crate::storage::SqliteStorage;
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tracing::{error, info, warn};
use uuid::Uuid;

const DEFAULT_WORKER_TIMEOUT_SECS: u64 = 300;
const DEFAULT_PHASE_TIMEOUT_SECS: u64 = 600;
const DEFAULT_MAX_WORKERS: usize = 3;
const SCRATCHPAD_DIR_NAME: &str = ".osagent_scratchpad";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CoordinatorPhase {
    Research,
    Synthesis,
    Implementation,
    Verification,
    Complete,
}

impl std::fmt::Display for CoordinatorPhase {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            CoordinatorPhase::Research => write!(f, "research"),
            CoordinatorPhase::Synthesis => write!(f, "synthesis"),
            CoordinatorPhase::Implementation => write!(f, "implementation"),
            CoordinatorPhase::Verification => write!(f, "verification"),
            CoordinatorPhase::Complete => write!(f, "complete"),
        }
    }
}

#[derive(Debug, Clone)]
pub struct WorkerSpec {
    pub id: String,
    pub description: String,
    pub prompt: String,
    pub agent_type: String,
    pub timeout_secs: u64,
}

#[derive(Debug, Clone)]
pub struct WorkerResult {
    pub worker_id: String,
    pub session_id: String,
    pub agent_type: String,
    pub status: String,
    pub result: String,
    pub tool_count: i32,
    pub duration_ms: u64,
}

#[derive(Debug, Clone)]
pub struct CoordinatorOutcome {
    pub research_results: Vec<WorkerResult>,
    pub synthesis: String,
    pub implementation_results: Vec<WorkerResult>,
    pub verification_result: Option<WorkerResult>,
    pub verdict: String,
    pub total_duration_ms: u64,
}

impl CoordinatorOutcome {
    pub fn to_summary(&self) -> String {
        let mut lines = Vec::new();

        lines.push(format!(
            "## Coordinator Complete ({}ms, verdict: {})",
            self.total_duration_ms, self.verdict
        ));

        if !self.research_results.is_empty() {
            lines.push("\n### Research Phase".to_string());
            for r in &self.research_results {
                lines.push(format!(
                    "- Worker [{}]: {} ({} tools, {}ms)",
                    r.agent_type, r.status, r.tool_count, r.duration_ms
                ));
            }
        }

        if !self.synthesis.is_empty() {
            lines.push("\n### Synthesis".to_string());
            lines.push(self.synthesis.clone());
        }

        if !self.implementation_results.is_empty() {
            lines.push("\n### Implementation Phase".to_string());
            for r in &self.implementation_results {
                lines.push(format!(
                    "- Worker [{}]: {} ({} tools, {}ms)",
                    r.agent_type, r.status, r.tool_count, r.duration_ms
                ));
            }
        }

        if let Some(ref v) = self.verification_result {
            lines.push("\n### Verification".to_string());
            lines.push(format!(
                "- Verify [{}]: {} ({} tools, {}ms)",
                v.agent_type, v.status, v.tool_count, v.duration_ms
            ));
            lines.push(v.result.clone());
        }

        lines.join("\n")
    }
}

struct Scratchpad {
    base_dir: PathBuf,
}

impl Scratchpad {
    fn new(workspace_root: &PathBuf, coordinator_id: &str) -> Self {
        let base_dir = workspace_root
            .join(SCRATCHPAD_DIR_NAME)
            .join(coordinator_id);
        let _ = fs::create_dir_all(&base_dir);
        Self { base_dir }
    }

    fn write(&self, worker_id: &str, filename: &str, content: &str) -> Result<PathBuf> {
        let dir = self.base_dir.join(worker_id);
        fs::create_dir_all(&dir).ok();
        let path = dir.join(filename);
        fs::write(&path, content).map_err(|e| {
            crate::error::OSAgentError::ToolExecution(format!(
                "Failed to write scratchpad file: {}",
                e
            ))
        })?;
        Ok(path)
    }

    fn read(&self, worker_id: &str, filename: &str) -> Option<String> {
        let path = self.base_dir.join(worker_id).join(filename);
        fs::read_to_string(path).ok()
    }

    fn read_all_findings(&self) -> Vec<(String, String)> {
        let mut results = Vec::new();
        if let Ok(entries) = fs::read_dir(&self.base_dir) {
            for entry in entries.flatten() {
                if entry.path().is_dir() {
                    let worker_id = entry.file_name().to_string_lossy().to_string();
                    let findings_path = entry.path().join("findings.md");
                    if findings_path.exists() {
                        if let Ok(content) = fs::read_to_string(&findings_path) {
                            results.push((worker_id, content));
                        }
                    }
                }
            }
        }
        results
    }

    fn cleanup(&self) {
        if self.base_dir.exists() {
            let _ = fs::remove_dir_all(&self.base_dir);
        }
    }
}

pub struct Coordinator {
    storage: Arc<SqliteStorage>,
    event_bus: Arc<EventBus>,
    subagent_manager: Arc<SubagentManager>,
    config: Arc<tokio::sync::RwLock<Config>>,
    workspace_root: PathBuf,
}

impl Coordinator {
    pub fn new(
        storage: Arc<SqliteStorage>,
        event_bus: Arc<EventBus>,
        subagent_manager: Arc<SubagentManager>,
        config: Arc<tokio::sync::RwLock<Config>>,
        workspace_root: PathBuf,
    ) -> Self {
        Self {
            storage,
            event_bus,
            subagent_manager,
            config,
            workspace_root,
        }
    }

    pub async fn run(
        &self,
        parent_session_id: String,
        request: String,
        max_workers: usize,
    ) -> Result<CoordinatorOutcome> {
        let coordinator_id = Uuid::new_v4().to_string();
        let scratchpad = Scratchpad::new(&self.workspace_root, &coordinator_id);
        let start = Instant::now();
        let max_workers = max_workers.clamp(1, DEFAULT_MAX_WORKERS);

        let _cleanup_guard = ScratchpadGuard(scratchpad.base_dir.clone());

        self.emit_phase(&parent_session_id, &CoordinatorPhase::Research, 0);

        let research_results = self
            .run_research_phase(&parent_session_id, &request, max_workers, &scratchpad)
            .await;

        self.emit_phase(
            &parent_session_id,
            &CoordinatorPhase::Synthesis,
            research_results.len(),
        );

        let findings = scratchpad.read_all_findings();
        let synthesis = Self::build_synthesis(&request, &findings);
        scratchpad
            .write("coordinator", "synthesis.md", &synthesis)
            .ok();

        self.emit_phase(&parent_session_id, &CoordinatorPhase::Implementation, 0);

        let mut implementation_results = self
            .run_implementation_phase(&parent_session_id, &request, &synthesis, max_workers)
            .await;

        self.emit_phase(&parent_session_id, &CoordinatorPhase::Verification, 0);

        let verification_result = self
            .run_verification_phase(
                &parent_session_id,
                &request,
                &synthesis,
                &implementation_results,
            )
            .await;

        let verdict = Self::extract_verdict(&verification_result);

        let final_verdict = if verdict == "fail" {
            info!("Coordinator: verification failed, running fix loop");
            let fix_results = self
                .run_fix_phase(
                    &parent_session_id,
                    &request,
                    &synthesis,
                    &verification_result,
                )
                .await;

            if !fix_results.is_empty() {
                implementation_results.extend(fix_results);
            }

            self.emit_phase(&parent_session_id, &CoordinatorPhase::Verification, 0);

            let reverify_result = self
                .run_verification_phase(
                    &parent_session_id,
                    &request,
                    &synthesis,
                    &implementation_results,
                )
                .await;

            Self::extract_verdict(&reverify_result)
        } else {
            verdict
        };

        self.emit_phase(&parent_session_id, &CoordinatorPhase::Complete, 0);

        Ok(CoordinatorOutcome {
            research_results,
            synthesis,
            implementation_results,
            verification_result,
            verdict: final_verdict,
            total_duration_ms: start.elapsed().as_millis() as u64,
        })
    }

    async fn run_research_phase(
        &self,
        parent_session_id: &str,
        request: &str,
        max_workers: usize,
        scratchpad: &Scratchpad,
    ) -> Vec<WorkerResult> {
        let specs = self.design_research_workers(request, max_workers);
        let count = specs.len();

        info!("Coordinator: spawning {} research workers", count);

        let results = self.spawn_parallel_workers(parent_session_id, specs).await;

        for result in &results {
            let worker_id = &result.worker_id;
            let content = if result.status == "completed" {
                result.result.clone()
            } else {
                format!("Worker failed: {}", result.result)
            };
            if let Err(e) = scratchpad.write(worker_id, "findings.md", &content) {
                warn!("Failed to write research findings: {}", e);
            }
        }

        results
    }

    async fn run_implementation_phase(
        &self,
        parent_session_id: &str,
        request: &str,
        synthesis: &str,
        max_workers: usize,
    ) -> Vec<WorkerResult> {
        let specs = self.design_implementation_workers(request, synthesis, max_workers);
        let count = specs.len();

        if count == 0 {
            info!("Coordinator: no implementation workers needed");
            return Vec::new();
        }

        info!(
            "Coordinator: running {} implementation workers sequentially",
            count
        );

        self.spawn_serial_workers(parent_session_id, specs).await
    }

    async fn run_verification_phase(
        &self,
        parent_session_id: &str,
        request: &str,
        synthesis: &str,
        implementation_results: &[WorkerResult],
    ) -> Option<WorkerResult> {
        if implementation_results.is_empty() {
            return None;
        }

        let impl_summary: Vec<String> = implementation_results
            .iter()
            .map(|r| format!("- [{}] {}: {}", r.agent_type, r.worker_id, r.status))
            .collect();

        let verify_prompt = format!(
            "You are a verification agent. Your job is to try to BREAK the implementation, not confirm it works.\n\n\
             ## Original Request\n{}\n\n\
             ## Implementation Plan\n{}\n\n\
             ## Implementation Results\n{}\n\n\
             ## Verification Protocol\n\
             1. Read the changed files listed above\n\
             2. Run the project's tests if possible (cargo test, npm test, pytest, etc.)\n\
             3. Check for edge cases: off-by-one, null handling, error paths, race conditions\n\
             4. Verify imports and dependencies are correct\n\
             5. Look for regressions in unchanged code\n\n\
             ## Required Output Format\n\
             ### Check: [what you're checking]\n\
             Command: [command run]\n\
             Output: [observed output]\n\
             Result: PASS|FAIL\n\n\
             End with exactly one of: VERDICT: PASS, VERDICT: FAIL, or VERDICT: PARTIAL",
            request,
            synthesis,
            impl_summary.join("\n")
        );

        let spec = WorkerSpec {
            id: format!("verify-{}", Uuid::new_v4()),
            description: "Verify implementation".to_string(),
            prompt: verify_prompt,
            agent_type: "verify".to_string(),
            timeout_secs: DEFAULT_WORKER_TIMEOUT_SECS,
        };

        let results = self
            .spawn_parallel_workers(parent_session_id, vec![spec])
            .await;

        results.into_iter().next()
    }

    async fn run_fix_phase(
        &self,
        parent_session_id: &str,
        request: &str,
        synthesis: &str,
        verification_result: &Option<WorkerResult>,
    ) -> Vec<WorkerResult> {
        let verify_feedback = match verification_result {
            Some(v) => v.result.clone(),
            None => return Vec::new(),
        };

        info!("Coordinator: spawning fix worker to address verification failures");

        let fix_prompt = format!(
            "You are a fix agent. The verification phase found bugs that must be fixed.\n\n\
             ## Original Request\n{}\n\n\
             ## Implementation Plan\n{}\n\n\
             ## Verification Failures\n{}\n\n\
             ## Instructions\n\
             - Read each file mentioned in the verification failures\n\
             - Fix the specific issues identified\n\
             - Do not refactor or change unrelated code\n\
             - Use write_file or edit_file as needed\n\
             - Do NOT use bash to create directories; write_file already creates parent directories\n\
             - Run the same tests/checks the verifier ran to confirm the fix works\n\
             - Report what you changed and why",
            request,
            synthesis,
            verify_feedback,
        );

        let spec = WorkerSpec {
            id: format!("fix-{}", Uuid::new_v4()),
            description: "Fix verification failures".to_string(),
            prompt: fix_prompt,
            agent_type: "general".to_string(),
            timeout_secs: DEFAULT_WORKER_TIMEOUT_SECS,
        };

        self.spawn_serial_workers(parent_session_id, vec![spec])
            .await
    }

    async fn spawn_parallel_workers(
        &self,
        parent_session_id: &str,
        specs: Vec<WorkerSpec>,
    ) -> Vec<WorkerResult> {
        let mut handles: Vec<tokio::task::JoinHandle<WorkerResult>> = Vec::new();

        for spec in specs {
            let sm = self.subagent_manager.clone();
            let parent = parent_session_id.to_string();

            handles.push(tokio::spawn(async move {
                let start = Instant::now();

                let spawn_result = sm
                    .spawn_subagent(
                        parent.clone(),
                        spec.description.clone(),
                        spec.prompt.clone(),
                        spec.agent_type.clone(),
                    )
                    .await;

                let session_id = match spawn_result {
                    Ok(id) => id,
                    Err(e) => {
                        return WorkerResult {
                            worker_id: spec.id,
                            session_id: String::new(),
                            agent_type: spec.agent_type,
                            status: "failed".to_string(),
                            result: format!("Failed to spawn: {}", e),
                            tool_count: 0,
                            duration_ms: start.elapsed().as_millis() as u64,
                        };
                    }
                };

                let (status, result, actual_tool_count) = sm
                    .wait_for_subagent(&session_id, spec.timeout_secs)
                    .await
                    .unwrap_or(("error".to_string(), "Wait failed".to_string(), 0));

                WorkerResult {
                    worker_id: spec.id,
                    session_id,
                    agent_type: spec.agent_type,
                    status,
                    result,
                    tool_count: actual_tool_count,
                    duration_ms: start.elapsed().as_millis() as u64,
                }
            }));
        }

        let mut results = Vec::new();
        for handle in handles {
            match handle.await {
                Ok(result) => results.push(result),
                Err(e) => error!("Coordinator worker task panicked: {}", e),
            }
        }
        results
    }

    async fn spawn_serial_workers(
        &self,
        parent_session_id: &str,
        specs: Vec<WorkerSpec>,
    ) -> Vec<WorkerResult> {
        let mut results = Vec::new();

        for spec in specs {
            let start = Instant::now();

            let spawn_result = self
                .subagent_manager
                .spawn_subagent(
                    parent_session_id.to_string(),
                    spec.description.clone(),
                    spec.prompt.clone(),
                    spec.agent_type.clone(),
                )
                .await;

            let session_id = match spawn_result {
                Ok(id) => id,
                Err(e) => {
                    results.push(WorkerResult {
                        worker_id: spec.id,
                        session_id: String::new(),
                        agent_type: spec.agent_type,
                        status: "failed".to_string(),
                        result: format!("Failed to spawn: {}", e),
                        tool_count: 0,
                        duration_ms: start.elapsed().as_millis() as u64,
                    });
                    continue;
                }
            };

            let (status, result, actual_tool_count) = self
                .subagent_manager
                .wait_for_subagent(&session_id, spec.timeout_secs)
                .await
                .unwrap_or(("error".to_string(), "Wait failed".to_string(), 0));

            results.push(WorkerResult {
                worker_id: spec.id,
                session_id,
                agent_type: spec.agent_type,
                status,
                result,
                tool_count: actual_tool_count,
                duration_ms: start.elapsed().as_millis() as u64,
            });
        }

        results
    }

    fn design_research_workers(&self, request: &str, max_workers: usize) -> Vec<WorkerSpec> {
        let mut specs = Vec::new();
        let worker_count = max_workers.clamp(1, 3);

        let areas = Self::extract_research_areas(request, worker_count);

        for (i, area) in areas.into_iter().enumerate() {
            specs.push(WorkerSpec {
                id: format!("research-{}-{}", i, Uuid::new_v4()),
                description: format!("Explore: {}", area),
                prompt: format!(
                    "Research the following area of the codebase for the task: \"{}\"\n\n\
                     Focus area: {}\n\n\
                     Instructions:\n\
                     - Use grep, glob, and read_file to find relevant files\n\
                     - Read key files to understand the structure\n\
                     - Identify functions, types, and patterns relevant to the task\n\
                     - Report your findings concisely with file paths and line numbers\n\
                     - Do NOT make any changes\n\
                     - Write a summary of what you found",
                    request, area
                ),
                agent_type: "explore".to_string(),
                timeout_secs: DEFAULT_WORKER_TIMEOUT_SECS,
            });
        }

        specs
    }

    fn extract_research_areas(request: &str, count: usize) -> Vec<String> {
        let request_lower = request.to_lowercase();

        let mut areas = Vec::new();

        let keywords = [
            (
                "frontend|ui|component|page|view|css|html|react|vue|svelte",
                "Frontend/UI code",
            ),
            (
                "backend|api|server|route|handler|controller|endpoint",
                "Backend/API code",
            ),
            (
                "database|db|sql|migration|model|schema|query",
                "Database layer",
            ),
            ("test|spec|integration|e2e|benchmark", "Test infrastructure"),
            (
                "config|setting|env|constant|util|helper",
                "Configuration and utilities",
            ),
            (
                "auth|security|token|session|permission",
                "Authentication and security",
            ),
            (
                "error|log|trace|debug|monitor",
                "Error handling and logging",
            ),
            ("build|deploy|ci|cd|pipeline|docker", "Build and deployment"),
        ];

        for (pattern, label) in keywords {
            if request_lower.contains(pattern.split('|').next().unwrap_or("")) {
                areas.push(label.to_string());
                if areas.len() >= count {
                    return areas;
                }
            }
        }

        for (_, label) in keywords {
            if !areas.contains(&label.to_string()) && areas.len() < count {
                areas.push(label.to_string());
            }
        }

        if areas.is_empty() {
            areas.push("General codebase structure".to_string());
        }

        areas
    }

    fn design_implementation_workers(
        &self,
        request: &str,
        synthesis: &str,
        max_workers: usize,
    ) -> Vec<WorkerSpec> {
        if Self::is_greenfield_task(synthesis, &self.workspace_root) {
            return vec![Self::single_implementation_worker(request, synthesis)];
        }

        let lines: Vec<&str> = synthesis.lines().collect();
        let mut file_groups: Vec<(String, Vec<String>)> = Vec::new();
        let mut current_group: Option<(String, Vec<String>)> = None;

        for line in lines {
            let trimmed = line.trim();
            if trimmed.starts_with("##") || trimmed.starts_with("**") {
                if let Some(group) = current_group.take() {
                    if !group.1.is_empty() {
                        file_groups.push(group);
                    }
                }
                let title = trimmed
                    .trim_start_matches('#')
                    .trim_start_matches('*')
                    .trim()
                    .to_string();
                current_group = Some((title, Vec::new()));
            } else if trimmed.starts_with("- ") || trimmed.starts_with("* ") {
                if let Some(ref mut group) = current_group {
                    group.1.push(
                        trimmed
                            .trim_start_matches("- ")
                            .trim_start_matches("* ")
                            .to_string(),
                    );
                }
            }
        }
        if let Some(group) = current_group.take() {
            if !group.1.is_empty() {
                file_groups.push(group);
            }
        }

        for (_, files) in &mut file_groups {
            files.retain(|item| Self::looks_like_file_assignment(item));
        }
        file_groups.retain(|(_, files)| !files.is_empty());

        if file_groups.is_empty() {
            return vec![Self::single_implementation_worker(request, synthesis)];
        }

        let worker_count = file_groups.len().min(max_workers).max(1);
        file_groups.truncate(worker_count);

        file_groups
            .into_iter()
            .enumerate()
            .map(|(i, (title, files))| WorkerSpec {
                id: format!("impl-{}-{}", i, Uuid::new_v4()),
                description: format!("Implement: {}", title),
                prompt: format!(
                    "Implement the following part of a larger task.\n\n\
                         ## Overall Task\n{}\n\n\
                         ## Implementation Plan\n{}\n\n\
                         ## Your Assignment: {}\n\
                         Files to modify: {}\n\n\
                         Instructions:\n\
                         - Read each file before editing\n\
                         - Make the smallest correct changes\n\
                         - Preserve existing formatting and conventions\n\
                         - Do NOT modify files outside your assignment\n\
                         - If you need to create a new file, use write_file with both the path and content fields\n\
                         - Do NOT use bash to create directories; write_file already creates parent directories\n\
                         - Run validation (tests/lint) if available after changes",
                    request,
                    synthesis,
                    title,
                    files.join(", ")
                ),
                agent_type: "general".to_string(),
                timeout_secs: DEFAULT_WORKER_TIMEOUT_SECS,
            })
            .collect()
    }

    fn single_implementation_worker(request: &str, synthesis: &str) -> WorkerSpec {
        WorkerSpec {
            id: format!("impl-{}", Uuid::new_v4()),
            description: "Implement main task".to_string(),
            prompt: format!(
                "Implement the requested change end-to-end.\n\n\
                 ## Overall Task\n{}\n\n\
                 ## Research and Plan Context\n{}\n\n\
                 ## Instructions\n\
                 - Treat this as a single-writer task; you own the whole implementation\n\
                 - The workspace may be empty or partially empty, so do not assume files already exist\n\
                 - Create new files directly with write_file, always providing both the path and content fields\n\
                 - Do NOT use bash to create directories; write_file already creates parent directories\n\
                 - Use glob or list_files to inspect what exists before reading guessed paths\n\
                 - Prefer a coherent, working implementation over splitting work across multiple files speculatively\n\
                 - Run validation if available after making changes",
                request, synthesis
            ),
            agent_type: "general".to_string(),
            timeout_secs: DEFAULT_WORKER_TIMEOUT_SECS,
        }
    }

    fn is_greenfield_task(synthesis: &str, workspace_root: &PathBuf) -> bool {
        if let Ok(entries) = fs::read_dir(workspace_root) {
            let mut count = 0;
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if !name_str.starts_with('.') && name_str != "target" && name_str != "node_modules"
                {
                    count += 1;
                }
            }
            if count == 0 {
                return true;
            }
        }

        let lower = synthesis.to_lowercase();
        [
            "workspace is empty",
            "completely empty",
            "no existing codebase",
            "greenfield",
            "doesn't exist yet",
            "do not exist yet",
        ]
        .iter()
        .any(|needle| lower.contains(needle))
    }

    fn looks_like_file_assignment(item: &str) -> bool {
        let cleaned = item
            .trim()
            .trim_matches('`')
            .trim_matches('"')
            .trim_matches('(')
            .trim_matches(')');
        let lower = cleaned.to_lowercase();

        let has_path_sep = cleaned.contains('/') || cleaned.contains('\\');

        let has_known_ext = [
            ".rs", ".ts", ".tsx", ".js", ".jsx", ".html", ".css", ".json", ".toml", ".yaml",
            ".yml", ".md", ".py", ".sh", ".sql", ".gif", ".png", ".jpg", ".webp", ".mp4",
        ]
        .iter()
        .any(|ext| lower.contains(ext));

        if has_path_sep && has_known_ext {
            return true;
        }

        has_path_sep
            && (lower.ends_with(".html")
                || lower.ends_with(".css")
                || lower.ends_with(".js")
                || lower.ends_with(".ts")
                || lower.ends_with(".rs")
                || lower.ends_with(".json")
                || lower.ends_with(".toml")
                || lower.ends_with(".md")
                || lower.ends_with(".py")
                || lower.ends_with(".sh")
                || lower.ends_with(".sql"))
    }

    fn build_synthesis(request: &str, findings: &[(String, String)]) -> String {
        if findings.is_empty() {
            return format!(
                "## Synthesis\n\n\
                 Task: {}\n\n\
                 No research findings available. Proceed with direct implementation.",
                request
            );
        }

        let findings_text: Vec<String> = findings
            .iter()
            .map(|(worker_id, content)| format!("### Research Worker: {}\n{}", worker_id, content))
            .collect();

        format!(
            "## Synthesis\n\n\
             Task: {}\n\n\
             ## Research Findings\n\
             {}\n\n\
             ## Approach\n\
             Based on the research findings above, implement the changes file by file.",
            request,
            findings_text.join("\n\n")
        )
    }

    fn extract_verdict(verification: &Option<WorkerResult>) -> String {
        if let Some(ref v) = verification {
            let result_lower = v.result.to_lowercase();
            if result_lower.contains("verdict: pass") {
                "pass".to_string()
            } else if result_lower.contains("verdict: fail") {
                "fail".to_string()
            } else if result_lower.contains("verdict: partial") {
                "partial".to_string()
            } else if v.status == "completed" {
                "pass".to_string()
            } else {
                "unknown".to_string()
            }
        } else {
            "skipped".to_string()
        }
    }

    fn emit_phase(&self, session_id: &str, phase: &CoordinatorPhase, workers_spawned: usize) {
        self.event_bus.emit(AgentEvent::CoordinatorPhase {
            session_id: session_id.to_string(),
            parent_session_id: session_id.to_string(),
            phase: phase.to_string(),
            workers_spawned,
            timestamp: SystemTime::now(),
        });
    }
}

struct ScratchpadGuard(PathBuf);

impl Drop for ScratchpadGuard {
    fn drop(&mut self) {
        if self.0.exists() {
            let _ = fs::remove_dir_all(&self.0);
        }
        let parent = self.0.parent();
        if let Some(p) = parent {
            if p.exists()
                && p.file_name()
                    .map(|n| n == SCRATCHPAD_DIR_NAME)
                    .unwrap_or(false)
            {
                let _ = fs::remove_dir(p);
            }
        }
    }
}
