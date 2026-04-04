use crate::agent::memory::MemoryEntry;
use crate::agent::persona::{ActivePersona, PersonaOption};
use crate::agent::runtime::AgentRuntime;
use crate::config::{Config, DiscordConfig, WorkspaceConfig, WorkspacePath, WorkspacePermission};
use crate::external::{PermissionAction, PermissionPrompt};
use crate::oauth::provider::{
    get_oauth_provider, is_device_code_oauth_provider, is_pkce_oauth_provider, OAuthFlowType,
};
use crate::oauth::{generate_oauth_state, generate_pkce_pair, PkcePair};
use crate::plugin::LoadedPlugin;
use crate::skills::SkillService;
use crate::storage::{FileSnapshotSummary, QueuedMessage, Session, StoredSessionEvent};
use crate::web::auth;
use crate::workflow::api::WorkflowState;
use crate::workflow::artifact_store::ArtifactStore;
use crate::workflow::db::WorkflowDb;
use crate::workflow::executor::WorkflowExecutor;
use axum::{
    extract::{Extension, Json, Path, Query},
    http::StatusCode,
    response::{sse::Event, Sse},
    routing::{delete, get, patch, post, put},
    Router,
};
use futures::stream::Stream;
use serde::{Deserialize, Serialize};
use std::convert::Infallible;
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;
use uuid::Uuid;

#[derive(Clone)]
struct PendingPkceSession {
    provider_id: String,
    code_verifier: String,
    redirect_uri: String,
}

fn oauth_pkce_sessions() -> &'static dashmap::DashMap<String, PendingPkceSession> {
    static STORE: OnceLock<dashmap::DashMap<String, PendingPkceSession>> = OnceLock::new();
    STORE.get_or_init(dashmap::DashMap::new)
}

#[derive(Debug, Deserialize)]
pub struct LoginRequest {
    pub password: String,
}

#[derive(Debug, Serialize)]
pub struct LoginResponse {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct AuthStatusResponse {
    pub required: bool,
}

#[derive(Debug, Serialize)]
pub struct ErrorResponse {
    pub error: String,
}

#[derive(Debug, Deserialize)]
pub struct ChangePasswordRequest {
    pub old_password: String,
    pub new_password: String,
}

#[derive(Debug, Serialize)]
pub struct RestartResponse {
    pub restarting: bool,
}

#[derive(Debug, Serialize)]
pub struct DiscordBotStatusResponse {
    pub available: bool,
    pub enabled: bool,
    pub configured: bool,
    pub running: bool,
}

#[derive(Debug, Serialize)]
pub struct DiscordBotActionResponse {
    pub running: bool,
    pub message: String,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct CreateSessionRequest {
    pub model: Option<String>,
    pub provider: Option<String>,
    pub workspace_id: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Deserialize)]
pub struct SendMessageRequest {
    pub message: String,
    pub session_id: String,
    pub client_message_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct SendMessageResponse {
    pub accepted: bool,
    pub session_id: String,
    pub status: String,
    pub queued: bool,
    pub queue_position: Option<i64>,
    pub queue_item: Option<QueuedMessage>,
}

#[derive(Debug, Deserialize)]
pub struct RollbackRequest {
    pub checkpoint_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SnapshotRevertRequest {
    pub snapshot_id: String,
}

#[derive(Debug, Deserialize)]
pub struct AuditQuery {
    pub limit: Option<usize>,
    pub offset: Option<usize>,
}

#[derive(Debug, Serialize)]
pub struct ModelResponse {
    pub model: String,
    pub provider_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SetModelRequest {
    pub model: String,
    pub provider_id: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct MemoryListResponse {
    pub enabled: bool,
    pub file_path: String,
    pub memories: Vec<MemoryEntry>,
}

#[derive(Debug, Deserialize)]
pub struct AddMemoryRequest {
    pub title: String,
    pub content: String,
    #[serde(default)]
    pub tags: Vec<String>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateMemoryRequest {
    pub title: Option<String>,
    pub content: Option<String>,
    pub tags: Option<Vec<String>>,
}

#[derive(Debug, Deserialize)]
pub struct SetPersonaRequest {
    pub persona_id: String,
    pub roleplay_character: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct PersonaCatalogResponse {
    pub personas: Vec<PersonaOption>,
}

#[derive(Debug, Serialize)]
pub struct SessionPersonaResponse {
    pub active: Option<ActivePersona>,
}

pub fn create_router(config: Config, agent: Arc<AgentRuntime>, config_path: PathBuf) -> Router {
    let secret = uuid::Uuid::new_v4().to_string();

    let db_path = PathBuf::from(std::env::var("OSAGENT_DATA_DIR").unwrap_or_else(|_| {
        std::env::var("OSAGENT_WORKSPACE").unwrap_or_else(|_| ".".to_string())
    }))
    .join("workflow.db");

    let artifact_path = PathBuf::from(std::env::var("OSAGENT_DATA_DIR").unwrap_or_else(|_| {
        std::env::var("OSAGENT_WORKSPACE").unwrap_or_else(|_| ".".to_string())
    }))
    .join("workflow_artifacts");

    let workflow_db = Arc::new(WorkflowDb::new(db_path));
    if let Err(e) = workflow_db.init_tables() {
        tracing::warn!("Failed to initialize workflow tables: {}", e);
    }

    let artifact_store = Arc::new(ArtifactStore::new(artifact_path.clone()));
    if let Err(e) = artifact_store.init() {
        tracing::warn!("Failed to initialize artifact store: {}", e);
    }

    let subagent_manager = agent.get_subagent_manager();
    let (executor, _event_rx) = WorkflowExecutor::new(
        workflow_db.clone(),
        artifact_store.clone(),
        subagent_manager,
    );
    let workflow_executor = Arc::new(executor);

    let workflow_state = WorkflowState {
        db: workflow_db,
        executor: workflow_executor,
        artifact_store,
    };

    let workflow_router = crate::workflow::api::create_workflow_router(workflow_state);

    let skill_service = Arc::new(SkillService::new());
    let skills_router = crate::skills::create_skills_router(skill_service);

    Router::new()
        .merge(workflow_router)
        .merge(skills_router)
        .route("/api/auth/login", post(login))
        .route("/api/auth/status", get(auth_status))
        .route("/api/auth/password", post(change_password))
        .route("/api/admin/restart", post(restart_server))
        .route("/api/config", get(get_config).put(update_config))
        .route("/api/discord/status", get(get_discord_bot_status))
        .route("/api/discord/start", post(start_discord_bot))
        .route("/api/discord/stop", post(stop_discord_bot))
        .route("/api/reasoning/options", get(get_reasoning_options))
        .route(
            "/api/workspaces",
            get(list_workspaces).post(create_workspace),
        )
        .route(
            "/api/workspaces/active",
            get(get_active_workspace).post(set_active_workspace),
        )
        .route("/api/workspaces/browse", get(browse_workspace_path))
        .route(
            "/api/workspaces/:id",
            post(update_workspace).delete(delete_workspace),
        )
        .route("/api/workspaces/:id/paths", post(add_workspace_path))
        .route(
            "/api/workspaces/:id/paths/:path_index",
            patch(update_workspace_path).delete(remove_workspace_path),
        )
        .route(
            "/api/sessions/:id/workspace",
            get(get_session_workspace).post(set_session_workspace),
        )
        .route("/api/memories", get(list_memories).post(add_memory))
        .route(
            "/api/memories/:id",
            put(update_memory).delete(delete_memory),
        )
        .route("/api/personas", get(list_personas))
        .route(
            "/api/sessions",
            get(list_sessions)
                .post(create_session)
                .delete(delete_sessions),
        )
        .route(
            "/api/sessions/:id",
            get(get_session).patch(patch_session).delete(delete_session),
        )
        .route(
            "/api/sessions/:id/tools",
            get(get_session_tools).post(add_session_tool),
        )
        .route(
            "/api/sessions/:id/tools/:tool_call_id",
            post(update_session_tool),
        )
        .route("/api/sessions/:id/queue", get(list_session_queue))
        .route("/api/sessions/:id/send", post(send_message))
        .route("/api/sessions/:id/cancel", post(cancel_session))
        .route("/api/sessions/:id/events", get(session_events))
        .route("/api/sessions/:id/history", get(session_history))
        .route("/api/sessions/:id/todos", get(session_todos))
        .route("/api/sessions/:id/snapshots", get(list_file_snapshots))
        .route(
            "/api/sessions/:id/snapshots/revert",
            post(revert_file_snapshot),
        )
        .route(
            "/api/sessions/:id/persona",
            get(get_session_persona)
                .post(set_session_persona)
                .delete(clear_session_persona),
        )
        .route(
            "/api/sessions/:id/checkpoints",
            get(list_checkpoints).post(create_checkpoint),
        )
        .route("/api/sessions/:id/rollback", post(rollback))
        .route("/api/sessions/:id/children", get(get_child_sessions))
        .route("/api/sessions/:id/subagents", get(get_session_subagents))
        .route("/api/sessions/:id/parent", get(get_parent_session))
        .route(
            "/api/subagents/:id",
            get(get_subagent_status).delete(cancel_subagent),
        )
        .route("/api/subagents/:id/result", get(get_subagent_result))
        .route("/api/subagents/cleanup", post(cleanup_subagents))
        .route("/api/tools", get(list_tools))
        .route("/api/lsp/status", get(lsp_status))
        .route("/api/agents", get(list_agents))
        .route(
            "/api/agents/:id/mode",
            get(get_agent_mode).post(set_agent_mode),
        )
        .route("/api/model", get(get_model).post(set_model))
        .route("/api/providers", get(get_providers).post(add_provider))
        .route("/api/providers/catalog", get(catalog_handler))
        .route("/api/providers/models", get(models_handler))
        .route("/api/providers/switch", post(switch_provider_model))
        .route("/api/providers/search", get(search_models))
        .route("/api/providers/validate", post(validate_provider))
        .route("/api/providers/:id", delete(delete_provider))
        .route("/api/oauth/providers", get(oauth_list_providers))
        .route("/api/oauth/:provider_id/start", post(oauth_start))
        .route("/api/oauth/:provider_id/device", post(oauth_device_code))
        .route(
            "/api/oauth/:provider_id/authorize",
            post(oauth_authorize).get(oauth_authorize),
        )
        .route("/api/oauth/:provider_id/callback", get(oauth_callback))
        .route("/api/oauth/:provider_id/status", get(oauth_status))
        .route("/api/oauth/:provider_id/refresh", post(oauth_refresh))
        .route("/api/oauth/:provider_id", delete(oauth_revoke))
        .route("/api/audit", get(list_audit))
        .route("/api/voice/status", get(voice_status))
        .route("/api/voice/install", post(voice_install))
        .route("/api/voice/transcribe", post(voice_transcribe))
        .route("/api/tts/synthesize", post(tts_synthesize))
        .route("/api/voice/models", get(voice_models))
        .route("/api/voice/installed", get(voice_installed))
        .route("/api/voice/download", post(voice_download))
        .route("/api/voice/progress", get(voice_progress))
        .route("/api/voice/upload", post(voice_upload))
        .route("/api/voice/model/:type/:id", delete(voice_delete_model))
        .route("/api/permissions", get(list_permission_prompts))
        .route("/api/permissions/respond", post(respond_permission_prompt))
        .route("/api/permissions/check", post(check_permission))
        .route(
            "/api/permissions/rules",
            get(list_permission_rules).post(create_permission_rule),
        )
        .route("/api/permissions/rules/:id", delete(delete_permission_rule))
        .route("/api/questions/answer", post(answer_question))
        .route("/api/plugins", get(list_plugins))
        .route("/api/plugins/enable", post(enable_plugin))
        .route("/api/plugins/disable", post(disable_plugin))
        .route("/api/plugins/reload", post(reload_plugins))
        .route("/api/update/check", get(check_update))
        .route("/api/update/download", post(download_update))
        .route("/api/update/install", post(install_update))
        .route("/api/update/status", get(update_status))
        .layer(Extension(config))
        .layer(Extension(agent))
        .layer(Extension(Arc::new(secret)))
        .layer(Extension(config_path))
}

#[derive(Debug, Serialize)]
pub struct WorkspaceListResponse {
    pub active_workspace: String,
    pub workspaces: Vec<WorkspaceConfig>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspacePathRequest {
    pub path: String,
    pub permission: Option<WorkspacePermission>,
    pub description: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct WorkspaceRequest {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub path: String,
    #[serde(default)]
    pub paths: Vec<WorkspacePathRequest>,
    pub description: Option<String>,
    pub permission: Option<WorkspacePermission>,
}

impl WorkspaceRequest {
    pub fn to_workspace_paths(&self) -> Vec<WorkspacePath> {
        if !self.paths.is_empty() {
            self.paths
                .iter()
                .filter_map(|wp| {
                    let path = wp.path.trim();
                    if path.is_empty() {
                        return None;
                    }

                    Some(WorkspacePath {
                        path: path.to_string(),
                        permission: wp
                            .permission
                            .clone()
                            .unwrap_or(WorkspacePermission::ReadWrite),
                        description: wp.description.clone(),
                    })
                })
                .collect()
        } else if !self.path.is_empty() {
            vec![WorkspacePath {
                path: self.path.trim().to_string(),
                permission: self
                    .permission
                    .clone()
                    .unwrap_or(WorkspacePermission::ReadWrite),
                description: self.description.clone(),
            }]
        } else {
            vec![]
        }
    }
}

#[derive(Debug, Serialize)]
pub struct WorkspaceBrowseResponse {
    pub path: String,
}

#[derive(Debug, Deserialize)]
pub struct SetActiveWorkspaceRequest {
    pub workspace_id: String,
}

#[derive(Debug, Deserialize)]
pub struct SetSessionWorkspaceRequest {
    pub workspace_id: String,
}

async fn get_model(Extension(agent): Extension<Arc<AgentRuntime>>) -> Json<ModelResponse> {
    let model = agent.get_current_model().await;
    let provider_id = agent.get_config().await.default_provider;
    Json(ModelResponse { model, provider_id })
}

async fn set_model(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<SetModelRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    if payload.model.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Model cannot be empty".to_string(),
            }),
        ));
    }

    let model = payload.model.trim().to_string();
    let provider_id = payload.provider_id.as_deref().unwrap_or("");

    if !provider_id.is_empty() {
        if let Err(e) = agent
            .switch_provider_model(provider_id.to_string(), model.clone())
            .await
        {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            ));
        }
    } else {
        agent.set_current_model(model.clone()).await;
        agent.set_provider_model_in_config(model).await;
    }

    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

async fn login(
    Extension(config): Extension<Config>,
    Extension(secret): Extension<Arc<String>>,
    Json(payload): Json<LoginRequest>,
) -> Result<Json<LoginResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !config.server.password_enabled || config.server.password.is_empty() {
        let token = auth::generate_token("user", &secret).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
        return Ok(Json(LoginResponse { token }));
    }

    if !auth::verify_password(&payload.password, &config.server.password).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })? {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Invalid password".to_string(),
            }),
        ));
    }

    let token = auth::generate_token("user", &secret).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(LoginResponse { token }))
}

async fn auth_status(Extension(config): Extension<Config>) -> Json<AuthStatusResponse> {
    Json(AuthStatusResponse {
        required: config.server.password_enabled && !config.server.password.is_empty(),
    })
}

async fn change_password(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<ChangePasswordRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;

    if config.server.password_enabled
        && !config.server.password.is_empty()
        && !auth::verify_password(&payload.old_password, &config.server.password).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
    {
        return Err((
            StatusCode::UNAUTHORIZED,
            Json(ErrorResponse {
                error: "Current password is incorrect".to_string(),
            }),
        ));
    }

    let new_hash = auth::hash_password(&payload.new_password).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to hash password: {}", e),
            }),
        )
    })?;

    let mut new_config = config.clone();
    new_config.server.password = new_hash;
    new_config.server.password_enabled = true;

    agent.replace_config(new_config.clone()).await;
    new_config.save(&config_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

async fn restart_server(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<RestartResponse>, (StatusCode, Json<ErrorResponse>)> {
    agent.signal_shutdown();

    std::fs::write(".osagent_restart_flag", "1").map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to write restart flag: {}", e),
            }),
        )
    })?;

    Ok(Json(RestartResponse { restarting: true }))
}

async fn get_config(Extension(agent): Extension<Arc<AgentRuntime>>) -> Json<Config> {
    Json(agent.get_config().await)
}

async fn get_discord_bot_status(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Json<DiscordBotStatusResponse> {
    let config = agent.get_config().await;
    let discord = config.discord.unwrap_or_default();

    #[cfg(feature = "discord")]
    let running = crate::discord::is_discord_bot_running().await;
    #[cfg(not(feature = "discord"))]
    let running = false;

    Json(DiscordBotStatusResponse {
        available: cfg!(feature = "discord"),
        enabled: discord.enabled,
        configured: !discord.token.trim().is_empty(),
        running,
    })
}

async fn start_discord_bot(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
) -> Result<Json<DiscordBotActionResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !cfg!(feature = "discord") {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(ErrorResponse {
                error: "Discord support is not compiled into this build".to_string(),
            }),
        ));
    }

    let config = agent.get_config().await;
    let discord = config.discord.unwrap_or_default();

    if !discord.enabled {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Discord bot is disabled in settings".to_string(),
            }),
        ));
    }

    if discord.token.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Discord bot token is missing".to_string(),
            }),
        ));
    }

    #[cfg(feature = "discord")]
    crate::discord::spawn_discord_bot(discord, config_path, agent)
        .await
        .map_err(|error| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error })))?;

    Ok(Json(DiscordBotActionResponse {
        running: true,
        message: "Discord bot starting".to_string(),
    }))
}

async fn stop_discord_bot(
) -> Result<Json<DiscordBotActionResponse>, (StatusCode, Json<ErrorResponse>)> {
    if !cfg!(feature = "discord") {
        return Err((
            StatusCode::NOT_IMPLEMENTED,
            Json(ErrorResponse {
                error: "Discord support is not compiled into this build".to_string(),
            }),
        ));
    }

    #[cfg(feature = "discord")]
    let stopped = crate::discord::stop_discord_bot().await;
    #[cfg(not(feature = "discord"))]
    let stopped = false;

    Ok(Json(DiscordBotActionResponse {
        running: false,
        message: if stopped {
            "Discord bot stopped".to_string()
        } else {
            "Discord bot was not running".to_string()
        },
    }))
}

async fn get_reasoning_options(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<crate::agent::reasoning::ThinkingOptionsState> {
    let config = agent.get_config().await;
    let provider_id = params
        .get("provider_id")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.default_provider.clone());
    let model = params
        .get("model")
        .cloned()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| config.active_model());

    Json(
        agent
            .get_reasoning_state(&provider_id, &model, &config.agent.thinking_level)
            .await,
    )
}

async fn update_config(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(mut new_config): Json<Config>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    if let Some(discord) = &mut new_config.discord {
        discord.allowed_users.sort_unstable();
        discord.allowed_users.dedup();
    } else {
        new_config.discord = Some(DiscordConfig::default());
    }

    agent.replace_config(new_config.clone()).await;
    new_config.save(&config_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

async fn list_workspaces(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<WorkspaceListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let active = agent.get_active_workspace().await;
    let workspaces = agent.get_workspaces().await;
    Ok(Json(WorkspaceListResponse {
        active_workspace: active.id,
        workspaces,
    }))
}

async fn get_active_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    Ok(Json(agent.get_active_workspace().await))
}

async fn set_active_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<SetActiveWorkspaceRequest>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    let workspace = agent
        .set_active_workspace(&payload.workspace_id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(workspace))
}

async fn create_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<WorkspaceRequest>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    if payload.id.trim().is_empty() || payload.name.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Workspace id and name are required".to_string(),
            }),
        ));
    }

    let paths = payload.to_workspace_paths();
    if paths.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "At least one workspace path is required".to_string(),
            }),
        ));
    }

    let workspace = WorkspaceConfig {
        id: payload.id.trim().to_string(),
        name: payload.name.trim().to_string(),
        paths,
        path: String::new(),
        description: payload.description,
        permission: payload.permission.unwrap_or(WorkspacePermission::ReadWrite),
        created_at: chrono::Utc::now().to_rfc3339(),
        last_used: None,
    };

    let created = agent.add_workspace(workspace).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(created))
}

async fn update_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(id): Path<String>,
    Json(payload): Json<WorkspaceRequest>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    let paths = payload.to_workspace_paths();
    if paths.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "At least one workspace path is required".to_string(),
            }),
        ));
    }

    let workspace = WorkspaceConfig {
        id,
        name: payload.name.trim().to_string(),
        paths,
        path: String::new(),
        description: payload.description,
        permission: payload.permission.unwrap_or(WorkspacePermission::ReadWrite),
        created_at: String::new(),
        last_used: None,
    };

    let updated = agent.update_workspace(workspace).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(updated))
}

async fn delete_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    agent.remove_workspace(&id).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(StatusCode::OK)
}

async fn add_workspace_path(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(id): Path<String>,
    Json(payload): Json<WorkspacePathRequest>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    let path = WorkspacePath {
        path: payload.path,
        permission: payload.permission.unwrap_or(WorkspacePermission::ReadWrite),
        description: payload.description,
    };

    agent.add_workspace_path(&id, path).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    let workspace = agent.get_workspace(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(workspace))
}

async fn update_workspace_path(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path((id, path_index)): Path<(String, usize)>,
    Json(payload): Json<WorkspacePathRequest>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    let path = WorkspacePath {
        path: payload.path,
        permission: payload.permission.unwrap_or(WorkspacePermission::ReadWrite),
        description: payload.description,
    };

    agent
        .update_workspace_path(&id, path_index, path)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    let workspace = agent.get_workspace(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(workspace))
}

async fn remove_workspace_path(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path((id, path_index)): Path<(String, usize)>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    agent
        .remove_workspace_path(&id, path_index)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(StatusCode::OK)
}

async fn browse_workspace_path(
) -> Result<Json<WorkspaceBrowseResponse>, (StatusCode, Json<ErrorResponse>)> {
    let picked = pick_workspace_folder().await?;

    let path = picked.ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Folder selection was cancelled".to_string(),
            }),
        )
    })?;

    Ok(Json(WorkspaceBrowseResponse {
        path: path.to_string_lossy().to_string(),
    }))
}

#[cfg(any(target_os = "windows", target_os = "macos"))]
async fn pick_workspace_folder() -> Result<Option<PathBuf>, (StatusCode, Json<ErrorResponse>)> {
    tokio::task::spawn_blocking(|| {
        rfd::FileDialog::new()
            .set_title("Choose workspace folder")
            .pick_folder()
    })
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to open folder picker: {}", e),
            }),
        )
    })
}

#[cfg(target_os = "linux")]
async fn pick_workspace_folder() -> Result<Option<PathBuf>, (StatusCode, Json<ErrorResponse>)> {
    Err((
        StatusCode::NOT_IMPLEMENTED,
        Json(ErrorResponse {
            error: "Workspace folder picker is not available on this platform".to_string(),
        }),
    ))
}

async fn get_session_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    let workspace = agent.get_session_workspace(&id).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(workspace))
}

async fn set_session_workspace(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
    Json(payload): Json<SetSessionWorkspaceRequest>,
) -> Result<Json<WorkspaceConfig>, (StatusCode, Json<ErrorResponse>)> {
    let workspace = agent
        .set_session_workspace(&id, &payload.workspace_id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    Ok(Json(workspace))
}

async fn list_memories(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<MemoryListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let status = agent.memory_status();
    let memories = agent.list_memories().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(Json(MemoryListResponse {
        enabled: status.enabled,
        file_path: status.file_path,
        memories,
    }))
}

async fn add_memory(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<AddMemoryRequest>,
) -> Result<Json<MemoryEntry>, (StatusCode, Json<ErrorResponse>)> {
    let entry = agent
        .add_memory(
            payload.title,
            payload.content,
            payload.tags,
            "user".to_string(),
        )
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    Ok(Json(entry))
}

async fn update_memory(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
    Json(payload): Json<UpdateMemoryRequest>,
) -> Result<Json<MemoryEntry>, (StatusCode, Json<ErrorResponse>)> {
    let entry = agent
        .update_memory(&id, payload.title, payload.content, payload.tags)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;
    Ok(Json(entry))
}

async fn delete_memory(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let deleted = agent.delete_memory(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    if deleted {
        Ok(StatusCode::OK)
    } else {
        Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Memory not found".to_string(),
            }),
        ))
    }
}

async fn list_personas(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Json<PersonaCatalogResponse> {
    Json(PersonaCatalogResponse {
        personas: agent.list_personas(),
    })
}

async fn get_session_persona(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<SessionPersonaResponse>, (StatusCode, Json<ErrorResponse>)> {
    let active = agent.get_session_persona(&id).await.map_err(|e| {
        (
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(SessionPersonaResponse { active }))
}

async fn set_session_persona(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
    Json(payload): Json<SetPersonaRequest>,
) -> Result<Json<SessionPersonaResponse>, (StatusCode, Json<ErrorResponse>)> {
    let active = agent
        .set_session_persona(&id, payload.persona_id, payload.roleplay_character)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(SessionPersonaResponse {
        active: Some(active),
    }))
}

async fn clear_session_persona(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    agent.reset_session_persona(&id).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

async fn list_sessions(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<Vec<Session>>, (StatusCode, Json<ErrorResponse>)> {
    let sessions = agent.list_sessions().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(sessions))
}

async fn create_session(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<CreateSessionRequest>,
) -> Result<Json<Session>, (StatusCode, Json<ErrorResponse>)> {
    let session = agent.create_session().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    if let Some(workspace_id) = payload.workspace_id {
        agent
            .set_session_workspace(&session.id, &workspace_id)
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: e.to_string(),
                    }),
                )
            })?;
    }

    let fresh = agent
        .get_session(&session.id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found after creation".to_string(),
            }),
        ))?;

    Ok(Json(fresh))
}

async fn get_session(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<Session>, (StatusCode, Json<ErrorResponse>)> {
    let session = agent
        .get_session(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        ))?;

    Ok(Json(session))
}

async fn delete_session(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    agent.delete_session(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

#[derive(Debug, Deserialize)]
pub struct PatchSessionRequest {
    pub name: Option<String>,
}

async fn patch_session(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
    Json(payload): Json<PatchSessionRequest>,
) -> Result<Json<Session>, (StatusCode, Json<ErrorResponse>)> {
    let session = agent
        .get_session(&id)
        .await
        .map_err(|e| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?
        .ok_or((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Session not found".to_string(),
            }),
        ))?;

    let mut updated_session = session;
    if let Some(name) = payload.name {
        updated_session.metadata["name"] = serde_json::json!(name);
    }

    agent.update_session(&updated_session).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(updated_session))
}

#[derive(Debug, Serialize)]
pub struct SessionToolEvent {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub completed: bool,
    pub success: Option<bool>,
    pub output: Option<String>,
    pub timestamp: i64,
    pub message_index: i32,
}

#[derive(Debug, Deserialize)]
pub struct AddToolRequest {
    pub tool_call_id: String,
    pub tool_name: String,
    pub arguments: serde_json::Value,
    pub message_index: Option<i32>,
}

#[derive(Debug, Deserialize)]
pub struct UpdateToolRequest {
    pub success: bool,
    pub output: Option<String>,
}

async fn get_session_tools(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<SessionToolEvent>>, (StatusCode, Json<ErrorResponse>)> {
    let history = agent.list_session_history(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    let mut completions: std::collections::HashMap<String, (bool, Option<String>)> =
        std::collections::HashMap::new();
    for e in &history {
        if e.event_type == "tool_complete" {
            if let Some(call_id) = e.data.get("tool_call_id").and_then(|v| v.as_str()) {
                let success = e
                    .data
                    .get("success")
                    .and_then(|v| v.as_bool())
                    .unwrap_or(false);
                let output = e
                    .data
                    .get("output")
                    .and_then(|v| v.as_str())
                    .map(|s| s.to_string());
                completions.insert(call_id.to_string(), (success, output));
            }
        }
    }

    let tools: Vec<SessionToolEvent> = history
        .into_iter()
        .filter(|e| e.event_type == "tool_start")
        .filter_map(|e| {
            let data = e.data;
            let tool_call_id = data.get("tool_call_id")?.as_str()?.to_string();
            let tool_name = data.get("tool_name")?.as_str()?.to_string();
            let arguments = data.get("arguments")?.clone();
            let message_index = data
                .get("message_index")
                .and_then(|v| v.as_i64())
                .unwrap_or(0) as i32;
            let (completed, success, output) = completions
                .remove(&tool_call_id)
                .map(|(s, o)| (true, Some(s), o))
                .unwrap_or((false, None, None));
            let timestamp = e.timestamp.timestamp();
            Some(SessionToolEvent {
                tool_call_id,
                tool_name,
                arguments,
                completed,
                success,
                output,
                timestamp,
                message_index,
            })
        })
        .collect();

    Ok(Json(tools))
}

async fn add_session_tool(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
    Json(payload): Json<AddToolRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let data = serde_json::json!({
        "tool_call_id": payload.tool_call_id,
        "tool_name": payload.tool_name,
        "arguments": payload.arguments,
        "message_index": payload.message_index.unwrap_or(0),
    });

    agent
        .append_session_event(&id, "tool_start", data)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(StatusCode::OK)
}

async fn update_session_tool(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path((id, tool_call_id)): Path<(String, String)>,
    Json(payload): Json<UpdateToolRequest>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let data = serde_json::json!({
        "tool_call_id": tool_call_id,
        "success": payload.success,
        "output": payload.output,
    });

    agent
        .append_session_event(&id, "tool_complete", data)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(StatusCode::OK)
}

async fn delete_sessions(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    agent.delete_all_sessions().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(StatusCode::OK)
}

async fn send_message(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
    Json(payload): Json<SendMessageRequest>,
) -> Result<Json<SendMessageResponse>, (StatusCode, Json<ErrorResponse>)> {
    let session_id = id.clone();
    let client_message_id = payload
        .client_message_id
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| Uuid::new_v4().to_string());

    let (queue_item, created) = agent
        .enqueue_message(&session_id, &client_message_id, &payload.message)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let started_queue_id = agent
        .clone()
        .spawn_next_queued_message_run(session_id.clone(), "web".to_string())
        .map_err(|e| {
            let status = if matches!(&e, crate::error::OSAgentError::Session(message) if message.contains("already in progress")) {
                StatusCode::CONFLICT
            } else {
                StatusCode::INTERNAL_SERVER_ERROR
            };
            (
                status,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let started_this_message = started_queue_id.as_deref() == Some(queue_item.id.as_str());

    Ok(Json(SendMessageResponse {
        accepted: true,
        session_id,
        status: if started_this_message {
            "started".to_string()
        } else if created {
            "queued".to_string()
        } else {
            "duplicate".to_string()
        },
        queued: !started_this_message,
        queue_position: if started_this_message {
            None
        } else {
            Some(queue_item.position)
        },
        queue_item: if started_this_message {
            None
        } else {
            Some(queue_item)
        },
    }))
}

async fn list_session_queue(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<QueuedMessage>>, (StatusCode, Json<ErrorResponse>)> {
    let items = agent.list_queued_messages(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(items))
}

fn strip_tool_blocks(text: &str) -> String {
    let mut output = String::new();
    let mut in_tool_block = false;

    for line in text.lines() {
        let trimmed = line.trim();
        if trimmed.starts_with("```tool") {
            in_tool_block = true;
            continue;
        }
        if in_tool_block && trimmed == "```" {
            in_tool_block = false;
            continue;
        }
        if in_tool_block {
            continue;
        }
        if trimmed == "Tool Results:" {
            continue;
        }
        output.push_str(line);
        output.push('\n');
    }

    output.trim().to_string()
}

async fn cancel_session(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    agent.cancel_session(&id);
    agent.cancel_subagents_for_parent(&id).await;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Cancellation requested for session {}", id)
    })))
}

async fn session_events(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(session_id): Path<String>,
) -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let rx = agent.subscribe_to_events();
    let session_id_filter = session_id.clone();

    let initial_event = if agent.is_session_busy(&session_id) {
        Some(Ok(Event::default().data(
            serde_json::to_string(&crate::agent::events::AgentEvent::Thinking {
                session_id: session_id.clone(),
                message: "Session is processing...".to_string(),
                timestamp: std::time::SystemTime::now(),
            })
            .unwrap_or_default(),
        )))
    } else {
        None
    };

    let event_stream = BroadcastStream::new(rx).filter_map(
        move |result: Result<crate::agent::events::AgentEvent, _>| {
            let session_id = session_id_filter.clone();
            match result {
                Ok(event) => {
                    if event.session_id() == session_id {
                        let json = serde_json::to_string(&event).unwrap_or_default();
                        Some(Ok(Event::default().data(json)))
                    } else {
                        None
                    }
                }
                Err(_) => None,
            }
        },
    );

    let stream = if let Some(initial) = initial_event {
        let initial_stream = futures::stream::once(async move { initial });
        Box::pin(initial_stream.chain(event_stream))
            as std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>
    } else {
        Box::pin(event_stream)
            as std::pin::Pin<Box<dyn Stream<Item = Result<Event, Infallible>> + Send>>
    };

    Sse::new(stream)
}

async fn session_history(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<StoredSessionEvent>>, (StatusCode, Json<ErrorResponse>)> {
    let history = agent.list_session_history(&session_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(history))
}

async fn session_todos(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<crate::tools::todo::TodoItem>>, (StatusCode, Json<ErrorResponse>)> {
    let todos = agent.list_todo_items(&session_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(todos))
}

async fn list_file_snapshots(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(session_id): Path<String>,
) -> Result<Json<Vec<FileSnapshotSummary>>, (StatusCode, Json<ErrorResponse>)> {
    let snapshots = agent.list_file_snapshots(&session_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(snapshots))
}

async fn revert_file_snapshot(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(session_id): Path<String>,
    Json(payload): Json<SnapshotRevertRequest>,
) -> Result<Json<FileSnapshotSummary>, (StatusCode, Json<ErrorResponse>)> {
    let summary = agent
        .revert_file_snapshot(&session_id, &payload.snapshot_id)
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(summary))
}

async fn list_checkpoints(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<Vec<crate::storage::Checkpoint>>, (StatusCode, Json<ErrorResponse>)> {
    let checkpoints = agent.list_checkpoints(&id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(checkpoints))
}

async fn create_checkpoint(
    Extension(_agent): Extension<Arc<AgentRuntime>>,
    Path(_id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    Ok(StatusCode::OK)
}

async fn rollback(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(_id): Path<String>,
    Json(payload): Json<RollbackRequest>,
) -> Result<Json<Session>, (StatusCode, Json<ErrorResponse>)> {
    let session = agent.rollback(&payload.checkpoint_id).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(session))
}

async fn list_tools(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let allowed_tools = &config.tools.allowed;

    let all_tools = vec![
        "batch",
        "bash",
        "read_file",
        "write_file",
        "edit_file",
        "apply_patch",
        "list_files",
        "delete_file",
        "grep",
        "glob",
        "web_fetch",
        "web_search",
        "code_python",
        "code_node",
        "code_bash",
        "task",
        "persona",
        "reflect",
        "todowrite",
        "todoread",
        "question",
        "skill",
        "lsp",
        "subagent",
        "plan_exit",
        "process",
    ];

    let tools: Vec<serde_json::Value> = all_tools
        .iter()
        .map(|&name| {
            serde_json::json!({
                "name": name,
                "enabled": allowed_tools.contains(&name.to_string()),
                "category": match name {
                    "batch" => "management",
                    "bash" => "shell",
                    "read_file" | "write_file" | "edit_file" | "list_files" | "delete_file" | "apply_patch" => "files",
                    "grep" | "glob" => "search",
                    "web_fetch" | "web_search" => "web",
                    "code_python" | "code_node" | "code_bash" => "code",
                    "task" | "subagent" => "management",
                    "persona" => "management",
                    "reflect" | "todowrite" | "todoread" => "meta",
                    "question" | "skill" => "management",
                    "lsp" => "code",
                    "plan_exit" => "agent",
                    "process" => "shell",
                    _ => "other",
                }
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "tools": tools,
        "total": tools.len(),
        "enabled": tools.iter().filter(|t| t["enabled"].as_bool().unwrap_or(false)).count()
    })))
}

async fn list_audit(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(query): Query<AuditQuery>,
) -> Result<Json<Vec<crate::storage::AuditEntry>>, (StatusCode, Json<ErrorResponse>)> {
    let storage =
        crate::storage::SqliteStorage::new(&agent.get_database_path().await).map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    let entries = storage
        .list_audit(query.limit.unwrap_or(100), query.offset.unwrap_or(0))
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    Ok(Json(entries))
}

#[derive(Debug, Deserialize)]
pub struct TtsRequest {
    pub text: String,
}

#[derive(Debug, Serialize)]
pub struct VoiceStatusResponse {
    pub enabled: bool,
    pub provider: Option<String>,
    pub language: Option<String>,
}

async fn voice_status(Extension(_config): Extension<Config>) -> Json<serde_json::Value> {
    let status = crate::voice::get_status();
    Json(serde_json::json!({
        "whisper_installed": status.whisper_installed,
        "whisper_model": status.whisper_model,
        "piper_installed": status.piper_installed,
        "piper_voice": status.piper_voice,
        "models_dir": status.models_dir
    }))
}

fn normalized_stt_provider(provider: &str) -> &str {
    match provider {
        "whisper" => "whisper-local",
        "whisper-api" => "browser",
        _ => provider,
    }
}

fn normalized_tts_provider(provider: &str) -> &str {
    match provider {
        "piper" => "piper-local",
        _ => provider,
    }
}

async fn tts_synthesize(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<TtsRequest>,
) -> Result<Vec<u8>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    tracing::info!("tts_synthesize config.voice={:?}", config.voice);
    let voice_config = config.voice.as_ref().filter(|v| v.enabled).ok_or((
        StatusCode::SERVICE_UNAVAILABLE,
        Json(ErrorResponse {
            error: "TTS not enabled in configuration".to_string(),
        }),
    ))?;

    if payload.text.trim().is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Text cannot be empty".to_string(),
            }),
        ));
    }

    match normalized_tts_provider(voice_config.tts_provider.as_str()) {
        "browser" => {
            Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "Browser TTS does not require server-side synthesis. Use Web Speech API directly.".to_string(),
                }),
            ))
        }
        "piper-local" => {
            let output_dir = std::env::temp_dir();
            let output_path = output_dir.join(format!("tts_{}.wav", uuid::Uuid::new_v4()));

            crate::voice::piper::synthesize(&payload.text, voice_config.piper_voice.as_deref(), &output_path).await
                .map_err(|e| (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse { error: e }),
                ))?;

            let audio_data = std::fs::read(&output_path)
                .map_err(|e| (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse { error: format!("Failed to read audio: {}", e) }),
                ))?;

            let _ = std::fs::remove_file(&output_path);

            Ok(audio_data)
        }
        _ => {
            Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Unknown TTS provider: {}", voice_config.tts_provider),
                }),
            ))
        }
    }
}

#[derive(Debug, Serialize)]
pub struct VoiceInstallStatus {
    pub whisper_installed: bool,
    pub whisper_model: Option<String>,
    pub piper_installed: bool,
    pub piper_voice: Option<String>,
    pub models_dir: String,
}

#[derive(Debug, Deserialize)]
pub struct VoiceInstallRequest {
    pub install_whisper: Option<bool>,
    pub whisper_model: Option<String>,
    pub install_piper: Option<bool>,
    pub language: Option<String>,
    pub piper_voice: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct VoiceInstallResponse {
    pub success: bool,
    pub message: String,
    pub status: VoiceInstallStatus,
}

async fn voice_install(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<VoiceInstallRequest>,
) -> Result<Json<VoiceInstallResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let lang = payload
        .language
        .clone()
        .or_else(|| config.voice.as_ref().map(|v| v.language.clone()))
        .unwrap_or_else(|| "en".to_string());

    let mut messages = Vec::new();

    if payload.install_whisper.unwrap_or(false) {
        let model = payload
            .whisper_model
            .as_deref()
            .and_then(crate::voice::whisper::WhisperModel::from_str)
            .unwrap_or(crate::voice::whisper::WhisperModel::Base);

        match crate::voice::whisper::install_all(model.clone()).await {
            Ok(()) => {
                messages.push(format!(
                    "Whisper {} model installed successfully",
                    model.id()
                ));
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to install Whisper: {}", e),
                    }),
                ));
            }
        }
    }

    if payload.install_piper.unwrap_or(false) {
        match crate::voice::piper::install_all(&lang, payload.piper_voice.as_deref()).await {
            Ok(()) => {
                messages.push("Piper TTS installed successfully".to_string());
            }
            Err(e) => {
                return Err((
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(ErrorResponse {
                        error: format!("Failed to install Piper: {}", e),
                    }),
                ));
            }
        }
    }

    let status = crate::voice::get_status();

    Ok(Json(VoiceInstallResponse {
        success: true,
        message: if messages.is_empty() {
            "No installation requested".to_string()
        } else {
            messages.join("; ")
        },
        status: VoiceInstallStatus {
            whisper_installed: status.whisper_installed,
            whisper_model: status.whisper_model,
            piper_installed: status.piper_installed,
            piper_voice: status.piper_voice,
            models_dir: status.models_dir,
        },
    }))
}

#[derive(Debug, Deserialize)]
pub struct VoiceTranscribeRequest {
    pub audio_data: String,
}

#[derive(Debug, Serialize)]
pub struct VoiceTranscribeResponse {
    pub text: String,
}

async fn voice_transcribe(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<VoiceTranscribeRequest>,
) -> Result<Json<VoiceTranscribeResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let voice_config = config
        .voice
        .as_ref()
        .filter(|v| {
            v.enabled && normalized_stt_provider(v.stt_provider.as_str()) == "whisper-local"
        })
        .ok_or((
            StatusCode::SERVICE_UNAVAILABLE,
            Json(ErrorResponse {
                error: "Local Whisper not enabled. Enable it in settings and install models first."
                    .to_string(),
            }),
        ))?;

    use base64::{engine::general_purpose, Engine as _};
    let audio_bytes = general_purpose::STANDARD
        .decode(&payload.audio_data)
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid audio data (expected base64): {}", e),
                }),
            )
        })?;

    let temp_dir = tempfile::tempdir().map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to create temp dir: {}", e),
            }),
        )
    })?;

    let audio_path = temp_dir.path().join("audio.wav");
    std::fs::write(&audio_path, &audio_bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to write audio file: {}", e),
            }),
        )
    })?;

    let text = crate::voice::whisper::transcribe(
        &audio_path,
        Some(&voice_config.language),
        voice_config.whisper_model.as_deref(),
    )
    .await
    .map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Transcription failed: {}", e),
            }),
        )
    })?;

    Ok(Json(VoiceTranscribeResponse { text }))
}

#[derive(Debug, Serialize)]
pub struct LspStatusResponse {
    pub enabled: bool,
    pub available_servers: Vec<String>,
}

async fn lsp_status(Extension(agent): Extension<Arc<AgentRuntime>>) -> Json<LspStatusResponse> {
    let config = agent.get_config().await;
    let servers: Vec<String> = config.lsp.servers.keys().cloned().collect();
    Json(LspStatusResponse {
        enabled: config.lsp.enabled,
        available_servers: servers,
    })
}

#[derive(Debug, Serialize)]
pub struct AgentInfo {
    pub name: String,
    pub description: String,
    pub mode: String,
}

#[derive(Debug, Serialize)]
pub struct ListAgentsResponse {
    pub agents: Vec<AgentInfo>,
}

async fn list_agents() -> Json<ListAgentsResponse> {
    let agents = vec![
        AgentInfo {
            name: "build".to_string(),
            description: "Default editing agent with full tool access".to_string(),
            mode: "build".to_string(),
        },
        AgentInfo {
            name: "plan".to_string(),
            description: "Read-only planning agent".to_string(),
            mode: "plan".to_string(),
        },
        AgentInfo {
            name: "general".to_string(),
            description: "Multi-step parallel execution subagent".to_string(),
            mode: "subagent".to_string(),
        },
        AgentInfo {
            name: "explore".to_string(),
            description: "Fast codebase exploration subagent".to_string(),
            mode: "subagent".to_string(),
        },
    ];
    Json(ListAgentsResponse { agents })
}

#[derive(Debug, Deserialize)]
pub struct SetAgentModeRequest {
    pub mode: String,
}

async fn get_agent_mode(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let _config = agent.get_config().await;
    Ok(Json(serde_json::json!({
        "mode": "build",
        "available_modes": ["build", "plan"]
    })))
}

async fn set_agent_mode(
    Extension(_agent): Extension<Arc<AgentRuntime>>,
    Path(_id): Path<String>,
    Json(payload): Json<SetAgentModeRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    if payload.mode != "build" && payload.mode != "plan" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Invalid mode. Must be 'build' or 'plan'".to_string(),
            }),
        ));
    }
    Ok(Json(serde_json::json!({
        "mode": payload.mode,
        "message": format!("Agent mode set to {}", payload.mode)
    })))
}

#[derive(Debug, Serialize)]
pub struct PermissionPromptResponse {
    pub prompts: Vec<PermissionPrompt>,
}

#[derive(Debug, Deserialize)]
pub struct RespondPermissionRequest {
    pub prompt_id: String,
    pub allowed: bool,
    pub always: bool,
}

#[derive(Debug, Deserialize)]
pub struct CheckPermissionRequest {
    pub path: String,
}

#[derive(Debug, Serialize)]
pub struct CheckPermissionResponse {
    pub allowed: bool,
    pub action: String,
}

#[derive(Debug, Deserialize)]
pub struct CreatePermissionRuleRequest {
    pub permission: String,
    pub pattern: String,
    pub action: String,
}

#[derive(Debug, Serialize)]
pub struct PermissionRuleResponse {
    pub rules: Vec<crate::permission::PermissionRule>,
}

async fn list_permission_prompts(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<PermissionPromptResponse>, (StatusCode, Json<ErrorResponse>)> {
    let prompts = agent.get_pending_permission_prompts().await;
    Ok(Json(PermissionPromptResponse { prompts }))
}

async fn respond_permission_prompt(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<RespondPermissionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let prompt = agent
        .respond_to_permission_prompt(&payload.prompt_id, payload.allowed, payload.always)
        .await;

    match prompt {
        Some(_) => Ok(Json(serde_json::json!({
            "success": true,
            "message": if payload.allowed { "Permission granted" } else { "Permission denied" }
        }))),
        None => Err((
            StatusCode::NOT_FOUND,
            Json(ErrorResponse {
                error: "Permission prompt not found".to_string(),
            }),
        )),
    }
}

async fn check_permission(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<CheckPermissionRequest>,
) -> Result<Json<CheckPermissionResponse>, (StatusCode, Json<ErrorResponse>)> {
    let action = agent
        .check_external_directory_permission(&payload.path)
        .await;
    let allowed = matches!(action, PermissionAction::Allow);
    Ok(Json(CheckPermissionResponse {
        allowed,
        action: match action {
            PermissionAction::Allow => "allow".to_string(),
            PermissionAction::Deny => "deny".to_string(),
            PermissionAction::Ask => "ask".to_string(),
        },
    }))
}

async fn list_permission_rules(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<PermissionRuleResponse>, (StatusCode, Json<ErrorResponse>)> {
    let rules = agent.get_permission_rules().await;
    Ok(Json(PermissionRuleResponse { rules }))
}

async fn create_permission_rule(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<CreatePermissionRuleRequest>,
) -> Result<Json<crate::permission::PermissionRule>, (StatusCode, Json<ErrorResponse>)> {
    let action = match payload.action.as_str() {
        "allow" => crate::permission::PermissionAction::Allow,
        "deny" => crate::permission::PermissionAction::Deny,
        "ask" => crate::permission::PermissionAction::Ask,
        _ => {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "Invalid action '{}'. Must be 'allow', 'deny', or 'ask'.",
                        payload.action
                    ),
                }),
            ));
        }
    };

    let rule = crate::permission::PermissionRule::new(payload.permission, payload.pattern, action);

    agent.add_permission_rule(rule.clone()).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(rule))
}

async fn delete_permission_rule(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    agent.remove_permission_rule(&id).await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;
    Ok(StatusCode::OK)
}

#[derive(Debug, Serialize)]
pub struct PluginListResponse {
    pub plugins: Vec<LoadedPlugin>,
}

async fn list_plugins(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<PluginListResponse>, (StatusCode, Json<ErrorResponse>)> {
    let plugins = agent.list_plugins().await;
    Ok(Json(PluginListResponse { plugins }))
}

#[derive(Debug, Deserialize)]
pub struct PluginActionRequest {
    pub name: String,
}

async fn enable_plugin(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<PluginActionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    agent
        .enable_plugin(&payload.name)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Plugin '{}' enabled", payload.name)
    })))
}

async fn disable_plugin(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<PluginActionRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    agent
        .disable_plugin(&payload.name)
        .await
        .map_err(|e| (StatusCode::BAD_REQUEST, Json(ErrorResponse { error: e })))?;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Plugin '{}' disabled", payload.name)
    })))
}

async fn reload_plugins(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    agent.reload_plugins().await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )
    })?;
    let plugins = agent.list_plugins().await;
    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Plugins reloaded",
        "plugins": plugins
    })))
}

async fn get_providers(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let providers: Vec<serde_json::Value> = config
        .providers
        .iter()
        .map(|p| {
            let api_key_preview = if p.auth_type.as_deref() == Some("oauth") {
                "(oauth)".to_string()
            } else if p.api_key.is_empty() {
                String::new()
            } else if p.api_key.len() > 10 {
                format!(
                    "{}...{}",
                    &p.api_key[..7],
                    &p.api_key[p.api_key.len() - 4..]
                )
            } else {
                "(configured)".to_string()
            };
            serde_json::json!({
                "id": p.provider_type,
                "base_url": p.base_url,
                "model": p.model,
                "api_key_preview": api_key_preview,
                "is_default": p.provider_type == config.default_provider,
            })
        })
        .collect();

    Ok(Json(serde_json::json!({
        "providers": providers,
        "default_provider": config.default_provider,
        "default_model": config.default_model,
    })))
}

#[derive(Debug, Deserialize)]
pub struct AddProviderRequest {
    pub provider_id: String,
    pub api_key: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
    pub is_default: Option<bool>,
    #[serde(flatten)]
    pub oauth_data: Option<OAuthProviderData>,
}

#[derive(Debug, Deserialize)]
pub struct OAuthProviderData {
    pub oauth_token: Option<String>,
    pub oauth_refresh_token: Option<String>,
    pub oauth_expires_at: Option<i64>,
    pub oauth_client_id: Option<String>,
    pub oauth_client_secret: Option<String>,
    pub oauth_authorization_url: Option<String>,
    pub oauth_token_url: Option<String>,
    pub oauth_scopes: Option<Vec<String>>,
    pub redirect_url: Option<String>,
}

async fn add_provider(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<AddProviderRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    if payload.provider_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "provider_id is required".to_string(),
            }),
        ));
    }

    let config_dir = config_path.parent().unwrap_or(&config_path).to_path_buf();
    let oauth_storage =
        crate::oauth::OAuthStorage::new(crate::oauth::get_oauth_storage_path(&config_dir));
    let stored_oauth = oauth_storage.get_token(&payload.provider_id).ok().flatten();
    let has_oauth = payload
        .oauth_data
        .as_ref()
        .and_then(|d| d.oauth_token.as_ref())
        .is_some()
        || stored_oauth.is_some();
    let preset = crate::agent::provider_presets::get_preset(&payload.provider_id);
    let default_model = if payload.provider_id == "openai" && has_oauth {
        Some("gpt-5.3-codex".to_string())
    } else {
        preset
            .as_ref()
            .and_then(|provider| provider.models.first().map(|model| model.id.clone()))
    };

    let provider_config = crate::config::ProviderConfig {
        provider_type: payload.provider_id.clone(),
        api_key: payload.api_key.unwrap_or_default(),
        base_url: payload
            .base_url
            .filter(|value| !value.trim().is_empty())
            .or_else(|| preset.as_ref().map(|p| p.base_url.clone()))
            .unwrap_or_default(),
        model: payload
            .model
            .filter(|value| !value.trim().is_empty())
            .or(default_model)
            .unwrap_or_default(),
        fallbacks: vec![],
        auth_type: has_oauth.then(|| "oauth".to_string()),
        oauth_client_id: payload
            .oauth_data
            .as_ref()
            .and_then(|d| d.oauth_client_id.clone()),
        oauth_client_secret: payload
            .oauth_data
            .as_ref()
            .and_then(|d| d.oauth_client_secret.clone()),
        oauth_authorization_url: payload
            .oauth_data
            .as_ref()
            .and_then(|d| d.oauth_authorization_url.clone()),
        oauth_token_url: payload
            .oauth_data
            .as_ref()
            .and_then(|d| d.oauth_token_url.clone()),
        oauth_scopes: payload
            .oauth_data
            .as_ref()
            .and_then(|d| d.oauth_scopes.clone()),
        custom_headers: None,
        redirect_url: payload
            .oauth_data
            .as_ref()
            .and_then(|d| d.redirect_url.clone()),
    };

    agent
        .add_provider(provider_config.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    if payload.is_default.unwrap_or(false) {
        let mut config = agent.get_config().await;
        config.default_provider = payload.provider_id.clone();
        agent.replace_config(config.clone()).await;
        if let Err(e) = config.save(&config_path) {
            return Err((
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to save config: {}", e),
                }),
            ));
        }
    }

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Provider added successfully",
        "provider_id": payload.provider_id
    })))
}

#[derive(Debug, Deserialize)]
pub struct ValidateProviderRequest {
    pub provider_id: String,
    pub api_key: String,
    pub base_url: Option<String>,
}

async fn validate_provider(
    Json(payload): Json<ValidateProviderRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let base_url = if let Some(url) = payload.base_url {
        url
    } else {
        crate::agent::provider_presets::get_preset(&payload.provider_id)
            .map(|p| p.base_url)
            .unwrap_or_default()
    };

    if payload.api_key.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "API key is empty".to_string(),
            }),
        ));
    }

    let client = reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(10))
        .build()
        .map_err(|_| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: "Failed to create HTTP client".to_string(),
                }),
            )
        })?;

    let test_url = format!("{}/models", base_url.trim_end_matches('/'));
    let res = client
        .get(&test_url)
        .header("Authorization", format!("Bearer {}", payload.api_key))
        .send()
        .await;

    match res {
        Ok(response) => {
            let status = response.status();
            if status.is_success() {
                Ok(Json(serde_json::json!({
                    "valid": true
                })))
            } else if status == 401 || status == 403 {
                Ok(Json(serde_json::json!({
                    "valid": false,
                    "error": "Invalid API key"
                })))
            } else {
                Ok(Json(serde_json::json!({
                    "valid": false,
                    "error": format!("Unexpected response: {}", status)
                })))
            }
        }
        Err(e) => Ok(Json(serde_json::json!({
            "valid": false,
            "error": format!("Connection failed: {}", e)
        }))),
    }
}

async fn delete_provider(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(provider_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    agent
        .remove_provider(provider_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "Provider deleted successfully"
    })))
}

async fn oauth_list_providers() -> Json<serde_json::Value> {
    let providers = crate::oauth::provider::get_oauth_providers();
    let provider_infos: Vec<serde_json::Value> = providers
        .into_iter()
        .map(|p| {
            serde_json::json!({
                "id": p.id,
                "name": p.name,
                "flow_type": match p.flow_type {
                    OAuthFlowType::Pkce => "pkce",
                    OAuthFlowType::DeviceCode => "device_code",
                    OAuthFlowType::Basic => "basic",
                },
                "requires_client_id": !p.client_id.is_empty() || p.flow_type != OAuthFlowType::Pkce,
            })
        })
        .collect();

    Json(serde_json::json!({ "providers": provider_infos }))
}

#[derive(Debug, Deserialize)]
pub struct OAuthStartPayload {
    pub code_challenge: Option<String>,
}

async fn oauth_start(
    Extension(_agent): Extension<Arc<AgentRuntime>>,
    Path(provider_id): Path<String>,
    Json(_payload): Json<OAuthStartPayload>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let provider = get_oauth_provider(&provider_id).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Unknown OAuth provider: {}", provider_id),
            }),
        )
    })?;

    if is_pkce_oauth_provider(&provider_id) {
        let PkcePair {
            verifier,
            challenge,
        } = generate_pkce_pair();
        let state = generate_oauth_state();

        // Resolve client_id: env var takes priority over the hardcoded value.
        // Env var format: OPENAI_OAUTH_CLIENT_ID, ANTHROPIC_OAUTH_CLIENT_ID, etc.
        let env_var_name = format!(
            "{}_OAUTH_CLIENT_ID",
            provider_id.to_uppercase().replace('-', "_")
        );
        let client_id =
            std::env::var(&env_var_name).unwrap_or_else(|_| provider.client_id.to_string());

        if client_id.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!(
                        "No OAuth client ID configured for {}. Set the {} environment variable.",
                        provider.name, env_var_name
                    ),
                }),
            ));
        }

        let base_url =
            std::env::var("OSA_BASE_URL").unwrap_or_else(|_| "http://localhost:8765".to_string());
        let redirect_uri = format!("{}/api/oauth/{}/callback", base_url, provider_id);
        oauth_pkce_sessions().insert(
            state.clone(),
            PendingPkceSession {
                provider_id: provider_id.clone(),
                code_verifier: verifier.clone(),
                redirect_uri: redirect_uri.clone(),
            },
        );

        let mut params = vec![
            ("client_id", client_id),
            ("redirect_uri", redirect_uri.clone()),
            ("response_type", "code".to_string()),
            ("scope", provider.scopes.join(" ")),
            ("state", state.clone()),
            ("code_challenge", challenge),
            ("code_challenge_method", "S256".to_string()),
        ];

        if provider_id == "openai" {
            params.push(("id_token_add_organizations", "true".to_string()));
            params.push(("codex_cli_simplified_flow", "true".to_string()));
            params.push(("originator", "osagent".to_string()));
        }

        let auth_url = format!(
            "{}?{}",
            provider.authorization_url,
            params
                .into_iter()
                .map(|(key, value)| format!(
                    "{}={}",
                    urlencoding::encode(key),
                    urlencoding::encode(&value)
                ))
                .collect::<Vec<_>>()
                .join("&")
        );

        Ok(Json(serde_json::json!({
            "success": true,
            "flow_type": "pkce",
            "auth_url": auth_url,
            "state": state,
            "code_verifier": verifier,
            "redirect_uri": redirect_uri,
        })))
    } else if is_device_code_oauth_provider(&provider_id) {
        let device_url = provider.device_code_url.ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Provider {} does not support device code flow", provider_id),
                }),
            )
        })?;

        let client_id = &provider.client_id;
        if client_id.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("No client ID configured for provider: {}", provider_id),
                }),
            ));
        }

        let scopes_str = provider.scopes.join(" ");
        let body = format!(
            "client_id={}&scope={}",
            urlencoding::encode(client_id),
            urlencoding::encode(&scopes_str)
        );

        let client = reqwest::Client::new();
        let resp = client
            .post(device_url)
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(body)
            .send()
            .await
            .map_err(|e| {
                (
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("Failed to request device code: {}", e),
                    }),
                )
            })?;

        let data: serde_json::Value = resp.json().await.map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Invalid device code response: {}", e),
                }),
            )
        })?;

        Ok(Json(serde_json::json!({
            "success": true,
            "flow_type": "device_code",
            "device_code": data["device_code"],
            "user_code": data["user_code"],
            "verification_uri": data["verification_uri"],
            "interval": data.get("interval").and_then(|v| v.as_u64()).unwrap_or(5),
            "expires_in": data.get("expires_in").and_then(|v| v.as_u64()).unwrap_or(300),
        })))
    } else {
        Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "Provider {} uses basic OAuth, use /authorize endpoint",
                    provider_id
                ),
            }),
        ))
    }
}

async fn oauth_device_code(
    Extension(_agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(provider_id): Path<String>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let provider = get_oauth_provider(&provider_id).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Unknown OAuth provider: {}", provider_id),
            }),
        )
    })?;

    let device_code = payload["device_code"].as_str().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "device_code is required".to_string(),
            }),
        )
    })?;

    let client_id = &provider.client_id;
    if client_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("No client ID configured for provider: {}", provider_id),
            }),
        ));
    }

    let body = format!(
        "grant_type=urn:ietf:params:oauth:grant-type:device_code&client_id={}&device_code={}",
        urlencoding::encode(client_id),
        urlencoding::encode(device_code)
    );

    let client = reqwest::Client::new();
    let token_url: &str = provider.token_url;
    let resp = client
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(body)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Failed to check device code: {}", e),
                }),
            )
        })?;

    let data: serde_json::Value = resp.json().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Invalid token response: {}", e),
            }),
        )
    })?;

    if let Some(error) = data.get("error") {
        let error_str = error.as_str().unwrap_or("unknown_error");
        if error_str == "authorization_pending" {
            return Ok(Json(serde_json::json!({
                "pending": true,
            })));
        }
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Device code error: {}", error_str),
            }),
        ));
    }

    let access_token = data["access_token"].as_str().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No access_token in response".to_string(),
            }),
        )
    })?;

    let refresh_token = data.get("refresh_token").and_then(|v| v.as_str());
    let expires_in = data.get("expires_in").and_then(|v| v.as_i64());

    let config_dir = config_path.parent().unwrap_or(&config_path).to_path_buf();
    let storage =
        crate::oauth::OAuthStorage::new(crate::oauth::get_oauth_storage_path(&config_dir));
    let entry = crate::oauth::OAuthTokenEntry {
        access_token: access_token.to_string(),
        refresh_token: refresh_token.map(String::from),
        expires_at: expires_in.map(|e| chrono::Utc::now().timestamp() + e),
        scopes: Some(provider.scopes.iter().map(|s| s.to_string()).collect()),
        account_id: None,
    };
    storage.set_token(&provider_id, entry).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to store token: {}", e),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "connected": true,
    })))
}

async fn oauth_authorize(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(provider_id): Path<String>,
    Json(payload): Json<OAuthAuthorizePayload>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let provider_config = config
        .providers
        .iter()
        .find(|p| p.provider_type == provider_id);

    let user_client_id = payload.client_id.filter(|c| !c.is_empty());

    let (client_id, oauth_url, scopes): (String, String, Vec<String>) = if let Some(ref user_cid) =
        user_client_id
    {
        (user_cid.clone(), String::new(), Vec::new())
    } else if let Some(provider) = provider_config {
        let client_id = provider
            .oauth_client_id
            .clone()
            .or_else(|| {
                std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase())).ok()
            })
            .unwrap_or_default();

        if client_id.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "OAuth client ID not configured. Please provide one or set {PROVIDER}_OAUTH_CLIENT_ID environment variable.".to_string(),
                }),
            ));
        }

        let oauth_url = if let Some(ref url) = provider.oauth_authorization_url {
            url.clone()
        } else {
            match provider_id.as_str() {
                "openai" => "https://oauth.openai.com/authorize".to_string(),
                "anthropic" => "https://auth.anthropic.com/oauth/authorize".to_string(),
                "google_ai" => "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
                _ => {
                    return Err((
                        StatusCode::BAD_REQUEST,
                        Json(ErrorResponse {
                            error: format!("No OAuth URL configured for provider: {}", provider_id),
                        }),
                    ))
                }
            }
        };

        let scopes = if let Some(ref s) = provider.oauth_scopes {
            s.clone()
        } else {
            match provider_id.as_str() {
                "openai" => vec!["api.full-access".to_string()],
                "anthropic" => vec!["api:read".to_string(), "api:write".to_string()],
                "google_ai" => vec!["https://www.googleapis.com/auth/cloud-platform".to_string()],
                _ => vec!["api.full-access".to_string()],
            }
        };

        (client_id, oauth_url, scopes)
    } else {
        let client_id = std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase()))
            .unwrap_or_else(|_| "".to_string());

        if client_id.is_empty() {
            return Err((
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "OAuth client ID not configured. Please provide one or set {PROVIDER}_OAUTH_CLIENT_ID environment variable.".to_string(),
                }),
            ));
        }

        let oauth_url = match provider_id.as_str() {
            "openai" => "https://oauth.openai.com/authorize".to_string(),
            "anthropic" => "https://auth.anthropic.com/oauth/authorize".to_string(),
            "google_ai" => "https://accounts.google.com/o/oauth2/v2/auth".to_string(),
            _ => {
                return Err((
                    StatusCode::BAD_REQUEST,
                    Json(ErrorResponse {
                        error: format!("OAuth not supported for provider: {}", provider_id),
                    }),
                ));
            }
        };

        let scopes = match provider_id.as_str() {
            "openai" => vec!["api.full-access".to_string()],
            "anthropic" => vec!["api:read".to_string(), "api:write".to_string()],
            "google_ai" => vec!["https://www.googleapis.com/auth/cloud-platform".to_string()],
            _ => vec!["api.full-access".to_string()],
        };

        (client_id, oauth_url, scopes)
    };

    let redirect_uri = provider_config
        .as_ref()
        .and_then(|p| p.redirect_url.clone())
        .unwrap_or_else(|| {
            format!(
                "{}/api/oauth/{}/callback",
                std::env::var("OSA_BASE_URL")
                    .unwrap_or_else(|_| "http://localhost:8765".to_string()),
                provider_id
            )
        });

    let state = format!("osa_oauth_{}", uuid::Uuid::new_v4());

    let scopes_str = scopes.join(" ");

    let auth_url = format!(
        "{}?client_id={}&redirect_uri={}&response_type=code&scope={}&state={}",
        oauth_url,
        urlencoding::encode(&client_id),
        urlencoding::encode(&redirect_uri),
        urlencoding::encode(&scopes_str),
        state
    );

    Ok(Json(serde_json::json!({
        "success": true,
        "auth_url": auth_url,
        "state": state,
        "provider_id": provider_id
    })))
}

#[derive(Debug, Deserialize)]
pub struct OAuthAuthorizePayload {
    pub client_id: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct OAuthCallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    pub error: Option<String>,
    pub error_description: Option<String>,
}

async fn oauth_callback(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(provider_id): Path<String>,
    Query(params): Query<OAuthCallbackQuery>,
) -> axum::response::Html<String> {
    let popup_error = |message: String| {
        axum::response::Html(format!(
            r#"<!DOCTYPE html>
<html>
<head><title>OAuth Error</title></head>
<body>
<script>
if (window.opener) {{
    window.opener.oauthCallback({{ error: "{}" }});
    window.close();
}} else {{
    document.body.innerHTML = "<h1>OAuth Error</h1><p>{}</p><p>You can close this window.</p>";
}}
</script>
</body>
</html>"#,
            message.replace('"', "&quot;"),
            message.replace('"', "&quot;")
        ))
    };

    if let Some(error) = params.error.clone() {
        return popup_error(params.error_description.clone().unwrap_or(error));
    }

    let code = match params.code.clone() {
        Some(code) if !code.trim().is_empty() => code,
        _ => return popup_error("Missing authorization code".to_string()),
    };
    let state = match params.state.clone() {
        Some(state) if !state.trim().is_empty() => state,
        _ => return popup_error("Missing OAuth state".to_string()),
    };
    let Some((_, pending)) = oauth_pkce_sessions().remove(&state) else {
        return popup_error("OAuth session expired or invalid state".to_string());
    };
    if pending.provider_id != provider_id {
        return popup_error("OAuth state/provider mismatch".to_string());
    }

    let config = agent.get_config().await;
    let provider_config = config
        .providers
        .iter()
        .find(|p| p.provider_type == provider_id);
    let Some(default_provider) = get_oauth_provider(&provider_id) else {
        return popup_error(format!("Unknown OAuth provider: {}", provider_id));
    };

    let (client_id, client_secret, token_url): (String, String, String) = if let Some(provider) =
        provider_config
    {
        let client_id = provider
            .oauth_client_id
            .clone()
            .or_else(|| {
                std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase())).ok()
            })
            .unwrap_or_else(|| default_provider.client_id.to_string());
        let client_secret = provider
            .oauth_client_secret
            .clone()
            .or_else(|| {
                std::env::var(format!(
                    "{}_OAUTH_CLIENT_SECRET",
                    provider_id.to_uppercase()
                ))
                .ok()
            })
            .unwrap_or_default();
        let token_url = provider
            .oauth_token_url
            .clone()
            .unwrap_or_else(|| default_provider.token_url.to_string());
        (client_id, client_secret, token_url)
    } else {
        let client_id = std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase()))
            .unwrap_or_else(|_| default_provider.client_id.to_string());
        let client_secret = std::env::var(format!(
            "{}_OAUTH_CLIENT_SECRET",
            provider_id.to_uppercase()
        ))
        .unwrap_or_default();
        let token_url = default_provider.token_url.to_string();
        (client_id, client_secret, token_url)
    };

    if client_id.is_empty() {
        return popup_error("OAuth client ID not configured".to_string());
    }

    let redirect_uri = pending.redirect_uri;
    let mut form = vec![
        ("grant_type".to_string(), "authorization_code".to_string()),
        ("client_id".to_string(), client_id.clone()),
        ("code".to_string(), code),
        ("redirect_uri".to_string(), redirect_uri),
        ("code_verifier".to_string(), pending.code_verifier),
    ];
    if !client_secret.trim().is_empty() {
        form.push(("client_secret".to_string(), client_secret));
    }

    let client = reqwest::Client::new();
    let token_response = client.post(&token_url).form(&form).send().await;

    match token_response {
        Ok(response) if response.status().is_success() => {
            let token_data: serde_json::Value = match response.json().await {
                Ok(data) => data,
                Err(_) => serde_json::json!({}),
            };

            let access_token = token_data["access_token"].as_str().unwrap_or_default();
            let refresh_token = token_data.get("refresh_token").and_then(|v| v.as_str());
            let expires_in = token_data.get("expires_in").and_then(|v| v.as_i64());
            let expires_at = expires_in.map(|e| {
                std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64
                    + e
            });

            let config_dir = config_path.parent().unwrap_or(&config_path).to_path_buf();
            let oauth_storage =
                crate::oauth::OAuthStorage::new(crate::oauth::get_oauth_storage_path(&config_dir));

            let token_entry = crate::oauth::OAuthTokenEntry {
                access_token: access_token.to_string(),
                refresh_token: refresh_token.map(|s| s.to_string()),
                expires_at,
                scopes: Some(
                    default_provider
                        .scopes
                        .iter()
                        .map(|scope| scope.to_string())
                        .collect(),
                ),
                account_id: crate::oauth::extract_account_id(
                    token_data.get("id_token").and_then(|v| v.as_str()),
                    Some(access_token),
                ),
            };

            let _ = oauth_storage.set_token(&provider_id, token_entry);

            axum::response::Html(format!(
                r#"<!DOCTYPE html>
<html>
<head><title>OAuth Success</title></head>
<body>
<script>
if (window.opener) {{
    window.opener.oauthCallback({{ success: true, provider_id: "{}" }});
    window.close();
}} else {{
    document.body.innerHTML = "<h1>OAuth Success!</h1><p>You can close this window.</p>";
}}
</script>
</body>
</html>"#,
                provider_id
            ))
        }
        Ok(response) => {
            let error_text = response.text().await.unwrap_or_default();
            popup_error(format!("OAuth token exchange failed: {}", error_text))
        }
        Err(e) => popup_error(format!("Network error: {}", e)),
    }
}

async fn oauth_status(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(provider_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let config_dir = config_path.parent().unwrap_or(&config_path).to_path_buf();
    let oauth_storage =
        crate::oauth::OAuthStorage::new(crate::oauth::get_oauth_storage_path(&config_dir));

    let token_info = oauth_storage.get_token(&provider_id).ok().flatten();
    let token_info_clone = token_info.clone();
    let provider_config = config
        .providers
        .iter()
        .find(|p| p.provider_type == provider_id);
    let is_configured = provider_config
        .map(|provider| {
            !provider.api_key.is_empty()
                || provider.auth_type.as_deref() == Some("oauth")
                || token_info_clone.is_some()
        })
        .unwrap_or_else(|| token_info_clone.is_some());

    let status = if is_configured {
        if let Some(info) = token_info {
            if let Some(expires_at) = info.expires_at {
                let now = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .unwrap()
                    .as_secs() as i64;
                if expires_at < now {
                    "expired"
                } else {
                    "active"
                }
            } else {
                "active"
            }
        } else {
            "active"
        }
    } else {
        "not_configured"
    };

    Ok(Json(serde_json::json!({
        "provider_id": provider_id,
        "status": status,
        "configured": is_configured,
        "expires_at": token_info_clone.as_ref().and_then(|v| v.expires_at),
        "account": token_info_clone.as_ref().and_then(|v| v.account_id.clone()),
    })))
}

async fn oauth_refresh(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(provider_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let config_dir = config_path.parent().unwrap_or(&config_path).to_path_buf();
    let oauth_storage =
        crate::oauth::OAuthStorage::new(crate::oauth::get_oauth_storage_path(&config_dir));

    let token_info = oauth_storage
        .get_token(&provider_id)
        .map_err(|_| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "No OAuth token found for provider".to_string(),
                }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: "No OAuth token found for provider".to_string(),
                }),
            )
        })?;

    let refresh_token = token_info.refresh_token.as_ref().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No refresh token found".to_string(),
            }),
        )
    })?;

    let config = agent.get_config().await;
    let provider_config = config
        .providers
        .iter()
        .find(|p| p.provider_type == provider_id);
    let provider_defaults = get_oauth_provider(&provider_id).ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("OAuth not supported for provider: {}", provider_id),
            }),
        )
    })?;

    let (client_id, client_secret, token_url): (String, String, String) = if let Some(provider) =
        provider_config
    {
        let client_id = provider
            .oauth_client_id
            .clone()
            .or_else(|| {
                std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase())).ok()
            })
            .unwrap_or_else(|| provider_defaults.client_id.to_string());
        let client_secret = provider
            .oauth_client_secret
            .clone()
            .or_else(|| {
                std::env::var(format!(
                    "{}_OAUTH_CLIENT_SECRET",
                    provider_id.to_uppercase()
                ))
                .ok()
            })
            .unwrap_or_default();
        let token_url = provider
            .oauth_token_url
            .clone()
            .unwrap_or_else(|| provider_defaults.token_url.to_string());
        (client_id, client_secret, token_url)
    } else {
        let client_id = std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase()))
            .unwrap_or_else(|_| provider_defaults.client_id.to_string());
        let client_secret = std::env::var(format!(
            "{}_OAUTH_CLIENT_SECRET",
            provider_id.to_uppercase()
        ))
        .unwrap_or_default();
        let token_url = provider_defaults.token_url.to_string();
        (client_id, client_secret, token_url)
    };

    if client_id.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "OAuth client ID not configured".to_string(),
            }),
        ));
    }

    let client = reqwest::Client::new();
    let mut form = vec![
        ("grant_type".to_string(), "refresh_token".to_string()),
        ("client_id".to_string(), client_id.clone()),
        ("refresh_token".to_string(), refresh_token.to_string()),
    ];
    if !client_secret.trim().is_empty() {
        form.push(("client_secret".to_string(), client_secret));
    }
    let token_response = client
        .post(&token_url)
        .form(&form)
        .send()
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: format!("Failed to refresh token: {}", e),
                }),
            )
        })?;

    if !token_response.status().is_success() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "Failed to refresh OAuth token".to_string(),
            }),
        ));
    }

    let token_data: serde_json::Value = token_response.json().await.map_err(|e| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!("Failed to parse refresh response: {}", e),
            }),
        )
    })?;

    let access_token = token_data["access_token"].as_str().unwrap_or_default();
    let new_refresh_token = token_data.get("refresh_token").and_then(|v| v.as_str());
    let expires_in = token_data.get("expires_in").and_then(|v| v.as_i64());
    let expires_at = expires_in.map(|e| {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64
            + e
    });

    let mut config = agent.get_config().await;
    if let Some(provider) = config
        .providers
        .iter_mut()
        .find(|p| p.provider_type == provider_id)
    {
        provider.api_key = access_token.to_string();
    }

    let new_token_entry = crate::oauth::OAuthTokenEntry {
        access_token: access_token.to_string(),
        refresh_token: new_refresh_token
            .map(|s| s.to_string())
            .or(token_info.refresh_token),
        expires_at,
        scopes: token_info.scopes,
        account_id: crate::oauth::extract_account_id(
            token_data.get("id_token").and_then(|v| v.as_str()),
            Some(access_token),
        )
        .or(token_info.account_id),
    };

    if let Err(e) = oauth_storage.set_token(&provider_id, new_token_entry) {
        tracing::warn!("Failed to save OAuth tokens: {}", e);
    }

    agent.replace_config(config.clone()).await;
    config.save(&config_path).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to save config: {}", e),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "OAuth token refreshed",
        "provider_id": provider_id,
        "expires_at": expires_at,
    })))
}

async fn oauth_revoke(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Path(provider_id): Path<String>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let config_dir = config_path.parent().unwrap_or(&config_path).to_path_buf();
    let oauth_storage =
        crate::oauth::OAuthStorage::new(crate::oauth::get_oauth_storage_path(&config_dir));

    let token_info = oauth_storage.get_token(&provider_id).ok().flatten();

    if let Some(token_info) = token_info {
        if let Some(refresh_token) = token_info.refresh_token {
            let config = agent.get_config().await;
            let provider_config = config
                .providers
                .iter()
                .find(|p| p.provider_type == provider_id);

            let (client_id, client_secret, revoke_url) = if let Some(provider) = provider_config {
                let client_id = provider
                    .oauth_client_id
                    .clone()
                    .or_else(|| {
                        std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase()))
                            .ok()
                    })
                    .unwrap_or_default();
                let client_secret = provider
                    .oauth_client_secret
                    .clone()
                    .or_else(|| {
                        std::env::var(format!(
                            "{}_OAUTH_CLIENT_SECRET",
                            provider_id.to_uppercase()
                        ))
                        .ok()
                    })
                    .unwrap_or_default();
                let revoke_url = provider
                    .oauth_token_url
                    .as_ref()
                    .map(|u| u.replace("/token", "/revoke"))
                    .unwrap_or_else(|| match provider_id.as_str() {
                        "openai" => "https://auth.openai.com/oauth/revoke".to_string(),
                        "anthropic" => "https://auth.anthropic.com/oauth/revoke".to_string(),
                        "google_ai" => "https://oauth2.googleapis.io/revoke".to_string(),
                        _ => "".to_string(),
                    });
                (client_id, client_secret, revoke_url)
            } else {
                let client_id =
                    std::env::var(format!("{}_OAUTH_CLIENT_ID", provider_id.to_uppercase()))
                        .unwrap_or_default();
                let client_secret = std::env::var(format!(
                    "{}_OAUTH_CLIENT_SECRET",
                    provider_id.to_uppercase()
                ))
                .unwrap_or_default();
                let revoke_url = match provider_id.as_str() {
                    "openai" => "https://auth.openai.com/oauth/revoke",
                    "anthropic" => "https://auth.anthropic.com/oauth/revoke",
                    "google_ai" => "https://oauth2.googleapis.io/revoke",
                    _ => "",
                };
                (client_id, client_secret, revoke_url.to_string())
            };

            if !revoke_url.is_empty() {
                let client = reqwest::Client::new();
                let _ = client
                    .post(&revoke_url)
                    .form(&[
                        ("token", &refresh_token),
                        ("client_id", &client_id),
                        ("client_secret", &client_secret),
                    ])
                    .send()
                    .await;
            }
        }
    }

    let _ = oauth_storage.remove_token(&provider_id);

    agent
        .remove_provider(provider_id.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "message": "OAuth connection revoked and provider removed"
    })))
}

async fn switch_provider_model(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(payload): Json<serde_json::Value>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let provider_id = payload["provider_id"].as_str().unwrap_or("").to_string();
    let model = payload["model"].as_str().unwrap_or("").to_string();

    if provider_id.is_empty() || model.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "provider_id and model are required".to_string(),
            }),
        ));
    }

    agent
        .switch_provider_model(provider_id.clone(), model.clone())
        .await
        .map_err(|e| {
            (
                StatusCode::BAD_REQUEST,
                Json(ErrorResponse {
                    error: e.to_string(),
                }),
            )
        })?;

    agent.save_config(&config_path).await.map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: e.to_string(),
            }),
        )
    })?;

    Ok(Json(serde_json::json!({
        "success": true,
        "provider_id": provider_id,
        "model": model,
        "message": format!("Switched to {}/{}", provider_id, model)
    })))
}

async fn search_models(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let query = params.get("q").cloned().unwrap_or_default();
    let models = agent.search_catalog_models(query).await;
    Json(serde_json::to_value(models).unwrap_or(serde_json::json!([])))
}

#[derive(Deserialize)]
struct AnswerQuestionRequest {
    question_id: String,
    answers: Vec<Vec<String>>,
}

async fn answer_question(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(body): Json<AnswerQuestionRequest>,
) -> Json<serde_json::Value> {
    let found = agent.answer_question(&body.question_id, body.answers).await;
    Json(serde_json::json!({
        "success": found,
        "message": if found { "Answer received" } else { "Question not found or already answered" }
    }))
}

async fn catalog_handler(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Json<serde_json::Value> {
    let catalog = agent.get_catalog_state().await;
    Json(serde_json::to_value(&catalog).unwrap_or(serde_json::json!({})))
}

async fn models_handler(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
) -> Json<serde_json::Value> {
    let provider_id = params.get("provider_id").cloned().unwrap_or_default();
    let models = if !provider_id.is_empty() {
        agent.get_provider_models(provider_id).await
    } else {
        let catalog = agent.get_catalog_state().await;
        catalog.all_models
    };
    Json(serde_json::to_value(&models).unwrap_or(serde_json::json!([])))
}

#[derive(Debug, Serialize)]
pub struct ChildSessionsResponse {
    pub sessions: Vec<Session>,
}

async fn get_child_sessions(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<ChildSessionsResponse>, StatusCode> {
    match agent.get_child_sessions(&id).await {
        Ok(sessions) => Ok(Json(ChildSessionsResponse { sessions })),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Debug, Serialize)]
pub struct ParentSessionResponse {
    pub session: Option<Session>,
}

async fn get_parent_session(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<ParentSessionResponse>, StatusCode> {
    match agent.get_parent_session(&id).await {
        Ok(session) => Ok(Json(ParentSessionResponse { session })),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Debug, Serialize)]
pub struct SessionSubagentTaskResponse {
    pub id: String,
    pub session_id: String,
    pub description: String,
    pub prompt: String,
    pub agent_type: String,
    pub status: String,
    pub tool_count: i32,
    pub result: Option<String>,
    pub is_running: bool,
    pub created_at: i64,
    pub completed_at: Option<i64>,
    pub context_state: Option<crate::storage::SessionContextState>,
}

#[derive(Debug, Serialize)]
pub struct SessionSubagentsResponse {
    pub subagents: Vec<SessionSubagentTaskResponse>,
    pub has_running: bool,
}

async fn get_session_subagents(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<SessionSubagentsResponse>, (StatusCode, Json<ErrorResponse>)> {
    match agent.list_subagent_tasks(&id).await {
        Ok(tasks) => {
            let has_running = agent.is_any_subagent_running(&id);
            let mut subagents = Vec::new();
            for t in tasks {
                let is_running = agent
                    .get_subagent_manager()
                    .is_subagent_running(&t.session_id);
                let context_state = agent
                    .get_session(&t.session_id)
                    .await
                    .ok()
                    .and_then(|s| s)
                    .and_then(|s| s.context_state);
                subagents.push(SessionSubagentTaskResponse {
                    id: t.id,
                    session_id: t.session_id,
                    description: t.description,
                    prompt: t.prompt,
                    agent_type: t.agent_type,
                    status: t.status,
                    tool_count: t.tool_count,
                    result: t.result,
                    is_running,
                    created_at: t.created_at.timestamp_millis(),
                    completed_at: t.completed_at.map(|dt| dt.timestamp_millis()),
                    context_state,
                });
            }
            Ok(Json(SessionSubagentsResponse {
                subagents,
                has_running,
            }))
        }
        Err(_) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: "Failed to list subagent tasks".to_string(),
            }),
        )),
    }
}

#[derive(Debug, Serialize)]
pub struct SubagentStatusResponse {
    pub session_id: String,
    pub status: String,
    pub agent_type: String,
    pub description: String,
    pub tool_count: i32,
    pub is_running: bool,
}

async fn get_subagent_status(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<SubagentStatusResponse>, StatusCode> {
    match agent.get_subagent_status(&id).await {
        Ok((session, task, is_running)) => Ok(Json(SubagentStatusResponse {
            session_id: session.id.clone(),
            status: session.task_status.clone(),
            agent_type: session.agent_type.clone(),
            description: task.description.clone(),
            tool_count: task.tool_count,
            is_running,
        })),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

#[derive(Debug, Serialize)]
pub struct SubagentResultResponse {
    pub status: String,
    pub result: Option<String>,
    pub tool_count: i32,
    pub completed_at: Option<i64>,
}

async fn get_subagent_result(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<SubagentResultResponse>, StatusCode> {
    match agent.get_subagent_result(&id).await {
        Ok(task) => Ok(Json(SubagentResultResponse {
            status: task.status.clone(),
            result: task.result.clone(),
            tool_count: task.tool_count,
            completed_at: task.completed_at.map(|dt| dt.timestamp()),
        })),
        Err(_) => Err(StatusCode::NOT_FOUND),
    }
}

async fn cancel_subagent(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<Json<serde_json::Value>, StatusCode> {
    match agent.cancel_subagent(&id).await {
        Ok(true) => Ok(Json(serde_json::json!({
            "success": true,
            "message": "Subagent cancelled"
        }))),
        Ok(false) => Ok(Json(serde_json::json!({
            "success": false,
            "message": "Subagent not found or already completed"
        }))),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

#[derive(Debug, Deserialize)]
pub struct CleanupRequest {
    pub days: Option<i64>,
}

#[derive(Debug, Serialize)]
pub struct CleanupResponse {
    pub removed_count: usize,
}

async fn cleanup_subagents(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(req): Json<CleanupRequest>,
) -> Result<Json<CleanupResponse>, StatusCode> {
    let days = req.days.unwrap_or(7);
    match agent.cleanup_completed_subagents(days).await {
        Ok(count) => Ok(Json(CleanupResponse {
            removed_count: count,
        })),
        Err(_) => Err(StatusCode::INTERNAL_SERVER_ERROR),
    }
}

async fn voice_models() -> Json<crate::voice::VoiceModelsResponse> {
    Json(crate::voice::get_available_models())
}

async fn voice_installed() -> Json<crate::voice::InstalledModelsResponse> {
    Json(crate::voice::get_installed_models())
}

#[derive(Debug, Deserialize)]
pub struct VoiceDownloadRequest {
    pub model_type: String,
    pub model_id: String,
}

async fn voice_download(
    Json(payload): Json<VoiceDownloadRequest>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let model_type = payload.model_type.clone();
    let model_id = payload.model_id.clone();

    tokio::spawn(async move {
        match model_type.as_str() {
            "whisper" => crate::voice::whisper::download_model(&model_id).await,
            "piper" => crate::voice::piper::download_voice(&model_id).await,
            _ => Err(format!("Unknown model type: {}", model_type)),
        }
    });

    Ok(Json(serde_json::json!({
        "success": true,
        "message": format!("Download started for {} {}", payload.model_type, payload.model_id)
    })))
}

async fn voice_progress() -> Sse<impl Stream<Item = Result<Event, Infallible>>> {
    let mut receiver = crate::voice::get_progress_receiver();

    let stream = async_stream::stream! {
        while let Ok(progress) = receiver.recv().await {
            let data = serde_json::to_string(&progress).unwrap_or_default();
            yield Ok(Event::default().event("progress").data(data));
        }
    };

    Sse::new(stream)
}

async fn voice_upload(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(params): Query<std::collections::HashMap<String, String>>,
    body: axum::body::Bytes,
) -> Result<Json<crate::voice::UploadResponse>, (StatusCode, Json<ErrorResponse>)> {
    let config = agent.get_config().await;
    let _voice_config = config.voice.as_ref().ok_or((
        StatusCode::BAD_REQUEST,
        Json(ErrorResponse {
            error: "Voice not configured".to_string(),
        }),
    ))?;

    let model_type = params.get("type").map(|s| s.as_str()).unwrap_or("whisper");
    if model_type != "whisper" && model_type != "piper" {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: format!(
                    "Invalid model type '{}'. Must be 'whisper' or 'piper'.",
                    model_type
                ),
            }),
        ));
    }

    let bytes = body.to_vec();

    if bytes.is_empty() {
        return Err((
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No file data received".to_string(),
            }),
        ));
    }

    if bytes.len() > 500 * 1024 * 1024 {
        return Err((
            StatusCode::PAYLOAD_TOO_LARGE,
            Json(ErrorResponse {
                error: "File too large (max 500MB)".to_string(),
            }),
        ));
    }

    let models_dir = crate::voice::get_models_dir();
    std::fs::create_dir_all(&models_dir).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to create models dir: {}", e),
            }),
        )
    })?;

    let timestamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let (model_id, filename, file_path) = if model_type == "whisper" {
        let id = format!("custom_whisper_{}", timestamp);
        let name = format!("ggml-{}.bin", id);
        (id, name.clone(), models_dir.join(&name))
    } else {
        let id = format!("custom_piper_{}", timestamp);
        let name = format!("{}.onnx", id);
        (id, name.clone(), models_dir.join(&name))
    };

    std::fs::write(&file_path, &bytes).map_err(|e| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse {
                error: format!("Failed to write file: {}", e),
            }),
        )
    })?;

    let size_bytes = bytes.len() as u64;
    let model = crate::voice::InstalledModel {
        id: model_id.clone(),
        model_type: model_type.to_string(),
        name: format!("Custom {} {}", model_type, model_id),
        path: file_path.to_string_lossy().to_string(),
        size_bytes,
    };

    Ok(Json(crate::voice::UploadResponse {
        success: true,
        message: format!(
            "Uploaded custom {} model ({} bytes)",
            model_type, size_bytes
        ),
        model: Some(model),
    }))
}

async fn voice_delete_model(
    Path((model_type, model_id)): Path<(String, String)>,
) -> Result<Json<serde_json::Value>, (StatusCode, Json<ErrorResponse>)> {
    let result = match model_type.as_str() {
        "whisper" => crate::voice::whisper::delete_model(&model_id),
        "piper" => crate::voice::piper::delete_voice(&model_id),
        _ => Err(format!("Unknown model type: {}", model_type)),
    };

    match result {
        Ok(()) => Ok(Json(serde_json::json!({
            "success": true,
            "message": format!("Deleted {} model '{}'", model_type, model_id)
        }))),
        Err(e) => Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(ErrorResponse { error: e }),
        )),
    }
}

#[derive(Debug, Deserialize)]
struct CheckUpdateQuery {
    channel: Option<String>,
}

async fn check_update(
    Extension(config): Extension<Config>,
    Query(params): Query<CheckUpdateQuery>,
) -> Result<Json<crate::update::UpdateCheckResult>, (StatusCode, Json<ErrorResponse>)> {
    let channel = params
        .channel
        .as_deref()
        .or(Some(config.update.channel.as_str()))
        .and_then(|c| c.parse::<crate::update::UpdateChannel>().ok())
        .unwrap_or(crate::update::UpdateChannel::Stable);

    let checker = crate::update::UpdateChecker::new(crate::update::build_version());

    let result = checker.check(channel).await;

    Ok(Json(result))
}

#[derive(Debug, Deserialize)]
struct DownloadUpdateRequest {
    tag: String,
    channel: Option<String>,
}

#[derive(Debug, Serialize)]
struct UpdateStatusResponse {
    status: String,
    progress: Option<f32>,
    bytes_downloaded: Option<u64>,
    total_bytes: Option<u64>,
    tag: Option<String>,
    version: Option<String>,
    message: Option<String>,
}

async fn download_update(
    Extension(config): Extension<Config>,
    Json(payload): Json<DownloadUpdateRequest>,
) -> Result<Json<UpdateStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let channel = payload
        .channel
        .as_ref()
        .and_then(|c| c.parse::<crate::update::UpdateChannel>().ok())
        .unwrap_or(crate::update::UpdateChannel::Stable);

    let installer = crate::update::UpdateInstaller::new();

    let (tag, archive_name, download_url) = installer
        .find_release_for_platform(channel)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
        })?
        .ok_or_else(|| {
            (
                StatusCode::NOT_FOUND,
                Json(ErrorResponse {
                    error: "No release found for this platform and channel".to_string(),
                }),
            )
        })?;

    let archive_path = installer
        .download_release(&download_url, &tag, &archive_name, |downloaded, total| {
            tracing::debug!(
                "Download progress: {}/{} bytes ({:.1}%)",
                downloaded,
                total,
                if total > 0 {
                    (downloaded as f64 / total as f64 * 100.0) as f32
                } else {
                    0.0
                }
            );
        })
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Download failed: {}", e),
                }),
            )
        })?;

    let staged_update = installer
        .prepare_update(&archive_path, &tag)
        .await
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to prepare update: {}", e),
                }),
            )
        })?;

    if archive_path != staged_update {
        let _ = std::fs::remove_file(&archive_path).ok();
    }

    installer
        .mark_prepared_update(&tag, &staged_update)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse {
                    error: format!("Failed to mark update as prepared: {}", e),
                }),
            )
        })?;

    let version = tag.trim_start_matches('v').to_string();

    Ok(Json(UpdateStatusResponse {
        status: "ready".to_string(),
        progress: Some(100.0),
        bytes_downloaded: None,
        total_bytes: None,
        tag: Some(tag),
        version: Some(version),
        message: Some("Update ready to install".to_string()),
    }))
}

async fn install_update(
    Extension(config): Extension<Config>,
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(payload): Json<DownloadUpdateRequest>,
) -> Result<Json<UpdateStatusResponse>, (StatusCode, Json<ErrorResponse>)> {
    let installer = crate::update::UpdateInstaller::new();

    let pending = crate::update::get_prepared_update().ok_or_else(|| {
        (
            StatusCode::BAD_REQUEST,
            Json(ErrorResponse {
                error: "No pending update found. Download an update first.".to_string(),
            }),
        )
    })?;

    installer
        .mark_update_pending(&pending.tag, &pending.staged_path, true)
        .map_err(|e| {
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(ErrorResponse { error: e }),
            )
        })?;

    let _ = installer.clear_prepared_update();

    agent.signal_shutdown();

    Ok(Json(UpdateStatusResponse {
        status: "restarting".to_string(),
        progress: None,
        bytes_downloaded: None,
        total_bytes: None,
        tag: Some(pending.tag),
        version: None,
        message: Some("Shutting down for update...".to_string()),
    }))
}

async fn update_status() -> Json<UpdateStatusResponse> {
    let pending = crate::update::get_pending_update();

    if let Some(pending) = pending {
        return Json(UpdateStatusResponse {
            status: "ready".to_string(),
            progress: Some(100.0),
            bytes_downloaded: None,
            total_bytes: None,
            tag: Some(pending.tag.clone()),
            version: Some(pending.tag.trim_start_matches('v').to_string()),
            message: Some("Update ready to install".to_string()),
        });
    }

    let prepared = crate::update::get_prepared_update();

    if let Some(prepared) = prepared {
        return Json(UpdateStatusResponse {
            status: "ready".to_string(),
            progress: Some(100.0),
            bytes_downloaded: None,
            total_bytes: None,
            tag: Some(prepared.tag.clone()),
            version: Some(prepared.tag.trim_start_matches('v').to_string()),
            message: Some("Update ready to install".to_string()),
        });
    }

    Json(UpdateStatusResponse {
        status: "idle".to_string(),
        progress: None,
        bytes_downloaded: None,
        total_bytes: None,
        tag: None,
        version: None,
        message: None,
    })
}
