use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Session {
    pub id: String,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    pub messages: Vec<Message>,
    pub model: String,
    pub provider: String,
    pub metadata: serde_json::Value,
    pub parent_id: Option<String>,
    pub agent_type: String,
    pub task_status: String,
    #[serde(default)]
    pub context_state: Option<SessionContextState>,
}

impl Session {
    pub fn new(model: String, provider: String, name: Option<String>) -> Self {
        let metadata = if let Some(n) = name {
            serde_json::json!({ "name": n })
        } else {
            serde_json::json!({})
        };
        Self {
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            messages: vec![],
            model,
            provider,
            metadata,
            parent_id: None,
            agent_type: "primary".to_string(),
            task_status: "active".to_string(),
            context_state: None,
        }
    }

    pub fn new_subagent(
        parent_id: String,
        model: String,
        provider: String,
        agent_type: String,
    ) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            created_at: Utc::now(),
            updated_at: Utc::now(),
            messages: vec![],
            model,
            provider,
            metadata: serde_json::json!({}),
            parent_id: Some(parent_id),
            agent_type,
            task_status: "running".to_string(),
            context_state: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum QueuedMessageStatus {
    Pending,
    Dispatching,
}

impl QueuedMessageStatus {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Pending => "pending",
            Self::Dispatching => "dispatching",
        }
    }

    pub fn from_str(value: &str) -> Self {
        match value {
            "dispatching" => Self::Dispatching,
            _ => Self::Pending,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueuedMessage {
    pub id: String,
    pub session_id: String,
    pub client_message_id: String,
    pub content: String,
    pub status: QueuedMessageStatus,
    pub position: i64,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
    #[serde(default)]
    pub dispatched_at: Option<DateTime<Utc>>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MessageTokens {
    pub input: usize,
    pub output: usize,
    #[serde(default)]
    pub total: usize,
    #[serde(default)]
    pub cached_read: Option<usize>,
    #[serde(default)]
    pub cached_write: Option<usize>,
    #[serde(default)]
    pub reasoning: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct ToolUsageStats {
    pub tool_name: String,
    pub invocation_count: usize,
    pub estimated_tokens: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CompactionStats {
    pub total_compactions: usize,
    pub total_pruned_messages: usize,
    pub total_compacted_messages: usize,
    pub estimated_tokens_saved: usize,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SessionContextState {
    pub estimated_tokens: usize,
    pub context_window: usize,
    pub budget_tokens: usize,
    pub actual_usage: Option<MessageTokens>,
    #[serde(default)]
    pub tool_usage: Vec<ToolUsageStats>,
    #[serde(default)]
    pub compaction_stats: CompactionStats,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Message {
    pub role: String,
    pub content: String,
    #[serde(default)]
    pub thinking: Option<String>,
    pub timestamp: DateTime<Utc>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub tool_call_id: Option<String>,
    #[serde(default = "default_message_metadata")]
    pub metadata: serde_json::Value,
    #[serde(default)]
    pub tokens: Option<MessageTokens>,
}

fn default_message_metadata() -> serde_json::Value {
    serde_json::json!({})
}

impl Message {
    pub fn system(content: String) -> Self {
        Self {
            role: "system".to_string(),
            content,
            thinking: None,
            timestamp: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            metadata: default_message_metadata(),
            tokens: None,
        }
    }

    pub fn user(content: String) -> Self {
        Self {
            role: "user".to_string(),
            content,
            thinking: None,
            timestamp: Utc::now(),
            tool_calls: None,
            tool_call_id: None,
            metadata: default_message_metadata(),
            tokens: None,
        }
    }

    pub fn synthetic_user(content: String, kind: &str) -> Self {
        let mut message = Self::user(content);
        message.metadata = serde_json::json!({
            "synthetic": true,
            "kind": kind,
        });
        message
    }

    pub fn assistant(content: String, tool_calls: Option<Vec<ToolCall>>) -> Self {
        Self {
            role: "assistant".to_string(),
            content,
            thinking: None,
            timestamp: Utc::now(),
            tool_calls,
            tool_call_id: None,
            metadata: default_message_metadata(),
            tokens: None,
        }
    }

    pub fn synthetic_assistant(content: String, kind: &str) -> Self {
        let mut message = Self::assistant(content, None);
        message.metadata = serde_json::json!({
            "synthetic": true,
            "kind": kind,
        });
        message
    }

    pub fn tool_result(tool_call_id: String, content: String) -> Self {
        Self {
            role: "tool".to_string(),
            content,
            thinking: None,
            timestamp: Utc::now(),
            tool_calls: None,
            tool_call_id: Some(tool_call_id),
            metadata: default_message_metadata(),
            tokens: None,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    pub name: String,
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Checkpoint {
    pub id: String,
    pub session_id: String,
    pub created_at: DateTime<Utc>,
    pub state: Vec<u8>,
    pub tool_name: Option<String>,
    pub tool_input: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AuditEntry {
    pub id: String,
    pub timestamp: DateTime<Utc>,
    pub session_id: String,
    pub tool: String,
    pub input: String,
    pub output: String,
    pub duration_ms: u64,
    pub user: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionEventRecord {
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StoredSessionEvent {
    pub id: String,
    pub session_id: String,
    pub event_type: String,
    pub timestamp: DateTime<Utc>,
    pub data: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshotRecord {
    pub id: String,
    pub snapshot_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub path: String,
    pub existed: bool,
    pub content: Option<Vec<u8>>,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct FileSnapshotSummary {
    pub snapshot_id: String,
    pub session_id: String,
    pub tool_name: String,
    pub created_at: DateTime<Utc>,
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TodoItem {
    pub id: String,
    pub session_id: String,
    pub content: String,
    pub status: String,
    pub priority: String,
    pub position: i32,
    pub created_at: DateTime<Utc>,
    pub updated_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SubagentTask {
    pub id: String,
    pub session_id: String,
    pub parent_session_id: String,
    pub description: String,
    pub prompt: String,
    pub agent_type: String,
    pub status: String,
    pub tool_count: i32,
    pub result: Option<String>,
    pub created_at: DateTime<Utc>,
    pub completed_at: Option<DateTime<Utc>>,
}
