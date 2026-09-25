use crate::agent::events::{AgentEvent, EventBus};
use crate::error::{OSAgentError, Result};
use crate::storage::models::ScheduledJob;
use tokio::sync::{mpsc, oneshot};
use tracing::{info, warn};

pub struct RunPromptRequest {
    pub session_id: Option<String>,
    pub prompt: String,
    pub response_tx: Option<oneshot::Sender<std::result::Result<RunPromptResponse, String>>>,
    pub source: Option<String>,
    pub job_id: String,
    pub job_type: String,
    pub run_agent: bool,
}

#[derive(Debug, Clone)]
pub struct RunPromptResponse {
    pub session_id: String,
    pub response: String,
}

#[derive(Debug, Clone)]
pub struct JobExecutionOutcome {
    pub message: String,
    pub session_id: Option<String>,
}

#[derive(Clone)]
pub struct JobExecutor {
    event_bus: EventBus,
    run_prompt_tx: Option<mpsc::UnboundedSender<RunPromptRequest>>,
}

impl JobExecutor {
    pub fn new(event_bus: EventBus) -> Self {
        Self {
            event_bus,
            run_prompt_tx: None,
        }
    }

    pub fn with_prompt_sender(mut self, tx: mpsc::UnboundedSender<RunPromptRequest>) -> Self {
        self.run_prompt_tx = Some(tx);
        self
    }

    pub async fn execute(&self, job: &ScheduledJob) -> Result<JobExecutionOutcome> {
        // Reminders are notifications, not model work. Keeping them out of the
        // agent path makes them reliable even when the provider is unavailable.
        if job.job_type == "reminder" {
            return self.execute_run_prompt(job, false).await;
        }

        match job.job_type.as_str() {
            "run_prompt" | "daily_briefing" => self.execute_run_prompt(job, true).await,
            other => Err(OSAgentError::Config(format!("Unknown job type: {}", other))),
        }
    }

    async fn execute_run_prompt(
        &self,
        job: &ScheduledJob,
        run_agent: bool,
    ) -> Result<JobExecutionOutcome> {
        info!("Executing job: {} (type={})", job.id, job.job_type);

        let tx = self.run_prompt_tx.as_ref().ok_or_else(|| {
            OSAgentError::Config("No agent available to process the prompt.".to_string())
        })?;
        let (response_tx, response_rx) = oneshot::channel();
        let source = job
            .discord_channel_id()
            .is_some()
            .then(|| "discord".to_string());

        tx.send(RunPromptRequest {
            session_id: job.session_id.clone(),
            prompt: job.message.clone(),
            response_tx: Some(response_tx),
            source,
            job_id: job.id.clone(),
            job_type: job.job_type.clone(),
            run_agent,
        })
        .map_err(|e| OSAgentError::Config(format!("Failed to dispatch prompt: {}", e)))?;

        let response =
            match tokio::time::timeout(std::time::Duration::from_secs(300), response_rx).await {
                Ok(Ok(Ok(response))) => response,
                Ok(Ok(Err(error))) => {
                    return Err(OSAgentError::Config(error));
                }
                Ok(Err(_)) => {
                    return Err(OSAgentError::Config(
                        "Agent response channel closed".to_string(),
                    ));
                }
                Err(_) => {
                    return Err(OSAgentError::Timeout);
                }
            };

        let outcome = JobExecutionOutcome {
            message: response.response,
            session_id: Some(response.session_id),
        };
        self.emit_fired(job, &outcome);
        Ok(outcome)
    }

    pub fn emit_failure(&self, job: &ScheduledJob, error_message: &str) {
        let outcome = JobExecutionOutcome {
            message: format!("Scheduled job failed: {}", error_message),
            session_id: job.session_id.clone(),
        };
        warn!("Job {} failed: {}", job.id, error_message);
        self.emit_fired(job, &outcome);
    }

    fn emit_fired(&self, job: &ScheduledJob, outcome: &JobExecutionOutcome) {
        self.event_bus.emit(AgentEvent::ScheduledJobFired {
            // Use the session that actually ran the prompt. This fixes the old
            // case where a newly-created session was not attached to the event.
            session_id: outcome.session_id.clone(),
            sequence: 0,
            job_id: job.id.clone(),
            job_type: job.job_type.clone(),
            message: outcome.message.clone(),
            notify_channels: job.notify_channels.clone(),
            discord_channel_id: job.discord_channel_id(),
            timestamp: std::time::SystemTime::now(),
        });
    }
}
