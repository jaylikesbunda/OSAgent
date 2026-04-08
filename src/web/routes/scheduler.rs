use crate::agent::runtime::AgentRuntime;
use crate::storage::models::ScheduledJob;
use axum::{
    extract::Path,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{delete, get, patch, post},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use std::sync::Arc;

#[derive(Debug, Deserialize)]
pub struct CreateJobRequest {
    pub when: String,
    pub message: String,
    pub job_type: Option<String>,
    pub session_id: Option<String>,
    pub notify_via: Option<Vec<String>>,
    pub discord_channel_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct JobResponse {
    pub id: String,
    pub cron_expr: String,
    pub message: String,
    pub job_type: String,
    pub session_id: Option<String>,
    pub enabled: bool,
    pub created_at: chrono::DateTime<chrono::Utc>,
    pub last_run_at: Option<chrono::DateTime<chrono::Utc>>,
    pub next_run_at: chrono::DateTime<chrono::Utc>,
    pub failure_count: u32,
    pub notify_channels: Vec<String>,
}

impl From<&ScheduledJob> for JobResponse {
    fn from(job: &ScheduledJob) -> Self {
        Self {
            id: job.id.clone(),
            cron_expr: job.cron_expr.clone(),
            message: job.message.clone(),
            job_type: job.job_type.clone(),
            session_id: job.session_id.clone(),
            enabled: job.enabled,
            created_at: job.created_at,
            last_run_at: job.last_run_at,
            next_run_at: job.next_run_at,
            failure_count: job.failure_count,
            notify_channels: job.notify_channels.clone(),
        }
    }
}

pub enum AppError {
    Internal(String),
    BadRequest(String),
    NotFound(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
            AppError::NotFound(msg) => (StatusCode::NOT_FOUND, msg),
        };
        (status, msg).into_response()
    }
}

pub async fn list_jobs(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<Vec<JobResponse>>, AppError> {
    let jobs = runtime
        .scheduler()
        .list_jobs()
        .map_err(|e: crate::error::OSAgentError| AppError::Internal(e.to_string()))?;
    Ok(Json(jobs.iter().map(JobResponse::from).collect()))
}

pub async fn create_job(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Json(req): Json<CreateJobRequest>,
) -> Result<(StatusCode, Json<JobResponse>), AppError> {
    let channels = req.notify_via.unwrap_or_else(|| vec!["web".to_string()]);
    let valid_channels: Vec<String> = channels
        .into_iter()
        .filter(|c| matches!(c.as_str(), "web" | "discord"))
        .collect();
    let job = ScheduledJob::new(
        req.when,
        req.message,
        req.job_type.unwrap_or_else(|| "reminder".to_string()),
        req.session_id,
    )
    .with_channels(if valid_channels.is_empty() {
        vec!["web".to_string()]
    } else {
        valid_channels
    })
    .with_discord_channel_id(req.discord_channel_id);

    let job = runtime
        .scheduler()
        .add_job(job)
        .map_err(|e: crate::error::OSAgentError| AppError::BadRequest(e.to_string()))?;
    Ok((StatusCode::CREATED, Json(JobResponse::from(&job))))
}

pub async fn get_job(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<JobResponse>, AppError> {
    let jobs = runtime
        .scheduler()
        .list_jobs()
        .map_err(|e: crate::error::OSAgentError| AppError::Internal(e.to_string()))?;
    let job = jobs
        .iter()
        .find(|j| j.id == id)
        .ok_or_else(|| AppError::NotFound("Job not found".to_string()))?;
    Ok(Json(JobResponse::from(job)))
}

pub async fn delete_job(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<StatusCode, AppError> {
    runtime
        .scheduler()
        .remove_job(&id)
        .map_err(|e: crate::error::OSAgentError| AppError::Internal(e.to_string()))?;
    Ok(StatusCode::NO_CONTENT)
}

pub async fn toggle_job(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<JobResponse>, AppError> {
    let job = runtime
        .scheduler()
        .toggle_job(&id)
        .map_err(|e: crate::error::OSAgentError| AppError::NotFound(e.to_string()))?;
    Ok(Json(JobResponse::from(&job)))
}

pub fn create_scheduler_router() -> Router {
    Router::new()
        .route("/api/scheduler/jobs", get(list_jobs).post(create_job))
        .route("/api/scheduler/jobs/:id", get(get_job).delete(delete_job))
        .route("/api/scheduler/jobs/:id/toggle", patch(toggle_job))
}
