use crate::agent::runtime::AgentRuntime;
use crate::config::Config;
use crate::web::routes::create_router;
use axum::http::{header, HeaderMap, HeaderValue, Method, StatusCode, Uri};
use axum::response::{IntoResponse, Response};
use rust_embed::RustEmbed;
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::watch;
use tower_http::compression::CompressionLayer;
use tower_http::cors::{AllowOrigin, CorsLayer};
use tower_http::limit::RequestBodyLimitLayer;
use tower_http::set_header::SetResponseHeaderLayer;
use tracing::{info, warn};

#[derive(RustEmbed)]
#[folder = "frontend/"]
struct FrontendAssets;

static INDEX_HTML: &str = "index.html";

fn build_etag(bytes: &[u8]) -> String {
    let mut hasher = DefaultHasher::new();
    bytes.hash(&mut hasher);
    format!("W/\"{:x}-{}\"", hasher.finish(), bytes.len())
}

fn matches_if_none_match(headers: &HeaderMap, etag: &str) -> bool {
    headers
        .get(header::IF_NONE_MATCH)
        .and_then(|value| value.to_str().ok())
        .map(|value| {
            value
                .split(',')
                .any(|candidate| candidate.trim() == etag || candidate.trim() == "*")
        })
        .unwrap_or(false)
}

fn static_asset_response(
    request_headers: &HeaderMap,
    bytes: &[u8],
    content_type: &str,
    cache_control: &'static str,
) -> Response {
    let etag = build_etag(bytes);
    if matches_if_none_match(request_headers, &etag) {
        return Response::builder()
            .status(StatusCode::NOT_MODIFIED)
            .header(header::ETAG, etag)
            .header(header::CACHE_CONTROL, cache_control)
            .body(axum::body::Body::empty())
            .unwrap();
    }

    Response::builder()
        .status(StatusCode::OK)
        .header(header::CONTENT_TYPE, content_type)
        .header(header::ETAG, etag)
        .header(header::CACHE_CONTROL, cache_control)
        .body(axum::body::Body::from(bytes.to_vec()))
        .unwrap()
}

fn build_cors_layer(config: &Config) -> CorsLayer {
    let allowed_origins: Vec<HeaderValue> = config
        .server
        .cors_allowed_origins
        .iter()
        .filter_map(|origin| match HeaderValue::from_str(origin) {
            Ok(value) => Some(value),
            Err(error) => {
                warn!("Ignoring invalid CORS origin '{}': {}", origin, error);
                None
            }
        })
        .collect();

    let layer = CorsLayer::new()
        .allow_methods([
            Method::GET,
            Method::POST,
            Method::PUT,
            Method::PATCH,
            Method::DELETE,
            Method::OPTIONS,
        ])
        .allow_headers([header::AUTHORIZATION, header::CONTENT_TYPE, header::ACCEPT]);

    if allowed_origins.is_empty() {
        layer
    } else {
        layer.allow_origin(AllowOrigin::list(allowed_origins))
    }
}

async fn serve_static_handler(uri: Uri, headers: HeaderMap) -> impl IntoResponse {
    let path = uri.path();

    if path == "/" || path.is_empty() {
        if let Some(content) = FrontendAssets::get(INDEX_HTML) {
            return static_asset_response(
                &headers,
                content.data.as_ref(),
                "text/html; charset=utf-8",
                "no-cache",
            )
            .into_response();
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

            return static_asset_response(
                &headers,
                content.data.as_ref(),
                &mime_type,
                "public, max-age=0, must-revalidate",
            )
            .into_response();
        }
    }

    if let Some(content) = FrontendAssets::get(INDEX_HTML) {
        return static_asset_response(
            &headers,
            content.data.as_ref(),
            "text/html; charset=utf-8",
            "no-cache",
        )
        .into_response();
    }

    (StatusCode::NOT_FOUND, "Not Found").into_response()
}

pub async fn run(config: Config, config_path: PathBuf) -> crate::error::Result<()> {
    let agent = Arc::new(AgentRuntime::new(config.clone())?);
    if let Err(e) = agent.start_scheduler().await {
        tracing::warn!("Failed to start scheduler: {}", e);
    }
    run_with_agent(config, agent, config_path, None).await
}

pub async fn run_with_agent(
    config: Config,
    agent: Arc<AgentRuntime>,
    config_path: PathBuf,
    shutdown_rx: Option<watch::Receiver<bool>>,
) -> crate::error::Result<()> {
    let startup_total = Instant::now();

    let workspace_start = Instant::now();
    for workspace in config.list_workspaces() {
        std::fs::create_dir_all(shellexpand::tilde(&workspace.resolved_path()).to_string())
            .map_err(crate::error::OSAgentError::Io)?;
    }
    info!(
        target: "osagent::startup",
        "phase=web_workspace_dirs elapsed_ms={:.2}",
        workspace_start.elapsed().as_secs_f64() * 1000.0
    );

    let router_start = Instant::now();
    let api_routes = create_router(config.clone(), agent.clone(), config_path);
    info!(
        target: "osagent::startup",
        "phase=web_router_create elapsed_ms={:.2}",
        router_start.elapsed().as_secs_f64() * 1000.0
    );

    let keep_alive = SetResponseHeaderLayer::if_not_present(
        header::CONNECTION,
        HeaderValue::from_static("keep-alive"),
    );

    let app_build_start = Instant::now();
    let app = api_routes
        .fallback(serve_static_handler)
        .layer(keep_alive)
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(50 * 1024 * 1024))
        .layer(build_cors_layer(&config));
    info!(
        target: "osagent::startup",
        "phase=web_app_layers elapsed_ms={:.2}",
        app_build_start.elapsed().as_secs_f64() * 1000.0
    );

    let bind_addr = format!("{}:{}", config.server.bind, config.server.port);
    info!("OSA web server listening on http://{}", bind_addr);

    let bind_start = Instant::now();
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| crate::error::OSAgentError::Unknown(e.to_string()))?;
    info!(
        target: "osagent::startup",
        "phase=web_listener_bind elapsed_ms={:.2}",
        bind_start.elapsed().as_secs_f64() * 1000.0
    );
    info!(
        target: "osagent::startup",
        "phase=web_run_with_agent_ready elapsed_ms={:.2}",
        startup_total.elapsed().as_secs_f64() * 1000.0
    );

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
