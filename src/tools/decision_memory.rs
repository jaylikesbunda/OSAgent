use crate::agent::decision_memory::{DecisionMemory, DecisionSuggestionStatus};
use crate::config::LearningMode;
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

        if self.store.learning_mode() == LearningMode::Review {
            let suggestion = self
                .store
                .suggest(
                    key.clone(),
                    value,
                    rationale,
                    "tool".to_string(),
                    "agent".to_string(),
                )
                .await?;
            return Ok(format!(
                "Decision suggestion queued for review: '{}' = '{}' (id: {})",
                suggestion.key, suggestion.value, suggestion.id
            ));
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

pub struct ListDecisionSuggestionsTool {
    store: Arc<DecisionMemory>,
}

impl ListDecisionSuggestionsTool {
    pub fn new(store: Arc<DecisionMemory>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ListDecisionSuggestionsTool {
    fn name(&self) -> &str {
        "list_decision_suggestions"
    }

    fn description(&self) -> &str {
        "List pending or resolved decision suggestions for review"
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
                "approved" => s.status == DecisionSuggestionStatus::Approved,
                "rejected" => s.status == DecisionSuggestionStatus::Rejected,
                _ => s.status == DecisionSuggestionStatus::Pending,
            })
            .collect::<Vec<_>>();

        if filtered.is_empty() {
            return Ok("No decision suggestions found for the requested filter.".to_string());
        }

        let mut output = String::from("Decision suggestions:\n");
        for suggestion in filtered {
            output.push_str(&format!(
                "- [{}] {} = {} (id: {})\n",
                serde_json::to_string(&suggestion.status)
                    .unwrap_or_else(|_| "\"pending\"".to_string())
                    .trim_matches('"'),
                suggestion.key,
                suggestion.value,
                suggestion.id
            ));
        }
        Ok(output)
    }
}

pub struct ApproveDecisionSuggestionTool {
    store: Arc<DecisionMemory>,
}

impl ApproveDecisionSuggestionTool {
    pub fn new(store: Arc<DecisionMemory>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for ApproveDecisionSuggestionTool {
    fn name(&self) -> &str {
        "approve_decision_suggestion"
    }

    fn description(&self) -> &str {
        "Approve a pending decision suggestion and store it as approved decision memory"
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

        let decision = self
            .store
            .approve_suggestion(id, "agent".to_string())
            .await?;
        Ok(format!(
            "Approved decision suggestion '{}' as '{}'='{}'",
            id, decision.key, decision.value
        ))
    }
}

pub struct RejectDecisionSuggestionTool {
    store: Arc<DecisionMemory>,
}

impl RejectDecisionSuggestionTool {
    pub fn new(store: Arc<DecisionMemory>) -> Self {
        Self { store }
    }
}

#[async_trait]
impl Tool for RejectDecisionSuggestionTool {
    fn name(&self) -> &str {
        "reject_decision_suggestion"
    }

    fn description(&self) -> &str {
        "Reject a pending decision suggestion"
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
                "No pending decision suggestion found for id '{}'.",
                id
            ));
        }
        Ok(format!("Rejected decision suggestion '{}'.", id))
    }
}
