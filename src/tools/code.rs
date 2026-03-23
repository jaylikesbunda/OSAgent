use crate::config::{CodeToolConfig, Config};
use crate::error::{OSAgentError, Result};
use crate::tools::guard::command_touches_backups;
use crate::tools::output::maybe_store_large_output;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::fs;
use std::process::Command;
use std::time::Duration;
use tempfile::NamedTempFile;
use tokio::time::timeout;

pub struct CodeInterpreterTool {
    language: String,
    config: CodeToolConfig,
    workspace: String,
    writable: bool,
}

impl CodeInterpreterTool {
    pub fn python(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        Self {
            language: "python".to_string(),
            config: config.tools.code_python,
            workspace: config.agent.workspace,
            writable,
        }
    }

    pub fn node(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        Self {
            language: "node".to_string(),
            config: config.tools.code_node,
            workspace: config.agent.workspace,
            writable,
        }
    }

    pub fn bash(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        Self {
            language: "bash".to_string(),
            config: config.tools.code_bash,
            workspace: config.agent.workspace,
            writable,
        }
    }

    fn get_interpreter(&self) -> Result<&'static str> {
        match self.language.as_str() {
            "python" => {
                if cfg!(windows) {
                    Ok("python")
                } else {
                    Ok("python3")
                }
            }
            "node" => Ok("node"),
            "bash" => {
                if cfg!(windows) {
                    Ok("cmd")
                } else {
                    Ok("bash")
                }
            }
            _ => Err(OSAgentError::ToolExecution(format!(
                "Unsupported language: {}",
                self.language
            ))),
        }
    }

    fn ensure_read_only_safe(&self, code: &str) -> Result<()> {
        if command_touches_backups(code) {
            return Err(OSAgentError::ToolExecution(
                "Access to backup files and .osagent_backups is blocked".to_string(),
            ));
        }

        let lowered = code.to_lowercase();
        let delete_patterns = [
            " rm ",
            "del ",
            "erase ",
            "rmdir ",
            "remove-item",
            "remove_file",
            "remove_dir",
            "fs.unlink",
            "fs.rm",
            "unlink(",
            "os.remove",
            "os.unlink",
            "path.unlink",
        ];
        if delete_patterns
            .iter()
            .any(|pattern| lowered.contains(pattern))
        {
            return Err(OSAgentError::ToolExecution(
                "Direct delete commands are blocked. Use delete_file or apply_patch so OSA can create managed backups first."
                    .to_string(),
            ));
        }

        if self.writable {
            return Ok(());
        }

        let mutating_patterns = [
            "write_file",
            "append_file",
            "remove_file",
            "create_dir",
            "create_dir_all",
            "rename(",
            "fs.write",
            "fs.writefile",
            "fs.promises.writefile",
            "fs.unlink",
            "fs.rm",
            "fs.mkdir",
            "open(",
            "with open(",
            "pathlib.path(",
            "touch(",
            "mkdir ",
            "rm ",
            "del ",
        ];

        if mutating_patterns
            .iter()
            .any(|pattern| lowered.contains(pattern))
        {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; code execution is limited to non-mutating scripts"
                    .to_string(),
            ));
        }

        Ok(())
    }
}

#[async_trait]
impl Tool for CodeInterpreterTool {
    fn name(&self) -> &str {
        match self.language.as_str() {
            "python" => "code_python",
            "node" => "code_node",
            "bash" => "code_bash",
            _ => "code_unknown",
        }
    }

    fn description(&self) -> &str {
        match self.language.as_str() {
            "python" => "Execute Python code in a sandboxed environment",
            "node" => "Execute Node.js/JavaScript code in a sandboxed environment",
            "bash" => "Execute Bash script in a sandboxed environment",
            _ => "Execute code",
        }
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "code": {
                    "type": "string",
                    "description": format!("The {} code to execute", self.language)
                }
            },
            "required": ["code"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if !self.config.enabled {
            return Err(OSAgentError::ToolExecution(format!(
                "{} interpreter is disabled",
                self.language
            )));
        }

        let code = args["code"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'code' parameter".to_string()))?;

        self.ensure_read_only_safe(code)?;

        let interpreter = self.get_interpreter()?;

        let temp_file = NamedTempFile::new().map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to create temp file: {}", e))
        })?;

        let extension = match self.language.as_str() {
            "python" => ".py",
            "node" => ".js",
            "bash" => {
                if cfg!(windows) {
                    ".cmd"
                } else {
                    ".sh"
                }
            }
            _ => ".txt",
        };

        let temp_path = temp_file.path().with_extension(extension.trim_matches('.'));
        fs::write(&temp_path, code)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write code: {}", e)))?;

        let workspace = shellexpand::tilde(&self.workspace).to_string();

        let result = timeout(
            Duration::from_secs(self.config.timeout_seconds),
            tokio::task::spawn_blocking(move || {
                if cfg!(windows) && interpreter == "cmd" {
                    Command::new(interpreter)
                        .arg("/C")
                        .arg(&temp_path)
                        .current_dir(&workspace)
                        .output()
                } else {
                    Command::new(interpreter)
                        .arg(&temp_path)
                        .current_dir(&workspace)
                        .output()
                }
            }),
        )
        .await
        .map_err(|_| OSAgentError::Timeout)?
        .map_err(|e| OSAgentError::ToolExecution(e.to_string()))?
        .map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&result.stdout).to_string();
        let stderr = String::from_utf8_lossy(&result.stderr).to_string();

        let output = if result.status.success() {
            if stderr.is_empty() {
                stdout
            } else {
                format!("{}\n{}", stdout, stderr)
            }
        } else {
            format!(
                "Error (exit {}):\n{}",
                result.status.code().unwrap_or(-1),
                stderr
            )
        };

        let workspace_path =
            std::path::PathBuf::from(shellexpand::tilde(&self.workspace).to_string());
        Ok(maybe_store_large_output(
            &workspace_path,
            self.writable,
            &format!("code_{}", self.language),
            &output,
        ))
    }
}
