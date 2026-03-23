use crate::agent::runtime::AgentRuntime;
use crate::config::Config;
use crate::web::routes::create_router;
use axum::http::{header, StatusCode, Uri};
use axum::response::{Html, IntoResponse, Response};
use rust_embed::RustEmbed;
use std::path::PathBuf;
use std::sync::Arc;
use tokio::sync::watch;
use tower_http::cors::CorsLayer;
use tracing::info;

#[derive(RustEmbed)]
#[folder = "frontend/"]
struct FrontendAssets;

static INDEX_HTML: &str = "index.html";

async fn serve_static_handler(uri: Uri) -> impl IntoResponse {
    let path = uri.path();

    if path == "/" || path.is_empty() {
        if let Some(content) = FrontendAssets::get(INDEX_HTML) {
            return Html(String::from_utf8_lossy(&content.data).to_string()).into_response();
        }
    }

    let asset_path = if path.starts_with("/static/") {
        path.strip_prefix("/static/").unwrap_or(path)
    } else if path.starts_with('/') {
        path.strip_prefix('/').unwrap_or(path)
    } else {
        path
    };

    if !asset_path.is_empty() && asset_path != "/" {
        if let Some(content) = FrontendAssets::get(asset_path) {
            let mime_type = mime_guess::from_path(asset_path)
                .first_or_octet_stream()
                .as_ref()
                .to_string();

            return Response::builder()
                .status(StatusCode::OK)
                .header(header::CONTENT_TYPE, mime_type)
                .body(axum::body::Body::from(content.data.to_vec()))
                .unwrap()
                .into_response();
        }
    }

    if let Some(content) = FrontendAssets::get(INDEX_HTML) {
        return Html(String::from_utf8_lossy(&content.data).to_string()).into_response();
    }

    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

pub async fn run(config: Config, config_path: PathBuf) -> crate::error::Result<()> {
    let agent = Arc::new(AgentRuntime::new(config.clone())?);
    run_with_agent(config, agent, config_path, None).await
}

pub async fn run_with_agent(
    config: Config,
    agent: Arc<AgentRuntime>,
    config_path: PathBuf,
    mut shutdown_rx: Option<watch::Receiver<bool>>,
) -> crate::error::Result<()> {
    for workspace in config.list_workspaces() {
        std::fs::create_dir_all(shellexpand::tilde(&workspace.path).to_string())
            .map_err(|e| crate::error::OSAgentError::Io(e))?;
    }

    let api_routes = create_router(config.clone(), agent.clone(), config_path);

    let app = api_routes
        .fallback(serve_static_handler)
        .layer(CorsLayer::permissive());

    let bind_addr = format!("{}:{}", config.server.bind, config.server.port);
    info!("OSA web server listening on http://{}", bind_addr);

    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| crate::error::OSAgentError::Unknown(e.to_string()))?;

    let graceful = axum::serve(listener, app);

    if let Some(mut rx) = shutdown_rx {
        let _restart_tx = agent.get_restart_sender();
        if _restart_tx.is_some() {
            info!("Server shutdown signaled, initiating graceful shutdown...");
        }
        graceful
            .with_graceful_shutdown(async move {
                loop {
                    if rx.changed().await.is_err() {
                        break;
                    }
                    if *rx.borrow() {
                        info!("Shutdown signal received, stopping server...");
                        break;
                    }
                }
            })
            .await
            .map_err(|e| crate::error::OSAgentError::Unknown(e.to_string()))?;
    } else {
        graceful
            .await
            .map_err(|e| crate::error::OSAgentError::Unknown(e.to_string()))?;
    }

    info!("Server shutdown complete");
    Ok(())
}
