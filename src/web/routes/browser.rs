//! Settings API for the agent browser's imported sessions.
//!
//! Registered behind the same auth layer as the rest of the settings API.
//! Responses carry domains and counts only; cookie values never leave the
//! server.

use crate::agent::runtime::AgentRuntime;
use crate::tools::browser::import::{self, CookieJar};
use axum::{
    extract::{Extension, Json, Query},
    http::StatusCode,
    routing::{get, post},
    Router,
};
use serde::Deserialize;
use serde_json::{json, Value};
use std::sync::Arc;

type ApiError = (StatusCode, Json<Value>);

fn error(status: StatusCode, message: impl Into<String>) -> ApiError {
    (status, Json(json!({ "error": message.into() })))
}

pub fn create_browser_router() -> Router {
    Router::new()
        .route("/api/browser/profiles", get(list_profiles))
        .route("/api/browser/import/scan", post(scan_profile))
        .route("/api/browser/import", post(import_profile))
        .route(
            "/api/browser/cookies",
            get(jar_summary).delete(clear_cookies),
        )
}

fn jar_json(jar: &CookieJar) -> Value {
    json!({
        "total": jar.cookies.len(),
        "domains": jar.domains(),
        "sources": jar.sources,
        "imported_at": jar.imported_at,
    })
}

async fn config_dir(agent: &Arc<AgentRuntime>) -> std::path::PathBuf {
    agent.get_config().await.config_dir()
}

async fn list_profiles(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<Value>, ApiError> {
    let dir = config_dir(&agent).await;
    let profiles = tokio::task::spawn_blocking(import::detect_profiles)
        .await
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(json!({
        "profiles": profiles,
        "jar": jar_json(&CookieJar::load(&dir)),
    })))
}

#[derive(Debug, Deserialize)]
struct ProfileRequest {
    profile_id: String,
}

async fn scan_profile(
    Json(request): Json<ProfileRequest>,
) -> Result<Json<Value>, ApiError> {
    let outcome = tokio::task::spawn_blocking(move || {
        let profile = import::find_profile(&request.profile_id)
            .ok_or_else(|| "that browser profile was not found".to_string())?;
        import::read_cookies(&profile)
    })
    .await
    .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| error(StatusCode::BAD_REQUEST, e))?;

    Ok(Json(json!({
        "total": outcome.cookies.len(),
        "domains": import::domain_counts(&outcome.cookies),
        "skipped": outcome.skipped,
    })))
}

#[derive(Debug, Deserialize)]
struct ImportRequest {
    profile_id: String,
    /// Domains to share with the agent browser. Required: nothing is
    /// imported implicitly.
    domains: Vec<String>,
}

async fn import_profile(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Json(request): Json<ImportRequest>,
) -> Result<Json<Value>, ApiError> {
    if request.domains.iter().all(|d| d.trim().is_empty()) {
        return Err(error(StatusCode::BAD_REQUEST, "select at least one domain to import"));
    }
    let dir = config_dir(&agent).await;
    let result = tokio::task::spawn_blocking(move || {
        let profile = import::find_profile(&request.profile_id)
            .ok_or_else(|| "that browser profile was not found".to_string())?;
        let outcome = import::read_cookies(&profile)?;
        let selected = import::filter_by_domains(outcome.cookies, &request.domains);
        let imported = selected.len();
        let mut jar = CookieJar::load(&dir);
        jar.merge(format!("{} ({})", profile.browser_label, profile.name), selected);
        jar.save(&dir)?;
        Ok::<_, String>((imported, outcome.skipped, jar_json(&jar)))
    })
    .await
    .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?
    .map_err(|e| error(StatusCode::BAD_REQUEST, e))?;

    Ok(Json(json!({
        "imported": result.0,
        "skipped": result.1,
        "jar": result.2,
        "note": "Applies to browsers the agent opens from now on; close any open agent browser to pick it up.",
    })))
}

async fn jar_summary(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Json<Value> {
    Json(jar_json(&CookieJar::load(&config_dir(&agent).await)))
}

#[derive(Debug, Deserialize)]
struct ClearQuery {
    domain: Option<String>,
}

async fn clear_cookies(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Query(query): Query<ClearQuery>,
) -> Result<Json<Value>, ApiError> {
    let dir = config_dir(&agent).await;
    match query.domain.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
        Some(domain) => {
            let mut jar = CookieJar::load(&dir);
            let removed = jar.remove_domain(domain);
            jar.save(&dir).map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            Ok(Json(json!({ "removed": removed, "jar": jar_json(&jar) })))
        }
        None => {
            CookieJar::clear(&dir).map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e))?;
            Ok(Json(json!({ "jar": jar_json(&CookieJar::default()) })))
        }
    }
}
