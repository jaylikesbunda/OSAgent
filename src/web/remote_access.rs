//! Opt-in public tunnel for remote (phone) access, with QR pairing.
//!
//! The local web UI still binds to loopback. When the user opts in, a
//! `cloudflared` child process exposes it, and devices are admitted by
//! redeeming a short-lived one-time pairing code (delivered as a QR code)
//! instead of typing the web password on the phone.
//!
//! cloudflared is fetched automatically from Cloudflare's official GitHub
//! release the first time remote access is enabled, so the user does not have
//! to install anything.

use crate::config::RemoteAccessConfig;
use crate::error::{OSAgentError, Result};
use serde::Serialize;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::io::{AsyncBufReadExt, BufReader};
use tokio::process::{Child, Command};
use tokio::sync::{Mutex, RwLock};
use tracing::{info, warn};

const CLOUDFLARED_RELEASE_BASE: &str =
    "https://github.com/cloudflare/cloudflared/releases/latest/download";

/// State of the tunnel process, as surfaced to the settings UI.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TunnelStatus {
    Disabled,
    Provisioning,
    Installing,
    Starting,
    Running,
    Error,
}

impl TunnelStatus {
    fn as_str(self) -> &'static str {
        match self {
            TunnelStatus::Disabled => "disabled",
            TunnelStatus::Provisioning => "provisioning",
            TunnelStatus::Installing => "installing",
            TunnelStatus::Starting => "starting",
            TunnelStatus::Running => "running",
            TunnelStatus::Error => "error",
        }
    }
}

#[derive(Debug, Clone)]
struct PairingCode {
    token: String,
    expires_at: Instant,
}

/// Redacted, serializable snapshot for the settings UI.
#[derive(Debug, Clone, Serialize)]
pub struct RemoteAccessView {
    pub enabled: bool,
    pub provider: String,
    pub status: String,
    pub public_url: Option<String>,
    pub error: Option<String>,
    /// Whether a usable cloudflared binary already exists, or can be fetched
    /// for this platform. Effectively always true unless the platform is
    /// unsupported. OpenTunnel needs no binary, so it is always true there.
    pub cloudflared_available: bool,
    /// 0 means paired devices stay signed in until revoked.
    pub device_token_ttl_days: u64,
    /// Seconds until the current pairing code expires, if one is active.
    pub pairing_expires_in_secs: Option<u64>,
    /// Public URL a phone should open (includes the pairing code when active).
    pub pairing_url: Option<String>,
}

pub struct RemoteAccessManager {
    cfg: RwLock<RemoteAccessConfig>,
    port: u16,
    child: Mutex<Option<Child>>,
    opentunnel: Mutex<Option<opentunnel::Tunnel>>,
    state: RwLock<(TunnelStatus, Option<String>, Option<String>)>,
    pairing: RwLock<Option<PairingCode>>,
    starting: AtomicBool,
    /// Bumped whenever the tunnel is (re)started or stopped, so a status
    /// watcher from an older tunnel cannot overwrite the current state.
    generation: AtomicU64,
}

impl RemoteAccessManager {
    pub fn new(cfg: RemoteAccessConfig, port: u16) -> Arc<Self> {
        Arc::new(Self {
            cfg: RwLock::new(cfg),
            port,
            child: Mutex::new(None),
            opentunnel: Mutex::new(None),
            state: RwLock::new((TunnelStatus::Disabled, None, None)),
            pairing: RwLock::new(None),
            starting: AtomicBool::new(false),
            generation: AtomicU64::new(0),
        })
    }

    async fn enabled(&self) -> bool {
        self.cfg.read().await.enabled
    }

    /// Replace the manager's view of the remote-access config (e.g. after the
    /// settings UI changed the tunnel name or binary).
    pub async fn set_config(&self, cfg: RemoteAccessConfig) {
        *self.cfg.write().await = cfg;
    }

    /// Whether cloudflared can be used: either it is already present, or this
    /// platform has a downloadable official build.
    pub async fn cloudflared_available(&self) -> bool {
        if self.existing_binary().await.is_some() {
            return true;
        }
        cloudflared_asset().is_some()
    }

    pub async fn view(&self) -> RemoteAccessView {
        let (enabled, provider, device_token_ttl_days) = {
            let cfg = self.cfg.read().await;
            (cfg.enabled, cfg.provider.clone(), cfg.device_token_ttl_days)
        };
        let (status, public_url, error) = {
            let guard = self.state.read().await;
            (guard.0, guard.1.clone(), guard.2.clone())
        };
        let (pairing_url, pairing_expires_in_secs) = {
            let guard = self.pairing.read().await;
            match guard.as_ref() {
                Some(code) if code.expires_at > Instant::now() => {
                    let url = public_url
                        .as_ref()
                        .map(|base| format!("{}?pair={}", base.trim_end_matches('/'), code.token));
                    let secs = code.expires_at.saturating_duration_since(Instant::now());
                    (url, Some(secs.as_secs()))
                }
                _ => (None, None),
            }
        };
        // Only a cloudflared provider needs the binary; OpenTunnel is built in.
        let cloudflared_available = if provider == "opentunnel" {
            true
        } else {
            self.cloudflared_available().await
        };
        RemoteAccessView {
            enabled,
            provider,
            status: status.as_str().to_string(),
            public_url,
            error,
            cloudflared_available,
            device_token_ttl_days,
            pairing_expires_in_secs,
            pairing_url,
        }
    }

    /// Create a fresh one-time pairing code honoring the configured TTL.
    pub async fn rotate_pairing(&self) -> RemoteAccessView {
        let ttl = {
            let cfg = self.cfg.read().await;
            Duration::from_secs(cfg.pairing_ttl_secs.clamp(60, 3600))
        };
        let token = generate_pairing_token();
        *self.pairing.write().await = Some(PairingCode {
            token,
            expires_at: Instant::now() + ttl,
        });
        self.view().await
    }

    /// Redeem a pairing code. Codes are one-time: a successful redemption
    /// clears the code so a leaked QR cannot be replayed.
    pub async fn redeem(&self, token: &str) -> bool {
        let mut guard = self.pairing.write().await;
        match guard.as_ref() {
            Some(code) if code.expires_at > Instant::now() && code.token == token => {
                *guard = None;
                true
            }
            _ => false,
        }
    }

    /// Turn the tunnel on. Returns immediately: the binary download and start
    /// run in the background so the request does not block on the network.
    /// Progress and failures surface through [`Self::view`].
    pub async fn enable(self: &Arc<Self>) -> RemoteAccessView {
        {
            let mut cfg = self.cfg.write().await;
            cfg.enabled = true;
        }
        *self.state.write().await = (TunnelStatus::Starting, None, None);
        self.begin_start();
        self.view().await
    }

    /// Turn the tunnel off and stop the child process.
    pub async fn disable(self: &Arc<Self>) -> RemoteAccessView {
        {
            let mut cfg = self.cfg.write().await;
            cfg.enabled = false;
        }
        self.stop().await;
        *self.pairing.write().await = None;
        self.view().await
    }

    /// Start the tunnel if it is enabled and not already running/starting.
    pub async fn ensure_started(self: &Arc<Self>) {
        if !self.enabled().await {
            return;
        }
        if self.child.lock().await.is_some()
            || self.opentunnel.lock().await.is_some()
            || self.starting.load(Ordering::SeqCst)
        {
            return;
        }
        {
            let mut state = self.state.write().await;
            if state.0 == TunnelStatus::Error {
                state.2 = None;
            }
            state.0 = TunnelStatus::Starting;
        }
        self.begin_start();
    }

    /// Spawn the (install-then-start) worker once.
    fn begin_start(self: &Arc<Self>) {
        if self.starting.swap(true, Ordering::SeqCst) {
            return;
        }
        let this = Arc::clone(self);
        tokio::spawn(async move {
            if let Err(error) = this.install_and_start().await {
                warn!("Remote access: {}", error);
                // A stop or provider switch during provisioning already set
                // the state it wants; do not overwrite it with a stale error.
                if this.enabled().await {
                    let mut state = this.state.write().await;
                    state.0 = TunnelStatus::Error;
                    state.1 = None;
                    state.2 = Some(error.to_string());
                    drop(state);
                }
            }
            this.starting.store(false, Ordering::SeqCst);
        });
    }

    async fn install_and_start(self: &Arc<Self>) -> Result<()> {
        if self.child.lock().await.is_some() || self.opentunnel.lock().await.is_some() {
            return Ok(());
        }

        let (binary_cfg, tunnel_name, public_hostname, provider) = {
            let cfg = self.cfg.read().await;
            (
                cfg.binary.trim().to_string(),
                cfg.tunnel_name.trim().to_string(),
                cfg.public_hostname.trim().to_string(),
                cfg.provider.trim().to_string(),
            )
        };

        if provider == "opentunnel" {
            return self.start_opentunnel().await;
        }

        if provider != "cloudflared" {
            return Err(OSAgentError::Config(format!(
                "Unsupported remote-access provider: {}",
                provider
            )));
        }

        // Resolve (or fetch) the binary before announcing "starting".
        let binary = match self.resolve_binary(&binary_cfg).await {
            Ok(path) => path,
            Err(error) => return Err(error),
        };

        self.spawn_tunnel(&binary, &tunnel_name, &public_hostname)
            .await
    }

    /// Bring up the built-in OpenTunnel bridge. The first run creates the
    /// tunnel and a local key, then waits for the certificate; later runs load
    /// the same identity, so the hostname does not change across restarts.
    async fn start_opentunnel(self: &Arc<Self>) -> Result<()> {
        let generation = self.generation.fetch_add(1, Ordering::SeqCst) + 1;
        *self.state.write().await = (TunnelStatus::Provisioning, None, None);

        let client = crate::web::opentunnel_provider::client();
        let hostname = crate::web::opentunnel_provider::ensure_identity(&client, "default").await?;

        if self.generation.load(Ordering::SeqCst) != generation {
            return Ok(());
        }
        *self.state.write().await = (TunnelStatus::Starting, None, None);

        let tunnel = crate::web::opentunnel_provider::connect(&client, "default", self.port)?;
        *self.opentunnel.lock().await = Some(tunnel);

        let url = format!("https://{}", hostname);
        let this = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(1)).await;
                if this.generation.load(Ordering::SeqCst) != generation {
                    return;
                }
                let snapshot = {
                    let guard = this.opentunnel.lock().await;
                    match guard.as_ref() {
                        Some(tunnel) => tunnel.status(),
                        None => return,
                    }
                };
                match snapshot.state {
                    opentunnel::State::Connected => {
                        this.set_running(Some(url.clone())).await;
                    }
                    // Reconnects are automatic, so the address is still valid;
                    // keep it visible and surface why it is reconnecting.
                    opentunnel::State::Reconnecting => {
                        let mut state = this.state.write().await;
                        if this.generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        state.0 = TunnelStatus::Running;
                        state.1 = Some(url.clone());
                        state.2 = snapshot.last_error;
                    }
                    opentunnel::State::Stopped => {
                        let mut state = this.state.write().await;
                        if this.generation.load(Ordering::SeqCst) != generation {
                            return;
                        }
                        state.0 = TunnelStatus::Error;
                        state.1 = Some(url.clone());
                        state.2 = snapshot
                            .last_error
                            .or_else(|| Some("OpenTunnel stopped".to_string()));
                        return;
                    }
                    _ => {}
                }
            }
        });

        Ok(())
    }

    async fn spawn_tunnel(
        self: &Arc<Self>,
        binary: &Path,
        tunnel_name: &str,
        public_hostname: &str,
    ) -> Result<()> {
        let named = !tunnel_name.is_empty();
        let mut cmd = Command::new(binary);
        if named {
            cmd.args(["tunnel", "--no-autoupdate", "run", tunnel_name]);
        } else {
            cmd.args([
                "tunnel",
                "--no-autoupdate",
                "--url",
                &format!("http://127.0.0.1:{}", self.port),
            ]);
        }
        cmd.stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .stdin(Stdio::null())
            .kill_on_drop(true);

        let mut child = cmd.spawn().map_err(|e| {
            OSAgentError::Unknown(format!("Could not start '{}': {}", binary.display(), e))
        })?;

        let stderr = child.stderr.take();
        let stdout = child.stdout.take();

        *self.state.write().await = (
            TunnelStatus::Starting,
            if named && !public_hostname.is_empty() {
                Some(format!("https://{}", public_hostname))
            } else {
                None
            },
            None,
        );
        info!("Remote access: starting tunnel via '{}'", binary.display());

        *self.child.lock().await = Some(child);

        if let Some(stderr) = stderr {
            let this = Arc::clone(self);
            let hostname = public_hostname.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    this.observe_tunnel_log(&line, &hostname).await;
                }
            });
        }
        if let Some(stdout) = stdout {
            let this = Arc::clone(self);
            let hostname = public_hostname.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stdout).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    this.observe_tunnel_log(&line, &hostname).await;
                }
            });
        }

        // Watch for the process dying so the UI does not show a stale "running".
        let this = Arc::clone(self);
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(2)).await;
                let exited = {
                    let mut guard = this.child.lock().await;
                    match guard.as_mut() {
                        Some(child) => match child.try_wait() {
                            Ok(Some(status)) => Some(Some(status)),
                            Ok(None) => None,
                            Err(_) => Some(None),
                        },
                        None => return,
                    }
                };
                if let Some(status) = exited {
                    let mut state = this.state.write().await;
                    if state.0 != TunnelStatus::Disabled {
                        state.0 = TunnelStatus::Error;
                        state.1 = None;
                        state.2 = Some(match status {
                            Some(s) => format!("cloudflared exited ({})", s),
                            None => "cloudflared exited".to_string(),
                        });
                    }
                    let mut child_guard = this.child.lock().await;
                    *child_guard = None;
                    return;
                }
            }
        });

        Ok(())
    }

    /// Resolve a runnable cloudflared path: an explicitly configured binary, a
    /// copy already on PATH, a previously downloaded managed copy, or a fresh
    /// download from Cloudflare's official release.
    async fn resolve_binary(&self, configured: &str) -> Result<PathBuf> {
        if !configured.is_empty() {
            let path = PathBuf::from(configured);
            if path.exists() {
                return Ok(path);
            }
            if let Ok(found) = which::which(configured) {
                return Ok(found);
            }
        }
        if let Ok(found) = which::which("cloudflared") {
            return Ok(found);
        }

        let target = managed_binary_path()?;
        if target.exists() {
            return Ok(target);
        }

        {
            let mut state = self.state.write().await;
            state.0 = TunnelStatus::Installing;
            state.2 = None;
        }
        download_cloudflared(&target).await?;
        Ok(target)
    }

    /// Check for an existing binary without downloading anything.
    async fn existing_binary(&self) -> Option<PathBuf> {
        let configured = self.cfg.read().await.binary.trim().to_string();
        if !configured.is_empty() {
            let path = PathBuf::from(&configured);
            if path.exists() {
                return Some(path);
            }
            if let Ok(found) = which::which(&configured) {
                return Some(found);
            }
        }
        if let Ok(found) = which::which("cloudflared") {
            return Some(found);
        }
        managed_binary_path().ok().filter(|path| path.exists())
    }

    /// Parse a cloudflared log line for the assigned public URL or a clear
    /// failure, updating state when found.
    async fn observe_tunnel_log(&self, line: &str, public_hostname: &str) {
        if let Some(url) = extract_quick_tunnel_url(line) {
            self.set_running(Some(url)).await;
            return;
        }
        if line.contains("Registered tunnel connection") || line.contains("Connection registered") {
            let url = if public_hostname.is_empty() {
                None
            } else {
                Some(format!("https://{}", public_hostname))
            };
            self.set_running(url).await;
        }
    }

    async fn set_running(&self, public_url: Option<String>) {
        let mut state = self.state.write().await;
        if state.0 != TunnelStatus::Running {
            info!("Remote access: tunnel running at {:?}", public_url);
        }
        state.0 = TunnelStatus::Running;
        if public_url.is_some() {
            state.1 = public_url;
        }
        state.2 = None;
    }

    pub async fn stop(&self) {
        // Invalidate any status watcher from the tunnel being stopped.
        self.generation.fetch_add(1, Ordering::SeqCst);

        if let Some(tunnel) = self.opentunnel.lock().await.take() {
            let _ = tunnel.close().await;
        }

        let child = self.child.lock().await.take();
        if let Some(mut child) = child {
            let _ = child.start_kill();
            let _ = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        }
        *self.state.write().await = (TunnelStatus::Disabled, None, None);
        // A pairing code is bound to the address that was live when it was
        // minted, so it cannot survive a tunnel restart.
        *self.pairing.write().await = None;
        info!("Remote access: tunnel stopped");
    }

    /// Render the current pairing URL as an inline SVG QR code.
    pub async fn qr_svg(&self) -> Option<String> {
        let url = self.view().await.pairing_url?;
        qr_svg(&url)
    }
}

/// Directory for the managed cloudflared download: `~/.osagent/bin`.
fn managed_binary_path() -> Result<PathBuf> {
    let base = dirs_next::home_dir()
        .map(|home| home.join(".osagent").join("bin"))
        .unwrap_or_else(|| std::env::temp_dir().join("osagent-bin"));
    let name = if cfg!(windows) {
        "cloudflared.exe"
    } else {
        "cloudflared"
    };
    Ok(base.join(name))
}

/// The official release asset for this platform, and whether it is a tarball.
fn cloudflared_asset() -> Option<(&'static str, bool)> {
    let asset = match (std::env::consts::OS, std::env::consts::ARCH) {
        ("windows", "x86_64") => ("cloudflared-windows-amd64.exe", false),
        ("windows", "aarch64") => ("cloudflared-windows-arm64.exe", false),
        ("linux", "x86_64") => ("cloudflared-linux-amd64", false),
        ("linux", "aarch64") => ("cloudflared-linux-arm64", false),
        ("linux", "arm") => ("cloudflared-linux-arm", false),
        ("macos", "x86_64") => ("cloudflared-darwin-amd64.tgz", true),
        ("macos", "aarch64") => ("cloudflared-darwin-arm64.tgz", true),
        _ => return None,
    };
    Some(asset)
}

/// Download cloudflared from Cloudflare's official GitHub release into
/// `target`, handling the macOS tarball and setting the executable bit.
async fn download_cloudflared(target: &Path) -> Result<()> {
    let (asset, is_tarball) = cloudflared_asset().ok_or_else(|| {
        OSAgentError::Config(
            "No prebuilt cloudflared for this platform. Install it manually and set \
             remote_access.binary."
                .to_string(),
        )
    })?;
    let url = format!("{}/{}", CLOUDFLARED_RELEASE_BASE, asset);
    info!("Remote access: downloading cloudflared from {}", url);

    if let Some(parent) = target.parent() {
        std::fs::create_dir_all(parent).map_err(OSAgentError::Io)?;
    }

    let client = reqwest::Client::builder()
        .user_agent("osagent-remote-access")
        .build()
        .map_err(|e| OSAgentError::Unknown(format!("HTTP client init failed: {e}")))?;
    let response = client
        .get(&url)
        .send()
        .await
        .map_err(|e| OSAgentError::Unknown(format!("Could not download cloudflared: {e}")))?;
    if !response.status().is_success() {
        return Err(OSAgentError::Unknown(format!(
            "cloudflared download failed: HTTP {}",
            response.status()
        )));
    }
    let bytes = response
        .bytes()
        .await
        .map_err(|e| OSAgentError::Unknown(format!("cloudflared download interrupted: {e}")))?;

    let temp = target.with_extension("download");
    std::fs::write(&temp, &bytes).map_err(OSAgentError::Io)?;

    if is_tarball {
        if let Err(error) = extract_cloudflared(&temp, target) {
            let _ = std::fs::remove_file(&temp);
            return Err(error);
        }
        let _ = std::fs::remove_file(&temp);
    } else {
        std::fs::rename(&temp, target)
            .or_else(|_| {
                std::fs::copy(&temp, target)
                    .map(|_| ())
                    .and_then(|_| std::fs::remove_file(&temp))
            })
            .map_err(OSAgentError::Io)?;
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        if let Ok(metadata) = std::fs::metadata(target) {
            let mut perms = metadata.permissions();
            perms.set_mode(0o755);
            let _ = std::fs::set_permissions(target, perms);
        }
    }

    info!(
        "Remote access: cloudflared installed at {}",
        target.display()
    );
    Ok(())
}

fn extract_cloudflared(archive_path: &Path, target: &Path) -> Result<()> {
    let file = std::fs::File::open(archive_path).map_err(OSAgentError::Io)?;
    let decoder = flate2::read::GzDecoder::new(file);
    let mut archive = tar::Archive::new(decoder);
    let entries = archive.entries().map_err(OSAgentError::Io)?;
    for entry in entries {
        let mut entry = entry.map_err(OSAgentError::Io)?;
        let path = entry.path().map_err(OSAgentError::Io)?;
        if path.file_name().and_then(|name| name.to_str()) == Some("cloudflared") {
            entry.unpack(target).map_err(OSAgentError::Io)?;
            return Ok(());
        }
    }
    Err(OSAgentError::Unknown(
        "cloudflared tarball did not contain the binary".to_string(),
    ))
}

/// Find the `https://<slug>.trycloudflare.com` URL a quick tunnel prints.
fn extract_quick_tunnel_url(line: &str) -> Option<String> {
    let start = line.find("https://")?;
    let rest = &line[start..];
    let end = rest
        .find(|c: char| c.is_whitespace() || c == '|' || c == '"')
        .unwrap_or(rest.len());
    let candidate = rest[..end].trim_end_matches(['.', ',']);
    if candidate.ends_with(".trycloudflare.com") {
        Some(candidate.to_string())
    } else {
        None
    }
}

fn generate_pairing_token() -> String {
    use base64::Engine;
    use rand::RngCore;
    let mut bytes = [0u8; 24];
    rand::thread_rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// Render `data` as a scalable SVG QR code. Uses the encoder's module matrix
/// directly, so no image backend is required.
fn qr_svg(data: &str) -> Option<String> {
    let code = qrcode::QrCode::new(data.as_bytes()).ok()?;
    let width = code.width();
    let colors = code.to_colors();
    let mut svg = String::with_capacity(width * width * 24);
    svg.push_str(&format!(
        "<svg xmlns=\"http://www.w3.org/2000/svg\" viewBox=\"0 0 {w} {w}\" shape-rendering=\"crispEdges\" role=\"img\" aria-label=\"Pairing QR code\">",
        w = width
    ));
    svg.push_str(&format!(
        "<rect width=\"{w}\" height=\"{w}\" fill=\"#ffffff\"/>",
        w = width
    ));
    for (index, color) in colors.iter().enumerate() {
        if *color == qrcode::Color::Dark {
            let x = index % width;
            let y = index / width;
            svg.push_str(&format!(
                "<rect x=\"{x}\" y=\"{y}\" width=\"1\" height=\"1\" fill=\"#000000\"/>"
            ));
        }
    }
    svg.push_str("</svg>");
    Some(svg)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_quick_tunnel_url_from_box_line() {
        let line = "2024-01-01 |  https://free-river-fox-1234.trycloudflare.com   |";
        assert_eq!(
            extract_quick_tunnel_url(line).as_deref(),
            Some("https://free-river-fox-1234.trycloudflare.com")
        );
    }

    #[test]
    fn ignores_non_quick_urls() {
        assert!(extract_quick_tunnel_url("visit https://example.com now").is_none());
    }

    #[test]
    fn qr_svg_renders_rects() {
        let svg = qr_svg("https://x.trycloudflare.com?pair=abc").expect("qr");
        assert!(svg.starts_with("<svg"));
        assert!(svg.contains("fill=\"#000000\""));
    }

    #[test]
    fn managed_binary_path_is_under_home_bin() {
        let path = managed_binary_path().expect("path");
        assert!(path.to_string_lossy().contains("bin"));
    }

    #[tokio::test]
    async fn pairing_codes_are_one_time() {
        let mgr = RemoteAccessManager::new(RemoteAccessConfig::default(), 8765);
        // A pairing URL only exists once the tunnel has a public URL.
        *mgr.state.write().await = (
            TunnelStatus::Running,
            Some("https://x.trycloudflare.com".to_string()),
            None,
        );
        let view = mgr.rotate_pairing().await;
        let url = view.pairing_url.expect("pairing url");
        assert!(url.starts_with("https://x.trycloudflare.com?pair="));
        let token = url.split("pair=").nth(1).unwrap().to_string();
        assert!(mgr.redeem(&token).await);
        assert!(!mgr.redeem(&token).await, "replay must fail");
    }
}
