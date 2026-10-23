use crate::agent::runtime::AgentRuntime;
use crate::config::Config;
use crate::web::routes::create_router;
use axum::http::{header, HeaderMap, HeaderName, HeaderValue, Method, StatusCode, Uri};
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
#[cfg_attr(feature = "production-frontend", folder = "frontend/dist/")]
#[cfg_attr(not(feature = "production-frontend"), folder = "frontend/")]
struct FrontendAssets;

static INDEX_HTML: &str = "index.html";

/// Web app manifest so iOS/Android can install the UI as a standalone app.
/// Served with an explicit content type (`mime_guess` does not map
/// `.webmanifest`) and as a Rust constant so it is identical in dev and
/// bundled builds.
const PWA_MANIFEST: &str = r##"{
  "name": "OSAgent",
  "short_name": "OSA",
  "description": "Secure local AI agent with a web UI.",
  "start_url": "/",
  "scope": "/",
  "display": "standalone",
  "orientation": "any",
  "background_color": "#0a0a0a",
  "theme_color": "#0a0a0a",
  "icons": [
    { "src": "/images/pwa-192.png", "sizes": "192x192", "type": "image/png", "purpose": "any" },
    { "src": "/images/pwa-512.png", "sizes": "512x512", "type": "image/png", "purpose": "any" },
    { "src": "/images/pwa-maskable-512.png", "sizes": "512x512", "type": "image/png", "purpose": "maskable" }
  ]
}"##;

/// Minimal service worker. Its presence is what lets Chrome/Android offer
/// "Install app"; it deliberately caches nothing so a self-updating local
/// server can never serve a stale UI from the cache.
const PWA_SERVICE_WORKER: &str = r##"// OSA service worker: network-only. Present so the UI is installable as a
// standalone web app; no caching, so updates are always picked up.
self.addEventListener('install', () => self.skipWaiting());
self.addEventListener('activate', (event) => event.waitUntil(self.clients.claim()));
self.addEventListener('fetch', () => { /* network-only, intentionally */ });
"##;

async fn pwa_manifest() -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, "application/manifest+json")],
        PWA_MANIFEST,
    )
}

async fn pwa_service_worker() -> impl IntoResponse {
    (
        [
            (header::CONTENT_TYPE, "text/javascript; charset=utf-8"),
            (header::CACHE_CONTROL, "no-cache"),
        ],
        PWA_SERVICE_WORKER,
    )
}

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
    let agent = AgentRuntime::new(config.clone())?;
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
    let remote_access = crate::web::remote_access::RemoteAccessManager::new(
        config.remote_access.clone(),
        config.server.port,
    );
    let api_routes = create_router(
        config.clone(),
        agent.clone(),
        config_path,
        remote_access.clone(),
    );
    info!(
        target: "osagent::startup",
        "phase=web_router_create elapsed_ms={:.2}",
        router_start.elapsed().as_secs_f64() * 1000.0
    );

    let app_build_start = Instant::now();
    let app = api_routes
        .route("/manifest.webmanifest", axum::routing::get(pwa_manifest))
        .route("/sw.js", axum::routing::get(pwa_service_worker))
        .fallback(serve_static_handler)
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_CONTENT_TYPE_OPTIONS,
            HeaderValue::from_static("nosniff"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::X_FRAME_OPTIONS,
            HeaderValue::from_static("DENY"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            header::REFERRER_POLICY,
            HeaderValue::from_static("strict-origin-when-cross-origin"),
        ))
        .layer(SetResponseHeaderLayer::if_not_present(
            HeaderName::from_static("cross-origin-opener-policy"),
            HeaderValue::from_static("same-origin"),
        ))
        .layer(CompressionLayer::new())
        .layer(RequestBodyLimitLayer::new(50 * 1024 * 1024))
        .layer(build_cors_layer(&config));
    info!(
        target: "osagent::startup",
        "phase=web_app_layers elapsed_ms={:.2}",
        app_build_start.elapsed().as_secs_f64() * 1000.0
    );

    let bind_addr = format!("{}:{}", config.server.bind, config.server.port);

    let bind_start = Instant::now();
    // Bind before announcing. This used to log "listening on ..." first, so a
    // port conflict produced a confident success line immediately followed by a
    // raw OS error, which reads like the server started and then broke.
    let listener = tokio::net::TcpListener::bind(&bind_addr)
        .await
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::AddrInUse {
                crate::error::OSAgentError::Unknown(format!(
                    "Port {} is already in use. Another OSA instance may still be running, \
                     or a leftover child process is holding the port. Stop it, or start with \
                     a different port using `osagent start -p <port>`.",
                    config.server.port
                ))
            } else {
                crate::error::OSAgentError::Unknown(format!("Could not bind {bind_addr}: {e}"))
            }
        })?;

    // Windows creates sockets as inheritable. Child processes (cloudflared, MCP
    // servers, the browser tool, background `process` jobs) are spawned with
    // handle inheritance enabled, so they would inherit this listening socket.
    // If osagent then dies abruptly, an orphaned child keeps the port bound and
    // the next start fails with "port already in use". Clear the inherit flag
    // so the listener can never leak into a child.
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawSocket;
        use windows_sys::Win32::Foundation::{SetHandleInformation, HANDLE_FLAG_INHERIT};
        // SAFETY: `raw` is a live socket handle owned by `listener`.
        let _ = unsafe {
            SetHandleInformation(
                listener.as_raw_socket() as _,
                HANDLE_FLAG_INHERIT,
                0,
            )
        };
    }

    info!("OSA web server listening on http://{}", bind_addr);
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

    // Auto-start the opt-in tunnel once the server is actually listening, so
    // cloudflared's first connection attempt succeeds instead of racing the
    // bind. Enabling without a password is refused: the public URL would
    // otherwise expose an unauthenticated UI.
    if config.remote_access.enabled {
        let password_ready =
            config.server.password_enabled && !config.server.password.is_empty();
        if password_ready {
            let manager = remote_access.clone();
            tokio::spawn(async move {
                manager.ensure_started().await;
            });
        } else {
            tracing::warn!(
                "Remote access is enabled in config but no web UI password is set; tunnel not started."
            );
        }
    }

    // GitHub auto-reviewer: polls watched repos for PRs with new commits.
    // The loop re-reads config every cycle and exits on shutdown, so it
    // follows settings changes without a restart.
    crate::github::spawn_auto_reviewer(agent.clone());

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

    agent.stop_scheduler().await;
    info!("Server shutdown complete");
    Ok(())
}
