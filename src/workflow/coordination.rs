use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TaskType {
    Trigger,
    Agent,
    Condition,
    Transform,
    Delay,
    Output,
    Approval,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobCard {
    pub job_id: String,
    pub parent_job_id: Option<String>,
    pub task: String,
    pub task_type: TaskType,
    pub input: JobInput,
    pub output_schema: OutputSchema,
    pub timeout_seconds: u64,
    pub capabilities: Vec<Capability>,
    pub created_at: String,
    pub metadata: HashMap<String, serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobInput {
    pub data: serde_json::Value,
    pub source_agent_id: Option<String>,
    pub source_job_id: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputSchema {
    pub required_fields: Vec<String>,
    pub optional_fields: Vec<String>,
    pub result_type: ResultType,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ResultType {
    Boolean,
    String,
    Object,
    Array,
    Number,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct JobResult {
    pub job_id: String,
    pub status: JobStatus,
    pub output: Option<serde_json::Value>,
    pub error: Option<String>,
    pub verification: Option<OutputVerification>,
    pub completed_at: Option<String>,
    pub token_usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum JobStatus {
    Pending,
    Running,
    Completed,
    Failed,
    Cancelled,
    Verified,
    Escalated,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OutputVerification {
    pub verified: bool,
    pub schema_valid: bool,
    pub required_fields_present: Vec<String>,
    pub required_fields_missing: Vec<String>,
    pub warnings: Vec<String>,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TokenUsage {
    pub input_tokens: u64,
    pub output_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub cap_type: CapabilityType,
    pub resource: Option<String>,
    pub permissions: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CapabilityType {
    ReadOnly,
    FileRead,
    FileWrite,
    FileDelete,
    Bash,
    WebFetch,
    WebSearch,
    SpawnAgent,
    SendMessage,
    ReceiveMessage,
    AccessContext,
}

impl Capability {
    pub fn read_only() -> Self {
        Self {
            cap_type: CapabilityType::ReadOnly,
            resource: None,
            permissions: vec!["read".to_string()],
        }
    }

    pub fn read_write(path: &str) -> Self {
        Self {
            cap_type: CapabilityType::FileWrite,
            resource: Some(path.to_string()),
            permissions: vec!["read".to_string(), "write".to_string()],
        }
    }

    pub fn bash(allowed_commands: Vec<String>) -> Self {
        Self {
            cap_type: CapabilityType::Bash,
            resource: None,
            permissions: allowed_commands,
        }
    }

    pub fn web() -> Self {
        Self {
            cap_type: CapabilityType::WebFetch,
            resource: None,
            permissions: vec!["fetch".to_string(), "search".to_string()],
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatedContext {
    pub context_id: String,
    pub version: u64,
    pub parent_context_id: Option<String>,
    pub jobs: Vec<JobCard>,
    pub results: HashMap<String, JobResult>,
    pub shared_state: serde_json::Value,
    pub messages: Vec<StructuredMessage>,
    pub created_at: String,
    pub updated_at: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StructuredMessage {
    pub message_id: String,
    pub from_job_id: String,
    pub to_job_id: Option<String>,
    pub message_type: MessageType,
    pub content: serde_json::Value,
    pub timestamp: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MessageType {
    TaskAssigned,
    TaskCompleted,
    TaskFailed,
    ResultShared,
    RequestApproval,
    ApprovalGranted,
    ApprovalDenied,
    Timeout,
    Cancel,
    Heartbeat,
}

impl CoordinatedContext {
    pub fn new(parent_id: Option<String>) -> Self {
        let now = chrono::Utc::now().to_rfc3339();
        Self {
            context_id: uuid::Uuid::new_v4().to_string(),
            version: 1,
            parent_context_id: parent_id,
            jobs: Vec::new(),
            results: HashMap::new(),
            shared_state: serde_json::json!({}),
            messages: Vec::new(),
            created_at: now.clone(),
            updated_at: now,
        }
    }

    pub fn add_job(&mut self, job: JobCard) {
        self.jobs.push(job);
        self.version += 1;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn add_result(&mut self, result: JobResult) {
        self.results.insert(result.job_id.clone(), result);
        self.version += 1;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }

    pub fn verify_output(&self, job_id: &str, output: &serde_json::Value) -> OutputVerification {
        let job = self.jobs.iter().find(|j| j.job_id == job_id);

        let mut verification = OutputVerification {
            verified: false,
            schema_valid: true,
            required_fields_present: Vec::new(),
            required_fields_missing: Vec::new(),
            warnings: Vec::new(),
            errors: Vec::new(),
        };

        let Some(job) = job else {
            verification.schema_valid = false;
            verification.errors.push("Job not found".to_string());
            return verification;
        };

        let obj = match output.as_object() {
            Some(o) => o,
            None => {
                verification.schema_valid = false;
                verification
                    .errors
                    .push("Output is not an object".to_string());
                return verification;
            }
        };

        for field in &job.output_schema.required_fields {
            if obj.contains_key(field) {
                verification.required_fields_present.push(field.clone());
            } else {
                verification.required_fields_missing.push(field.clone());
            }
        }

        verification.schema_valid = verification.required_fields_missing.is_empty();
        verification.verified = verification.schema_valid;

        if !verification.required_fields_missing.is_empty() {
            verification.errors.push(format!(
                "Missing required fields: {}",
                verification.required_fields_missing.join(", ")
            ));
        }

        verification
    }

    pub fn get_job_result(&self, job_id: &str) -> Option<&JobResult> {
        self.results.get(job_id)
    }

    pub fn get_all_completed_results(&self) -> Vec<&JobResult> {
        self.results
            .values()
            .filter(|r| r.status == JobStatus::Completed || r.status == JobStatus::Verified)
            .collect()
    }

    pub fn send_message(
        &mut self,
        from_job_id: &str,
        to_job_id: Option<&str>,
        msg_type: MessageType,
        content: serde_json::Value,
    ) {
        self.messages.push(StructuredMessage {
            message_id: uuid::Uuid::new_v4().to_string(),
            from_job_id: from_job_id.to_string(),
            to_job_id: to_job_id.map(|s| s.to_string()),
            message_type: msg_type,
            content,
            timestamp: chrono::Utc::now().to_rfc3339(),
        });
        self.version += 1;
        self.updated_at = chrono::Utc::now().to_rfc3339();
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinationConfig {
    pub max_parallel_jobs: usize,
    pub default_timeout_seconds: u64,
    pub retry_on_failure: bool,
    pub max_retries: u32,
    pub escalation_policy: EscalationPolicy,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum EscalationPolicy {
    NotifyParent,
    CancelSiblings,
    RunFallbackAgent,
    HaltWorkflow,
}

impl Default for CoordinationConfig {
    fn default() -> Self {
        Self {
            max_parallel_jobs: 5,
            default_timeout_seconds: 300,
            retry_on_failure: true,
            max_retries: 2,
            escalation_policy: EscalationPolicy::NotifyParent,
        }
    }
}
