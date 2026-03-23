use crate::agent::persona::list_personas_text;
use crate::error::Result;
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct PersonaTool;

impl Default for PersonaTool {
    fn default() -> Self {
        Self::new()
    }
}

impl PersonaTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for PersonaTool {
    fn name(&self) -> &str {
        "persona"
    }

    fn description(&self) -> &str {
        "Manage assistant persona mode (list, get, set, reset), including custom styles"
    }

    fn when_to_use(&self) -> &str {
        "Use when user asks to change assistant behavior style, mode, or custom persona"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for normal coding steps unless a persona change is requested"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "List available personas".to_string(),
                input: json!({ "action": "list" }),
            },
            ToolExample {
                description: "Set plan persona".to_string(),
                input: json!({ "action": "set", "persona_id": "plan" }),
            },
            ToolExample {
                description: "Set custom persona".to_string(),
                input: json!({ "action": "set", "persona_id": "custom", "roleplay_character": "ship engineer" }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "get", "set", "reset"],
                    "description": "Operation to perform"
                },
                "persona_id": {
                    "type": "string",
                    "description": "Persona identifier for action=set"
                },
                "roleplay_character": {
                    "type": "string",
                    "description": "Optional character description when persona_id=custom"
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        Ok(format!(
            "Persona tool is enabled. Runtime applies persona state per session.\n{}",
            list_personas_text()
        ))
    }
}
