use crate::agent::subagent_manager::SubagentManager;
use crate::error::{OSAgentError, Result};
use crate::storage::SqliteStorage;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info;

pub struct SubagentTool {
    storage: Arc<SqliteStorage>,
    subagent_manager: Option<Arc<SubagentManager>>,
}

impl SubagentTool {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self {
            storage,
            subagent_manager: None,
        }
    }

    pub fn with_manager(storage: Arc<SqliteStorage>, manager: Arc<SubagentManager>) -> Self {
        Self {
            storage,
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
        "Launch a specialized subagent for complex multi-step tasks. The subagent runs as a proper standalone agent session, blocks until complete, and returns its result. In your prompt, specify exactly what information the subagent should return in its final message. The subagent will only produce one final response back to you."
    }

    fn when_to_use(&self) -> &str {
        "Use this tool when you need to delegate work to a specialized agent. The subagent will run autonomously with its own tools, and you will receive its final response. You should summarize the result for the user. The result returned by the agent is not visible to the user."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
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
                    "enum": ["general", "explore"]
                },
                "task_id": {
                    "type": "string",
                    "description": "Resume a previous task by its session ID (optional)"
                },
                "session_id": {
                    "type": "string",
                    "description": "Parent session ID (injected automatically)"
                }
            },
            "required": ["description", "prompt", "subagent_type"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
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

        let task_id = args["task_id"].as_str();

        let manager = self.subagent_manager.as_ref().ok_or_else(|| {
            OSAgentError::ToolExecution("Subagent manager not available".to_string())
        })?;

        let subagent_session_id = if let Some(resume_id) = task_id {
            resume_id.to_string()
        } else {
            manager
                .spawn_subagent(
                    session_id.to_string(),
                    description.to_string(),
                    prompt.to_string(),
                    subagent_type.to_string(),
                )
                .await?
        };

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
            "completed" => Ok(format!("{}\n\nsession: {}", result, subagent_session_id)),
            "cancelled" => Ok(format!(
                "Subagent was cancelled.\nsession: {}",
                subagent_session_id
            )),
            "timeout" => Ok(format!(
                "Subagent timed out.\nsession: {}",
                subagent_session_id
            )),
            _ => Ok(format!(
                "Subagent finished with status '{}'.\nResult: {}\nsession: {}",
                status, result, subagent_session_id
            )),
        }
    }
}
