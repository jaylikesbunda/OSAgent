use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct BatchTool;

impl BatchTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for BatchTool {
    fn name(&self) -> &str {
        "batch"
    }

    fn description(&self) -> &str {
        "Execute multiple independent tool calls in parallel from one request, including read-only bash commands"
    }

    fn when_to_use(&self) -> &str {
        "Use when multiple reads, searches, web fetches, or explicitly read-only bash commands can run at the same time"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when calls depend on each other or when you need sequential edits based on prior results"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "Run two searches in parallel".to_string(),
            input: json!({
                "tool_calls": [
                    {"tool": "glob", "parameters": {"pattern": "src/**/*.rs"}},
                    {"tool": "bash", "parameters": {"command": "git status", "read_only": true}}
                ]
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "tool_calls": {
                    "type": "array",
                    "minItems": 1,
                    "maxItems": 25,
                    "items": {
                        "type": "object",
                        "properties": {
                            "tool": {"type": "string"},
                            "parameters": {"type": "object"}
                        },
                        "required": ["tool", "parameters"]
                    },
                    "description": "Independent tool calls to execute in parallel"
                }
            },
            "required": ["tool_calls"]
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        Err(OSAgentError::ToolExecution(
            "The batch tool is handled by the OSA runtime and should not be executed directly"
                .to_string(),
        ))
    }
}
