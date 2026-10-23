//! GitHub integration API: connection (token / OAuth device flow),
//! settings, repository picker, review history, and manual review runs.
//! All routes live under `/api/github` behind the standard auth layer.

use crate::agent::runtime::AgentRuntime;
use crate::config::GithubConfig;
use crate::github::client::GithubClient;
use crate::github::engine::{normalize_repo, DeviceFlowPoll, ReviewOptions};
use axum::{
    extract::Query,
    http::StatusCode,
    response::{IntoResponse, Json, Response},
    routing::{get, post},
    Extension, Router,
};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;

pub enum AppError {
    Internal(String),
    BadRequest(String),
}

impl IntoResponse for AppError {
    fn into_response(self) -> Response {
        let (status, msg) = match self {
            AppError::Internal(msg) => (StatusCode::INTERNAL_SERVER_ERROR, msg),
            AppError::BadRequest(msg) => (StatusCode::BAD_REQUEST, msg),
        };
        (status, Json(serde_json::json!({ "error": msg }))).into_response()
    }
}

impl From<crate::error::OSAgentError> for AppError {
    fn from(error: crate::error::OSAgentError) -> Self {
        let message = error.to_string();
        let lower = message.to_ascii_lowercase();
        if lower.contains("not connected")
            || lower.contains("settings > github")
            || lower.contains("is required")
            || lower.contains("invalid repository")
            || lower.contains("device flow")
            || lower.contains("private key")
            || lower.contains("app id")
        {
            AppError::BadRequest(message)
        } else {
            AppError::Internal(message)
        }
    }
}

/// Wire view of the GitHub settings (never includes the token).
#[derive(Debug, Serialize)]
pub struct GithubConfigView {
    pub has_token: bool,
    pub oauth_client_id: String,
    pub auto_review: bool,
    pub trigger_mentions: bool,
    pub mention_handle: String,
    pub watch_repos: Vec<String>,
    pub poll_seconds: u64,
    pub include_drafts: bool,
    pub ignore_authors: Vec<String>,
    pub max_inline_comments: usize,
    pub review_event: String,
    pub review_model: String,
    pub review_instructions: String,
}

impl From<&GithubConfig> for GithubConfigView {
    fn from(config: &GithubConfig) -> Self {
        Self {
            has_token: !config.token.trim().is_empty(),
            oauth_client_id: config.oauth_client_id.clone(),
            auto_review: config.auto_review,
            trigger_mentions: config.trigger_mentions,
            mention_handle: config.mention_handle.clone(),
            watch_repos: config.watch_repos.clone(),
            poll_seconds: config.poll_interval_secs(),
            include_drafts: config.include_drafts,
            ignore_authors: config.ignore_authors.clone(),
            max_inline_comments: config.max_inline_comments,
            review_event: config.normalized_review_event().to_string(),
            review_model: config.review_model.clone(),
            review_instructions: config.review_instructions.clone(),
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct GithubConfigUpdate {
    pub oauth_client_id: Option<String>,
    pub auto_review: Option<bool>,
    pub trigger_mentions: Option<bool>,
    pub mention_handle: Option<String>,
    pub watch_repos: Option<Vec<String>>,
    pub poll_seconds: Option<u64>,
    pub include_drafts: Option<bool>,
    pub ignore_authors: Option<Vec<String>>,
    pub max_inline_comments: Option<usize>,
    pub review_event: Option<String>,
    pub review_model: Option<String>,
    pub review_instructions: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct TokenRequest {
    pub token: String,
}

#[derive(Debug, Deserialize)]
pub struct RunReviewRequest {
    pub repo: String,
    pub number: u64,
    #[serde(default)]
    pub post: Option<bool>,
    #[serde(default)]
    pub force: Option<bool>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

#[derive(Debug, Deserialize)]
pub struct ReviewsQuery {
    pub limit: Option<usize>,
    pub repo: Option<String>,
}

#[derive(Debug, Serialize)]
pub struct StatusResponse {
    pub configured: bool,
    pub token_source: Option<String>,
    pub login: Option<String>,
    pub name: Option<String>,
    pub avatar_url: Option<String>,
    pub html_url: Option<String>,
    pub scopes: Vec<String>,
    pub auth_error: Option<String>,
    pub auto_review: bool,
    pub trigger_mentions: bool,
    pub watch_repos: Vec<String>,
    pub poll_seconds: u64,
    /// Whether "Sign in with GitHub" has a usable client id.
    pub oauth_client_id_set: bool,
    /// "custom" (user-configured), "shared" (baked into this build), or
    /// "none". Drives the setup hint in the settings pane.
    pub oauth_app_source: crate::github::OAuthAppSource,
    /// In-flight GitHub sign-in, so a reloaded pane can resume it.
    pub device_flow: Option<crate::github::engine::DeviceFlowSnapshot>,
    pub last_poll_at: Option<String>,
    pub last_poll_error: Option<String>,
    pub poll_running: bool,
    pub reviewing: Vec<String>,
}

/// Apply a mutation to the `[github]` section, hot-swap it into the
/// runtime, and persist. Secrets are untouched here (token flows through
/// its own endpoints).
async fn mutate_github_config(
    runtime: &Arc<AgentRuntime>,
    config_path: &std::path::Path,
    mutate: impl FnOnce(&mut GithubConfig),
) -> Result<GithubConfig, AppError> {
    let mut config = runtime.get_config().await;
    mutate(&mut config.github);
    config.github.poll_seconds = config.github.poll_interval_secs();
    runtime.replace_config(config.clone()).await;
    runtime.save_config(config_path).await?;
    Ok(config.github)
}

async fn get_config(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<GithubConfigView>, AppError> {
    let config = runtime.get_config().await;
    Ok(Json(GithubConfigView::from(&config.github)))
}

async fn update_config(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(update): Json<GithubConfigUpdate>,
) -> Result<Json<GithubConfigView>, AppError> {
    if let Some(event) = update.review_event.as_deref() {
        let normalized = event.trim().to_ascii_uppercase();
        if !["COMMENT", "REQUEST_CHANGES", "APPROVE"].contains(&normalized.as_str()) {
            return Err(AppError::BadRequest(
                "review_event must be COMMENT, REQUEST_CHANGES, or APPROVE".to_string(),
            ));
        }
    }
    if let Some(instructions) = update.review_instructions.as_deref() {
        if instructions.chars().count() > 8_000 {
            return Err(AppError::BadRequest(
                "review_instructions is limited to 8000 characters".to_string(),
            ));
        }
    }

    // Mention handle: store without the leading '@' and validate the shape
    // GitHub accepts for usernames (letters, digits, hyphen; underscore for
    // legacy accounts). Matching is case-insensitive.
    let mut normalized_handle: Option<String> = None;
    if let Some(handle) = update.mention_handle.as_deref() {
        let cleaned = handle.trim().trim_start_matches('@').to_string();
        if cleaned.is_empty() {
            normalized_handle = Some(String::new());
        } else {
            if cleaned.chars().count() > 39
                || !cleaned
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
            {
                return Err(AppError::BadRequest(
                    "mention_handle must be a GitHub username (letters, digits, hyphens; max 39 characters)"
                        .to_string(),
                ));
            }
            normalized_handle = Some(cleaned);
        }
    }

    let mut normalized_repos: Option<Vec<String>> = None;
    if let Some(repos) = &update.watch_repos {
        let mut cleaned: Vec<String> = Vec::new();
        for repo in repos {
            if repo.trim().is_empty() {
                continue;
            }
            let normalized = normalize_repo(repo)?;
            if !cleaned.contains(&normalized) {
                cleaned.push(normalized);
            }
        }
        normalized_repos = Some(cleaned);
    }

    let config = mutate_github_config(&runtime, &config_path, |github| {
        if let Some(value) = update.oauth_client_id {
            github.oauth_client_id = value.trim().to_string();
        }
        if let Some(value) = update.auto_review {
            github.auto_review = value;
        }
        if let Some(value) = update.trigger_mentions {
            github.trigger_mentions = value;
        }
        if let Some(handle) = normalized_handle {
            github.mention_handle = handle;
        }
        // A mention trigger without a handle can never fire; keep the
        // stored state honest instead of silently dead.
        if github.normalized_mention_handle().is_empty() {
            github.trigger_mentions = false;
        }
        if let Some(repos) = normalized_repos {
            github.watch_repos = repos;
        }
        if let Some(value) = update.poll_seconds {
            github.poll_seconds = value;
        }
        if let Some(value) = update.include_drafts {
            github.include_drafts = value;
        }
        if let Some(authors) = update.ignore_authors {
            github.ignore_authors = authors
                .into_iter()
                .map(|author| author.trim().to_string())
                .filter(|author| !author.is_empty())
                .collect();
        }
        if let Some(value) = update.max_inline_comments {
            github.max_inline_comments = value.clamp(1, 100);
        }
        if let Some(value) = update.review_event {
            github.review_event = value.trim().to_ascii_uppercase();
        }
        if let Some(value) = update.review_model {
            github.review_model = value.trim().to_string();
        }
        if let Some(value) = update.review_instructions {
            github.review_instructions = value.trim().to_string();
        }
    })
    .await?;
    Ok(Json(GithubConfigView::from(&config)))
}

/// Validate a token against GET /user, then persist it.
async fn set_token(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
    Json(request): Json<TokenRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let token = request.token.trim().to_string();
    if token.is_empty() {
        return Err(AppError::BadRequest("token is required".to_string()));
    }
    let client = GithubClient::new(token.clone());
    let (user, scopes) = client
        .whoami()
        .await
        .map_err(|error| AppError::BadRequest(format!("Token check failed: {}", error)))?;
    mutate_github_config(&runtime, &config_path, |github| {
        github.token = token;
        // A pasted PAT is not refreshable; drop any sign-in leftovers.
        github.token_refresh.clear();
        github.token_expires_at = 0;
        github.token_client_id.clear();
    })
    .await?;
    Ok(Json(serde_json::json!({
        "login": user.login,
        "name": user.name,
        "avatar_url": user.avatar_url,
        "html_url": user.html_url,
        "scopes": scopes,
    })))
}

async fn clear_token(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
) -> Result<StatusCode, AppError> {
    mutate_github_config(&runtime, &config_path, |github| {
        github.token.clear();
        github.token_refresh.clear();
        github.token_expires_at = 0;
        github.token_client_id.clear();
    })
    .await?;
    Ok(StatusCode::NO_CONTENT)
}

async fn get_status(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<StatusResponse>, AppError> {
    let config = runtime.get_config().await.github;
    let engine = runtime.github();
    let snapshot = engine.status_snapshot().await;

    let mut response = StatusResponse {
        configured: false,
        token_source: None,
        login: None,
        name: None,
        avatar_url: None,
        html_url: None,
        scopes: Vec::new(),
        auth_error: None,
        auto_review: config.auto_review,
        trigger_mentions: config.trigger_mentions,
        watch_repos: config.watch_repos.clone(),
        poll_seconds: config.poll_interval_secs(),
        oauth_client_id_set: !crate::github::effective_oauth_client_id(&config.oauth_client_id)
            .is_empty(),
        oauth_app_source: crate::github::oauth_app_source(&config.oauth_client_id),
        device_flow: engine.device_flow_snapshot().await,
        last_poll_at: snapshot.last_poll_at,
        last_poll_error: snapshot.last_poll_error,
        poll_running: snapshot.poll_running,
        reviewing: snapshot.reviewing,
    };

    if let Some((token, source)) = crate::github::client::resolve_token(
        &runtime.get_config().await,
    ) {
        response.configured = true;
        response.token_source = Some(source.as_str().to_string());
        let client = GithubClient::new(token);
        match client.whoami().await {
            Ok((user, scopes)) => {
                response.login = Some(user.login);
                response.name = user.name;
                response.avatar_url = user.avatar_url;
                response.html_url = user.html_url;
                response.scopes = scopes;
            }
            Err(error) => {
                response.auth_error = Some(error.to_string());
            }
        }
    }
    Ok(Json(response))
}

async fn list_repos(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let engine = runtime.github();
    let resolved = engine.resolve().await?;
    let repos = resolved.client.list_user_repos(3).await?;
    let items = repos
        .into_iter()
        .map(|repo| {
            serde_json::json!({
                "full_name": repo.full_name,
                "private": repo.private,
                "description": repo.description,
                "pushed_at": repo.pushed_at,
                "default_branch": repo.default_branch,
                "can_push": repo
                    .permissions
                    .as_ref()
                    .and_then(|p| p.push)
                    .unwrap_or(false),
            })
        })
        .collect::<Vec<_>>();
    Ok(Json(serde_json::json!({ "repos": items })))
}

async fn list_reviews(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Query(query): Query<ReviewsQuery>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo = query
        .repo
        .as_deref()
        .map(str::trim)
        .filter(|repo| !repo.is_empty())
        .map(normalize_repo)
        .transpose()?;
    let reviews = runtime
        .github()
        .storage()
        .list_github_reviews(repo.as_deref(), query.limit.unwrap_or(50))?;
    Ok(Json(serde_json::json!({ "reviews": reviews })))
}

async fn run_review(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Json(request): Json<RunReviewRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    let repo = normalize_repo(&request.repo)?;
    if request.number == 0 {
        return Err(AppError::BadRequest(
            "number is required (pull request number)".to_string(),
        ));
    }
    let engine = runtime.github();
    let outcome = engine
        .review_pr(
            &repo,
            request.number,
            ReviewOptions {
                post: request.post.unwrap_or(true),
                force: request.force.unwrap_or(true),
                event: request.event,
                model: request.model,
                source: "manual",
            },
        )
        .await?;
    Ok(Json(serde_json::json!(outcome)))
}

async fn poll_now(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let engine = runtime.github();
    tokio::spawn(async move {
        if let Err(error) = engine.poll_once().await {
            tracing::warn!("GitHub manual poll failed: {}", error);
        }
    });
    Ok(Json(serde_json::json!({ "started": true })))
}

async fn oauth_start(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, AppError> {
    let engine = runtime.github();
    let start = engine.start_device_flow().await?;
    Ok(Json(serde_json::json!({
        "resumed": start.resumed,
        "user_code": start.snapshot.user_code,
        "verification_uri": start.snapshot.verification_uri,
        "verification_uri_complete": start.snapshot.verification_uri_complete,
        "expires_in": start.snapshot.expires_in,
        "interval": start.snapshot.interval,
    })))
}

async fn oauth_cancel(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
) -> Result<Json<serde_json::Value>, AppError> {
    runtime.github().cancel_device_flow().await;
    Ok(Json(serde_json::json!({ "status": "cancelled" })))
}

async fn oauth_poll(
    Extension(runtime): Extension<Arc<AgentRuntime>>,
    Extension(config_path): Extension<PathBuf>,
) -> Result<Json<serde_json::Value>, AppError> {
    let engine = runtime.github();
    match engine.poll_device_flow().await? {
        DeviceFlowPoll::Pending => Ok(Json(serde_json::json!({ "status": "pending" }))),
        DeviceFlowPoll::Expired => Ok(Json(serde_json::json!({ "status": "expired" }))),
        DeviceFlowPoll::Complete {
            token,
            refresh_token,
            expires_in,
            client_id,
            login,
            scopes,
        } => {
            mutate_github_config(&runtime, &config_path, |github| {
                github.token = token;
                // Expiring apps return a refresh token; OSA renews in the
                // background from here on.
                github.token_refresh = refresh_token.unwrap_or_default();
                github.token_expires_at = expires_in
                    .map(|secs| chrono::Utc::now().timestamp() + secs as i64)
                    .unwrap_or(0);
                github.token_client_id = client_id;
            })
            .await?;
            Ok(Json(serde_json::json!({
                "status": "complete",
                "login": login,
                "scopes": scopes,
            })))
        }
    }
}

#[derive(Debug, Deserialize)]
pub struct OpenBrowserRequest {
    pub url: String,
}

/// Pure validation for `github_open_browser`: https on github.com only.
/// The desktop shell has no working popups, so the pane routes external
/// links through the server, which opens the system browser.
fn validate_github_open_url(url: &str) -> Result<(), String> {
    let trimmed = url.trim();
    if trimmed.is_empty() {
        return Err("url is required".to_string());
    }
    if trimmed.len() > 4096 {
        return Err("url is too long".to_string());
    }
    if !trimmed.to_ascii_lowercase().starts_with("https://") {
        return Err("url must use https".to_string());
    }
    let after_scheme = &trimmed["https://".len()..];
    let host_port = after_scheme.split('/').next().unwrap_or("").to_ascii_lowercase();
    let host = host_port.split(':').next().unwrap_or("");
    if host != "github.com" {
        return Err("url must target github.com".to_string());
    }
    Ok(())
}

/// Open a GitHub page in the OS default browser. Used by the settings
/// pane inside the Tauri desktop shell, where `window.open` popups do
/// not function; the browser UI keeps opening tabs itself.
async fn github_open_browser(
    Json(request): Json<OpenBrowserRequest>,
) -> Result<Json<serde_json::Value>, AppError> {
    if let Err(message) = validate_github_open_url(&request.url) {
        return Err(AppError::BadRequest(message));
    }
    open::that(request.url.trim())
        .map_err(|error| AppError::Internal(format!("Could not open the system browser: {}", error)))?;
    Ok(Json(serde_json::json!({ "success": true })))
}

pub fn create_github_router() -> Router {
    Router::new()
        .route("/api/github/config", get(get_config).put(update_config))
        .route("/api/github/token", post(set_token).delete(clear_token))
        .route("/api/github/status", get(get_status))
        .route("/api/github/repos", get(list_repos))
        .route("/api/github/reviews", get(list_reviews))
        .route("/api/github/reviews/run", post(run_review))
        .route("/api/github/poll-now", post(poll_now))
        .route("/api/github/oauth/start", post(oauth_start))
        .route("/api/github/oauth/poll", post(oauth_poll))
        .route("/api/github/oauth/cancel", post(oauth_cancel))
        .route("/api/github/open", post(github_open_browser))
}

#[cfg(test)]
mod tests {
    use super::validate_github_open_url;

    #[test]
    fn open_browser_urls_are_confined_to_github() {
        assert!(validate_github_open_url("https://github.com/login/device?user_code=AB-CD").is_ok());
        assert!(validate_github_open_url("https://github.com/settings/tokens").is_ok());
        assert!(validate_github_open_url("https://github.com:443/x").is_ok());
        // Scheme, host, and confusion cases are rejected.
        assert!(validate_github_open_url("http://github.com/x").is_err());
        assert!(validate_github_open_url("https://evil.com/x").is_err());
        assert!(validate_github_open_url("https://github.com.evil.com/x").is_err());
        assert!(validate_github_open_url("https://github.com@evil.com/x").is_err());
        assert!(validate_github_open_url("https://sub.github.com/x").is_err());
        assert!(validate_github_open_url("file:///etc/passwd").is_err());
        assert!(validate_github_open_url("").is_err());
    }
}
