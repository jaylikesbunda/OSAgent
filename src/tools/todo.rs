use crate::error::{OSAgentError, Result};
use crate::storage::SqliteStorage;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub status: TodoStatus,
    pub priority: TodoPriority,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoStatus {
    Pending,
    InProgress,
    Completed,
    Cancelled,
}

impl TodoStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoStatus::Pending => "pending",
            TodoStatus::InProgress => "in_progress",
            TodoStatus::Completed => "completed",
            TodoStatus::Cancelled => "cancelled",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "pending" => Some(TodoStatus::Pending),
            "in_progress" => Some(TodoStatus::InProgress),
            "completed" => Some(TodoStatus::Completed),
            "cancelled" => Some(TodoStatus::Cancelled),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum TodoPriority {
    High,
    Medium,
    Low,
}

impl TodoPriority {
    pub fn as_str(&self) -> &'static str {
        match self {
            TodoPriority::High => "high",
            TodoPriority::Medium => "medium",
            TodoPriority::Low => "low",
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s {
            "high" => Some(TodoPriority::High),
            "medium" => Some(TodoPriority::Medium),
            "low" => Some(TodoPriority::Low),
            _ => None,
        }
    }
}

pub struct TodoWriteTool {
    storage: Arc<SqliteStorage>,
}

impl TodoWriteTool {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl Tool for TodoWriteTool {
    fn name(&self) -> &str {
        "todowrite"
    }

    fn description(&self) -> &str {
        "Create and manage a structured task list for tracking progress on complex multi-step tasks"
    }

    fn when_to_use(&self) -> &str {
        "Use for complex multi-step tasks that benefit from progress tracking"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for simple single-step operations or trivial tasks"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Create task list for feature implementation".to_string(),
                input: json!({
                    "todos": [
                        {"content": "Create database schema", "status": "completed", "priority": "high"},
                        {"content": "Implement API endpoints", "status": "in_progress", "priority": "high"},
                        {"content": "Add unit tests", "status": "pending", "priority": "medium"}
                    ]
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Update task status".to_string(),
                input: json!({
                    "todos": [
                        {"content": "Implement API endpoints", "status": "completed", "priority": "high"}
                    ]
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "todos": {
                    "type": "array",
                    "description": "List of todo items to create or update",
                    "items": {
                        "type": "object",
                        "properties": {
                            "content": {
                                "type": "string",
                                "description": "Brief description of the task"
                            },
                            "status": {
                                "type": "string",
                                "enum": ["pending", "in_progress", "completed", "cancelled"],
                                "description": "Current status of the task"
                            },
                            "priority": {
                                "type": "string",
                                "enum": ["high", "medium", "low"],
                                "description": "Priority level"
                            }
                        },
                        "required": ["content", "status", "priority"]
                    }
                }
            },
            "required": ["todos"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let todos_input = args["todos"]
            .as_array()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'todos' array".to_string()))?;

        let session_id = args["session_id"].as_str().unwrap_or("default").to_string();

        for (position, todo_input) in todos_input.iter().enumerate() {
            let content = todo_input["content"].as_str().ok_or_else(|| {
                OSAgentError::ToolExecution("Missing 'content' field".to_string())
            })?;

            let status_str = todo_input["status"].as_str().unwrap_or("pending");
            let status = TodoStatus::from_str(status_str).ok_or_else(|| {
                OSAgentError::ToolExecution(format!("Invalid status: {}", status_str))
            })?;

            let priority_str = todo_input["priority"].as_str().unwrap_or("medium");
            let priority = TodoPriority::from_str(priority_str).ok_or_else(|| {
                OSAgentError::ToolExecution(format!("Invalid priority: {}", priority_str))
            })?;

            let id = todo_input["id"]
                .as_str()
                .map(|s| s.to_string())
                .unwrap_or_else(|| Uuid::new_v4().to_string());

            self.storage.upsert_todo_item(
                &id,
                &session_id,
                content,
                status,
                priority,
                position as i32,
            )?;
        }

        let items = self.storage.list_todo_items(&session_id)?;
        Ok(format_todo_list(&items))
    }
}

pub struct TodoReadTool {
    storage: Arc<SqliteStorage>,
}

impl TodoReadTool {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }
}

#[async_trait]
impl Tool for TodoReadTool {
    fn name(&self) -> &str {
        "todoread"
    }

    fn description(&self) -> &str {
        "Read the current todo list for a session"
    }

    fn when_to_use(&self) -> &str {
        "Use to retrieve the current state of tasks before or during work"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use if you just updated todos via todowrite"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let session_id = args["session_id"].as_str().unwrap_or("default").to_string();

        let items = self.storage.list_todo_items(&session_id)?;
        Ok(format_todo_list(&items))
    }
}

fn format_todo_list(items: &[TodoItem]) -> String {
    if items.is_empty() {
        return "No tasks in the list.".to_string();
    }

    let mut output = String::new();
    let mut pending: Vec<&TodoItem> = Vec::new();
    let mut in_progress: Vec<&TodoItem> = Vec::new();
    let mut completed: Vec<&TodoItem> = Vec::new();
    let mut cancelled: Vec<&TodoItem> = Vec::new();

    for item in items {
        match item.status {
            TodoStatus::Pending => pending.push(item),
            TodoStatus::InProgress => in_progress.push(item),
            TodoStatus::Completed => completed.push(item),
            TodoStatus::Cancelled => cancelled.push(item),
        }
    }

    if !in_progress.is_empty() {
        output.push_str("In Progress:\n");
        for item in &in_progress {
            output.push_str(&format!(
                "  [{}] {} ({})\n",
                item.status.as_str(),
                item.content,
                item.priority.as_str()
            ));
        }
    }

    if !pending.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("Pending:\n");
        for item in &pending {
            output.push_str(&format!(
                "  [{}] {} ({})\n",
                item.status.as_str(),
                item.content,
                item.priority.as_str()
            ));
        }
    }

    if !completed.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("Completed:\n");
        for item in &completed {
            output.push_str(&format!(
                "  [x] {} ({})\n",
                item.content,
                item.priority.as_str()
            ));
        }
    }

    if !cancelled.is_empty() {
        if !output.is_empty() {
            output.push('\n');
        }
        output.push_str("Cancelled:\n");
        for item in &cancelled {
            output.push_str(&format!(
                "  [-] {} ({})\n",
                item.content,
                item.priority.as_str()
            ));
        }
    }

    output.trim_end().to_string()
}
