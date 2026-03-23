use crate::config::{BashToolConfig, Config};
use crate::error::{OSAgentError, Result};
use crate::tools::guard::{command_touches_backups, ensure_relative_path_not_backups};
use crate::tools::output::maybe_store_large_output;
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::process::Command;
use std::time::Duration;

pub struct BashTool {
    config: BashToolConfig,
    workspace: PathBuf,
    writable: bool,
}

impl BashTool {
    pub fn new(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());

        if !workspace.exists() {
            let _ = std::fs::create_dir_all(&workspace);
        }

        Self {
            config: config.tools.bash,
            workspace,
            writable,
        }
    }

    fn validate_non_mutating_command(command: &str) -> Result<()> {
        let lowered = command.to_lowercase();
        let mutating_tokens = [
            "mkdir",
            "rmdir",
            "del",
            "rm",
            "copy",
            "cp",
            "move",
            "mv",
            "rename",
            "ren",
            "touch",
            "git add",
            "git apply",
            "git commit",
            "git checkout",
            "git clean",
            "git restore",
            "npm install",
            "npm update",
            "pnpm install",
            "yarn install",
            "cargo add",
            "cargo fix",
            "set-content",
            "add-content",
            "out-file",
            "new-item",
            "remove-item",
            "copy-item",
            "move-item",
            ">",
            ">>",
        ];

        if mutating_tokens.iter().any(|token| lowered.contains(token)) {
            return Err(OSAgentError::ToolExecution(
                "Bash read-only mode is limited to non-mutating commands".to_string(),
            ));
        }

        Ok(())
    }

    fn ensure_read_only_safe(&self, command: &str) -> Result<()> {
        if self.writable {
            return Ok(());
        }

        Self::validate_non_mutating_command(command)
    }

    pub fn validate_explicit_read_only(command: &str) -> Result<()> {
        Self::validate_non_mutating_command(command)
    }

    fn validate_workdir(&self, workdir: Option<&str>) -> Result<PathBuf> {
        let Some(workdir) = workdir.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(self.workspace.clone());
        };

        ensure_relative_path_not_backups(workdir)?;

        if workdir.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "workdir cannot contain '..'".to_string(),
            ));
        }

        let resolved = self.workspace.join(workdir);
        if !resolved.starts_with(&self.workspace) {
            return Err(OSAgentError::ToolExecution(
                "workdir must stay inside the workspace".to_string(),
            ));
        }

        if !resolved.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "workdir does not exist: {}",
                workdir
            )));
        }

        if !resolved.is_dir() {
            return Err(OSAgentError::ToolExecution(format!(
                "workdir is not a directory: {}",
                workdir
            )));
        }

        Ok(resolved)
    }

    fn first_token(segment: &str) -> Option<String> {
        let trimmed = segment.trim_start();
        if trimmed.is_empty() {
            return None;
        }

        let mut token = String::new();
        let mut in_single = false;
        let mut in_double = false;

        for ch in trimmed.chars() {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                c if c.is_whitespace() && !in_single && !in_double => break,
                _ => token.push(ch),
            }
        }

        let token = token
            .trim_matches(|ch| ch == '\'' || ch == '"')
            .trim()
            .to_string();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    }

    fn extract_command_heads(command: &str) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let chars: Vec<char> = command.chars().collect();
        let mut idx = 0usize;

        while idx < chars.len() {
            let ch = chars[idx];
            match ch {
                '\'' if !in_double => {
                    in_single = !in_single;
                    current.push(ch);
                    idx += 1;
                }
                '"' if !in_single => {
                    in_double = !in_double;
                    current.push(ch);
                    idx += 1;
                }
                '&' if !in_single
                    && !in_double
                    && idx + 1 < chars.len()
                    && chars[idx + 1] == '&' =>
                {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                    idx += 2;
                }
                '|' if !in_single
                    && !in_double
                    && idx + 1 < chars.len()
                    && chars[idx + 1] == '|' =>
                {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                    idx += 2;
                }
                '|' | ';' if !in_single && !in_double => {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                    idx += 1;
                }
                _ => {
                    current.push(ch);
                    idx += 1;
                }
            }
        }

        if !current.trim().is_empty() {
            segments.push(current.trim().to_string());
        }

        segments
            .into_iter()
            .filter_map(|segment| Self::first_token(&segment))
            .collect()
    }

    fn quote_arg(arg: &str) -> String {
        if arg.is_empty() {
            return "\"\"".to_string();
        }

        #[cfg(windows)]
        {
            if arg.contains([' ', '\t', '"', '&', '|', '<', '>']) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.to_string()
            }
        }

        #[cfg(not(windows))]
        {
            if arg
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-._/:=@".contains(ch))
            {
                arg.to_string()
            } else {
                format!("'{}'", arg.replace('\'', "'\"'\"'"))
            }
        }
    }

    pub fn build_command(command: &str, args_list: &[String]) -> String {
        if args_list.is_empty() {
            return command.to_string();
        }

        let suffix = args_list
            .iter()
            .map(|arg| Self::quote_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {}", command, suffix)
    }

    fn is_allowed_command(&self, command: &str) -> bool {
        self.config
            .allowed_commands
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(command))
    }

    fn contains_blocked_delete(command: &str) -> bool {
        command
            .to_ascii_lowercase()
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .any(|token| {
                matches!(
                    token,
                    "rm" | "del" | "erase" | "rmdir" | "rd" | "remove-item"
                )
            })
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn description(&self) -> &str {
        "Run shell commands inside the workspace with quoting-aware execution and optional working directory control"
    }

    fn when_to_use(&self) -> &str {
        "Use for build, test, run, and repo-native CLI commands after inspecting the relevant files"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for routine file reads, content search, or tiny edits that dedicated tools handle better"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Run focused validation".to_string(),
                input: json!({
                    "command": "cargo test",
                    "workdir": "osagent"
                }),
            },
            ToolExample {
                description: "Use a subdirectory with a timeout override".to_string(),
                input: json!({
                    "command": "npm run build",
                    "workdir": "frontend",
                    "timeout_seconds": 120
                }),
            },
            ToolExample {
                description: "Run bash in explicit read-only mode".to_string(),
                input: json!({
                    "command": "git status",
                    "read_only": true
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional extra arguments appended to the command"
                },
                "workdir": {
                    "type": "string",
                    "description": "Optional relative directory inside the workspace to run the command from"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional timeout override in seconds"
                },
                "read_only": {
                    "type": "boolean",
                    "description": "If true, enforce non-mutating read-only command validation even in writable workspaces"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let command = args["command"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'command' parameter".to_string())
        })?;
        let workdir = args["workdir"].as_str();
        let timeout_seconds = args["timeout_seconds"]
            .as_u64()
            .unwrap_or(self.config.timeout_seconds)
            .clamp(1, 300);
        let explicit_read_only = args["read_only"].as_bool().unwrap_or(false);

        let args_list: Vec<String> = args["args"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|value| value.as_str().map(|value| value.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let full_command = Self::build_command(command, &args_list);
        self.ensure_read_only_safe(&full_command)?;
        if explicit_read_only {
            Self::validate_explicit_read_only(&full_command)?;
        }

        if command_touches_backups(&full_command) {
            return Err(OSAgentError::ToolExecution(
                "Access to backup files and .osagent_backups is blocked".to_string(),
            ));
        }

        if Self::contains_blocked_delete(&full_command) {
            return Err(OSAgentError::ToolExecution(
                "Direct shell deletes are blocked. Use delete_file or apply_patch so OSA can create managed backups first."
                    .to_string(),
            ));
        }

        let command_heads = Self::extract_command_heads(&full_command);
        if command_heads.is_empty() {
            return Err(OSAgentError::ToolExecution("Empty command".to_string()));
        }

        for command_head in &command_heads {
            if !self.is_allowed_command(command_head) {
                return Err(OSAgentError::ToolExecution(format!(
                    "Command '{}' is not in the allowed list",
                    command_head
                )));
            }
        }

        let workspace = self.validate_workdir(workdir)?;
        let timeout_duration = Duration::from_secs(timeout_seconds);
        let full_command_for_exec = full_command.clone();

        let result = tokio::time::timeout(
            timeout_duration,
            tokio::task::spawn_blocking(move || {
                if cfg!(windows) {
                    Command::new("cmd")
                        .args(["/C", &full_command_for_exec])
                        .current_dir(&workspace)
                        .output()
                } else {
                    Command::new("sh")
                        .args(["-lc", &full_command_for_exec])
                        .current_dir(&workspace)
                        .output()
                }
            }),
        )
        .await;

        match result {
            Ok(Ok(output_result)) => {
                let output_result =
                    output_result.map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;

                let stdout = String::from_utf8_lossy(&output_result.stdout).to_string();
                let stderr = String::from_utf8_lossy(&output_result.stderr).to_string();

                if !output_result.status.success() {
                    Ok(maybe_store_large_output(
                        &self.workspace,
                        self.writable,
                        "bash",
                        &format!(
                            "Exit code: {}\nStdout:\n{}\nStderr:\n{}",
                            output_result.status.code().unwrap_or(-1),
                            stdout,
                            stderr
                        ),
                    ))
                } else if stderr.is_empty() {
                    Ok(maybe_store_large_output(
                        &self.workspace,
                        self.writable,
                        "bash",
                        &stdout,
                    ))
                } else {
                    Ok(maybe_store_large_output(
                        &self.workspace,
                        self.writable,
                        "bash",
                        &format!("{}\n{}", stdout, stderr),
                    ))
                }
            }
            Ok(Err(e)) => Err(OSAgentError::ToolExecution(format!(
                "Failed to spawn command: {}",
                e
            ))),
            Err(_) => Err(OSAgentError::Timeout),
        }
    }
}
