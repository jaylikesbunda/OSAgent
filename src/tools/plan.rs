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
