pub mod cron_parser;
pub mod executor;

use crate::config::SchedulerConfig;
use crate::error::{OSAgentError, Result};
use crate::scheduler::cron_parser::CronParser;
use crate::scheduler::executor::{JobExecutor, RunPromptRequest};
use crate::storage::models::ScheduledJob;
use crate::storage::SqliteStorage;
use chrono::{DateTime, Duration, Utc};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::{watch, Mutex, Semaphore};
use tracing::{error, info, warn};

pub struct Scheduler {
    storage: Arc<SqliteStorage>,
    executor: JobExecutor,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    shutdown_tx: watch::Sender<bool>,
    running: AtomicBool,
    parser: CronParser,
    config: SchedulerConfig,
    permits: Arc<Semaphore>,
}

impl Scheduler {
    pub fn new(
        storage: Arc<SqliteStorage>,
        event_bus: crate::agent::events::EventBus,
        config: SchedulerConfig,
    ) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        let parser = CronParser::new();
        let executor = JobExecutor::new(event_bus);
        let permits = Arc::new(Semaphore::new(config.max_concurrent.max(1)));
        Self {
            storage,
            executor,
            handle: Mutex::new(None),
            shutdown_tx,
            running: AtomicBool::new(false),
            parser,
            config,
            permits,
        }
    }

    pub fn set_prompt_sender(&mut self, tx: tokio::sync::mpsc::UnboundedSender<RunPromptRequest>) {
        self.executor = self.executor.clone().with_prompt_sender(tx);
    }

    pub async fn start(&self) -> Result<()> {
        if !self.config.enabled {
            info!("Scheduler disabled by configuration");
            return Ok(());
        }
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        match self.storage.recover_running_scheduled_jobs() {
            Ok(count) if count > 0 => {
                warn!("Recovered {} interrupted scheduled job(s) for retry", count)
            }
            Ok(_) => {}
            Err(e) => warn!("Failed to recover interrupted scheduled jobs: {}", e),
        }

        self.running.store(true, Ordering::SeqCst);
        let _ = self.shutdown_tx.send(false);
        info!("Starting scheduler loop");

        let storage = Arc::clone(&self.storage);
        let parser = self.parser.clone();
        let executor = self.executor.clone();
        let permits = Arc::clone(&self.permits);
        let max_retries = self.config.max_retries;
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            // One second keeps normal reminders close to their requested time;
            // the database state transition still makes polling idempotent.
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(1));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    changed = shutdown_rx.changed() => {
                        match changed {
                            Ok(()) if *shutdown_rx.borrow() => {
                                info!("Scheduler shutting down");
                                break;
                            }
                            Ok(()) => {}
                            Err(_) => break,
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = Self::tick(
                            &storage,
                            &parser,
                            &executor,
                            &permits,
                            max_retries,
                        ).await {
                            warn!("Scheduler tick error: {}", e);
                        }
                    }
                }
            }
        });

        *self.handle.lock().await = Some(handle);
        Ok(())
    }

    pub async fn stop(&self) {
        if !self.running.swap(false, Ordering::SeqCst) {
            return;
        }
        let _ = self.shutdown_tx.send(true);

        if let Some(handle) = self.handle.lock().await.take() {
            handle.abort();
        }

        info!("Scheduler stopped");
    }

    async fn tick(
        storage: &Arc<SqliteStorage>,
        parser: &CronParser,
        executor: &JobExecutor,
        permits: &Arc<Semaphore>,
        max_retries: usize,
    ) -> Result<()> {
        let now = Utc::now();
        let jobs = storage.list_enabled_scheduled_jobs()?;

        for job in jobs {
            if job.next_run_at > now {
                continue;
            }

            let next_run = if job.is_one_shot() {
                now
            } else {
                match parser.next_run_after(&job.cron_expr, job.next_run_at) {
                    Some(next) => next,
                    None => {
                        let error = format!("Invalid schedule expression: {}", job.cron_expr);
                        storage.finish_scheduled_job(
                            &job.id,
                            false,
                            now,
                            false,
                            false,
                            Some(error.clone()),
                        )?;
                        executor.emit_failure(&job, &error);
                        if job.notify_channels.iter().any(|channel| channel == "web") {
                            if let Err(notification_error) = storage
                                .insert_scheduled_job_notification(
                                    &job.id,
                                    &job.job_type,
                                    &error,
                                    job.session_id.as_deref(),
                                )
                            {
                                warn!(
                                    "Failed to persist invalid-schedule notification for job {}: {}",
                                    job.id, notification_error
                                );
                            }
                        }
                        continue;
                    }
                }
            };

            let permit = match permits.clone().try_acquire_owned() {
                Ok(permit) => permit,
                Err(_) => continue,
            };

            if !storage.claim_scheduled_job(&job.id, now, next_run)? {
                continue;
            }

            let executor = executor.clone();
            let storage = Arc::clone(storage);
            let parser = parser.clone();
            let schedule_base = job.next_run_at;
            tokio::spawn(async move {
                let _permit = permit;
                info!("Executing scheduled job: {} ({})", job.id, job.job_type);

                let execution = executor.execute(&job).await;
                let attempt_number = job.attempt_count.saturating_add(1);
                let (success, message, session_id) = match execution {
                    Ok(outcome) => (true, outcome.message, outcome.session_id),
                    Err(e) => (false, format!("Job failed: {}", e), job.session_id.clone()),
                };

                let retry = !success && attempt_number as usize <= max_retries;
                let next_run = if success {
                    if job.is_one_shot() {
                        Utc::now()
                    } else {
                        parser
                            .next_run_after(&job.cron_expr, schedule_base)
                            .unwrap_or_else(|| Utc::now() + Duration::hours(1))
                    }
                } else if retry {
                    Utc::now() + retry_delay(attempt_number as usize)
                } else {
                    Utc::now()
                };

                if let Err(e) = storage.finish_scheduled_job(
                    &job.id,
                    success,
                    next_run,
                    success && job.is_one_shot(),
                    retry,
                    if success { None } else { Some(message.clone()) },
                ) {
                    error!(
                        "Failed to persist result for scheduled job {}: {}",
                        job.id, e
                    );
                } else if !success {
                    executor.emit_failure(&job, &message);
                }

                if job.notify_channels.iter().any(|channel| channel == "web") {
                    if let Err(e) = storage.insert_scheduled_job_notification(
                        &job.id,
                        &job.job_type,
                        &message,
                        session_id.as_deref(),
                    ) {
                        warn!(
                            "Failed to persist web notification for job {}: {}",
                            job.id, e
                        );
                    }
                }
            });
        }

        Ok(())
    }

    pub fn add_job(&self, mut job: ScheduledJob) -> Result<ScheduledJob> {
        let next_run = self.parser.next_run(&job.cron_expr).ok_or_else(|| {
            OSAgentError::Config(format!("Invalid schedule expression: {}", job.cron_expr))
        })?;
        job.next_run_at = next_run;
        job.run_state = "scheduled".to_string();
        job.last_error = None;
        job.attempt_count = 0;
        self.storage.create_scheduled_job(&job)?;
        info!(
            "Created scheduled job: {} ({}, {})",
            job.id, job.cron_expr, job.schedule_type
        );
        Ok(job)
    }

    pub fn remove_job(&self, id: &str) -> Result<()> {
        if let Some(job) = self.storage.get_scheduled_job(id)? {
            if job.run_state == "running" {
                return Err(OSAgentError::Config(
                    "Cannot delete a running scheduled job; pause it first".to_string(),
                ));
            }
        }
        self.storage.delete_scheduled_job(id)?;
        info!("Removed scheduled job: {}", id);
        Ok(())
    }

    pub fn list_jobs(&self) -> Result<Vec<ScheduledJob>> {
        self.storage.list_scheduled_jobs()
    }

    pub fn run_now(&self, id: &str) -> Result<ScheduledJob> {
        let mut job = self
            .storage
            .get_scheduled_job(id)?
            .ok_or_else(|| OSAgentError::Config(format!("Job {} not found", id)))?;
        if job.run_state == "running" {
            return Err(OSAgentError::Config(
                "Scheduled job is already running".to_string(),
            ));
        }
        job.enabled = true;
        job.run_state = "scheduled".to_string();
        job.next_run_at = Utc::now();
        job.last_error = None;
        job.attempt_count = 0;
        self.storage.update_scheduled_job(&job)?;
        Ok(job)
    }

    pub fn toggle_job(&self, id: &str) -> Result<ScheduledJob> {
        let mut job = self
            .storage
            .get_scheduled_job(id)?
            .ok_or_else(|| OSAgentError::Config(format!("Job {} not found", id)))?;

        if !job.enabled {
            if job.is_one_shot() && job.run_state == "completed" {
                return Err(OSAgentError::Config(
                    "Completed one-time jobs cannot be resumed; create a new job".to_string(),
                ));
            }
            job.enabled = true;
            job.run_state = "scheduled".to_string();
            job.next_run_at = self.parser.next_run(&job.cron_expr).ok_or_else(|| {
                OSAgentError::Config(format!("Invalid schedule expression: {}", job.cron_expr))
            })?;
            job.last_error = None;
        } else {
            job.enabled = false;
        }

        self.storage.update_scheduled_job(&job)?;
        Ok(job)
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}

fn retry_delay(attempt: usize) -> Duration {
    let seconds = 30_i64.saturating_mul(2_i64.saturating_pow(attempt.min(7) as u32));
    Duration::seconds(seconds.min(3600))
}
