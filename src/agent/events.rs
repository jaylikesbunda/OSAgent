use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::SystemTime;
use tokio::sync::broadcast;
use tokio::sync::RwLock;
use uuid::Uuid;

use crate::storage::{MessageAttachment, MessageImage, SqliteStorage};
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
        sequence: u64,
        message: String,
        timestamp: SystemTime,
    },
    ToolStart {
        session_id: String,
        sequence: u64,
        tool_call_id: String,
        tool_name: String,
        arguments: serde_json::Value,
        message_index: i32,
        timestamp: SystemTime,
    },
    ToolProgress {
        session_id: String,
        sequence: u64,
        tool_call_id: String,
        tool_name: String,
        status: ToolStatus,
        message: Option<String>,
        progress_percent: Option<u8>,
        timestamp: SystemTime,
    },
    ToolComplete {
        session_id: String,
        sequence: u64,
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
        sequence: u64,
        timestamp: SystemTime,
    },
    ResponseChunk {
        session_id: String,
        sequence: u64,
        content: String,
        timestamp: SystemTime,
    },
    ResponseComplete {
        session_id: String,
        sequence: u64,
        timestamp: SystemTime,
        #[serde(default)]
        usage: Option<EventTokenUsage>,
    },
    QueuedMessageDispatched {
        session_id: String,
        sequence: u64,
        queue_entry_id: String,
        client_message_id: String,
        content: String,
        #[serde(default)]
        images: Vec<MessageImage>,
        #[serde(default)]
        attachments: Vec<MessageAttachment>,
        timestamp: SystemTime,
    },
    ContextUpdate {
        session_id: String,
        sequence: u64,
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
        sequence: u64,
        summary: String,
        timestamp: SystemTime,
    },
    ThinkingStart {
        session_id: String,
        sequence: u64,
        timestamp: SystemTime,
    },
    ThinkingDelta {
        session_id: String,
        sequence: u64,
        content: String,
        timestamp: SystemTime,
    },
    ThinkingEnd {
        session_id: String,
        sequence: u64,
        timestamp: SystemTime,
    },
    Retry {
        session_id: String,
        sequence: u64,
        scope: String,
        attempt_count: u32,
        reason: String,
        timestamp: SystemTime,
    },
    Compaction {
        session_id: String,
        sequence: u64,
        pruned_messages: usize,
        compacted_messages: usize,
        replayed: bool,
        timestamp: SystemTime,
    },
    StepFinish {
        session_id: String,
        sequence: u64,
        iteration: usize,
        tool_success_count: usize,
        tool_failure_count: usize,
        finish_reason: String,
        timestamp: SystemTime,
    },
    Error {
        session_id: String,
        sequence: u64,
        error: String,
        recoverable: bool,
        timestamp: SystemTime,
    },
    Cancelled {
        session_id: String,
        sequence: u64,
        timestamp: SystemTime,
    },
    LoopDetected {
        session_id: String,
        sequence: u64,
        detector: String,
        level: String,
        count: usize,
        message: String,
        timestamp: SystemTime,
    },
    ToolResultTruncated {
        session_id: String,
        sequence: u64,
        tool_name: String,
        original_chars: usize,
        truncated_chars: usize,
        head_tail_used: bool,
        timestamp: SystemTime,
    },
    ContextWindowWarning {
        session_id: String,
        sequence: u64,
        tokens: usize,
        source: String,
        should_warn: bool,
        should_block: bool,
        timestamp: SystemTime,
    },
    QuestionAsked {
        session_id: String,
        sequence: u64,
        question_id: String,
        questions: Vec<Question>,
    },
    QuestionResponse {
        sequence: u64,
        question_id: String,
        answers: Vec<Vec<String>>,
    },
    SubagentCreated {
        session_id: String,
        sequence: u64,
        parent_session_id: String,
        subagent_session_id: String,
        description: String,
        prompt: String,
        agent_type: String,
        timestamp: SystemTime,
    },
    SubagentProgress {
        session_id: String,
        sequence: u64,
        parent_session_id: String,
        subagent_session_id: String,
        tool_name: String,
        tool_count: i32,
        status: String,
        timestamp: SystemTime,
    },
    SubagentCompleted {
        session_id: String,
        sequence: u64,
        parent_session_id: String,
        subagent_session_id: String,
        status: String,
        result: String,
        tool_count: i32,
        timestamp: SystemTime,
    },
    CoordinatorPhase {
        session_id: String,
        sequence: u64,
        parent_session_id: String,
        phase: String,
        workers_spawned: usize,
        timestamp: SystemTime,
    },
    ScheduledJobFired {
        session_id: Option<String>,
        sequence: u64,
        job_id: String,
        job_type: String,
        message: String,
        notify_channels: Vec<String>,
        discord_channel_id: Option<u64>,
        timestamp: SystemTime,
    },
    WorkflowStarted {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        workflow_name: String,
        run_id: String,
        source: Option<String>,
        notify_channels: Vec<String>,
        discord_channel_id: Option<u64>,
        timestamp: SystemTime,
    },
    WorkflowNodeStarted {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        run_id: String,
        node_id: String,
        node_type: String,
        timestamp: SystemTime,
    },
    WorkflowNodeCompleted {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        run_id: String,
        node_id: String,
        node_type: String,
        #[serde(default)]
        output_preview: Option<String>,
        timestamp: SystemTime,
    },
    WorkflowNodeFailed {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        run_id: String,
        node_id: String,
        node_type: String,
        error: String,
        timestamp: SystemTime,
    },
    WorkflowApprovalRequested {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        run_id: String,
        node_id: String,
        question_id: String,
        prompt: String,
        approve_label: String,
        reject_label: String,
        notify_channels: Vec<String>,
        discord_channel_id: Option<u64>,
        timestamp: SystemTime,
    },
    WorkflowCompleted {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        run_id: String,
        #[serde(default)]
        output: Option<serde_json::Value>,
        notify_channels: Vec<String>,
        discord_channel_id: Option<u64>,
        timestamp: SystemTime,
    },
    WorkflowFailed {
        session_id: String,
        sequence: u64,
        workflow_id: String,
        run_id: String,
        error: String,
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
            AgentEvent::WorkflowStarted { session_id, .. } => session_id,
            AgentEvent::WorkflowNodeStarted { session_id, .. } => session_id,
            AgentEvent::WorkflowNodeCompleted { session_id, .. } => session_id,
            AgentEvent::WorkflowNodeFailed { session_id, .. } => session_id,
            AgentEvent::WorkflowApprovalRequested { session_id, .. } => session_id,
            AgentEvent::WorkflowCompleted { session_id, .. } => session_id,
            AgentEvent::WorkflowFailed { session_id, .. } => session_id,
        }
    }

    pub fn sequence(&self) -> u64 {
        match self {
            AgentEvent::Thinking { sequence, .. } => *sequence,
            AgentEvent::ToolStart { sequence, .. } => *sequence,
            AgentEvent::ToolProgress { sequence, .. } => *sequence,
            AgentEvent::ToolComplete { sequence, .. } => *sequence,
            AgentEvent::ResponseStart { sequence, .. } => *sequence,
            AgentEvent::ResponseChunk { sequence, .. } => *sequence,
            AgentEvent::ResponseComplete { sequence, .. } => *sequence,
            AgentEvent::QueuedMessageDispatched { sequence, .. } => *sequence,
            AgentEvent::ContextUpdate { sequence, .. } => *sequence,
            AgentEvent::Reasoning { sequence, .. } => *sequence,
            AgentEvent::ThinkingStart { sequence, .. } => *sequence,
            AgentEvent::ThinkingDelta { sequence, .. } => *sequence,
            AgentEvent::ThinkingEnd { sequence, .. } => *sequence,
            AgentEvent::Retry { sequence, .. } => *sequence,
            AgentEvent::Compaction { sequence, .. } => *sequence,
            AgentEvent::StepFinish { sequence, .. } => *sequence,
            AgentEvent::Error { sequence, .. } => *sequence,
            AgentEvent::Cancelled { sequence, .. } => *sequence,
            AgentEvent::LoopDetected { sequence, .. } => *sequence,
            AgentEvent::ToolResultTruncated { sequence, .. } => *sequence,
            AgentEvent::ContextWindowWarning { sequence, .. } => *sequence,
            AgentEvent::QuestionAsked { sequence, .. } => *sequence,
            AgentEvent::QuestionResponse { sequence, .. } => *sequence,
            AgentEvent::SubagentCreated { sequence, .. } => *sequence,
            AgentEvent::SubagentProgress { sequence, .. } => *sequence,
            AgentEvent::SubagentCompleted { sequence, .. } => *sequence,
            AgentEvent::CoordinatorPhase { sequence, .. } => *sequence,
            AgentEvent::ScheduledJobFired { sequence, .. } => *sequence,
            AgentEvent::WorkflowStarted { sequence, .. } => *sequence,
            AgentEvent::WorkflowNodeStarted { sequence, .. } => *sequence,
            AgentEvent::WorkflowNodeCompleted { sequence, .. } => *sequence,
            AgentEvent::WorkflowNodeFailed { sequence, .. } => *sequence,
            AgentEvent::WorkflowApprovalRequested { sequence, .. } => *sequence,
            AgentEvent::WorkflowCompleted { sequence, .. } => *sequence,
            AgentEvent::WorkflowFailed { sequence, .. } => *sequence,
        }
    }

    pub fn with_sequence(self, value: u64) -> Self {
        match self {
            AgentEvent::Thinking {
                session_id,
                message,
                timestamp,
                ..
            } => AgentEvent::Thinking {
                session_id,
                sequence: value,
                message,
                timestamp,
            },
            AgentEvent::ToolStart {
                session_id,
                tool_call_id,
                tool_name,
                arguments,
                message_index,
                timestamp,
                ..
            } => AgentEvent::ToolStart {
                session_id,
                sequence: value,
                tool_call_id,
                tool_name,
                arguments,
                message_index,
                timestamp,
            },
            AgentEvent::ToolProgress {
                session_id,
                tool_call_id,
                tool_name,
                status,
                message,
                progress_percent,
                timestamp,
                ..
            } => AgentEvent::ToolProgress {
                session_id,
                sequence: value,
                tool_call_id,
                tool_name,
                status,
                message,
                progress_percent,
                timestamp,
            },
            AgentEvent::ToolComplete {
                session_id,
                tool_call_id,
                tool_name,
                success,
                output,
                title,
                metadata,
                duration_ms,
                timestamp,
                ..
            } => AgentEvent::ToolComplete {
                session_id,
                sequence: value,
                tool_call_id,
                tool_name,
                success,
                output,
                title,
                metadata,
                duration_ms,
                timestamp,
            },
            AgentEvent::ResponseStart {
                session_id,
                timestamp,
                ..
            } => AgentEvent::ResponseStart {
                session_id,
                sequence: value,
                timestamp,
            },
            AgentEvent::ResponseChunk {
                session_id,
                content,
                timestamp,
                ..
            } => AgentEvent::ResponseChunk {
                session_id,
                sequence: value,
                content,
                timestamp,
            },
            AgentEvent::ResponseComplete {
                session_id,
                timestamp,
                usage,
                ..
            } => AgentEvent::ResponseComplete {
                session_id,
                sequence: value,
                timestamp,
                usage,
            },
            AgentEvent::QueuedMessageDispatched {
                session_id,
                queue_entry_id,
                client_message_id,
                content,
                images,
                attachments,
                timestamp,
                ..
            } => AgentEvent::QueuedMessageDispatched {
                session_id,
                sequence: value,
                queue_entry_id,
                client_message_id,
                content,
                images,
                attachments,
                timestamp,
            },
            AgentEvent::ContextUpdate {
                session_id,
                context_window,
                estimated_tokens,
                budget_tokens,
                condensed,
                actual_usage,
                timestamp,
                ..
            } => AgentEvent::ContextUpdate {
                session_id,
                sequence: value,
                context_window,
                estimated_tokens,
                budget_tokens,
                condensed,
                actual_usage,
                timestamp,
            },
            AgentEvent::Reasoning {
                session_id,
                summary,
                timestamp,
                ..
            } => AgentEvent::Reasoning {
                session_id,
                sequence: value,
                summary,
                timestamp,
            },
            AgentEvent::ThinkingStart {
                session_id,
                timestamp,
                ..
            } => AgentEvent::ThinkingStart {
                session_id,
                sequence: value,
                timestamp,
            },
            AgentEvent::ThinkingDelta {
                session_id,
                content,
                timestamp,
                ..
            } => AgentEvent::ThinkingDelta {
                session_id,
                sequence: value,
                content,
                timestamp,
            },
            AgentEvent::ThinkingEnd {
                session_id,
                timestamp,
                ..
            } => AgentEvent::ThinkingEnd {
                session_id,
                sequence: value,
                timestamp,
            },
            AgentEvent::Retry {
                session_id,
                scope,
                attempt_count,
                reason,
                timestamp,
                ..
            } => AgentEvent::Retry {
                session_id,
                sequence: value,
                scope,
                attempt_count,
                reason,
                timestamp,
            },
            AgentEvent::Compaction {
                session_id,
                pruned_messages,
                compacted_messages,
                replayed,
                timestamp,
                ..
            } => AgentEvent::Compaction {
                session_id,
                sequence: value,
                pruned_messages,
                compacted_messages,
                replayed,
                timestamp,
            },
            AgentEvent::StepFinish {
                session_id,
                iteration,
                tool_success_count,
                tool_failure_count,
                finish_reason,
                timestamp,
                ..
            } => AgentEvent::StepFinish {
                session_id,
                sequence: value,
                iteration,
                tool_success_count,
                tool_failure_count,
                finish_reason,
                timestamp,
            },
            AgentEvent::Error {
                session_id,
                error,
                recoverable,
                timestamp,
                ..
            } => AgentEvent::Error {
                session_id,
                sequence: value,
                error,
                recoverable,
                timestamp,
            },
            AgentEvent::Cancelled {
                session_id,
                timestamp,
                ..
            } => AgentEvent::Cancelled {
                session_id,
                sequence: value,
                timestamp,
            },
            AgentEvent::LoopDetected {
                session_id,
                detector,
                level,
                count,
                message,
                timestamp,
                ..
            } => AgentEvent::LoopDetected {
                session_id,
                sequence: value,
                detector,
                level,
                count,
                message,
                timestamp,
            },
            AgentEvent::ToolResultTruncated {
                session_id,
                tool_name,
                original_chars,
                truncated_chars,
                head_tail_used,
                timestamp,
                ..
            } => AgentEvent::ToolResultTruncated {
                session_id,
                sequence: value,
                tool_name,
                original_chars,
                truncated_chars,
                head_tail_used,
                timestamp,
            },
            AgentEvent::ContextWindowWarning {
                session_id,
                tokens,
                source,
                should_warn,
                should_block,
                timestamp,
                ..
            } => AgentEvent::ContextWindowWarning {
                session_id,
                sequence: value,
                tokens,
                source,
                should_warn,
                should_block,
                timestamp,
            },
            AgentEvent::QuestionAsked {
                session_id,
                question_id,
                questions,
                ..
            } => AgentEvent::QuestionAsked {
                session_id,
                sequence: value,
                question_id,
                questions,
            },
            AgentEvent::QuestionResponse {
                question_id,
                answers,
                ..
            } => AgentEvent::QuestionResponse {
                sequence: value,
                question_id,
                answers,
            },
            AgentEvent::SubagentCreated {
                session_id,
                parent_session_id,
                subagent_session_id,
                description,
                prompt,
                agent_type,
                timestamp,
                ..
            } => AgentEvent::SubagentCreated {
                session_id,
                sequence: value,
                parent_session_id,
                subagent_session_id,
                description,
                prompt,
                agent_type,
                timestamp,
            },
            AgentEvent::SubagentProgress {
                session_id,
                parent_session_id,
                subagent_session_id,
                tool_name,
                tool_count,
                status,
                timestamp,
                ..
            } => AgentEvent::SubagentProgress {
                session_id,
                sequence: value,
                parent_session_id,
                subagent_session_id,
                tool_name,
                tool_count,
                status,
                timestamp,
            },
            AgentEvent::SubagentCompleted {
                session_id,
                parent_session_id,
                subagent_session_id,
                status,
                result,
                tool_count,
                timestamp,
                ..
            } => AgentEvent::SubagentCompleted {
                session_id,
                sequence: value,
                parent_session_id,
                subagent_session_id,
                status,
                result,
                tool_count,
                timestamp,
            },
            AgentEvent::CoordinatorPhase {
                session_id,
                parent_session_id,
                phase,
                workers_spawned,
                timestamp,
                ..
            } => AgentEvent::CoordinatorPhase {
                session_id,
                sequence: value,
                parent_session_id,
                phase,
                workers_spawned,
                timestamp,
            },
            AgentEvent::ScheduledJobFired {
                session_id,
                job_id,
                job_type,
                message,
                notify_channels,
                discord_channel_id,
                timestamp,
                ..
            } => AgentEvent::ScheduledJobFired {
                session_id,
                sequence: value,
                job_id,
                job_type,
                message,
                notify_channels,
                discord_channel_id,
                timestamp,
            },
            AgentEvent::WorkflowStarted {
                session_id,
                workflow_id,
                workflow_name,
                run_id,
                source,
                notify_channels,
                discord_channel_id,
                timestamp,
                ..
            } => AgentEvent::WorkflowStarted {
                session_id,
                sequence: value,
                workflow_id,
                workflow_name,
                run_id,
                source,
                notify_channels,
                discord_channel_id,
                timestamp,
            },
            AgentEvent::WorkflowNodeStarted {
                session_id,
                workflow_id,
                run_id,
                node_id,
                node_type,
                timestamp,
                ..
            } => AgentEvent::WorkflowNodeStarted {
                session_id,
                sequence: value,
                workflow_id,
                run_id,
                node_id,
                node_type,
                timestamp,
            },
            AgentEvent::WorkflowNodeCompleted {
                session_id,
                workflow_id,
                run_id,
                node_id,
                node_type,
                output_preview,
                timestamp,
                ..
            } => AgentEvent::WorkflowNodeCompleted {
                session_id,
                sequence: value,
                workflow_id,
                run_id,
                node_id,
                node_type,
                output_preview,
                timestamp,
            },
            AgentEvent::WorkflowNodeFailed {
                session_id,
                workflow_id,
                run_id,
                node_id,
                node_type,
                error,
                timestamp,
                ..
            } => AgentEvent::WorkflowNodeFailed {
                session_id,
                sequence: value,
                workflow_id,
                run_id,
                node_id,
                node_type,
                error,
                timestamp,
            },
            AgentEvent::WorkflowApprovalRequested {
                session_id,
                workflow_id,
                run_id,
                node_id,
                question_id,
                prompt,
                approve_label,
                reject_label,
                notify_channels,
                discord_channel_id,
                timestamp,
                ..
            } => AgentEvent::WorkflowApprovalRequested {
                session_id,
                sequence: value,
                workflow_id,
                run_id,
                node_id,
                question_id,
                prompt,
                approve_label,
                reject_label,
                notify_channels,
                discord_channel_id,
                timestamp,
            },
            AgentEvent::WorkflowCompleted {
                session_id,
                workflow_id,
                run_id,
                output,
                notify_channels,
                discord_channel_id,
                timestamp,
                ..
            } => AgentEvent::WorkflowCompleted {
                session_id,
                sequence: value,
                workflow_id,
                run_id,
                output,
                notify_channels,
                discord_channel_id,
                timestamp,
            },
            AgentEvent::WorkflowFailed {
                session_id,
                workflow_id,
                run_id,
                error,
                notify_channels,
                discord_channel_id,
                timestamp,
                ..
            } => AgentEvent::WorkflowFailed {
                session_id,
                sequence: value,
                workflow_id,
                run_id,
                error,
                notify_channels,
                discord_channel_id,
                timestamp,
            },
        }
    }

    pub fn event_type(&self) -> &'static str {
        match self {
            AgentEvent::Thinking { .. } => "thinking",
            AgentEvent::ToolStart { .. } => "tool_start",
            AgentEvent::ToolProgress { .. } => "tool_progress",
            AgentEvent::ToolComplete { .. } => "tool_complete",
            AgentEvent::ResponseStart { .. } => "response_start",
            AgentEvent::ResponseChunk { .. } => "response_chunk",
            AgentEvent::ResponseComplete { .. } => "response_complete",
            AgentEvent::QueuedMessageDispatched { .. } => "queued_message_dispatched",
            AgentEvent::ContextUpdate { .. } => "context_update",
            AgentEvent::Reasoning { .. } => "reasoning",
            AgentEvent::ThinkingStart { .. } => "thinking_start",
            AgentEvent::ThinkingDelta { .. } => "thinking_delta",
            AgentEvent::ThinkingEnd { .. } => "thinking_end",
            AgentEvent::Retry { .. } => "retry",
            AgentEvent::Compaction { .. } => "compaction",
            AgentEvent::StepFinish { .. } => "step_finish",
            AgentEvent::Error { .. } => "error",
            AgentEvent::Cancelled { .. } => "cancelled",
            AgentEvent::LoopDetected { .. } => "loop_detected",
            AgentEvent::ToolResultTruncated { .. } => "tool_result_truncated",
            AgentEvent::ContextWindowWarning { .. } => "context_window_warning",
            AgentEvent::QuestionAsked { .. } => "question_asked",
            AgentEvent::QuestionResponse { .. } => "question_response",
            AgentEvent::SubagentCreated { .. } => "subagent_created",
            AgentEvent::SubagentProgress { .. } => "subagent_progress",
            AgentEvent::SubagentCompleted { .. } => "subagent_completed",
            AgentEvent::CoordinatorPhase { .. } => "coordinator_phase",
            AgentEvent::ScheduledJobFired { .. } => "scheduled_job_fired",
            AgentEvent::WorkflowStarted { .. } => "workflow_started",
            AgentEvent::WorkflowNodeStarted { .. } => "workflow_node_started",
            AgentEvent::WorkflowNodeCompleted { .. } => "workflow_node_completed",
            AgentEvent::WorkflowNodeFailed { .. } => "workflow_node_failed",
            AgentEvent::WorkflowApprovalRequested { .. } => "workflow_approval_requested",
            AgentEvent::WorkflowCompleted { .. } => "workflow_completed",
            AgentEvent::WorkflowFailed { .. } => "workflow_failed",
        }
    }
}

#[derive(Clone)]
pub struct EventBus {
    sender: broadcast::Sender<AgentEvent>,
    pending_questions: Arc<RwLock<HashMap<String, QuestionChannel>>>,
    sequences: Arc<Mutex<HashMap<String, u64>>>,
    storage: Option<Arc<SqliteStorage>>,
}

impl EventBus {
    pub fn new() -> Self {
        let (sender, _) = broadcast::channel(1000);
        Self {
            sender,
            pending_questions: Arc::new(RwLock::new(HashMap::new())),
            sequences: Arc::new(Mutex::new(HashMap::new())),
            storage: None,
        }
    }

    pub fn new_with_storage(storage: Arc<SqliteStorage>) -> Self {
        let mut bus = Self::new();
        bus.storage = Some(storage);
        bus
    }

    pub fn subscribe(&self) -> broadcast::Receiver<AgentEvent> {
        self.sender.subscribe()
    }

    pub fn subscribe_from(
        &self,
        session_id: &str,
        from_sequence: u64,
    ) -> crate::error::Result<(Vec<AgentEvent>, broadcast::Receiver<AgentEvent>)> {
        let receiver = self.sender.subscribe();
        let replay = if let Some(storage) = &self.storage {
            storage
                .list_session_events_from(session_id, from_sequence)?
                .into_iter()
                .filter_map(|record| serde_json::from_value::<AgentEvent>(record.data).ok())
                .collect()
        } else {
            Vec::new()
        };
        Ok((replay, receiver))
    }

    pub fn emit(&self, event: AgentEvent) {
        let session_id = event.session_id().to_string();
        let mut sequenced_event = event;

        if !session_id.is_empty() {
            let next_sequence = {
                let mut guard = self
                    .sequences
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let entry = guard.entry(session_id.clone()).or_insert_with(|| {
                    self.storage
                        .as_ref()
                        .and_then(|storage| storage.latest_session_event_sequence(&session_id).ok())
                        .unwrap_or(0)
                });
                *entry += 1;
                *entry
            };

            sequenced_event = sequenced_event.with_sequence(next_sequence);

            if let Some(storage) = &self.storage {
                let data = serde_json::to_value(&sequenced_event)
                    .unwrap_or_else(|_| serde_json::json!({ "error": "serialize_failed" }));
                let _ = storage.append_session_event_with_sequence(
                    &session_id,
                    sequenced_event.event_type(),
                    data,
                    next_sequence,
                );
            }
        }

        let _ = self.sender.send(sequenced_event);
    }

    pub async fn register_question(&self, session_id: String, channel: QuestionChannel) {
        let question_id = channel.question_id.clone();
        let questions = channel.questions.clone();
        let mut pending = self.pending_questions.write().await;
        pending.insert(question_id.clone(), channel);
        drop(pending);
        self.emit(AgentEvent::QuestionAsked {
            session_id,
            sequence: 0,
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
