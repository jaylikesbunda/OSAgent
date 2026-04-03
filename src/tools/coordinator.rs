use crate::agent::coordinator::Coordinator;
use crate::error::{OSAgentError, Result};
use crate::storage::SqliteStorage;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;
use tracing::info;

pub struct CoordinatorTool {
    storage: Arc<SqliteStorage>,
    coordinator: Arc<Coordinator>,
}

impl CoordinatorTool {
    pub fn new(storage: Arc<SqliteStorage>, coordinator: Arc<Coordinator>) -> Self {
        Self {
            storage,
            coordinator,
        }
    }
}

#[async_trait]
impl Tool for CoordinatorTool {
    fn name(&self) -> &str {
        "coordinator"
    }

    fn description(&self) -> &str {
        "Launch a coordinator that manages parallel worker agents through research, implementation, and verification phases for complex multi-file tasks"
    }

    fn when_to_use(&self) -> &str {
        "Use for complex multi-file changes that benefit from parallel research, coordinated implementation, and automated verification"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for simple single-file edits, quick lookups, or when a single subagent is sufficient"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Coordinate a complex feature implementation".to_string(),
            input: json!({
                "request": "Implement user authentication with JWT tokens",
                "max_workers": 3
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "request": {
                    "type": "string",
                    "description": "The user's request to coordinate across research, implementation, and verification phases"
                },
                "max_workers": {
                    "type": "integer",
                    "description": "Maximum number of parallel workers per phase (default: 3, max: 5)",
                    "default": 3,
                    "minimum": 1,
                    "maximum": 5
                },
                "session_id": {
                    "type": "string",
                    "description": "Parent session ID (injected automatically)"
                }
            },
            "required": ["request"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let request = args["request"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'request' parameter".to_string())
        })?;

        let max_workers = args["max_workers"].as_u64().unwrap_or(3).min(5) as usize;

        let session_id = args["session_id"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing session_id".to_string()))?;

        info!(
            "CoordinatorTool: starting coordination for session {} with {} max workers",
            session_id, max_workers
        );

        let outcome = self
            .coordinator
            .run(session_id.to_string(), request.to_string(), max_workers)
            .await?;

        info!(
            "CoordinatorTool: completed in {}ms with verdict: {}",
            outcome.total_duration_ms, outcome.verdict
        );

        Ok(outcome.to_summary())
    }
}
