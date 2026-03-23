use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkflowEvent {
    pub event_type: WorkflowEventType,
    pub run_id: String,
    pub workflow_id: String,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum WorkflowEventType {
    #[serde(rename = "workflow_run_started")]
    WorkflowRunStarted { version: i32 },
    #[serde(rename = "node_started")]
    NodeStarted { node_id: String, node_type: String },
    #[serde(rename = "node_completed")]
    NodeCompleted {
        node_id: String,
        node_type: String,
        output: Option<serde_json::Value>,
    },
    #[serde(rename = "node_failed")]
    NodeFailed {
        node_id: String,
        node_type: String,
        error: String,
    },
    #[serde(rename = "workflow_run_completed")]
    WorkflowRunCompleted {
        status: String,
        result: Option<serde_json::Value>,
    },
    #[serde(rename = "workflow_run_cancelled")]
    WorkflowRunCancelled,
    #[serde(rename = "workflow_run_failed")]
    WorkflowRunFailed { error: String },
}

impl WorkflowEvent {
    pub fn workflow_run_started(run_id: &str, workflow_id: &str, version: i32) -> Self {
        Self {
            event_type: WorkflowEventType::WorkflowRunStarted { version },
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn node_started(run_id: &str, workflow_id: &str, node_id: &str, node_type: &str) -> Self {
        Self {
            event_type: WorkflowEventType::NodeStarted {
                node_id: node_id.to_string(),
                node_type: node_type.to_string(),
            },
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn node_completed(
        run_id: &str,
        workflow_id: &str,
        node_id: &str,
        node_type: &str,
        output: Option<serde_json::Value>,
    ) -> Self {
        Self {
            event_type: WorkflowEventType::NodeCompleted {
                node_id: node_id.to_string(),
                node_type: node_type.to_string(),
                output,
            },
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn node_failed(
        run_id: &str,
        workflow_id: &str,
        node_id: &str,
        node_type: &str,
        error: &str,
    ) -> Self {
        Self {
            event_type: WorkflowEventType::NodeFailed {
                node_id: node_id.to_string(),
                node_type: node_type.to_string(),
                error: error.to_string(),
            },
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn workflow_run_completed(
        run_id: &str,
        workflow_id: &str,
        status: &str,
        result: Option<serde_json::Value>,
    ) -> Self {
        Self {
            event_type: WorkflowEventType::WorkflowRunCompleted {
                status: status.to_string(),
                result,
            },
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn workflow_run_cancelled(run_id: &str, workflow_id: &str) -> Self {
        Self {
            event_type: WorkflowEventType::WorkflowRunCancelled,
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn workflow_run_failed(run_id: &str, workflow_id: &str, error: &str) -> Self {
        Self {
            event_type: WorkflowEventType::WorkflowRunFailed {
                error: error.to_string(),
            },
            run_id: run_id.to_string(),
            workflow_id: workflow_id.to_string(),
            timestamp: chrono::Utc::now().to_rfc3339(),
        }
    }

    pub fn to_json(&self) -> String {
        serde_json::to_string(self).unwrap_or_default()
    }

    pub fn to_sse(&self) -> String {
        format!("data: {}\n\n", self.to_json())
    }
}
