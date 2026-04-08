use serde::{Deserialize, Serialize};
use std::time::SystemTime;

use crate::tools::question::Question;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct EventTokenUsage {
    pub input: usize,
    pub output: usize,
    pub total: usize,
    #[serde(default)]
    pub cached_read: Option<usize>,
    #[serde(default)]
    pub cached_write: Option<usize>,
    #[serde(default)]
    pub reasoning: Option<usize>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum AgentEvent {
    Thinking {
        session_id: String,
        message: String,
        timestamp: SystemTime,
    },
    ToolStart {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        message_index: i32,
        timestamp: SystemTime,
    },
    ToolProgress {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        status: ToolStatus,
        message: Option<String>,
        progress_percent: Option<u8>,
        timestamp: SystemTime,
    },
    ToolComplete {
        session_id: String,
        tool_call_id: String,
        tool_name: String,
        success: bool,
        output: String,
        #[serde(default)]
        title: Option<String>,
        #[serde(default)]
        metadata: Option<serde_json::Value>,
        duration_ms: u64,
        timestamp: SystemTime,
    },
    ResponseStart {
        session_id: String,
        timestamp: SystemTime,
    },
    ResponseChunk {
        session_id: String,
        content: String,
        timestamp: SystemTime,
    },
    ResponseComplete {
        session_id: String,
        timestamp: SystemTime,
        #[serde(default)]
        usage: Option<EventTokenUsage>,
    },
    QueuedMessageDispatched {
        session_id: String,
        queue_entry_id: String,
        client_message_id: String,
        content: String,
        timestamp: SystemTime,
    },
    ContextUpdate {
        session_id: String,
        context_window: usize,
        estimated_tokens: usize,
        budget_tokens: usize,
        condensed: bool,
        #[serde(default)]
        actual_usage: Option<EventTokenUsage>,
        timestamp: SystemTime,
    },
    Reasoning {
        session_id: String,
        summary: String,
        timestamp: SystemTime,
    },
    ThinkingStart {
        session_id: String,
        timestamp: SystemTime,
    },
    ThinkingDelta {
        session_id: String,
        content: String,
        timestamp: SystemTime,
    },
    ThinkingEnd {
        session_id: String,
        timestamp: SystemTime,
    },
    Retry {
        session_id: String,
        scope: String,
        attempt_count: u32,
        reason: String,
        timestamp: SystemTime,
    },
    Compaction {
        session_id: String,
        pruned_messages: usize,
        compacted_messages: usize,
        replayed: bool,
        timestamp: SystemTime,
    },
    StepFinish {
        session_id: String,
        iteration: usize,
        tool_success_count: usize,
        tool_failure_count: usize,
        finish_reason: String,
        timestamp: SystemTime,
    },
    Error {
        session_id: String,
        error: String,
        recoverable: bool,
        timestamp: SystemTime,
    },
    Cancelled {
        session_id: String,
        timestamp: SystemTime,
    },
    LoopDetected {
        session_id: String,
        detector: String,
        level: String,
        count: usize,
        message: String,
        timestamp: SystemTime,
    },
    ToolResultTruncated {
        session_id: String,
        tool_name: String,
        original_chars: usize,
        truncated_chars: usize,
        head_tail_used: bool,
        timestamp: SystemTime,
    },
    ContextWindowWarning {
        session_id: String,
        tokens: usize,
        source: String,
        should_warn: bool,
        should_block: bool,
        timestamp: SystemTime,
    },
    QuestionAsked {
        session_id: String,
        question_id: String,
        questions: Vec<Question>,
    },
    QuestionResponse {
        question_id: String,
        answers: Vec<Vec<String>>,
    },
    SubagentCreated {
        session_id: String,
        parent_session_id: String,
        subagent_session_id: String,
        description: String,
        prompt: String,
        agent_type: String,
        timestamp: SystemTime,
    },
    SubagentProgress {
        session_id: String,
        parent_session_id: String,
        subagent_session_id: String,
        tool_name: String,
        tool_count: i32,
        status: String,
        timestamp: SystemTime,
    },
    SubagentCompleted {
        session_id: String,
        parent_session_id: String,
        subagent_session_id: String,
        status: String,
        result: String,
        tool_count: i32,
        timestamp: SystemTime,
    },
    CoordinatorPhase {
        session_id: String,
        parent_session_id: String,
        phase: String,
        workers_spawned: usize,
        timestamp: SystemTime,
    },
    ScheduledJobFired {
        session_id: Option<String>,
        job_id: String,
        job_type: String,
        message: String,
        notify_channels: Vec<String>,
        discord_channel_id: Option<u64>,
        timestamp: SystemTime,
    },
}

pub struct QuestionChannel {
    pub question_id: String,
    pub questions: Vec<Question>,
    pub response_tx: tokio::sync::oneshot::Sender<Vec<Vec<String>>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ToolStatus {
    Preparing,
    Executing,
    Processing,
    Finalizing,
}

impl AgentEvent {
    pub fn session_id(&self) -> &str {
        match self {
            AgentEvent::Thinking { session_id, .. } => session_id,
            AgentEvent::ToolStart { session_id, .. } => session_id,
            AgentEvent::ToolProgress { session_id, .. } => session_id,
            AgentEvent::ToolComplete { session_id, .. } => session_id,
            AgentEvent::ResponseStart { session_id, .. } => session_id,
            AgentEvent::ResponseChunk { session_id, .. } => session_id,
            AgentEvent::ResponseComplete { session_id, .. } => session_id,
            AgentEvent::QueuedMessageDispatched { session_id, .. } => session_id,
            AgentEvent::ContextUpdate { session_id, .. } => session_id,
            AgentEvent::Reasoning { session_id, .. } => session_id,
            AgentEvent::ThinkingStart { session_id, .. } => session_id,
            AgentEvent::ThinkingDelta { session_id, .. } => session_id,
            AgentEvent::ThinkingEnd { session_id, .. } => session_id,
            AgentEvent::Retry { session_id, .. } => session_id,
            AgentEvent::Compaction { session_id, .. } => session_id,
            AgentEvent::StepFinish { session_id, .. } => session_id,
            AgentEvent::Error { session_id, .. } => session_id,
            AgentEvent::Cancelled { session_id, .. } => session_id,
            AgentEvent::LoopDetected { session_id, .. } => session_id,
            AgentEvent::ToolResultTruncated { session_id, .. } => session_id,
            AgentEvent::ContextWindowWarning { session_id, .. } => session_id,
            AgentEvent::QuestionAsked { session_id, .. } => session_id,
            AgentEvent::QuestionResponse { .. } => "",
            AgentEvent::SubagentCreated { session_id, .. } => session_id,
            AgentEvent::SubagentProgress { session_id, .. } => session_id,
            AgentEvent::SubagentCompleted { session_id, .. } => session_id,
            AgentEvent::CoordinatorPhase { session_id, .. } => session_id,
            AgentEvent::ScheduledJobFired { session_id, .. } => session_id.as_deref().unwrap_or(""),
        }
    }
}

use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use uuid::Uuid;

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<AgentEvent>,
    pending_questions: Arc<RwLock<HashMap<String, QuestionChannel>>>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1000);
        Self {
            sender,
            pending_questions: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.sender.subscribe()
    }

    pub fn emit(&self, event: AgentEvent) {
        let _ = self.sender.send(event);
    }

    pub async fn register_question(&self, session_id: String, channel: QuestionChannel) {
        let question_id = channel.question_id.clone();
        let questions = channel.questions.clone();
        let mut pending = self.pending_questions.write().await;
        pending.insert(question_id.clone(), channel);
        drop(pending);
        self.emit(AgentEvent::QuestionAsked {
            session_id,
            question_id,
            questions,
        });
    }

    pub async fn answer_question(&self, question_id: &str, answers: Vec<Vec<String>>) -> bool {
        let mut pending = self.pending_questions.write().await;
        if let Some(channel) = pending.remove(question_id) {
            let _ = channel.response_tx.send(answers);
            true
        } else {
            false
        }
    }
}

impl Default for EventBus {
    fn default() -> Self {
        Self::new()
    }
}

pub fn generate_question_id() -> String {
    Uuid::new_v4().to_string()
}
