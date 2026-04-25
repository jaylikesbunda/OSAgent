use crate::error::Result;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct PlanExitTool;

impl Default for PlanExitTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PlanExitTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PlanExitTool {
    fn name(&self) -> &str {
        "plan_exit"
    }

    fn description(&self) -> &str {
        "Exit plan mode and switch to build mode to implement the plan"
    }

    fn when_to_use(&self) -> &str {
        "Use when the plan is complete and you want to start implementing"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use while still gathering information, exploring the codebase, or when the plan has unresolved questions"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Exit plan mode to begin implementation".to_string(),
            input: json!({}),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {},
            "required": []
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        Ok("Plan mode exited. Ready to switch to build mode for implementation.".to_string())
    }
}
