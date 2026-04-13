use crate::agent::memory::{MemoryCategory, MemoryStore, MemorySuggestionStatus};
use crate::config::LearningMode;
use crate::error::Result;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

fn parse_category(value: Option<&str>) -> Option<MemoryCategory> {
    match value.map(|v| v.trim().to_ascii_lowercase()) {
        Some(v) if v == "user_preference" => Some(MemoryCategory::UserPreference),
        Some(v) if v == "project_context" => Some(MemoryCategory::ProjectContext),
        Some(v) if v == "workflow" => Some(MemoryCategory::Workflow),
        Some(v) if v == "fact" => Some(MemoryCategory::Fact),
        Some(v) if v == "general" => Some(MemoryCategory::General),
        _ => None,
    }
}

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
                },
                "category": {
                    "type": "string",
                    "enum": ["user_preference", "project_context", "workflow", "fact", "general"],
                    "description": "Optional memory category"
                },
                "rationale": {
                    "type": "string",
                    "description": "Optional reason for saving this memory"
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
        let category = parse_category(args["category"].as_str());
        let rationale = args["rationale"].as_str().map(|s| s.to_string());

        if self.store.learning_mode() == LearningMode::Review {
            let suggestion = self
                .store
                .suggest(
                    title.clone(),
                    content,
                    tags,
                    category,
                    "tool".to_string(),
                    "agent".to_string(),
                    rationale,
                )
                .await?;
            return Ok(format!(
                "Memory suggestion queued for review: '{}' (id: {})",
                suggestion.title, suggestion.id
            ));
        }

        let entry = self
            .store
            .add(
                title.clone(),
                content,
                tags,
                category,
                true,
                "agent".to_string(),
            )
            .await?;
        Ok(format!(
            "Memory recorded: '{}' (id: {})",
            entry.title, entry.id
        ))
    }
}

pub struct ListMemorySuggestionsTool {
    store: Arc<MemoryStore>,
}

impl ListMemorySuggestionsTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListMemorySuggestionsTool {
    fn name(&self) -> &str {
        "list_memory_suggestions"
    }

    fn description(&self) -> &str {
        "List pending or resolved memory suggestions for manual review"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "status": {
                    "type": "string",
                    "enum": ["pending", "approved", "rejected", "all"],
                    "description": "Filter suggestion status (default: pending)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let status_filter = args["status"]
            .as_str()
            .unwrap_or("pending")
            .to_ascii_lowercase();
        let suggestions = self.store.list_suggestions().await?;

        let filtered = suggestions
            .into_iter()
            .filter(|s| match status_filter.as_str() {
                "all" => true,
                "approved" => s.status == MemorySuggestionStatus::Approved,
                "rejected" => s.status == MemorySuggestionStatus::Rejected,
                _ => s.status == MemorySuggestionStatus::Pending,
            })
            .collect::<Vec<_>>();

        if filtered.is_empty() {
            return Ok("No memory suggestions found for the requested filter.".to_string());
        }

        let mut output = String::from("Memory suggestions:\n");
        for suggestion in filtered {
            output.push_str(&format!(
                "- [{}] {} = {} (id: {})\n",
                serde_json::to_string(&suggestion.status)
                    .unwrap_or_else(|_| "\"pending\"".to_string())
                    .trim_matches('"'),
                suggestion.title,
                suggestion.content,
                suggestion.id
            ));
        }
        Ok(output)
    }
}

pub struct ApproveMemorySuggestionTool {
    store: Arc<MemoryStore>,
}

impl ApproveMemorySuggestionTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ApproveMemorySuggestionTool {
    fn name(&self) -> &str {
        "approve_memory_suggestion"
    }

    fn description(&self) -> &str {
        "Approve a pending memory suggestion and store it as confirmed memory"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Suggestion id to approve"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let id = args["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            return Ok("Missing required field: id".to_string());
        }

        let entry = self
            .store
            .approve_suggestion(id, "agent".to_string())
            .await?;
        Ok(format!(
            "Approved memory suggestion '{}' as memory '{}'",
            id, entry.title
        ))
    }
}

pub struct RejectMemorySuggestionTool {
    store: Arc<MemoryStore>,
}

impl RejectMemorySuggestionTool {
    pub fn new(store: Arc<MemoryStore>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for RejectMemorySuggestionTool {
    fn name(&self) -> &str {
        "reject_memory_suggestion"
    }

    fn description(&self) -> &str {
        "Reject a pending memory suggestion"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "id": {
                    "type": "string",
                    "description": "Suggestion id to reject"
                },
                "reason": {
                    "type": "string",
                    "description": "Optional rejection reason"
                }
            },
            "required": ["id"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let id = args["id"].as_str().unwrap_or("").trim();
        if id.is_empty() {
            return Ok("Missing required field: id".to_string());
        }
        let reason = args["reason"].as_str().map(|s| s.to_string());
        let rejected = self
            .store
            .reject_suggestion(id, "agent".to_string(), reason)
            .await?;
        if !rejected {
            return Ok(format!(
                "No pending memory suggestion found for id '{}'.",
                id
            ));
        }
        Ok(format!("Rejected memory suggestion '{}'.", id))
    }
}
