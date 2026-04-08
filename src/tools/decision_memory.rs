use crate::agent::decision_memory::DecisionMemory;
use crate::error::Result;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct RecordDecisionTool {
    store: Arc<DecisionMemory>,
}

impl RecordDecisionTool {
    pub fn new(store: Arc<DecisionMemory>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for RecordDecisionTool {
    fn name(&self) -> &str {
        "record_decision"
    }

    fn description(&self) -> &str {
        "Record a durable approved decision that the agent should always follow in future sessions. Use this when the user states a strong preference, chooses a tooling convention, or the conversation makes a durable rule clear. Only record decisions that should be enforced across sessions — do NOT use for one-off facts or general memory."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "key": {
                    "type": "string",
                    "description": "Short stable name for the decision (e.g. 'indent_style', 'test_command', 'preferred_language')"
                },
                "value": {
                    "type": "string",
                    "description": "The chosen value (e.g. 'tabs', 'cargo test', 'rust')"
                },
                "rationale": {
                    "type": "string",
                    "description": "Optional short reason why this decision was made"
                }
            },
            "required": ["key", "value"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if !self.store.is_enabled() {
            return Ok("Decision memory is disabled. Enable it in Settings > Memory.".to_string());
        }

        let key = args["key"].as_str().unwrap_or("").to_string();
        let value = args["value"].as_str().unwrap_or("").to_string();
        let rationale = args["rationale"].as_str().map(|s| s.to_string());

        if key.is_empty() || value.is_empty() {
            return Ok("Both 'key' and 'value' are required.".to_string());
        }

        let entry = self
            .store
            .upsert_approved(
                key.clone(),
                value,
                rationale,
                "tool".to_string(),
                "agent".to_string(),
            )
            .await?;
        Ok(format!(
            "Decision recorded: '{}' = '{}' (id: {})",
            entry.key, entry.value, entry.id
        ))
    }
}
