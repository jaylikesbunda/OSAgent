use crate::agent::subagent_manager::SubagentManager;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info;

#[derive(Default)]
pub struct SubagentTool {
    subagent_manager: Option<Arc<SubagentManager>>,
}

impl SubagentTool {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_manager(manager: Arc<SubagentManager>) -> Self {
        Self {
            subagent_manager: Some(manager),
        }
    }
}

#[async_trait]
impl Tool for SubagentTool {
    fn name(&self) -> &str {
        "subagent"
    }

    fn description(&self) -> &str {
        "Manage a specialized subagent for complex multi-step tasks. action=run launches a new standalone session; action=status checks a prior task_id without changing it; action=resume continues a prior task_id with its full history. For run/resume, specify exactly what information the subagent should return. Set background=true to launch or resume asynchronously; completion is reported automatically."
    }

    fn when_to_use(&self) -> &str {
        "Use this tool when you need to delegate work to a specialized agent. The subagent will run autonomously with its own tools, and you will receive its final response. You should summarize the result for the user. The result returned by the agent is not visible to the user."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for trivial single-step operations (use read_file, grep, or bash directly). Do not use for simple todo tracking (use todowrite). Do not nest subagent calls inside subagents."
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Explore codebase structure".to_string(),
                input: json!({
                    "description": "Explore project layout",
                    "prompt": "Find all API endpoint definitions and report their paths and handlers. Return file paths and line numbers.",
                    "subagent_type": "explore"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "General research task".to_string(),
                input: json!({
                    "description": "Research error handling",
                    "prompt": "Search the codebase for all error types defined. For each, note the file, line number, and what scenarios trigger it. Return a structured summary.",
                    "subagent_type": "general"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Background research task".to_string(),
                input: json!({
                    "description": "Audit auth flows",
                    "prompt": "Audit all authentication flows in the codebase and report vulnerabilities.",
                    "subagent_type": "explore",
                    "background": true
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Resume a previous task".to_string(),
                input: json!({
                    "action": "resume",
                    "description": "Continue prior audit",
                    "prompt": "Continue the previous audit and focus on token expiry handling.",
                    "subagent_type": "explore",
                    "task_id": "previous-session-id"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Check a background task".to_string(),
                input: json!({
                    "action": "status",
                    "task_id": "previous-session-id"
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["run", "status", "resume"],
                    "description": "run: start a new task (default); status: inspect task_id without changing it; resume: continue task_id with its existing history"
                },
                "description": {
                    "type": "string",
                    "description": "A short (3-5 words) description of the task"
                },
                "prompt": {
                    "type": "string",
                    "description": "The detailed task for the subagent to perform"
                },
                "subagent_type": {
                    "type": "string",
                    "description": "Type of specialized agent",
                    "enum": ["general", "explore", "verify"]
                },
                "background": {
                    "type": "boolean",
                    "description": "Launch asynchronously and return immediately. You will be notified automatically when it completes. Do not sleep, poll for progress, or duplicate the task's work."
                },
                "task_id": {
                    "type": "string",
                    "description": "Resume a previous task by its session ID (optional); continues the same subagent session with its prior messages"
                },
                "session_id": {
                    "type": "string",
                    "description": "Parent session ID (injected automatically)"
                }
            },
            "required": []
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        Ok(self.execute_result(args).await?.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let action = args["action"].as_str().unwrap_or("run");
        let task_id = args["task_id"].as_str();

        let manager = self.subagent_manager.as_ref().ok_or_else(|| {
            OSAgentError::ToolExecution("Subagent manager not available".to_string())
        })?;

        if action == "status" {
            let task_id = task_id.ok_or_else(|| {
                OSAgentError::ToolExecution("Missing task_id for subagent status".to_string())
            })?;
            let Some((task, is_running)) = manager.get_subagent_status(task_id)? else {
                return Ok(ToolResult::failure(format!(
                    "<task id=\"{}\" state=\"unknown\">\n<task_result>Subagent task not found.</task_result>\n</task>",
                    task_id
                )));
            };
            let state = if is_running {
                "running"
            } else {
                task.status.as_str()
            };
            let result = task.result.unwrap_or_else(|| "No result yet.".to_string());
            return Ok(ToolResult::new(format!(
                "<task id=\"{}\" state=\"{}\" tools=\"{}\">\n<task_result>\n{}\n</task_result>\n</task>",
                task_id, state, task.tool_count, result
            )));
        }

        if action != "run" && action != "resume" {
            return Err(OSAgentError::ToolExecution(format!(
                "Unknown subagent action '{}'; use run, status, or resume",
                action
            )));
        }

        let description = args["description"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing description".to_string()))?;
        let prompt = args["prompt"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing prompt".to_string()))?;
        let subagent_type = args["subagent_type"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing subagent_type".to_string()))?;
        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing session_id".to_string()))?;
        let background = args["background"].as_bool().unwrap_or(false);

        let subagent_session_id = if action == "resume" || task_id.is_some() {
            let resume_id = task_id.ok_or_else(|| {
                OSAgentError::ToolExecution("Missing task_id for subagent resume".to_string())
            })?;
            manager
                .resume_subagent(
                    session_id.to_string(),
                    resume_id.to_string(),
                    description.to_string(),
                    prompt.to_string(),
                    subagent_type.to_string(),
                    background,
                )
                .await?
        } else {
            manager
                .spawn_subagent(
                    session_id.to_string(),
                    description.to_string(),
                    prompt.to_string(),
                    subagent_type.to_string(),
                    background,
                )
                .await?
        };

        if background {
            return Ok(ToolResult::new(format!(
                "Subagent launched in background.\nDescription: {}\nSession: {}\nYou will be notified automatically when it finishes. Do NOT poll for its progress or duplicate its work.",
                description, subagent_session_id
            )));
        }

        info!(
            "SubagentTool: waiting for subagent {} to complete...",
            subagent_session_id
        );

        let (status, result, _tool_count) =
            manager.wait_for_subagent(&subagent_session_id, 300).await?;

        info!(
            "SubagentTool: subagent {} finished with status={}",
            subagent_session_id, status
        );

        match status.as_str() {
            "completed" => Ok(ToolResult::new(format!(
                "<task id=\"{}\" state=\"completed\">\n<task_result>\n{}\n</task_result>\n</task>",
                subagent_session_id, result
            ))),
            "partial" => Ok(ToolResult::new(format!(
                "<task id=\"{}\" state=\"partial\">\n<task_result>\n{}\n</task_result>\n</task>",
                subagent_session_id, result
            ))),
            "cancelled" => Ok(ToolResult::failure(format!(
                "<task id=\"{}\" state=\"cancelled\">\n<task_result>\nSubagent was cancelled.\n</task_result>\n</task>",
                subagent_session_id
            ))),
            "timeout" => Ok(ToolResult::retryable(format!(
                "<task id=\"{}\" state=\"timeout\">\n<task_result>\n{}\n\nResume with task_id=\"{}\" when ready.\n</task_result>\n</task>",
                subagent_session_id, result, subagent_session_id
            ))),
            _ => Ok(ToolResult::failure(format!(
                "<task id=\"{}\" state=\"error\">\n<task_result>\nSubagent finished with status '{}'.\nResult: {}\n</task_result>\n</task>",
                subagent_session_id, status, result
            ))),
        }
    }
}
