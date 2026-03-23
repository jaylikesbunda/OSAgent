use crate::agent::memory::MemoryStore;
use crate::error::Result;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct RecordMemoryTool {
    store: Arc<MemoryStore>,
}

impl RecordMemoryTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for RecordMemoryTool {
    fn name(&self) -> &str {
        "record_memory"
    }

    fn description(&self) -> &str {
        "Record an important memory about the user, their preferences, goals, or project context that should persist across sessions. Use this when you learn something worth remembering for future conversations — user preferences, key facts, recurring patterns, or important project context. Do NOT record trivial or one-off information."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "title": {
                    "type": "string",
                    "description": "Short descriptive title for the memory (e.g. 'Preferred code style', 'Project: main database is PostgreSQL')"
                },
                "content": {
                    "type": "string",
                    "description": "Full memory content. Be specific and include enough context to be useful in future sessions."
                },
                "tags": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Optional tags for categorization (e.g. ['preference', 'project', 'workflow'])"
                }
            },
            "required": ["title", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if !self.store.is_enabled() {
            return Ok(
                "Memory system is disabled. The user can enable it in Settings > Memory."
                    .to_string(),
            );
        }

        let title = args["title"].as_str().unwrap_or("").to_string();
        let content = args["content"].as_str().unwrap_or("").to_string();
        let tags: Vec<String> = args["tags"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(|s| s.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let entry = self
            .store
            .add(title.clone(), content, tags, "agent".to_string())
            .await?;
        Ok(format!(
            "Memory recorded: '{}' (id: {})",
            entry.title, entry.id
        ))
    }
}
