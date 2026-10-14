//! Remote-access (public tunnel) routes, QR pairing, and paired devices.
use crate::agent::runtime::AgentRuntime;
use crate::storage::PairedDevice;
use crate::web::auth;
use crate::web::remote_access::{RemoteAccessManager, RemoteAccessView};
use axum::{
    extract::{Extension, Path},
    http::{header, HeaderMap, StatusCode},
    routing::{delete, get, post},
    Json, Router,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Debug, Serialize)]
struct ErrorResponse {
    error: String,
}

fn error(status: StatusCode, message: impl Into<String>) -> (StatusCode, Json<ErrorResponse>) {
    (status, Json(ErrorResponse { error: message.into() }))
}

#[derive(Debug, Deserialize)]
pub struct PairRequest {
    pub token: String,
}

#[derive(Debug, Serialize)]
pub struct PairResponse {
    pub token: String,
    pub device: PairedDevice,
}

/// Router fragment for authenticated remote-access controls.
pub fn protected_router() -> Router {
    Router::new()
        .route("/api/remote/status", get(status))
        .route("/api/remote/enable", post(enable))
        .route("/api/remote/disable", post(disable))
        .route("/api/remote/pairing", post(rotate_pairing))
        .route("/api/devices", get(list_devices).delete(revoke_all_devices))
        .route("/api/devices/:id", delete(revoke_device))
}

/// Router fragment for the unauthenticated pairing exchange. The pairing code
/// itself is the credential, so this must stay public.
pub fn public_router() -> Router {
    Router::new().route("/api/auth/pair", post(pair))
}

async fn status(
    Extension(manager): Extension<Arc<RemoteAccessManager>>,
) -> Json<RemoteAccessView> {
    Json(manager.view().await)
}

async fn enable(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(manager): Extension<Arc<RemoteAccessManager>>,
    Extension(config_path): Extension<PathBuf>,
) -> Result<Json<RemoteAccessView>, (StatusCode, Json<ErrorResponse>)> {
    let mut config = agent.get_config().await;

    // The tunnel publishes a UI that can run tools and edit files. A password
    // is the only thing standing between the public URL and that power, so
    // refuse to expose anything until one is set.
    if !config.server.password_enabled || config.server.password.is_empty() {
        return Err(error(
            StatusCode::BAD_REQUEST,
            "Set a web UI password before enabling remote access.",
        ));
    }

    config.remote_access.enabled = true;
    manager.set_config(config.remote_access.clone()).await;

    // enable() kicks off download + start in the background and returns the
    // status immediately; failures surface through /api/remote/status.
    let view = manager.enable().await;
    agent.replace_config(config.clone()).await;
    if let Err(e) = config.save(&config_path) {
        tracing::warn!("Remote access: failed to persist enabled flag: {}", e);
    }
    Ok(Json(view))
}

async fn disable(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(manager): Extension<Arc<RemoteAccessManager>>,
    Extension(config_path): Extension<PathBuf>,
) -> Json<RemoteAccessView> {
    let mut config = agent.get_config().await;
    config.remote_access.enabled = false;
    manager.set_config(config.remote_access.clone()).await;
    agent.replace_config(config.clone()).await;
    if let Err(e) = config.save(&config_path) {
        tracing::warn!("Remote access: failed to persist disabled flag: {}", e);
    }
    Json(manager.disable().await)
}

async fn rotate_pairing(
    Extension(manager): Extension<Arc<RemoteAccessManager>>,
) -> Json<serde_json::Value> {
    let view = manager.rotate_pairing().await;
    let qr_svg = manager.qr_svg().await;
    Json(serde_json::json!({
        "url": view.pairing_url,
        "qr_svg": qr_svg,
        "expires_in_secs": view.pairing_expires_in_secs,
        "status": view.status,
        "public_url": view.public_url,
    }))
}

async fn list_devices(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<Json<Vec<PairedDevice>>, (StatusCode, Json<ErrorResponse>)> {
    let devices = agent
        .storage()
        .list_paired_devices()
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(Json(devices))
}

async fn revoke_device(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Path(id): Path<String>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let removed = agent
        .storage()
        .delete_paired_device(&id)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    Ok(if removed {
        StatusCode::NO_CONTENT
    } else {
        StatusCode::NOT_FOUND
    })
}

async fn revoke_all_devices(
    Extension(agent): Extension<Arc<AgentRuntime>>,
) -> Result<StatusCode, (StatusCode, Json<ErrorResponse>)> {
    let devices = agent
        .storage()
        .list_paired_devices()
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;
    for device in devices {
        let _ = agent.storage().delete_paired_device(&device.id);
    }
    Ok(StatusCode::NO_CONTENT)
}

async fn pair(
    Extension(agent): Extension<Arc<AgentRuntime>>,
    Extension(manager): Extension<Arc<RemoteAccessManager>>,
    Extension(secret): Extension<Arc<String>>,
    headers: HeaderMap,
    Json(payload): Json<PairRequest>,
) -> Result<Json<PairResponse>, (StatusCode, Json<ErrorResponse>)> {
    if payload.token.trim().is_empty() {
        return Err(error(StatusCode::BAD_REQUEST, "Missing pairing token."));
    }
    if !manager.redeem(payload.token.trim()).await {
        return Err(error(
            StatusCode::UNAUTHORIZED,
            "Pairing code is invalid or expired. Scan the current code again.",
        ));
    }

    let user_agent = headers
        .get(header::USER_AGENT)
        .and_then(|value| value.to_str().ok())
        .map(|value| value.chars().take(300).collect::<String>());
    let now = Utc::now();
    let device = PairedDevice {
        id: Uuid::new_v4().to_string(),
        label: device_label(user_agent.as_deref()),
        user_agent,
        created_at: now,
        last_seen_at: Some(now),
    };
    agent
        .storage()
        .create_paired_device(&device)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    let ttl_days = agent
        .get_config()
        .await
        .remote_access
        .device_token_ttl_days
        .max(1);
    let token = auth::generate_device_token(&device.id, &secret, ttl_days * 86_400)
        .map_err(|e| error(StatusCode::INTERNAL_SERVER_ERROR, e.to_string()))?;

    Ok(Json(PairResponse { token, device }))
}

/// Best-effort human label from a User-Agent string (e.g. "iPhone · Safari").
fn device_label(user_agent: Option<&str>) -> String {
    let ua = user_agent.unwrap_or("");
    let platform = if ua.contains("iPhone") {
        "iPhone"
    } else if ua.contains("iPad") {
        "iPad"
    } else if ua.contains("Android") {
        "Android"
    } else if ua.contains("Windows") {
        "Windows"
    } else if ua.contains("Macintosh") || ua.contains("Mac OS X") {
        "Mac"
    } else if ua.contains("Linux") {
        "Linux"
    } else {
        "Device"
    };
    let browser = if ua.contains("Edg/") {
        "Edge"
    } else if ua.contains("Chrome") || ua.contains("CriOS") {
        "Chrome"
    } else if ua.contains("Firefox") || ua.contains("FxiOS") {
        "Firefox"
    } else if ua.contains("Safari") {
        "Safari"
    } else {
        ""
    };
    if browser.is_empty() {
        platform.to_string()
    } else {
        format!("{} · {}", platform, browser)
    }
}
