use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::{Command, Stdio};
use std::sync::{Arc, RwLock};
use std::time::Instant;
use uuid::Uuid;

const DEFAULT_LOG_TAIL_LINES: usize = 200;

#[derive(Debug, Clone)]
pub enum ProcessStatus {
    Running,
    Completed(i32),
    Failed(i32),
    Killed(String),
}

#[derive(Debug, Clone)]
pub struct ProcessSession {
    pub id: String,
    pub command: String,
    pub cwd: String,
    pub started_at: Instant,
    pub status: ProcessStatus,
    pub pid: Option<u32>,
    pub output: String,
    pub error_output: String,
}

impl ProcessSession {
    fn new(command: String, cwd: String, pid: Option<u32>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            command,
            cwd,
            started_at: Instant::now(),
            status: ProcessStatus::Running,
            pid,
            output: String::new(),
            error_output: String::new(),
        }
    }

    fn runtime_ms(&self) -> u64 {
        self.started_at.elapsed().as_millis() as u64
    }
}

struct ProcessRegistry {
    sessions: Arc<RwLock<HashMap<String, ProcessSession>>>,
}

impl ProcessRegistry {
    fn new() -> Self {
        Self {
            sessions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn register(&self, session: ProcessSession) -> String {
        let id = session.id.clone();
        if let Ok(mut sessions) = self.sessions.write() {
            sessions.insert(id.clone(), session);
        }
        id
    }

    fn get(&self, id: &str) -> Option<ProcessSession> {
        if let Ok(sessions) = self.sessions.read() {
            sessions.get(id).cloned()
        } else {
            None
        }
    }

    fn update(&self, id: &str, session: ProcessSession) -> bool {
        if let Ok(mut sessions) = self.sessions.write() {
            if sessions.contains_key(id) {
                sessions.insert(id.to_string(), session);
                return true;
            }
        }
        false
    }

    fn list_running(&self) -> Vec<ProcessSession> {
        if let Ok(sessions) = self.sessions.read() {
            sessions
                .values()
                .filter(|s| matches!(s.status, ProcessStatus::Running))
                .cloned()
                .collect()
        } else {
            vec![]
        }
    }

    fn list_finished(&self) -> Vec<ProcessSession> {
        if let Ok(sessions) = self.sessions.read() {
            sessions
                .values()
                .filter(|s| !matches!(s.status, ProcessStatus::Running))
                .cloned()
                .collect()
        } else {
            vec![]
        }
    }

    fn append_output(&self, id: &str, output: &str) {
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(session) = sessions.get_mut(id) {
                session.output.push_str(output);
            }
        }
    }

    fn append_error(&self, id: &str, output: &str) {
        if let Ok(mut sessions) = self.sessions.write() {
            if let Some(session) = sessions.get_mut(id) {
                session.error_output.push_str(output);
            }
        }
    }
}

pub struct ProcessTool {
    workspace: PathBuf,
    registry: Arc<ProcessRegistry>,
}

impl ProcessTool {
    pub fn new(config: Config) -> Self {
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        Self {
            workspace,
            registry: Arc::new(ProcessRegistry::new()),
        }
    }

    fn resolve_log_slice_window(
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> (usize, Option<usize>) {
        let using_default_tail = offset.is_none() && limit.is_none();
        let effective_limit = if let Some(limit) = limit {
            Some(limit)
        } else if using_default_tail {
            Some(DEFAULT_LOG_TAIL_LINES)
        } else {
            None
        };
        (offset.unwrap_or(0), effective_limit)
    }

    fn slice_log_lines(
        output: &str,
        offset: usize,
        limit: Option<usize>,
    ) -> (String, usize, usize) {
        let lines: Vec<&str> = output.lines().collect();
        let total_lines = lines.len();
        let total_chars = output.chars().count();

        let start = offset.min(total_lines);
        let end = limit
            .map(|l| (start + l).min(total_lines))
            .unwrap_or(total_lines);

        let slice = lines[start..end].join("\n");
        (slice, total_lines, total_chars)
    }

    fn default_tail_note(total_lines: usize, using_default_tail: bool) -> String {
        if !using_default_tail || total_lines <= DEFAULT_LOG_TAIL_LINES {
            return String::new();
        }
        format!(
            "\n\n[showing last {} of {} lines]",
            DEFAULT_LOG_TAIL_LINES, total_lines
        )
    }

    async fn start_process(&self, command: String, workdir: Option<String>) -> Result<String> {
        let cwd = workdir.unwrap_or_else(|| self.workspace.to_string_lossy().to_string());
        let cwd_path = PathBuf::from(&cwd);

        if !cwd_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "Working directory does not exist: {}",
                cwd
            )));
        }

        let mut cmd = if cfg!(windows) {
            let mut c = Command::new("cmd");
            c.args(["/C", &command]);
            c
        } else {
            let mut c = Command::new("sh");
            c.args(["-lc", &command]);
            c
        };
        cmd.current_dir(&cwd_path)
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        let child = cmd
            .spawn()
            .map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;

        let pid = child.id();
        let session = ProcessSession::new(command, cwd, Some(pid));
        let session_id = self.registry.register(session);

        let registry = self.registry.clone();
        let session_id_clone = session_id.clone();

        tokio::spawn(async move {
            let output = tokio::task::spawn_blocking(move || child.wait_with_output())
                .await
                .ok()
                .and_then(|r| r.ok());

            if let Some(output) = output {
                let stdout = String::from_utf8_lossy(&output.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output.stderr).to_string();

                registry.append_output(&session_id_clone, &stdout);
                registry.append_error(&session_id_clone, &stderr);

                if let Some(mut session) = registry.get(&session_id_clone) {
                    if output.status.success() {
                        session.status =
                            ProcessStatus::Completed(output.status.code().unwrap_or(0));
                    } else {
                        session.status = ProcessStatus::Failed(output.status.code().unwrap_or(-1));
                    }
                    let _ = registry.update(&session_id_clone, session);
                }
            }
        });

        Ok(session_id)
    }

    async fn poll_process(&self, session_id: &str) -> Result<String> {
        let session = self.registry.get(session_id).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Session not found: {}", session_id))
        })?;

        let status_text = match &session.status {
            ProcessStatus::Running => "running".to_string(),
            ProcessStatus::Completed(code) => format!("completed (exit code: {})", code),
            ProcessStatus::Failed(code) => format!("failed (exit code: {})", code),
            ProcessStatus::Killed(signal) => format!("killed ({})", signal),
        };

        let output = if session.output.is_empty() {
            "(no output)".to_string()
        } else {
            session.output.clone()
        };

        Ok(format!(
            "{}\n\nStatus: {}\nRuntime: {}ms",
            output.trim(),
            status_text,
            session.runtime_ms()
        ))
    }

    async fn kill_process(&self, session_id: &str) -> Result<String> {
        let session = self.registry.get(session_id).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Session not found: {}", session_id))
        })?;

        if let Some(pid) = session.pid {
            let pid_val: u32 = pid;
            let pid_str = pid_val.to_string();
            #[cfg(windows)]
            {
                let _ = Command::new("taskkill")
                    .args(["/F", "/PID", &pid_str])
                    .output();
            }
            #[cfg(not(windows))]
            {
                let _ = Command::new("kill").args(["-9", &pid_str]).output();
            }
        }

        if let Some(mut session) = self.registry.get(session_id) {
            session.status = ProcessStatus::Killed("SIGKILL".to_string());
            let _ = self.registry.update(session_id, session);
        }

        Ok(format!("Process {} killed", session_id))
    }

    async fn list_processes(&self) -> Result<String> {
        let running = self.registry.list_running();
        let finished = self.registry.list_finished();

        if running.is_empty() && finished.is_empty() {
            return Ok("No running or recent processes.".to_string());
        }

        let mut lines = Vec::new();

        for s in running.iter() {
            lines.push(format!(
                "{} RUNNING {}ms :: {}",
                s.id,
                s.runtime_ms(),
                s.command
            ));
        }

        for s in finished.iter() {
            let status = match &s.status {
                ProcessStatus::Completed(code) => format!("completed({})", code),
                ProcessStatus::Failed(code) => format!("failed({})", code),
                ProcessStatus::Killed(sig) => format!("killed({})", sig),
                ProcessStatus::Running => "running".to_string(),
            };
            lines.push(format!(
                "{} {} {}ms :: {}",
                s.id,
                status,
                s.runtime_ms(),
                s.command
            ));
        }

        Ok(lines.join("\n"))
    }

    async fn get_log(
        &self,
        session_id: &str,
        offset: Option<usize>,
        limit: Option<usize>,
    ) -> Result<String> {
        let session = self.registry.get(session_id).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Session not found: {}", session_id))
        })?;

        let (effective_offset, effective_limit) = Self::resolve_log_slice_window(offset, limit);
        let (slice, total_lines, total_chars) =
            Self::slice_log_lines(&session.output, effective_offset, effective_limit);

        let tail_note = Self::default_tail_note(total_lines, offset.is_none() && limit.is_none());

        let output = if slice.is_empty() {
            "(no output)".to_string()
        } else {
            slice
        };

        Ok(format!(
            "{}{}\n\n[total: {} lines, {} chars]",
            output, tail_note, total_lines, total_chars
        ))
    }
}

#[async_trait]
impl Tool for ProcessTool {
    fn name(&self) -> &str {
        "process"
    }

    fn description(&self) -> &str {
        "Manage background processes: list, start, poll, log, kill"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "start", "poll", "log", "kill"],
                    "description": "Process action to perform"
                },
                "sessionId": {
                    "type": "string",
                    "description": "Session ID for poll/log/kill actions"
                },
                "command": {
                    "type": "string",
                    "description": "Command to start (for start action)"
                },
                "workdir": {
                    "type": "string",
                    "description": "Working directory for the process"
                },
                "offset": {
                    "type": "integer",
                    "description": "Log offset for log action"
                },
                "limit": {
                    "type": "integer",
                    "description": "Log limit for log action"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let action = args
            .get("action")
            .and_then(|v| v.as_str())
            .ok_or_else(|| OSAgentError::ToolExecution("action is required".to_string()))?;

        match action {
            "list" => self.list_processes().await,
            "start" => {
                let command = args
                    .get("command")
                    .and_then(|v| v.as_str())
                    .ok_or_else(|| {
                        OSAgentError::ToolExecution("command is required for start".to_string())
                    })?;
                let workdir = args
                    .get("workdir")
                    .and_then(|v| v.as_str())
                    .map(String::from);
                self.start_process(command.to_string(), workdir).await
            }
            "poll" => {
                let session_id =
                    args.get("sessionId")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            OSAgentError::ToolExecution(
                                "sessionId is required for poll".to_string(),
                            )
                        })?;
                self.poll_process(session_id).await
            }
            "log" => {
                let session_id =
                    args.get("sessionId")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            OSAgentError::ToolExecution("sessionId is required for log".to_string())
                        })?;
                let offset = args
                    .get("offset")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                let limit = args
                    .get("limit")
                    .and_then(|v| v.as_u64())
                    .map(|v| v as usize);
                self.get_log(session_id, offset, limit).await
            }
            "kill" => {
                let session_id =
                    args.get("sessionId")
                        .and_then(|v| v.as_str())
                        .ok_or_else(|| {
                            OSAgentError::ToolExecution(
                                "sessionId is required for kill".to_string(),
                            )
                        })?;
                self.kill_process(session_id).await
            }
            _ => Err(OSAgentError::ToolExecution(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to run long-running commands in the background, monitor their output, or interact with running processes."
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "List all processes".to_string(),
                input: json!({"action": "list"}),
            },
            ToolExample {
                description: "Start a background process".to_string(),
                input: json!({"action": "start", "command": "python -m http.server 8080", "workdir": "/tmp"}),
            },
            ToolExample {
                description: "Poll process output".to_string(),
                input: json!({"action": "poll", "sessionId": "abc-123"}),
            },
            ToolExample {
                description: "Get process log".to_string(),
                input: json!({"action": "log", "sessionId": "abc-123", "offset": 0, "limit": 100}),
            },
            ToolExample {
                description: "Kill a process".to_string(),
                input: json!({"action": "kill", "sessionId": "abc-123"}),
            },
        ]
    }
}
