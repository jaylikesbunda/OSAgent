pub mod cron_parser;
pub mod executor;

use crate::error::{OSAgentError, Result};
use crate::scheduler::cron_parser::CronParser;
use crate::scheduler::executor::JobExecutor;
use crate::storage::models::ScheduledJob;
use crate::storage::SqliteStorage;
use chrono::Utc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::watch;
use tokio::sync::Mutex;
use tracing::{error, info, warn};

pub struct Scheduler {
    storage: Arc<SqliteStorage>,
    executor: JobExecutor,
    handle: Mutex<Option<tokio::task::JoinHandle<()>>>,
    shutdown_tx: watch::Sender<bool>,
    running: AtomicBool,
    parser: CronParser,
}

impl Scheduler {
    pub fn new(storage: Arc<SqliteStorage>, event_bus: crate::agent::events::EventBus) -> Self {
        let (shutdown_tx, _) = watch::channel(false);
        let parser = CronParser::new();
        let executor = JobExecutor::new(Arc::clone(&storage), event_bus);
        Self {
            storage,
            executor,
            handle: Mutex::new(None),
            shutdown_tx,
            running: AtomicBool::new(false),
            parser,
        }
    }

    pub fn set_prompt_sender(
        &mut self,
        tx: tokio::sync::mpsc::UnboundedSender<executor::RunPromptRequest>,
    ) {
        self.executor = self.executor.clone().with_prompt_sender(tx);
    }

    pub async fn start(&self) -> Result<()> {
        if self.running.load(Ordering::SeqCst) {
            return Ok(());
        }

        self.running.store(true, Ordering::SeqCst);
        info!("Starting scheduler loop");

        let storage = Arc::clone(&self.storage);
        let parser = self.parser.clone();
        let executor = self.executor.clone();
        let mut shutdown_rx = self.shutdown_tx.subscribe();

        let handle = tokio::spawn(async move {
            let mut interval = tokio::time::interval(tokio::time::Duration::from_secs(30));
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            loop {
                tokio::select! {
                    _ = shutdown_rx.changed() => {
                        if *shutdown_rx.borrow() {
                            info!("Scheduler shutting down");
                            break;
                        }
                    }
                    _ = interval.tick() => {
                        if let Err(e) = Self::tick(&storage, &parser, &executor).await {
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
        if !self.running.load(Ordering::SeqCst) {
            return;
        }

        self.running.store(false, Ordering::SeqCst);
        let _ = self.shutdown_tx.send(true);

        if let Some(handle) = self.handle.lock().await.take() {
            handle.abort();
        }

        info!("Scheduler stopped");
    }

    async fn tick(
        storage: &SqliteStorage,
        parser: &CronParser,
        executor: &JobExecutor,
    ) -> Result<()> {
        let jobs = storage.list_enabled_scheduled_jobs()?;
        let now = Utc::now();

        let due_jobs: Vec<ScheduledJob> = jobs
            .into_iter()
            .filter(|job| job.next_run_at <= now)
            .collect();

        for job in due_jobs {
            let is_one_shot = job.cron_expr.trim().starts_with("in ");

            if is_one_shot {
                if let Err(e) =
                    storage.record_scheduled_job_result(&job.id, true, Utc::now().timestamp())
                {
                    warn!("Failed to update one-shot job {}: {}", job.id, e);
                }
                if let Err(e) = storage.disable_scheduled_job(&job.id) {
                    warn!("Failed to disable one-shot job {}: {}", job.id, e);
                }
            } else {
                let next_run = parser
                    .next_run(&job.cron_expr)
                    .unwrap_or_else(|| Utc::now() + chrono::Duration::hours(24));

                if let Err(e) =
                    storage.record_scheduled_job_result(&job.id, true, next_run.timestamp())
                {
                    warn!("Failed to update job next_run for {}: {}", job.id, e);
                    continue;
                }
            }

            let executor = executor.clone();
            let job_id = job.id.clone();

            tokio::spawn(async move {
                info!("Executing scheduled job: {} ({})", job.id, job.job_type);

                if let Err(e) = executor.execute(&job).await {
                    error!("Job {} failed: {}", job_id, e);
                }
            });
        }

        Ok(())
    }

    pub fn add_job(&self, job: ScheduledJob) -> Result<ScheduledJob> {
        let next_run = self.parser.next_run(&job.cron_expr).ok_or_else(|| {
            OSAgentError::Config(format!("Invalid cron expression: {}", job.cron_expr))
        })?;

        let mut job = job;
        job.next_run_at = next_run;

        self.storage.create_scheduled_job(&job)?;
        info!("Created scheduled job: {} ({})", job.id, job.cron_expr);
        Ok(job)
    }

    pub fn remove_job(&self, id: &str) -> Result<()> {
        self.storage.delete_scheduled_job(id)?;
        info!("Removed scheduled job: {}", id);
        Ok(())
    }

    pub fn list_jobs(&self) -> Result<Vec<ScheduledJob>> {
        self.storage.list_scheduled_jobs()
    }

    pub fn toggle_job(&self, id: &str) -> Result<ScheduledJob> {
        let mut job = self
            .storage
            .get_scheduled_job(id)?
            .ok_or_else(|| OSAgentError::Config(format!("Job {} not found", id)))?;

        job.enabled = !job.enabled;
        self.storage.update_scheduled_job(&job)?;
        Ok(job)
    }

    pub fn is_running(&self) -> bool {
        self.running.load(Ordering::SeqCst)
    }
}
