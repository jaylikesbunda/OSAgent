use crate::agent::events::{AgentEvent, EventBus};
use crate::error::{OSAgentError, Result};
use crate::storage::models::ScheduledJob;
use crate::storage::SqliteStorage;
use std::sync::Arc;
use tokio::sync::{mpsc, oneshot};
use tracing::{error, info, warn};

pub struct RunPromptRequest {
    pub session_id: Option<String>,
    pub prompt: String,
    pub response_tx: Option<oneshot::Sender<String>>,
}

#[derive(Clone)]
pub struct JobExecutor {
    storage: Arc<SqliteStorage>,
    event_bus: EventBus,
    run_prompt_tx: Option<mpsc::UnboundedSender<RunPromptRequest>>,
}

impl JobExecutor {
    pub fn new(storage: Arc<SqliteStorage>, event_bus: EventBus) -> Self {
        Self {
            storage,
            event_bus,
            run_prompt_tx: None,
        }
    }

    pub fn with_prompt_sender(mut self, tx: mpsc::UnboundedSender<RunPromptRequest>) -> Self {
        self.run_prompt_tx = Some(tx);
        self
    }

    pub async fn execute(&self, job: &ScheduledJob) -> Result<()> {
        match job.job_type.as_str() {
            "reminder" | "run_prompt" | "daily_briefing" => self.execute_run_prompt(job).await,
            other => Err(OSAgentError::Config(format!("Unknown job type: {}", other))),
        }
    }

    async fn execute_run_prompt(&self, job: &ScheduledJob) -> Result<()> {
        info!("Executing job: {} (type={})", job.id, job.job_type);

        let message = if let Some(tx) = &self.run_prompt_tx {
            let (response_tx, response_rx) = oneshot::channel();

            if let Err(e) = tx.send(RunPromptRequest {
                session_id: None,
                prompt: job.message.clone(),
                response_tx: Some(response_tx),
            }) {
                error!("Failed to dispatch prompt for job {}: {}", job.id, e);
                return Err(OSAgentError::Config(format!(
                    "Failed to dispatch prompt: {}",
                    e
                )));
            }

            match tokio::time::timeout(std::time::Duration::from_secs(300), response_rx).await {
                Ok(Ok(response)) => response,
                Ok(Err(_)) => {
                    warn!("Response channel closed for job {}", job.id);
                    "Agent did not produce a response.".to_string()
                }
                Err(_) => {
                    warn!("Agent timed out for job {}", job.id);
                    "Agent timed out while processing the prompt.".to_string()
                }
            }
        } else {
            warn!("No prompt sender configured for job {}", job.id);
            "No agent available to process the prompt.".to_string()
        };

        self.event_bus.emit(AgentEvent::ScheduledJobFired {
            session_id: job.session_id.clone(),
            job_id: job.id.clone(),
            job_type: job.job_type.clone(),
            message,
            notify_channels: job.notify_channels.clone(),
            discord_channel_id: job.discord_channel_id(),
            timestamp: std::time::SystemTime::now(),
        });

        Ok(())
    }
}
