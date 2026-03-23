use crate::error::{OSAgentError, Result};
use crate::workflow::artifact_store::ArtifactStore;
use crate::workflow::db::WorkflowDb;
use crate::workflow::events::WorkflowEvent;
use crate::workflow::executor::WorkflowExecutor;
use crate::workflow::graph::{parse_litegraph_json, GraphValidator};
use crate::workflow::types::*;
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
    routing::{delete, get, post, put},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::sync::{Arc, Mutex};
use tokio::sync::broadcast;
use uuid::Uuid;

#[derive(Clone)]
pub struct WorkflowState {
    pub db: Arc<WorkflowDb>,
    pub executor: Arc<WorkflowExecutor>,
    pub artifact_store: Arc<ArtifactStore>,
}

#[derive(Debug, Deserialize)]
pub struct ListQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ApiResponse<T> {
    pub success: bool,
    pub data: Option<T>,
    pub error: Option<String>,
}

impl<T: serde::Serialize> ApiResponse<T> {
    pub fn success(data: T) -> Self {
        Self {
            success: true,
            data: Some(data),
            error: None,
        }
    }

    pub fn error(msg: String) -> Self {
        Self {
            success: false,
            data: None,
            error: Some(msg),
        }
    }
}

async fn create_workflow(
    State(state): State<WorkflowState>,
    Json(req): Json<CreateWorkflowRequest>,
) -> Response {
    let workflow_id = Uuid::new_v4().to_string();
    let workflow = Workflow {
        id: workflow_id.clone(),
        name: req.name,
        description: req.description,
        current_version: 1,
        created_at: Utc::now().to_rfc3339(),
        updated_at: Utc::now().to_rfc3339(),
    };

    if let Err(e) = state.db.create_workflow(&workflow) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response();
    }

    let graph_json = req.graph_json.unwrap_or_else(|| {
        serde_json::json!({
            "nodes": [],
            "edges": []
        })
        .to_string()
    });

    let version = WorkflowVersion {
        id: Uuid::new_v4().to_string(),
        workflow_id: workflow_id.clone(),
        version: 1,
        graph_json,
        created_at: Utc::now().to_rfc3339(),
    };

    if let Err(e) = state.db.create_version(&version) {
        return (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response();
    }

    (StatusCode::CREATED, Json(ApiResponse::success(workflow))).into_response()
}

async fn list_workflows(
    State(state): State<WorkflowState>,
    Query(query): Query<ListQuery>,
) -> Response {
    match state.db.list_workflows() {
        Ok(workflows) => {
            let offset = query.offset.unwrap_or(0);
            let limit = query.limit.unwrap_or(100);
            let workflows: Vec<_> = workflows.into_iter().skip(offset).take(limit).collect();
            Json(ApiResponse::success(workflows)).into_response()
        }
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn get_workflow(State(state): State<WorkflowState>, Path(id): Path<String>) -> Response {
    match state.db.get_workflow(&id) {
        Ok(Some(workflow)) => Json(ApiResponse::success(workflow)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error("Workflow not found".to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn update_workflow(
    State(state): State<WorkflowState>,
    Path(id): Path<String>,
    Json(req): Json<UpdateWorkflowRequest>,
) -> Response {
    match state.db.get_workflow(&id) {
        Ok(Some(workflow)) => {
            let new_version = workflow.current_version + 1;

            if let Err(e) = parse_litegraph_json(&req.graph_json) {
                return (
                    StatusCode::BAD_REQUEST,
                    Json(ApiResponse::<()>::error(format!(
                        "Invalid graph JSON: {}",
                        e
                    ))),
                )
                    .into_response();
            }

            let version = WorkflowVersion {
                id: Uuid::new_v4().to_string(),
                workflow_id: id.clone(),
                version: new_version,
                graph_json: req.graph_json.clone(),
                created_at: Utc::now().to_rfc3339(),
            };

            if let Err(e) = state.db.create_version(&version) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response();
            }

            if let Err(e) = state.db.update_workflow(&id, new_version) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response();
            }

            let _ = state.artifact_store.store_workflow_version(
                &id,
                new_version,
                req.graph_json.as_bytes(),
            );

            match state.db.get_workflow(&id) {
                Ok(Some(updated)) => Json(ApiResponse::success(updated)).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response(),
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(
                        "Failed to fetch updated workflow".to_string(),
                    )),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error("Workflow not found".to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn delete_workflow(State(state): State<WorkflowState>, Path(id): Path<String>) -> Response {
    match state.db.delete_workflow(&id) {
        Ok(_) => Json(ApiResponse::success("Workflow deleted")).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn get_versions(State(state): State<WorkflowState>, Path(id): Path<String>) -> Response {
    match state.db.list_versions(&id) {
        Ok(versions) => Json(ApiResponse::success(versions)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn get_version(
    State(state): State<WorkflowState>,
    Path((id, version)): Path<(String, i32)>,
) -> Response {
    match state.db.get_version(&id, version) {
        Ok(Some(v)) => Json(ApiResponse::success(v)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error("Version not found".to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn rollback_workflow(
    State(state): State<WorkflowState>,
    Path((id, version)): Path<(String, i32)>,
) -> Response {
    match state.db.get_version(&id, version) {
        Ok(Some(_target_version)) => {
            if let Err(e) = state.db.update_workflow(&id, version) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response();
            }

            match state.db.get_workflow(&id) {
                Ok(Some(workflow)) => Json(ApiResponse::success(workflow)).into_response(),
                Err(e) => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(e.to_string())),
                )
                    .into_response(),
                _ => (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ApiResponse::<()>::error(
                        "Failed to fetch workflow".to_string(),
                    )),
                )
                    .into_response(),
            }
        }
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error("Version not found".to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn execute_workflow(
    State(state): State<WorkflowState>,
    Path(id): Path<String>,
    Json(req): Json<ExecuteWorkflowRequest>,
) -> Response {
    match state.db.get_workflow(&id) {
        Ok(Some(workflow)) => match state.db.get_version(&id, workflow.current_version) {
            Ok(Some(version)) => {
                let result = state
                    .executor
                    .execute_workflow(
                        &id,
                        &version.graph_json,
                        workflow.current_version,
                        req.initial_context,
                        req.parameters,
                        req.parent_session_id,
                    )
                    .await;

                match result {
                    Ok(run_result) => Json(ApiResponse::success(run_result)).into_response(),
                    Err(e) => (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(ApiResponse::<()>::error(e.to_string())),
                    )
                        .into_response(),
                }
            }
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(ApiResponse::<()>::error(
                    "Current version not found".to_string(),
                )),
            )
                .into_response(),
            Err(e) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ApiResponse::<()>::error(e.to_string())),
            )
                .into_response(),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error("Workflow not found".to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn list_runs(State(state): State<WorkflowState>, Path(id): Path<String>) -> Response {
    match state.db.list_runs(&id) {
        Ok(runs) => Json(ApiResponse::success(runs)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn get_run(
    State(state): State<WorkflowState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Response {
    match state.db.get_run(&run_id) {
        Ok(Some(run)) => Json(ApiResponse::success(run)).into_response(),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(ApiResponse::<()>::error("Run not found".to_string())),
        )
            .into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn cancel_run(
    State(state): State<WorkflowState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Response {
    match state.executor.cancel_run(&run_id).await {
        Ok(_) => Json(ApiResponse::success("Run cancelled")).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

async fn get_run_logs(
    State(state): State<WorkflowState>,
    Path((_id, run_id)): Path<(String, String)>,
) -> Response {
    match state.db.get_node_logs(&run_id) {
        Ok(logs) => Json(ApiResponse::success(logs)).into_response(),
        Err(e) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ApiResponse::<()>::error(e.to_string())),
        )
            .into_response(),
    }
}

pub fn create_workflow_router(state: WorkflowState) -> Router {
    Router::new()
        .route("/api/workflows", post(create_workflow).get(list_workflows))
        .route(
            "/api/workflows/:id",
            get(get_workflow)
                .put(update_workflow)
                .delete(delete_workflow),
        )
        .route("/api/workflows/:id/versions", get(get_versions))
        .route("/api/workflows/:id/versions/:version", get(get_version))
        .route(
            "/api/workflows/:id/rollback/:version",
            post(rollback_workflow),
        )
        .route("/api/workflows/:id/execute", post(execute_workflow))
        .route("/api/workflows/:id/runs", get(list_runs))
        .route(
            "/api/workflows/:id/runs/:run_id",
            get(get_run).delete(cancel_run),
        )
        .route("/api/workflows/:id/runs/:run_id/logs", get(get_run_logs))
        .with_state(state)
}
