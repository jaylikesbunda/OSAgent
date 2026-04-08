use crate::error::Result;
use crate::scheduler::Scheduler;
use crate::storage::models::ScheduledJob;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ScheduleTool {
    scheduler: Arc<Scheduler>,
}

impl ScheduleTool {
    pub fn new(scheduler: Arc<Scheduler>) -> Self {
        Self { scheduler }
    }
}

#[async_trait]
impl Tool for ScheduleTool {
    fn name(&self) -> &str {
        "schedule"
    }

    fn description(&self) -> &str {
        "Schedule a reminder, task, or recurring job. Use this to set reminders, schedule tasks for later, or create recurring jobs like daily briefings. Can specify notification channels per job or per-job."
    }

    fn parameters(&self) -> Value {
        json!({
                    "type": "object",
                    "properties": {
                        "when": {
                            "type": "string",
                            "description": "When to fire. Supports: 'in 30m', 'in 2h', 'at 3pm', 'at 14:30', '@hourly', '@daily', '@weekly', cron expressions like '0 9 * * 1-5' (every weekday at 9am)"
        },
                        "message": {
                            "type": "string",
                            "description": "What to do or say when the job fires. For reminders: the reminder text. For run_prompt: the prompt to execute."
                        },
                        "session_id": {
                            "type": "string",
                            "description": "Session ID to deliver the message to. If not provided, a new session will be created for run_prompt jobs."
                        },
                        "job_type": {
                            "type": "string",
                            "enum": ["reminder", "run_prompt", "daily_briefing"],
                            "description": "Type of job. 'reminder' sends a notification, 'run_prompt' executes the message as an agent prompt, 'daily_briefing' generates a summary of recent activity."
        },
                        "notify_via": {
                            "type": "array",
                            "items": { "type": "string", "enum": ["web", "discord"] },
                            "description": "Which channels to notify on. Options: 'web' (browser), 'discord'. Defaults to ['web']. Example: ['web', 'discord'] to notify on all channels."
        }
                    },
                    "required": ["when", "message"]
                })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let when = args["when"].as_str().ok_or_else(|| {
            crate::error::OSAgentError::ToolExecution("Missing 'when' parameter".into())
        })?;

        let message = args["message"].as_str().ok_or_else(|| {
            crate::error::OSAgentError::ToolExecution("Missing 'message' parameter".into())
        })?;

        let session_id = args["session_id"].as_str().map(String::from);
        let job_type = args["job_type"].as_str().unwrap_or("reminder").to_string();

        if !matches!(
            job_type.as_str(),
            "reminder" | "run_prompt" | "daily_briefing"
        ) {
            return Err(crate::error::OSAgentError::ToolExecution(
                "Invalid job_type. Must be 'reminder', 'run_prompt', or 'daily_briefing'".into(),
            ));
        }

        let notify_via: Vec<String> = args["notify_via"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str().map(String::from))
                    .filter(|s| matches!(s.as_str(), "web" | "discord"))
                    .collect()
            })
            .unwrap_or_else(|| vec!["web".to_string()]);

        let job = ScheduledJob::new(when.to_string(), message.to_string(), job_type, session_id)
            .with_channels(notify_via);

        match self.scheduler.add_job(job) {
            Ok(job) => {
                let channels = job.notify_channels.join(", ");
                Ok(format!(
                    "Job scheduled!\n\
                     ID: {}\n\
                     When: {}\n\
                     Type: {}\n\
                     Notify: {}\n\
                     Message: {}",
                    job.id, when, job.job_type, channels, job.message
                ))
            }
            Err(e) => Err(crate::error::OSAgentError::ToolExecution(format!(
                "Failed to schedule job: {}",
                e
            ))),
        }
    }
}
