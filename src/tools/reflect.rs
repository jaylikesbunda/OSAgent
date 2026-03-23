use crate::error::Result;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::info;

pub struct ReflectTool;

impl ReflectTool {
    pub fn new() -> Self {
        Self
    }
}

#[async_trait]
impl Tool for ReflectTool {
    fn name(&self) -> &str {
        "reflect"
    }

    fn description(&self) -> &str {
        "Record thoughts, reasoning, and decision-making process for transparency and debugging"
    }

    fn when_to_use(&self) -> &str {
        "Use when making important decisions, planning approach, explaining reasoning, or debugging"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for trivial operations or when no reflection is needed"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Explain decision reasoning".to_string(),
                input: json!({
                    "thought": "I chose bash over code_python because listing files is simpler with ls",
                    "context": "file listing task"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Plan approach".to_string(),
                input: json!({
                    "thought": "I'll break this into 3 steps: 1) read config, 2) validate, 3) apply changes",
                    "context": "configuration update"
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "thought": {
                    "type": "string",
                    "description": "The thought process or reasoning to record"
                },
                "context": {
                    "type": "string",
                    "description": "Optional context about what task this relates to"
                },
                "confidence": {
                    "type": "number",
                    "minimum": 0,
                    "maximum": 1,
                    "description": "Confidence level in the decision (0-1)"
                }
            },
            "required": ["thought"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let thought = args["thought"].as_str().unwrap_or("");

        let context = args["context"].as_str().unwrap_or("");

        let confidence = args["confidence"].as_f64();

        let _log_entry = if !context.is_empty() {
            format!("[{}] {}", context, thought)
        } else {
            thought.to_string()
        };

        info!(
            target: "osagent::reflect",
            thought = %thought,
            context = %context,
            confidence = ?confidence,
            "AI reflection"
        );

        let mut response = "Reflection recorded".to_string();
        if let Some(c) = confidence {
            response.push_str(&format!(" (confidence: {:.0}%)", c * 100.0));
        }

        Ok(response)
    }
}
