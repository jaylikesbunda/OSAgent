use crate::error::{OSAgentError, Result};
use crate::storage::SqliteStorage;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Task {
    pub id: String,
    pub description: String,
    pub status: TaskStatus,
    pub parent_id: Option<String>,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub metadata: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TaskStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl Task {
    pub fn new(description: String, parent_id: Option<String>) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            description,
            status: TaskStatus::Pending,
            parent_id,
            created_at: Utc::now(),
            updated_at: Utc::now(),
            metadata: json!({}),
        }
    }
}

pub struct TaskTool {
    storage: std::sync::Arc<SqliteStorage>,
}

impl TaskTool {
    pub fn new(storage: std::sync::Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    fn humanize_status(status: &TaskStatus) -> &'static str {
        match status {
            TaskStatus::Pending => "pending",
            TaskStatus::InProgress => "in progress",
            TaskStatus::Completed => "completed",
            TaskStatus::Cancelled => "cancelled",
        }
    }
}

#[async_trait]
impl Tool for TaskTool {
    fn name(&self) -> &str {
        "task"
    }

    fn description(&self) -> &str {
        "Manage durable task and subtask records. Prefer todowrite for session progress tracking."
    }

    fn when_to_use(&self) -> &str {
        "Use when you need durable task records or explicit parent/subtask structure"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for ordinary session planning, progress tracking, or delegation; use todowrite or subagent instead"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Create a new task".to_string(),
                input: json!({
                    "action": "create",
                    "description": "Set up React project"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Update task status".to_string(),
                input: json!({
                    "action": "update",
                    "task_id": "abc123",
                    "status": "in_progress"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Break down into subtasks".to_string(),
                input: json!({
                    "action": "breakdown",
                    "task_id": "abc123",
                    "subtasks": [
                        "Create project directory",
                        "Initialize npm",
                        "Install dependencies"
                    ]
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
                    "enum": ["create", "update", "list", "breakdown", "delete"],
                    "description": "Action to perform"
                },
                "description": {
                    "type": "string",
                    "description": "Task description (for create)"
                },
                "task_id": {
                    "type": "string",
                    "description": "Task ID (for update/delete/breakdown)"
                },
                "status": {
                    "type": "string",
                    "enum": ["pending", "in_progress", "completed", "cancelled"],
                    "description": "New status (for update)"
                },
                "subtasks": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "List of subtask descriptions (for breakdown)"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let mut action = args["action"].as_str().unwrap_or("");
        if action.is_empty() {
            let has_task_id = args["task_id"].as_str().is_some();
            let has_status = args["status"].as_str().is_some();
            let has_subtasks = args["subtasks"].as_array().is_some();
            let has_description = args["description"].as_str().is_some();

            if has_description {
                action = "create";
            } else if has_task_id && has_subtasks {
                action = "breakdown";
            } else if has_task_id && has_status {
                action = "update";
            } else if has_task_id {
                action = "delete";
            }
        }

        if action.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Missing 'action' parameter".to_string(),
            ));
        }

        match action {
            "create" => {
                let description = args["description"].as_str().ok_or_else(|| {
                    OSAgentError::ToolExecution("Missing 'description'".to_string())
                })?;

                let parent_id = args["parent_id"].as_str().map(|s| s.to_string());
                let task = self
                    .storage
                    .create_task(description.to_string(), parent_id)?;

                Ok(format!("Created task: {}", task.description))
            }

            "update" => {
                let task_id = args["task_id"]
                    .as_str()
                    .ok_or_else(|| OSAgentError::ToolExecution("Missing 'task_id'".to_string()))?;

                let status_str = args["status"]
                    .as_str()
                    .ok_or_else(|| OSAgentError::ToolExecution("Missing 'status'".to_string()))?;

                let status: TaskStatus = serde_json::from_str(&format!("\"{}\"", status_str))
                    .map_err(|e| OSAgentError::Parse(e.to_string()))?;

                let status_label = Self::humanize_status(&status).to_string();
                self.storage.update_task_status(task_id, status)?;

                Ok(format!("Updated task status to {}.", status_label))
            }

            "list" => {
                let parent_id = args["parent_id"].as_str();
                let tasks = self.storage.list_tasks(parent_id)?;

                let output = tasks
                    .iter()
                    .map(|t| format!("[{}] {}", Self::humanize_status(&t.status), t.description))
                    .collect::<Vec<_>>()
                    .join("\n");

                Ok(if output.is_empty() {
                    "No tasks found".to_string()
                } else {
                    output
                })
            }

            "breakdown" => {
                let task_id = args["task_id"]
                    .as_str()
                    .ok_or_else(|| OSAgentError::ToolExecution("Missing 'task_id'".to_string()))?;

                let subtasks = args["subtasks"]
                    .as_array()
                    .ok_or_else(|| OSAgentError::ToolExecution("Missing 'subtasks'".to_string()))?;

                let descriptions: Vec<String> = subtasks
                    .iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect();

                let created = self.storage.create_subtasks(task_id, descriptions)?;

                let created_list: Vec<String> = created
                    .iter()
                    .map(|t| format!("- {}", t.description))
                    .collect();

                Ok(format!(
                    "Created {} subtasks:\n{}",
                    created.len(),
                    created_list.join("\n")
                ))
            }

            "delete" => {
                let task_id = args["task_id"]
                    .as_str()
                    .ok_or_else(|| OSAgentError::ToolExecution("Missing 'task_id'".to_string()))?;

                self.storage.delete_task(task_id)?;

                Ok("Deleted task.".to_string())
            }

            _ => Err(OSAgentError::ToolExecution(format!(
                "Unknown action: {}",
                action
            ))),
        }
    }
}
