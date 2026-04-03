#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{
    engine::general_purpose::STANDARD as BASE64,
    engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL, Engine,
};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::{BTreeMap, HashMap};
use std::fs;
use std::io::{BufRead, BufReader};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::sync::OnceLock;
use std::time::Duration;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, Manager, State, WebviewWindow,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::TcpListener;
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;

static CORE_BINARY: &[u8] = include_bytes!("core.bin");
static CORE_EXTRACTED_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();
static UPDATER_BINARY: &[u8] = include_bytes!("updater.bin");
static UPDATER_EXTRACTED_PATH: OnceLock<Option<PathBuf>> = OnceLock::new();

fn get_embedded_core_path() -> Option<PathBuf> {
    if CORE_BINARY.is_empty() || CORE_BINARY == b"placeholder" {
        return None;
    }

    let cached = CORE_EXTRACTED_PATH.get_or_init(|| {
        let exe_path = std::env::current_exe().ok()?;
        let exe_dir = exe_path.parent()?;
        let core_name = if cfg!(windows) {
            "osagent.exe"
        } else {
            "osagent"
        };
        let core_path = exe_dir.join(core_name);

        if core_path.exists() {
            return Some(core_path);
        }

        fs::write(&core_path, CORE_BINARY).ok()?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let _ = Command::new("attrib")
                .args(["+R", core_path.to_string_lossy().as_ref()])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok();
            let _ = Command::new("attrib")
                .args(["+H", core_path.to_string_lossy().as_ref()])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok();
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = fs::metadata(&core_path).map(|m| m.permissions()) {
                let mut mode = perms.mode();
                mode |= 0o111;
                perms.set_mode(mode);
                let _ = fs::set_permissions(&core_path, perms);
            }
        }

        info!("Extracted bundled osagent core to {}", core_path.display());
        Some(core_path)
    });

    cached.clone()
}

fn get_embedded_updater_path() -> Option<PathBuf> {
    if UPDATER_BINARY.is_empty() || UPDATER_BINARY == b"placeholder" {
        return None;
    }

    let cached = UPDATER_EXTRACTED_PATH.get_or_init(|| {
        let exe_path = std::env::current_exe().ok()?;
        let exe_dir = exe_path.parent()?;
        let updater_name = if cfg!(windows) {
            "osagent-updater.exe"
        } else {
            "osagent-updater"
        };
        let updater_path = exe_dir.join(updater_name);

        if updater_path.exists() {
            return Some(updater_path);
        }

        fs::write(&updater_path, UPDATER_BINARY).ok()?;
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            let _ = Command::new("attrib")
                .args(["+R", updater_path.to_string_lossy().as_ref()])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok();
            let _ = Command::new("attrib")
                .args(["+H", updater_path.to_string_lossy().as_ref()])
                .creation_flags(CREATE_NO_WINDOW)
                .output()
                .ok();
        }

        #[cfg(not(windows))]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = fs::metadata(&updater_path).map(|m| m.permissions()) {
                let mut mode = perms.mode();
                mode |= 0o111;
                perms.set_mode(mode);
                let _ = fs::set_permissions(&updater_path, perms);
            }
        }

        info!("Extracted bundled updater to {}", updater_path.display());
        Some(updater_path)
    });

    cached.clone()
}

// --- Helpers ---

/// Strip ANSI/VT escape sequences (CSI, OSC, etc.) from a string.
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next();
                    // CSI: read until the final byte (an ASCII letter)
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    chars.next();
                    // OSC: read until BEL or ESC backslash
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '\x07' {
                            break;
                        }
                        if nc == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    chars.next(); // skip 2-char ESC sequences
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

// --- State ---

pub struct AppState {
    osagent_process: Mutex<Option<Child>>,
    osagent_pid: Mutex<Option<u32>>,
    osagent_running: Mutex<bool>,
    build_running: Mutex<bool>,
    run_profile: Mutex<String>,
    osagent_path: PathBuf,
    config_path: PathBuf,
    logs: Mutex<Vec<LogEntry>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub osagent_path: String,
    pub config_path: String,
}

#[derive(Clone, Serialize)]
struct VoiceStatus {
    whisper_installed: bool,
    piper_installed: bool,
    whisper_model: Option<String>,
    piper_voice: Option<String>,
}

#[derive(Deserialize)]
struct VoiceInstallPayload {
    install_whisper: bool,
    whisper_model: String,
    install_piper: bool,
    piper_voice: String,
}

#[derive(Clone, Serialize)]
struct VoiceProgress {
    model_id: String,
    stage: String,
    progress: f32,
    message: String,
}

#[derive(Clone, Serialize)]
struct BuildProgress {
    compiling: u32,
    current_crate: String,
    warnings: u32,
    errors: u32,
    finished: bool,
    success: bool,
    profile: String,
}

#[derive(Serialize)]
struct BinaryStatus {
    debug_exists: bool,
    release_exists: bool,
}

#[derive(Clone, Serialize)]
pub struct SetupState {
    pub needs_setup: bool,
    pub has_config: bool,
    pub config_path: String,
    pub workspace_path: String,
    pub provider_type: String,
    pub provider_supported: bool,
    pub password_enabled: bool,
    pub password_configured: bool,
    pub api_key_configured: bool,
    pub osagent_found: bool,
}

#[derive(Clone, Serialize, Deserialize)]
struct LauncherOAuthTokenEntry {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Option<i64>,
    scopes: Option<Vec<String>>,
    #[serde(default)]
    account_id: Option<String>,
}

struct LauncherOAuthStorage {
    storage_path: PathBuf,
    encryption_key: Option<[u8; 32]>,
}

impl LauncherOAuthStorage {
    fn new(storage_path: PathBuf) -> Self {
        Self {
            storage_path,
            encryption_key: Self::derive_key(),
        }
    }

    fn derive_key() -> Option<[u8; 32]> {
        let machine_id = Self::machine_id()?;
        let mut hasher = Sha256::new();
        hasher.update(machine_id.as_bytes());
        hasher.update(b"osagent_oauth_salt_v1");
        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result[..32]);
        Some(key)
    }

    fn machine_id() -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            std::env::var("COMPUTERNAME").ok()
        }
        #[cfg(target_os = "macos")]
        {
            std::env::var("HOSTNAME").ok()
        }
        #[cfg(target_os = "linux")]
        {
            std::env::var("HOSTNAME").ok().or_else(|| {
                std::fs::read_to_string("/etc/machine-id")
                    .ok()
                    .map(|s| s.trim().to_string())
            })
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            std::env::var("HOSTNAME").ok()
        }
    }

    fn encrypt(&self, data: &str) -> Result<String, String> {
        let key = self
            .encryption_key
            .ok_or_else(|| "No encryption key available".to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);
        let ciphertext = cipher
            .encrypt(nonce, data.as_bytes())
            .map_err(|e| e.to_string())?;
        let mut combined = nonce_bytes.to_vec();
        combined.extend(ciphertext);
        Ok(BASE64.encode(combined))
    }

    fn decrypt(&self, data: &str) -> Result<String, String> {
        let key = self
            .encryption_key
            .ok_or_else(|| "No encryption key available".to_string())?;
        let cipher = Aes256Gcm::new_from_slice(&key).map_err(|e| e.to_string())?;
        let combined = BASE64.decode(data).map_err(|e| e.to_string())?;
        if combined.len() < 12 {
            return Err("Encrypted token data is too short".to_string());
        }
        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);
        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| e.to_string())?;
        String::from_utf8(plaintext).map_err(|e| e.to_string())
    }

    fn load(&self) -> Result<HashMap<String, LauncherOAuthTokenEntry>, String> {
        if !self.storage_path.exists() {
            return Ok(HashMap::new());
        }

        let raw = fs::read_to_string(&self.storage_path).map_err(|e| e.to_string())?;
        if raw.trim_start().starts_with('{') {
            return serde_json::from_str(&raw).map_err(|e| e.to_string());
        }

        let decrypted = self.decrypt(&raw)?;
        serde_json::from_str(&decrypted).map_err(|e| e.to_string())
    }

    fn save(&self, entries: &HashMap<String, LauncherOAuthTokenEntry>) -> Result<(), String> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }

        let json = serde_json::to_string_pretty(entries).map_err(|e| e.to_string())?;
        let content = if self.encryption_key.is_some() {
            self.encrypt(&json)?
        } else {
            json
        };
        fs::write(&self.storage_path, content).map_err(|e| e.to_string())
    }

    fn set_token(&self, provider_id: &str, entry: LauncherOAuthTokenEntry) -> Result<(), String> {
        let mut entries = self.load()?;
        entries.insert(provider_id.to_string(), entry);
        self.save(&entries)
    }
}

const OPENAI_OAUTH_PORT: u16 = 1455;

fn oauth_storage_path(config_path: &Path) -> PathBuf {
    config_path
        .parent()
        .unwrap_or(config_path)
        .join("oauth_tokens.json")
}

fn generate_pkce_verifier() -> String {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    BASE64_URL.encode(bytes)
}

fn pkce_challenge(verifier: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    BASE64_URL.encode(hash)
}

fn oauth_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill(&mut bytes);
    BASE64_URL.encode(bytes)
}

fn parse_jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = BASE64_URL.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn extract_account_id(id_token: Option<&str>, access_token: Option<&str>) -> Option<String> {
    let from_claims = |claims: &Value| {
        claims
            .get("chatgpt_account_id")
            .and_then(Value::as_str)
            .map(str::to_string)
            .or_else(|| {
                claims
                    .get("https://api.openai.com/auth")
                    .and_then(|v| v.get("chatgpt_account_id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
            .or_else(|| {
                claims
                    .get("organizations")
                    .and_then(Value::as_array)
                    .and_then(|items| items.first())
                    .and_then(|item| item.get("id"))
                    .and_then(Value::as_str)
                    .map(str::to_string)
            })
    };

    id_token
        .and_then(parse_jwt_claims)
        .as_ref()
        .and_then(from_claims)
        .or_else(|| {
            access_token
                .and_then(parse_jwt_claims)
                .as_ref()
                .and_then(from_claims)
        })
}

async fn write_oauth_response(
    stream: &mut tokio::net::TcpStream,
    status: &str,
    title: &str,
    body: &str,
) -> Result<(), String> {
    let html = format!(
        "<!doctype html><html><head><title>{}</title></head><body><h1>{}</h1><p>{}</p><script>setTimeout(() => window.close(), 1500)</script></body></html>",
        title, title, body
    );
    let response = format!(
        "HTTP/1.1 {}\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
        status,
        html.len(),
        html
    );
    stream
        .write_all(response.as_bytes())
        .await
        .map_err(|e| e.to_string())
}

async fn wait_for_oauth_callback(
    listener: TcpListener,
    expected_state: &str,
) -> Result<String, String> {
    let accepted = tokio::time::timeout(Duration::from_secs(300), listener.accept())
        .await
        .map_err(|_| "OAuth callback timed out".to_string())?
        .map_err(|e| e.to_string())?;
    let (mut stream, _) = accepted;
    let mut buffer = vec![0u8; 8192];
    let size = stream.read(&mut buffer).await.map_err(|e| e.to_string())?;
    let request = String::from_utf8_lossy(&buffer[..size]);
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .ok_or_else(|| "Invalid OAuth callback request".to_string())?;
    let url = reqwest::Url::parse(&format!("http://127.0.0.1{}", path))
        .map_err(|e| format!("Invalid OAuth callback URL: {}", e))?;

    if let Some(error) = url
        .query_pairs()
        .find_map(|(k, v)| (k == "error").then(|| v.to_string()))
    {
        let msg = url
            .query_pairs()
            .find_map(|(k, v)| (k == "error_description").then(|| v.to_string()))
            .unwrap_or(error);
        let _ = write_oauth_response(&mut stream, "400 Bad Request", "Authorization failed", &msg)
            .await;
        return Err(msg);
    }

    let state = url
        .query_pairs()
        .find_map(|(k, v)| (k == "state").then(|| v.to_string()))
        .ok_or_else(|| "Missing OAuth state".to_string())?;
    if state != expected_state {
        let _ = write_oauth_response(
            &mut stream,
            "400 Bad Request",
            "Authorization failed",
            "Invalid OAuth state.",
        )
        .await;
        return Err("Invalid OAuth state".to_string());
    }

    let code = url
        .query_pairs()
        .find_map(|(k, v)| (k == "code").then(|| v.to_string()))
        .ok_or_else(|| "Missing authorization code".to_string())?;
    let _ = write_oauth_response(
        &mut stream,
        "200 OK",
        "Authorization successful",
        "You can close this window and return to OSAgent Launcher.",
    )
    .await;
    Ok(code)
}

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
struct ExistingConfig {
    server: ExistingServerConfig,
    providers: Vec<ExistingProviderConfig>,
    default_provider: String,
    default_model: String,
    provider: ExistingProviderConfig,
    agent: ExistingAgentConfig,
}

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
struct ExistingServerConfig {
    bind: String,
    port: u16,
    password: String,
    password_enabled: bool,
}

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
struct ExistingProviderConfig {
    provider_type: String,
    api_key: String,
    base_url: String,
    model: String,
    auth_type: String,
    oauth_client_id: String,
}

#[derive(Clone, Deserialize, Default)]
#[serde(default)]
struct ExistingAgentConfig {
    workspace: String,
    active_workspace: Option<String>,
    workspaces: Vec<ExistingWorkspaceConfig>,
}

#[derive(Clone, Deserialize, Serialize, Default)]
#[serde(default)]
struct ExistingWorkspaceConfig {
    id: String,
    name: String,
    path: String,
    description: Option<String>,
    permission: Option<String>,
    created_at: String,
    last_used: Option<String>,
}

#[derive(Deserialize)]
struct SetupConfigPayload {
    provider_type: String,
    model: String,
    auth_mode: String,
    api_key: String,
    workspace_path: String,
    password_enabled: bool,
    password: String,
    #[serde(default)]
    stt_mode: String,
    #[serde(default)]
    stt_whisper_model: String,
    #[serde(default)]
    tts_mode: String,
    #[serde(default)]
    tts_piper_language: String,
    #[serde(default)]
    tts_piper_voice: String,
    #[serde(default)]
    discord_enabled: bool,
    #[serde(default)]
    discord_token: String,
    #[serde(default)]
    discord_allowed_users: String,
}

#[derive(Deserialize)]
struct ProviderValidationPayload {
    provider_type: String,
    api_key: String,
}

#[derive(Deserialize)]
struct SetupOAuthStartPayload {
    provider_type: String,
}

#[derive(Serialize)]
struct ProviderValidationResult {
    ok: bool,
    message: String,
}

#[derive(Clone, Serialize)]
struct SetupProviderModel {
    id: String,
    name: String,
}

#[derive(Clone, Serialize)]
struct SetupProviderOAuth {
    flow_type: String,
    client_id_configured: bool,
}

#[derive(Clone, Serialize)]
struct SetupProviderInfo {
    id: String,
    name: String,
    description: String,
    base_url: String,
    key_label: String,
    key_placeholder: String,
    key_help: String,
    api_key_required: bool,
    default_model: String,
    models: Vec<SetupProviderModel>,
    oauth: Option<SetupProviderOAuth>,
}

#[derive(Serialize)]
struct SetupOAuthStartResult {
    opened: bool,
    auth_url: String,
    connected: bool,
}

struct ProviderPreset {
    id: String,
    name: String,
    description: String,
    base_url: String,
    key_label: String,
    key_placeholder: String,
    key_help: String,
    api_key_required: bool,
    default_model: String,
    models: Vec<(String, String)>,
    oauth_flow_type: Option<String>,
    oauth_client_id: Option<String>,
    oauth_authorization_url: Option<String>,
    oauth_token_url: Option<String>,
    oauth_device_code_url: Option<String>,
    oauth_scopes: Vec<String>,
}

#[derive(Deserialize)]
struct DeviceCodeStartPayload {
    provider_type: String,
}

#[derive(Serialize)]
struct DeviceCodeStartResult {
    user_code: String,
    verification_uri: String,
    device_code: String,
    interval: u64,
}

#[derive(Deserialize)]
struct DeviceCodePollPayload {
    provider_type: String,
    device_code: String,
}

#[derive(Serialize)]
struct DeviceCodePollResult {
    status: String, // "pending", "success", "error"
    access_token: Option<String>,
    message: String,
}

#[derive(Debug, Deserialize)]
struct SnapshotProvider {
    id: String,
    name: String,
    #[serde(default)]
    api: String,
    #[serde(default)]
    doc: String,
    #[serde(default)]
    models: BTreeMap<String, SnapshotModel>,
}

#[derive(Debug, Deserialize)]
struct SnapshotModel {
    id: String,
    name: String,
    #[serde(default)]
    family: String,
}

type SnapshotCatalog = BTreeMap<String, SnapshotProvider>;

// --- Helper Functions ---

fn get_osagent_path() -> PathBuf {
    // First, try the embedded core bundled in the launcher
    if let Some(embedded) = get_embedded_core_path() {
        return embedded;
    }

    let exe_path = std::env::current_exe()
        .ok()
        .unwrap_or_else(|| PathBuf::from("."));

    // Check for bundled sidecar binary (Tauri externalBin naming)
    let target_triple = tauri_target_triple();
    let sidecar_name = if cfg!(windows) {
        format!("osagent-{}.exe", target_triple)
    } else {
        format!("osagent-{}", target_triple)
    };

    let sidecar_candidates: Vec<PathBuf> = vec![
        exe_path
            .parent()
            .map(|p| p.join(&sidecar_name))
            .unwrap_or_default(),
        exe_path
            .parent()
            .map(|p| p.join("binaries").join(&sidecar_name))
            .unwrap_or_default(),
    ];

    for candidate in &sidecar_candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    // exe is at: .../osagent/launcher/target/release/osagent-launcher.exe
    // We need:   .../osagent/target/release/osagent.exe
    let candidates: Vec<PathBuf> = vec![
        exe_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.join("target").join("release").join("osagent.exe"))
            .unwrap_or_default(),
        exe_path
            .parent()
            .map(|p| p.join("osagent.exe"))
            .unwrap_or_default(),
        PathBuf::from("osagent/target/release/osagent.exe"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    candidates[0].clone()
}

fn tauri_target_triple() -> &'static str {
    #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
    return "x86_64-pc-windows-msvc";
    #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
    return "aarch64-pc-windows-msvc";
    #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
    return "x86_64-unknown-linux-gnu";
    #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
    return "aarch64-unknown-linux-gnu";
    #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
    return "x86_64-apple-darwin";
    #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
    return "aarch64-apple-darwin";
    #[cfg(not(any(
        all(
            target_os = "windows",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "linux",
            any(target_arch = "x86_64", target_arch = "aarch64")
        ),
        all(
            target_os = "macos",
            any(target_arch = "x86_64", target_arch = "aarch64")
        )
    )))]
    return "unknown";
}

fn get_osagent_path_for_profile(profile: &str) -> PathBuf {
    let exe_path = std::env::current_exe()
        .ok()
        .unwrap_or_else(|| PathBuf::from("."));

    let binary_name = if cfg!(windows) {
        "osagent.exe"
    } else {
        "osagent"
    };

    // Check for bundled sidecar first
    let target_triple = tauri_target_triple();
    let sidecar_name = if cfg!(windows) {
        format!("osagent-{}.exe", target_triple)
    } else {
        format!("osagent-{}", target_triple)
    };

    let sidecar_path = exe_path
        .parent()
        .map(|p| p.join(&sidecar_name))
        .unwrap_or_default();
    if sidecar_path.exists() {
        return sidecar_path;
    }

    // exe is at: .../osagent/launcher/target/release/osagent-launcher.exe
    // We need:   .../osagent/target/{profile}/osagent[.exe]
    if let Some(root) = exe_path
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
    {
        return root.join("target").join(profile).join(binary_name);
    }

    PathBuf::from(format!("osagent/target/{}/{}", profile, binary_name))
}

fn get_config_path() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("config.toml")
}

fn default_workspace_path() -> PathBuf {
    if let Some(documents_dir) = dirs_next::document_dir() {
        return documents_dir.join("OSA Workspace");
    }

    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join("OSA Workspace")
}

const OPENROUTER_MODELS: [(&str, &str); 10] = [
    ("anthropic/claude-sonnet-4", "Claude Sonnet 4"),
    ("anthropic/claude-3.5-sonnet", "Claude 3.5 Sonnet"),
    ("openai/gpt-4.1", "GPT-4.1"),
    ("openai/gpt-4o", "GPT-4o"),
    ("openai/o3", "o3"),
    ("google/gemini-2.5-pro", "Gemini 2.5 Pro"),
    ("deepseek/deepseek-r1", "DeepSeek R1"),
    ("meta-llama/llama-3.3-70b-instruct", "Llama 3.3 70B"),
    ("mistralai/mistral-large", "Mistral Large"),
    ("qwen/qwen3-235b-a22b", "Qwen3 235B"),
];

const OPENAI_MODELS: [(&str, &str); 13] = [
    ("gpt-4.1", "GPT-4.1"),
    ("gpt-4.1-mini", "GPT-4.1 Mini"),
    ("gpt-4.1-nano", "GPT-4.1 Nano"),
    ("gpt-4o", "GPT-4o"),
    ("gpt-4o-mini", "GPT-4o Mini"),
    ("o3", "o3"),
    ("o3-mini", "o3-mini"),
    ("o1", "o1"),
    ("gpt-5.4", "GPT-5.4"),
    ("gpt-5.2", "GPT-5.2"),
    ("gpt-5.3-codex", "GPT-5.3 Codex"),
    ("gpt-5.2-codex", "GPT-5.2 Codex"),
    ("gpt-5.1-codex", "GPT-5.1 Codex"),
];

const ANTHROPIC_MODELS: [(&str, &str); 6] = [
    ("claude-opus-4-20250514", "Claude Opus 4"),
    ("claude-sonnet-4-20250514", "Claude Sonnet 4"),
    ("claude-haiku-4-5-20251001", "Claude Haiku 4.5"),
    ("claude-3-5-sonnet-20241022", "Claude 3.5 Sonnet"),
    ("claude-3-opus-20240229", "Claude 3 Opus"),
    ("claude-3-haiku-20240307", "Claude 3 Haiku"),
];

const GOOGLE_MODELS: [(&str, &str); 3] = [
    ("gemini-2.5-pro-preview-05-06", "Gemini 2.5 Pro"),
    ("gemini-2.5-flash-preview-05-20", "Gemini 2.5 Flash"),
    ("gemini-2.0-flash-001", "Gemini 2.0 Flash"),
];

const OLLAMA_MODELS: [(&str, &str); 5] = [
    ("llama3.1:70b", "Llama 3.1 70B"),
    ("qwen3:32b", "Qwen3 32B"),
    ("mistral:7b", "Mistral 7B"),
    ("codellama:13b", "CodeLlama 13B"),
    ("deepseek-r1:14b", "DeepSeek R1 14B"),
];

const GROQ_MODELS: [(&str, &str); 3] = [
    ("llama-3.3-70b-versatile", "Llama 3.3 70B"),
    ("llama-3.1-8b-instant", "Llama 3.1 8B"),
    ("mixtral-8x7b-32768", "Mixtral 8x7B"),
];

const DEEPSEEK_MODELS: [(&str, &str); 2] = [
    ("deepseek-r1", "DeepSeek R1"),
    ("deepseek-chat", "DeepSeek V3"),
];

const XAI_MODELS: [(&str, &str); 2] = [("grok-3", "Grok 3"), ("grok-3-mini", "Grok 3 Mini")];

fn map_models(models: &[(&str, &str)]) -> Vec<(String, String)> {
    models
        .iter()
        .map(|(id, name)| ((*id).to_string(), (*name).to_string()))
        .collect()
}

fn provider_presets() -> Vec<ProviderPreset> {
    vec![
        ProviderPreset {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            description: "Multi-provider aggregator with access to 200+ models".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            key_label: "OpenRouter API Key".to_string(),
            key_placeholder: "sk-or-v1-...".to_string(),
            key_help: "Required for OpenRouter. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "anthropic/claude-sonnet-4".to_string(),
            models: map_models(&OPENROUTER_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
        ProviderPreset {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            description: "OpenAI API direct access".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            key_label: "OpenAI API Key".to_string(),
            key_placeholder: "sk-...".to_string(),
            key_help: "Required for OpenAI. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "gpt-4.1".to_string(),
            models: map_models(&OPENAI_MODELS),
            oauth_flow_type: Some("pkce".to_string()),
            oauth_client_id: std::env::var("OPENAI_OAUTH_CLIENT_ID")
                .ok()
                .or_else(|| Some("app_EMoamEEZ73f0CkXaXp7hrann".to_string())),
            oauth_authorization_url: Some("https://auth.openai.com/oauth/authorize".to_string()),
            oauth_token_url: Some("https://auth.openai.com/oauth/token".to_string()),
            oauth_device_code_url: None,
            oauth_scopes: vec![
                "openid".to_string(),
                "profile".to_string(),
                "email".to_string(),
                "offline_access".to_string(),
            ],
        },
        ProviderPreset {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            description: "Anthropic API direct access".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            key_label: "Anthropic API Key".to_string(),
            key_placeholder: "sk-ant-...".to_string(),
            key_help: "Required for Anthropic. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "claude-sonnet-4-20250514".to_string(),
            models: map_models(&ANTHROPIC_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
        ProviderPreset {
            id: "google".to_string(),
            name: "Google AI".to_string(),
            description: "Google Gemini API (OpenAI-compatible endpoint)".to_string(),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
            key_label: "Google AI API Key".to_string(),
            key_placeholder: "AIza...".to_string(),
            key_help: "Required for Google AI. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "gemini-2.5-pro-preview-05-06".to_string(),
            models: map_models(&GOOGLE_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
        ProviderPreset {
            id: "github-copilot".to_string(),
            name: "GitHub Copilot".to_string(),
            description: "GitHub Copilot chat completions endpoint".to_string(),
            base_url: "https://api.githubcopilot.com".to_string(),
            key_label: "Copilot Token".to_string(),
            key_placeholder: "Optional for OAuth mode".to_string(),
            key_help: "Use sign-in when available, or provide a token if supported in your setup."
                .to_string(),
            api_key_required: false,
            default_model: "gpt-4.1".to_string(),
            models: vec![
                ("claude-sonnet-4".to_string(), "Claude Sonnet 4".to_string()),
                ("gpt-4.1".to_string(), "GPT-4.1".to_string()),
                ("gpt-4o".to_string(), "GPT-4o".to_string()),
                ("o3-mini".to_string(), "o3-mini".to_string()),
                ("o1".to_string(), "o1".to_string()),
            ],
            oauth_flow_type: Some("device_code".to_string()),
            oauth_client_id: std::env::var("GITHUB_COPILOT_OAUTH_CLIENT_ID")
                .ok()
                .or_else(|| std::env::var("GITHUBCOPILOT_OAUTH_CLIENT_ID").ok())
                .or_else(|| Some("Ov23li8tweQw6odWQebz".to_string())),
            oauth_authorization_url: Some("https://github.com/login/oauth/authorize".to_string()),
            oauth_token_url: Some("https://github.com/login/oauth/access_token".to_string()),
            oauth_device_code_url: Some("https://github.com/login/device/code".to_string()),
            oauth_scopes: vec!["read:user".to_string()],
        },
        ProviderPreset {
            id: "ollama".to_string(),
            name: "Ollama".to_string(),
            description: "Run models locally with Ollama".to_string(),
            base_url: "http://localhost:11434/v1".to_string(),
            key_label: "Optional API Key".to_string(),
            key_placeholder: "Usually leave blank".to_string(),
            key_help:
                "Local Ollama usually does not need a key. Keep Ollama running on this machine."
                    .to_string(),
            api_key_required: false,
            default_model: "llama3.1:70b".to_string(),
            models: map_models(&OLLAMA_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
        ProviderPreset {
            id: "groq".to_string(),
            name: "Groq".to_string(),
            description: "Ultra-fast inference with Groq".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            key_label: "Groq API Key".to_string(),
            key_placeholder: "gsk_...".to_string(),
            key_help: "Required for Groq. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "llama-3.3-70b-versatile".to_string(),
            models: map_models(&GROQ_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
        ProviderPreset {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            description: "DeepSeek API direct access".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            key_label: "DeepSeek API Key".to_string(),
            key_placeholder: "sk-...".to_string(),
            key_help: "Required for DeepSeek. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "deepseek-r1".to_string(),
            models: map_models(&DEEPSEEK_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
        ProviderPreset {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            description: "xAI Grok API".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            key_label: "xAI API Key".to_string(),
            key_placeholder: "xai-...".to_string(),
            key_help: "Required for xAI. This stays in your local config file.".to_string(),
            api_key_required: true,
            default_model: "grok-3".to_string(),
            models: map_models(&XAI_MODELS),
            oauth_flow_type: None,
            oauth_client_id: None,
            oauth_authorization_url: None,
            oauth_token_url: None,
            oauth_device_code_url: None,
            oauth_scopes: vec![],
        },
    ]
}

fn models_snapshot_catalog() -> &'static SnapshotCatalog {
    static SNAPSHOT: OnceLock<SnapshotCatalog> = OnceLock::new();
    SNAPSHOT.get_or_init(|| {
        serde_json::from_str(include_str!("../../src/agent/models_snapshot.json"))
            .unwrap_or_default()
    })
}

fn provider_preset(provider_type: &str) -> Option<ProviderPreset> {
    if let Some(preset) = provider_presets()
        .into_iter()
        .find(|preset| preset.id == provider_type)
    {
        return Some(preset);
    }

    let provider = models_snapshot_catalog().get(provider_type)?;
    let mut models: Vec<(String, String)> = provider
        .models
        .values()
        .filter(|m| {
            let family = m.family.to_lowercase();
            !family.contains("embedding") && !family.contains("whisper")
        })
        .map(|m| (m.id.clone(), m.name.clone()))
        .collect();

    if models.is_empty() {
        return None;
    }

    models.sort_by(|a, b| a.1.cmp(&b.1));
    let default_model = models[0].0.clone();

    Some(ProviderPreset {
        id: provider.id.clone(),
        name: provider.name.clone(),
        description: if provider.doc.trim().is_empty() {
            "Configured from models snapshot".to_string()
        } else {
            provider.doc.clone()
        },
        base_url: provider.api.clone(),
        key_label: format!("{} API Key", provider.name),
        key_placeholder: "Enter API key".to_string(),
        key_help: "This key is stored in your local config file.".to_string(),
        api_key_required: true,
        default_model,
        models,
        oauth_flow_type: None,
        oauth_client_id: None,
        oauth_authorization_url: None,
        oauth_token_url: None,
        oauth_device_code_url: None,
        oauth_scopes: vec![],
    })
}

fn setup_catalog_presets() -> Vec<ProviderPreset> {
    let mut by_id: BTreeMap<String, ProviderPreset> = BTreeMap::new();

    for preset in provider_presets() {
        by_id.insert(preset.id.clone(), preset);
    }

    for (provider_id, provider) in models_snapshot_catalog() {
        let entry = by_id.entry(provider_id.clone()).or_insert_with(|| {
            let mut models: Vec<(String, String)> = provider
                .models
                .values()
                .filter(|m| {
                    let family = m.family.to_lowercase();
                    !family.contains("embedding") && !family.contains("whisper")
                })
                .map(|m| (m.id.clone(), m.name.clone()))
                .collect();
            models.sort_by(|a, b| a.1.cmp(&b.1));
            let default_model = models.first().map(|m| m.0.clone()).unwrap_or_default();

            ProviderPreset {
                id: provider.id.clone(),
                name: provider.name.clone(),
                description: if provider.doc.trim().is_empty() {
                    "Configured from models snapshot".to_string()
                } else {
                    provider.doc.clone()
                },
                base_url: provider.api.clone(),
                key_label: format!("{} API Key", provider.name),
                key_placeholder: "Enter API key".to_string(),
                key_help: "This key is stored in your local config file.".to_string(),
                api_key_required: true,
                default_model,
                models,
                oauth_flow_type: None,
                oauth_client_id: None,
                oauth_authorization_url: None,
                oauth_token_url: None,
                oauth_device_code_url: None,
                oauth_scopes: vec![],
            }
        });

        let snapshot_models: Vec<(String, String)> = provider
            .models
            .values()
            .filter(|m| {
                let family = m.family.to_lowercase();
                !family.contains("embedding") && !family.contains("whisper")
            })
            .map(|m| (m.id.clone(), m.name.clone()))
            .collect();

        if !snapshot_models.is_empty() {
            for model in snapshot_models {
                if !entry.models.iter().any(|(id, _)| id == &model.0) {
                    entry.models.push(model);
                }
            }
            entry.models.sort_by(|a, b| a.1.cmp(&b.1));
            // Keep preset default_model if still present, otherwise fall back to first
            if !entry
                .models
                .iter()
                .any(|(id, _)| id == &entry.default_model)
            {
                if let Some((model_id, _)) = entry.models.first() {
                    entry.default_model = model_id.clone();
                }
            }
        }

        if entry.base_url.trim().is_empty() {
            entry.base_url = provider.api.clone();
        }

        if entry.description.trim().is_empty() {
            entry.description = provider.doc.clone();
        }
    }

    by_id
        .into_values()
        .filter(|p| !p.models.is_empty())
        .collect()
}

fn load_existing_config(path: &Path) -> Option<ExistingConfig> {
    let raw = fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

fn load_existing_document(path: &Path) -> Option<toml::Value> {
    let raw = fs::read_to_string(path).ok()?;
    toml::from_str(&raw).ok()
}

fn primary_provider(config: &ExistingConfig) -> Option<ExistingProviderConfig> {
    if !config.default_provider.trim().is_empty() {
        if let Some(provider) = config
            .providers
            .iter()
            .find(|provider| provider.provider_type == config.default_provider)
        {
            return Some(provider.clone());
        }
    }

    if let Some(provider) = config.providers.first() {
        return Some(provider.clone());
    }

    if !config.provider.provider_type.trim().is_empty() {
        return Some(config.provider.clone());
    }

    None
}

fn provider_for_type(
    config: &ExistingConfig,
    provider_type: &str,
) -> Option<ExistingProviderConfig> {
    config
        .providers
        .iter()
        .find(|provider| provider.provider_type == provider_type)
        .cloned()
        .or_else(|| {
            if config.provider.provider_type == provider_type {
                Some(config.provider.clone())
            } else {
                None
            }
        })
}

fn current_provider_type(config: &ExistingConfig, provider: &ExistingProviderConfig) -> String {
    if !config.default_provider.trim().is_empty() {
        config.default_provider.trim().to_string()
    } else {
        provider.provider_type.trim().to_string()
    }
}

fn current_model(config: &ExistingConfig, provider: &ExistingProviderConfig) -> String {
    if !config.default_model.trim().is_empty() {
        config.default_model.trim().to_string()
    } else {
        provider.model.trim().to_string()
    }
}

fn resolve_provider_api_key(
    state: &AppState,
    provider_type: &str,
    provided_api_key: &str,
) -> String {
    let provided_api_key = provided_api_key.trim();
    if !provided_api_key.is_empty() {
        return provided_api_key.to_string();
    }

    load_existing_config(&state.config_path)
        .and_then(|config| provider_for_type(&config, provider_type))
        .map(|provider| provider.api_key.trim().to_string())
        .unwrap_or_default()
}

fn ensure_table(value: &mut toml::Value) -> &mut toml::map::Map<String, toml::Value> {
    if !value.is_table() {
        *value = toml::Value::Table(toml::map::Map::new());
    }

    value.as_table_mut().unwrap()
}

fn ensure_child_table<'a>(
    table: &'a mut toml::map::Map<String, toml::Value>,
    key: &str,
) -> &'a mut toml::map::Map<String, toml::Value> {
    if !matches!(table.get(key), Some(toml::Value::Table(_))) {
        table.insert(key.to_string(), toml::Value::Table(toml::map::Map::new()));
    }

    table.get_mut(key).unwrap().as_table_mut().unwrap()
}

fn provider_value(
    provider_type: &str,
    api_key: &str,
    base_url: &str,
    model: &str,
    auth_mode: &str,
    preset: &ProviderPreset,
) -> toml::Value {
    let mut table = toml::map::Map::new();
    table.insert(
        "provider_type".to_string(),
        toml::Value::String(provider_type.to_string()),
    );
    table.insert(
        "api_key".to_string(),
        toml::Value::String(api_key.to_string()),
    );
    table.insert(
        "base_url".to_string(),
        toml::Value::String(base_url.to_string()),
    );
    table.insert("model".to_string(), toml::Value::String(model.to_string()));

    if auth_mode == "oauth" {
        table.insert(
            "auth_type".to_string(),
            toml::Value::String("oauth".to_string()),
        );
        if let Some(client_id) = &preset.oauth_client_id {
            if !client_id.trim().is_empty() {
                table.insert(
                    "oauth_client_id".to_string(),
                    toml::Value::String(client_id.clone()),
                );
            }
        }
        if let Some(auth_url) = &preset.oauth_authorization_url {
            table.insert(
                "oauth_authorization_url".to_string(),
                toml::Value::String(auth_url.clone()),
            );
        }
        if let Some(token_url) = &preset.oauth_token_url {
            table.insert(
                "oauth_token_url".to_string(),
                toml::Value::String(token_url.clone()),
            );
        }
        if !preset.oauth_scopes.is_empty() {
            table.insert(
                "oauth_scopes".to_string(),
                toml::Value::Array(
                    preset
                        .oauth_scopes
                        .iter()
                        .map(|scope| toml::Value::String(scope.clone()))
                        .collect(),
                ),
            );
        }
    }

    toml::Value::Table(table)
}

fn default_workspace_value(path: &str) -> toml::Value {
    let now = chrono::Utc::now().to_rfc3339();
    let mut table = toml::map::Map::new();
    table.insert("id".to_string(), toml::Value::String("default".to_string()));
    table.insert(
        "name".to_string(),
        toml::Value::String("Default Workspace".to_string()),
    );
    table.insert("path".to_string(), toml::Value::String(path.to_string()));
    table.insert(
        "description".to_string(),
        toml::Value::String("Default working directory".to_string()),
    );
    table.insert(
        "permission".to_string(),
        toml::Value::String("read_write".to_string()),
    );
    table.insert("created_at".to_string(), toml::Value::String(now.clone()));
    table.insert("last_used".to_string(), toml::Value::String(now));
    toml::Value::Table(table)
}

fn update_default_workspace(
    agent_table: &mut toml::map::Map<String, toml::Value>,
    workspace_path: &str,
) {
    let default_workspace = default_workspace_value(workspace_path);

    if !matches!(agent_table.get("workspaces"), Some(toml::Value::Array(_))) {
        agent_table.insert(
            "workspaces".to_string(),
            toml::Value::Array(vec![default_workspace]),
        );
        return;
    }

    let workspaces = agent_table
        .get_mut("workspaces")
        .unwrap()
        .as_array_mut()
        .unwrap();

    let mut replaced = false;
    for workspace in workspaces.iter_mut() {
        let is_default = workspace
            .get("id")
            .and_then(toml::Value::as_str)
            .map(|id| id == "default")
            .unwrap_or(false);
        if !is_default {
            continue;
        }

        *workspace = default_workspace.clone();
        replaced = true;
        break;
    }

    if !replaced {
        workspaces.push(default_workspace);
    }
}

fn config_is_ready(config: &ExistingConfig) -> bool {
    let provider = match primary_provider(config) {
        Some(provider) => provider,
        None => return false,
    };

    let provider_type = current_provider_type(config, &provider);
    if provider_type.is_empty() {
        return false;
    }

    let model = current_model(config, &provider);
    if model.is_empty() {
        return false;
    }

    if config.agent.workspace.trim().is_empty() {
        return false;
    }

    if config.server.password_enabled && config.server.password.trim().is_empty() {
        return false;
    }

    true
}

fn compute_setup_state(state: &AppState) -> SetupState {
    let has_config = state.config_path.exists();
    let default_workspace = default_workspace_path().display().to_string();

    if !has_config {
        return SetupState {
            needs_setup: true,
            has_config: false,
            config_path: state.config_path.display().to_string(),
            workspace_path: default_workspace,
            provider_type: "openrouter".to_string(),
            provider_supported: true,
            password_enabled: true,
            password_configured: false,
            api_key_configured: false,
            osagent_found: state.osagent_path.exists(),
        };
    }

    match load_existing_config(&state.config_path) {
        Some(config) => {
            let provider = primary_provider(&config).unwrap_or_default();
            let provider_type = current_provider_type(&config, &provider);
            let workspace_path = if config.agent.workspace.trim().is_empty() {
                default_workspace
            } else {
                config.agent.workspace.trim().to_string()
            };
            SetupState {
                needs_setup: !config_is_ready(&config),
                has_config: true,
                config_path: state.config_path.display().to_string(),
                workspace_path,
                provider_type: provider_type.clone(),
                provider_supported: provider_preset(&provider_type).is_some(),
                password_enabled: config.server.password_enabled,
                password_configured: !config.server.password.trim().is_empty(),
                api_key_configured: !provider.api_key.trim().is_empty()
                    || provider.auth_type.trim() == "oauth",
                osagent_found: state.osagent_path.exists(),
            }
        }
        None => SetupState {
            needs_setup: true,
            has_config: true,
            config_path: state.config_path.display().to_string(),
            workspace_path: default_workspace,
            provider_type: "openrouter".to_string(),
            provider_supported: true,
            password_enabled: true,
            password_configured: false,
            api_key_configured: false,
            osagent_found: state.osagent_path.exists(),
        },
    }
}

fn save_setup_config_file(
    state: &AppState,
    payload: SetupConfigPayload,
) -> Result<SetupState, String> {
    let provider_type = payload.provider_type.trim().to_lowercase();
    let selected_model = payload.model.trim().to_string();
    let auth_mode = payload.auth_mode.trim().to_lowercase();
    let workspace_path = payload.workspace_path.trim();
    let password = payload.password.trim().to_string();
    let api_key = if auth_mode == "oauth" {
        String::new()
    } else {
        resolve_provider_api_key(state, &provider_type, &payload.api_key)
    };

    let preset = provider_preset(&provider_type)
        .ok_or_else(|| "Unsupported provider selected".to_string())?;

    if workspace_path.is_empty() {
        return Err("Choose a workspace folder before continuing".to_string());
    }

    if auth_mode != "oauth" && preset.api_key_required && api_key.is_empty() {
        return Err("Enter an API key to continue".to_string());
    }

    if payload.password_enabled && password.is_empty() {
        return Err("Enter a password or turn password protection off".to_string());
    }

    let model = if selected_model.is_empty() {
        if provider_type == "openai" && auth_mode == "oauth" {
            "gpt-5.3-codex".to_string()
        } else {
            preset.default_model.clone()
        }
    } else if preset.models.iter().any(|(id, _)| id == &selected_model) {
        selected_model
    } else {
        return Err("Selected model is not available for this provider".to_string());
    };

    fs::create_dir_all(workspace_path)
        .map_err(|e| format!("Failed to create workspace folder: {}", e))?;

    if let Some(parent) = state.config_path.parent() {
        fs::create_dir_all(parent).map_err(|e| format!("Failed to create config folder: {}", e))?;
    }

    let password_hash = if payload.password_enabled {
        bcrypt::hash(&password, bcrypt::DEFAULT_COST)
            .map_err(|e| format!("Failed to hash password: {}", e))?
    } else {
        String::new()
    };

    let existing_config = load_existing_config(&state.config_path);
    let mut document = load_existing_document(&state.config_path)
        .unwrap_or_else(|| toml::Value::Table(toml::map::Map::new()));

    let root = ensure_table(&mut document);
    root.insert(
        "default_provider".to_string(),
        toml::Value::String(provider_type.clone()),
    );
    root.insert(
        "default_model".to_string(),
        toml::Value::String(model.clone()),
    );

    let server = ensure_child_table(root, "server");
    let bind = existing_config
        .as_ref()
        .map(|config| config.server.bind.trim())
        .filter(|bind| !bind.is_empty())
        .unwrap_or("127.0.0.1");
    let port = existing_config
        .as_ref()
        .map(|config| config.server.port)
        .filter(|port| *port != 0)
        .unwrap_or(8765);
    server.insert("bind".to_string(), toml::Value::String(bind.to_string()));
    server.insert("port".to_string(), toml::Value::Integer(i64::from(port)));
    server.insert("password".to_string(), toml::Value::String(password_hash));
    server.insert(
        "password_enabled".to_string(),
        toml::Value::Boolean(payload.password_enabled),
    );

    let provider_entry = provider_value(
        &provider_type,
        &api_key,
        &preset.base_url,
        &model,
        &auth_mode,
        &preset,
    );
    if !matches!(root.get("providers"), Some(toml::Value::Array(_))) {
        root.insert(
            "providers".to_string(),
            toml::Value::Array(vec![provider_entry.clone()]),
        );
    } else {
        let providers = root.get_mut("providers").unwrap().as_array_mut().unwrap();
        let mut updated = false;
        for existing in providers.iter_mut() {
            let matches_provider = existing
                .get("provider_type")
                .and_then(toml::Value::as_str)
                .map(|existing_type| existing_type == provider_type.as_str())
                .unwrap_or(false);
            if matches_provider {
                *existing = provider_entry.clone();
                updated = true;
                break;
            }
        }

        if !updated {
            providers.push(provider_entry);
        }
    }

    let agent = ensure_child_table(root, "agent");
    agent.insert(
        "workspace".to_string(),
        toml::Value::String(workspace_path.to_string()),
    );
    agent.insert(
        "active_workspace".to_string(),
        toml::Value::String("default".to_string()),
    );
    update_default_workspace(agent, workspace_path);

    // Voice section
    let stt_local = payload.stt_mode == "local";
    let tts_local = payload.tts_mode == "local";
    let voice_stt_provider = if stt_local {
        "whisper-local"
    } else {
        "browser"
    };
    let voice_tts_provider = if tts_local { "piper-local" } else { "browser" };
    let voice_lang = if payload.tts_piper_language.is_empty() {
        "en"
    } else {
        &payload.tts_piper_language
    };
    let whisper_model_val = if stt_local && !payload.stt_whisper_model.is_empty() {
        payload.stt_whisper_model.clone()
    } else {
        "base".to_string()
    };
    let piper_voice_val = if tts_local && !payload.tts_piper_voice.is_empty() {
        Some(payload.tts_piper_voice.clone())
    } else {
        None
    };
    let voice = ensure_child_table(root, "voice");
    voice.insert(
        "enabled".to_string(),
        toml::Value::Boolean(stt_local || tts_local),
    );
    voice.insert(
        "stt_provider".to_string(),
        toml::Value::String(voice_stt_provider.to_string()),
    );
    voice.insert(
        "tts_provider".to_string(),
        toml::Value::String(voice_tts_provider.to_string()),
    );
    voice.insert(
        "language".to_string(),
        toml::Value::String(voice_lang.to_string()),
    );
    voice.insert(
        "whisper_model".to_string(),
        toml::Value::String(whisper_model_val),
    );
    if let Some(pv) = piper_voice_val {
        voice.insert("piper_voice".to_string(), toml::Value::String(pv));
    }

    // Discord section
    if payload.discord_enabled && !payload.discord_token.is_empty() {
        let discord = ensure_child_table(root, "discord");
        discord.insert("enabled".to_string(), toml::Value::Boolean(true));
        discord.insert(
            "token".to_string(),
            toml::Value::String(payload.discord_token.clone()),
        );
        let users: Vec<u64> = payload
            .discord_allowed_users
            .split(',')
            .filter_map(|s| s.trim().parse().ok())
            .collect();
        discord.insert(
            "allowed_users".to_string(),
            toml::Value::Array(
                users
                    .iter()
                    .map(|u| toml::Value::Integer(*u as i64))
                    .collect(),
            ),
        );
    }

    let data = toml::to_string_pretty(&document)
        .map_err(|e| format!("Failed to serialize config: {}", e))?;
    fs::write(&state.config_path, data).map_err(|e| format!("Failed to save config: {}", e))?;

    add_log(
        state,
        "info",
        format!("Saved launcher setup to {}", state.config_path.display()),
    );

    Ok(compute_setup_state(state))
}

async fn validate_provider_connection(
    state: &AppState,
    payload: ProviderValidationPayload,
) -> Result<ProviderValidationResult, String> {
    let provider_type = payload.provider_type.trim().to_lowercase();
    let preset = provider_preset(&provider_type)
        .ok_or_else(|| "Unsupported provider selected".to_string())?;
    let api_key = resolve_provider_api_key(state, &provider_type, &payload.api_key);

    if preset.api_key_required && api_key.trim().is_empty() {
        return Err("Enter an API key before testing this provider".to_string());
    }

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(12))
        .build()
        .map_err(|e| format!("Could not create HTTP client: {}", e))?;

    let request = if provider_type == "ollama" {
        let request = client.get("http://localhost:11434/api/tags");
        if api_key.trim().is_empty() {
            request
        } else {
            request.bearer_auth(api_key)
        }
    } else {
        client
            .get(format!("{}/models", preset.base_url.trim_end_matches('/')))
            .bearer_auth(api_key)
    };

    let response = request.send().await.map_err(|e| match provider_type.as_str() {
        "ollama" => format!(
            "Could not reach Ollama. Make sure the Ollama app or service is running on this machine. {}",
            e
        ),
        _ => format!("Could not reach {}: {}", preset.name, e),
    })?;

    let status = response.status();
    if status.is_success() {
        let message = match provider_type.as_str() {
            "openrouter" => "OpenRouter connection looks good.".to_string(),
            "openai" => "OpenAI connection looks good.".to_string(),
            "ollama" => "Ollama responded and looks ready.".to_string(),
            _ => format!("{} connection looks good.", preset.name),
        };
        return Ok(ProviderValidationResult { ok: true, message });
    }

    let message = match status.as_u16() {
        401 | 403 => format!(
            "{} rejected the credentials. Double-check the API key and try again.",
            match provider_type.as_str() {
                "openrouter" => "OpenRouter",
                "openai" => "OpenAI",
                "ollama" => "Ollama",
                _ => &preset.name,
            }
        ),
        _ => format!(
            "{} returned HTTP {} while testing the connection.",
            match provider_type.as_str() {
                "openrouter" => "OpenRouter",
                "openai" => "OpenAI",
                "ollama" => "Ollama",
                _ => &preset.name,
            },
            status
        ),
    };

    Ok(ProviderValidationResult { ok: false, message })
}

fn add_log_to_state(logs: &Mutex<Vec<LogEntry>>, level: &str, message: String) {
    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
    let entry = LogEntry {
        timestamp: timestamp.clone(),
        level: level.to_string(),
        message: message.clone(),
    };

    if let Ok(mut logs) = logs.lock() {
        logs.push(entry);
        if logs.len() > 500 {
            let drain = logs.len() - 500;
            logs.drain(0..drain);
        }
    }
}

fn add_log(state: &AppState, level: &str, message: String) {
    add_log_to_state(&state.logs, level, message);
}

fn terminate_osagent_processes(state: &AppState) {
    if let Ok(mut process) = state.osagent_process.lock() {
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    if let Ok(mut pid) = state.osagent_pid.lock() {
        *pid = None;
    }
    if let Ok(mut running) = state.osagent_running.lock() {
        *running = false;
    }

    #[cfg(target_os = "windows")]
    {
        use std::os::windows::process::CommandExt;
        let _ = Command::new("taskkill")
            .args(["/F", "/IM", "osagent.exe", "/T"])
            .creation_flags(0x08000000)
            .output();
    }

    #[cfg(target_os = "linux")]
    {
        let _ = Command::new("pkill").args(["-f", "(^|/)osagent$"]).output();
    }

    #[cfg(target_os = "macos")]
    {
        let _ = Command::new("pkill").args(["-f", "(^|/)osagent$"]).output();
    }
}

// --- Voice helpers ---

fn get_voice_dir() -> Option<std::path::PathBuf> {
    dirs_next::home_dir().map(|h| h.join(".osagent").join("voice"))
}

fn check_whisper_installed() -> (bool, Option<String>) {
    let dir = match get_voice_dir() {
        Some(d) => d,
        None => return (false, None),
    };
    #[cfg(target_os = "windows")]
    let binary = dir.join("whisper.exe");
    #[cfg(not(target_os = "windows"))]
    let binary = dir.join("whisper");
    let installed = binary.exists();
    let model = if installed {
        std::fs::read_dir(&dir).ok().and_then(|entries| {
            entries
                .flatten()
                .find(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.starts_with("ggml-") && name.ends_with(".bin")
                })
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.trim_start_matches("ggml-")
                        .trim_end_matches(".bin")
                        .to_string()
                })
        })
    } else {
        None
    };
    (installed, model)
}

fn check_piper_installed() -> (bool, Option<String>) {
    let dir = match get_voice_dir() {
        Some(d) => d,
        None => return (false, None),
    };
    #[cfg(target_os = "windows")]
    let binary = dir.join("piper.exe");
    #[cfg(not(target_os = "windows"))]
    let binary = dir.join("piper");
    let installed = binary.exists();
    let voice = if installed {
        std::fs::read_dir(&dir).ok().and_then(|entries| {
            entries
                .flatten()
                .find(|e| e.file_name().to_string_lossy().ends_with(".onnx"))
                .map(|e| {
                    let name = e.file_name().to_string_lossy().to_string();
                    name.trim_end_matches(".onnx").to_string()
                })
        })
    } else {
        None
    };
    (installed, voice)
}

async fn stream_download(
    url: &str,
    dest: &std::path::Path,
    window: &WebviewWindow,
    model_id: &str,
    stage: &str,
    progress_offset: f32,
    progress_range: f32,
) -> Result<(), String> {
    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Download failed: {}", e))?;

    if !response.status().is_success() {
        return Err(format!("Download failed with HTTP {}", response.status()));
    }

    let total = response.content_length().unwrap_or(0);
    let mut bytes_received: u64 = 0;
    let mut last_pct: i32 = -1;
    let mut collected: Vec<u8> = if total > 0 {
        Vec::with_capacity(total as usize)
    } else {
        Vec::new()
    };

    let mut response = response;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|e| format!("Read error: {}", e))?
    {
        bytes_received += chunk.len() as u64;
        collected.extend_from_slice(&chunk);
        if total > 0 {
            let pct = ((bytes_received as f32 / total as f32) * 100.0) as i32;
            if pct != last_pct && pct % 5 == 0 {
                last_pct = pct;
                let p = progress_offset + (bytes_received as f32 / total as f32) * progress_range;
                let _ = window.emit(
                    "voice-progress",
                    VoiceProgress {
                        model_id: model_id.to_string(),
                        stage: stage.to_string(),
                        progress: p,
                        message: format!("{} {}%", stage, pct),
                    },
                );
            }
        }
    }

    std::fs::write(dest, &collected).map_err(|e| format!("Failed to save file: {}", e))?;
    Ok(())
}

#[allow(dead_code)]
fn extract_zip_powershell(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let output = std::process::Command::new("powershell")
        .args([
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            &format!(
                "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                archive.display(),
                dest.display()
            ),
        ])
        .output()
        .map_err(|e| format!("PowerShell error: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "Extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

#[allow(dead_code)]
fn extract_tar_gz(archive: &std::path::Path, dest: &std::path::Path) -> Result<(), String> {
    let output = std::process::Command::new("tar")
        .args([
            "-xzf",
            &archive.to_string_lossy(),
            "-C",
            &dest.to_string_lossy(),
        ])
        .output()
        .map_err(|e| format!("tar error: {}", e))?;
    if !output.status.success() {
        return Err(format!(
            "Extraction failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }
    Ok(())
}

fn copy_dir_recursive(src: &std::path::Path, dst: &std::path::Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dst)?;
    for entry in std::fs::read_dir(src)? {
        let entry = entry?;
        let src_path = entry.path();
        let dst_path = dst.join(entry.file_name());
        if src_path.is_dir() {
            copy_dir_recursive(&src_path, &dst_path)?;
        } else {
            std::fs::copy(&src_path, &dst_path)?;
        }
    }
    Ok(())
}

// --- Tauri Commands ---

#[tauri::command]
fn get_status(state: State<AppState>) -> AgentStatus {
    let running = *state.osagent_running.lock().unwrap();
    let pid = *state.osagent_pid.lock().unwrap();
    AgentStatus {
        running,
        pid,
        osagent_path: state.osagent_path.to_string_lossy().to_string(),
        config_path: state.config_path.to_string_lossy().to_string(),
    }
}

#[tauri::command]
fn get_logs(state: State<AppState>) -> Vec<LogEntry> {
    state.logs.lock().unwrap().clone()
}

#[tauri::command]
fn get_build_running(state: State<AppState>) -> bool {
    *state.build_running.lock().unwrap()
}

#[tauri::command]
fn get_binary_status() -> BinaryStatus {
    BinaryStatus {
        debug_exists: get_osagent_path_for_profile("debug").exists(),
        release_exists: get_osagent_path_for_profile("release").exists(),
    }
}

#[tauri::command]
fn get_setup_state(state: State<AppState>) -> SetupState {
    compute_setup_state(&state)
}

#[tauri::command]
fn get_setup_provider_catalog() -> Vec<SetupProviderInfo> {
    setup_catalog_presets()
        .into_iter()
        .map(|preset| {
            let oauth = match preset.oauth_flow_type {
                Some(ref flow_type) => {
                    let env_var_name = format!(
                        "{}_OAUTH_CLIENT_ID",
                        preset.id.to_uppercase().replace('-', "_")
                    );
                    let has_client_id = std::env::var(&env_var_name)
                        .map(|v| !v.trim().is_empty())
                        .unwrap_or(false)
                        || preset
                            .oauth_client_id
                            .as_deref()
                            .map(|s| !s.trim().is_empty())
                            .unwrap_or(false)
                        || load_existing_config(&get_config_path())
                            .map(|c| !c.provider.oauth_client_id.trim().is_empty())
                            .unwrap_or(false);
                    Some(SetupProviderOAuth {
                        flow_type: flow_type.clone(),
                        client_id_configured: has_client_id,
                    })
                }
                None => None,
            };

            SetupProviderInfo {
                id: preset.id,
                name: preset.name,
                description: preset.description,
                base_url: preset.base_url,
                key_label: preset.key_label,
                key_placeholder: preset.key_placeholder,
                key_help: preset.key_help,
                api_key_required: preset.api_key_required,
                default_model: preset.default_model,
                models: preset
                    .models
                    .into_iter()
                    .map(|(id, name)| SetupProviderModel { id, name })
                    .collect(),
                oauth,
            }
        })
        .collect()
}

#[derive(Deserialize)]
struct LauncherTokenResponse {
    access_token: String,
    #[serde(default)]
    refresh_token: String,
    #[serde(default)]
    expires_in: Option<i64>,
    #[serde(default)]
    id_token: Option<String>,
}

async fn exchange_setup_oauth_code(
    token_url: &str,
    client_id: &str,
    code: &str,
    redirect_uri: &str,
    code_verifier: &str,
) -> Result<LauncherTokenResponse, String> {
    let response = reqwest::Client::new()
        .post(token_url)
        .header("Content-Type", "application/x-www-form-urlencoded")
        .body(
            [
                ("grant_type", "authorization_code"),
                ("client_id", client_id),
                ("code", code),
                ("redirect_uri", redirect_uri),
                ("code_verifier", code_verifier),
            ]
            .iter()
            .map(|(key, value)| {
                format!(
                    "{}={}",
                    urlencoding::encode(key),
                    urlencoding::encode(value)
                )
            })
            .collect::<Vec<_>>()
            .join("&"),
        )
        .send()
        .await
        .map_err(|e| format!("OAuth token exchange failed: {}", e))?;

    if !response.status().is_success() {
        let status = response.status();
        let body = response.text().await.unwrap_or_default();
        return Err(format!(
            "OAuth token exchange failed ({}): {}",
            status, body
        ));
    }

    response
        .json::<LauncherTokenResponse>()
        .await
        .map_err(|e| format!("Invalid OAuth token response: {}", e))
}

#[tauri::command]
async fn start_setup_oauth(
    payload: SetupOAuthStartPayload,
) -> Result<SetupOAuthStartResult, String> {
    let provider_id = payload.provider_type.trim().to_lowercase();
    let preset =
        provider_preset(&provider_id).ok_or_else(|| "Unsupported provider selected".to_string())?;

    let flow_type = preset
        .oauth_flow_type
        .clone()
        .ok_or_else(|| format!("{} does not support sign-in", preset.name))?;
    if flow_type != "pkce" {
        return Err(format!(
            "{} sign-in flow is not supported in launcher",
            preset.name
        ));
    }

    // Resolution order (same convention as web UI):
    // 1. Standardized env var: {PROVIDER_UPPERCASE}_OAUTH_CLIENT_ID
    // 2. Preset-specific env var (e.g. GITHUB_COPILOT_OAUTH_CLIENT_ID)
    // 3. oauth_client_id stored in existing config.toml
    let env_var_name = format!(
        "{}_OAUTH_CLIENT_ID",
        provider_id.to_uppercase().replace('-', "_")
    );
    let client_id = std::env::var(&env_var_name)
        .ok()
        .or_else(|| {
            preset
                .oauth_client_id
                .clone()
                .filter(|s| !s.trim().is_empty())
        })
        .or_else(|| {
            load_existing_config(&get_config_path()).and_then(|c| {
                let id = c.provider.oauth_client_id.trim().to_string();
                if id.is_empty() {
                    None
                } else {
                    Some(id)
                }
            })
        })
        .filter(|id| !id.trim().is_empty())
        .ok_or_else(|| {
            format!(
                "No OAuth client ID configured for {}. Set the {} environment variable.",
                preset.name, env_var_name
            )
        })?;
    let auth_base = preset
        .oauth_authorization_url
        .clone()
        .ok_or_else(|| format!("No OAuth authorization URL configured for {}", preset.name))?;
    let token_url = preset
        .oauth_token_url
        .clone()
        .ok_or_else(|| format!("No OAuth token URL configured for {}", preset.name))?;

    let redirect_uri = format!("http://localhost:{}/auth/callback", OPENAI_OAUTH_PORT);
    let scope = if preset.oauth_scopes.is_empty() {
        "api.full-access".to_string()
    } else {
        preset.oauth_scopes.join(" ")
    };
    let state = oauth_state();
    let code_verifier = generate_pkce_verifier();
    let code_challenge = pkce_challenge(&code_verifier);

    let listener = TcpListener::bind(("127.0.0.1", OPENAI_OAUTH_PORT))
        .await
        .map_err(|e| {
            format!(
                "Failed to bind OAuth callback port {}: {}",
                OPENAI_OAUTH_PORT, e
            )
        })?;

    let mut url = reqwest::Url::parse(&auth_base)
        .map_err(|e| format!("Invalid OAuth URL for {}: {}", preset.name, e))?;
    url.query_pairs_mut()
        .append_pair("client_id", &client_id)
        .append_pair("redirect_uri", &redirect_uri)
        .append_pair("response_type", "code")
        .append_pair("scope", &scope)
        .append_pair("state", &state)
        .append_pair("code_challenge", &code_challenge)
        .append_pair("code_challenge_method", "S256");

    if provider_id == "openai" {
        url.query_pairs_mut()
            .append_pair("id_token_add_organizations", "true")
            .append_pair("codex_cli_simplified_flow", "true")
            .append_pair("originator", "osagent");
    }

    let auth_url = url.to_string();
    open::that(&auth_url).map_err(|e| format!("Failed to open browser: {}", e))?;
    let code = wait_for_oauth_callback(listener, &state).await?;
    let tokens =
        exchange_setup_oauth_code(&token_url, &client_id, &code, &redirect_uri, &code_verifier)
            .await?;

    let storage = LauncherOAuthStorage::new(oauth_storage_path(&get_config_path()));
    storage.set_token(
        &provider_id,
        LauncherOAuthTokenEntry {
            access_token: tokens.access_token.clone(),
            refresh_token: if tokens.refresh_token.trim().is_empty() {
                None
            } else {
                Some(tokens.refresh_token.clone())
            },
            expires_at: tokens
                .expires_in
                .map(|secs| chrono::Utc::now().timestamp() + secs),
            scopes: Some(preset.oauth_scopes.clone()),
            account_id: extract_account_id(tokens.id_token.as_deref(), Some(&tokens.access_token)),
        },
    )?;

    Ok(SetupOAuthStartResult {
        opened: true,
        auth_url,
        connected: true,
    })
}

#[tauri::command]
async fn start_device_code_oauth(
    payload: DeviceCodeStartPayload,
) -> Result<DeviceCodeStartResult, String> {
    let provider_id = payload.provider_type.trim().to_lowercase();
    let preset = provider_preset(&provider_id).ok_or_else(|| "Unsupported provider".to_string())?;

    let device_code_url = preset
        .oauth_device_code_url
        .ok_or_else(|| format!("{} does not support device code sign-in", preset.name))?;

    let client_id = preset
        .oauth_client_id
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("No OAuth client ID configured for {}", preset.name))?;

    let scope = preset.oauth_scopes.join(" ");

    #[derive(serde::Deserialize)]
    struct DeviceCodeResponse {
        device_code: String,
        user_code: String,
        verification_uri: String,
        #[serde(default)]
        interval: u64,
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(&device_code_url)
        .header("Accept", "application/json")
        .form(&[("client_id", client_id.as_str()), ("scope", scope.as_str())])
        .send()
        .await
        .map_err(|e| format!("Failed to start device code flow: {}", e))?;

    let data: DeviceCodeResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid device code response: {}", e))?;

    Ok(DeviceCodeStartResult {
        user_code: data.user_code,
        verification_uri: data.verification_uri,
        device_code: data.device_code,
        interval: if data.interval == 0 { 5 } else { data.interval },
    })
}

#[tauri::command]
async fn poll_device_code_oauth(
    payload: DeviceCodePollPayload,
) -> Result<DeviceCodePollResult, String> {
    let provider_id = payload.provider_type.trim().to_lowercase();
    let preset = provider_preset(&provider_id).ok_or_else(|| "Unsupported provider".to_string())?;

    let client_id = preset
        .oauth_client_id
        .filter(|s| !s.trim().is_empty())
        .ok_or_else(|| format!("No OAuth client ID for {}", preset.name))?;

    let token_url = preset
        .oauth_token_url
        .clone()
        .ok_or_else(|| format!("No token URL for device code flow for {}", preset.name))?;

    #[derive(serde::Deserialize)]
    struct PollResponse {
        #[serde(default)]
        access_token: String,
        #[serde(default)]
        error: String,
        #[serde(default)]
        error_description: String,
    }

    let client = reqwest::Client::new();
    let resp = client
        .post(token_url)
        .header("Accept", "application/json")
        .form(&[
            ("client_id", client_id.as_str()),
            ("device_code", payload.device_code.as_str()),
            ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
        ])
        .send()
        .await
        .map_err(|e| format!("Poll request failed: {}", e))?;

    let data: PollResponse = resp
        .json()
        .await
        .map_err(|e| format!("Invalid poll response: {}", e))?;

    if !data.access_token.is_empty() {
        let storage = LauncherOAuthStorage::new(oauth_storage_path(&get_config_path()));
        storage.set_token(
            &provider_id,
            LauncherOAuthTokenEntry {
                access_token: data.access_token.clone(),
                refresh_token: None,
                expires_at: None,
                scopes: Some(preset.oauth_scopes.clone()),
                account_id: None,
            },
        )?;
        Ok(DeviceCodePollResult {
            status: "success".to_string(),
            access_token: None,
            message: "Signed in successfully.".to_string(),
        })
    } else if data.error == "authorization_pending" || data.error == "slow_down" {
        Ok(DeviceCodePollResult {
            status: "pending".to_string(),
            access_token: None,
            message: "Waiting for authorization...".to_string(),
        })
    } else {
        Ok(DeviceCodePollResult {
            status: "error".to_string(),
            access_token: None,
            message: if data.error_description.is_empty() {
                data.error
            } else {
                data.error_description
            },
        })
    }
}

#[tauri::command]
fn browse_workspace_folder(state: State<AppState>) -> Option<String> {
    let current_setup = compute_setup_state(&state);
    let starting_dir = PathBuf::from(&current_setup.workspace_path);
    let dialog = rfd::FileDialog::new();
    let dialog = if starting_dir.exists() {
        dialog.set_directory(starting_dir)
    } else {
        dialog.set_directory(default_workspace_path())
    };

    dialog.pick_folder().map(|path| path.display().to_string())
}

#[tauri::command]
fn save_setup_config(
    window: WebviewWindow,
    state: State<AppState>,
    payload: SetupConfigPayload,
) -> Result<SetupState, String> {
    let setup_state = save_setup_config_file(&state, payload)?;
    let _ = window.emit("setup-state-changed", &setup_state);
    Ok(setup_state)
}

#[tauri::command]
async fn validate_setup_provider(
    state: State<'_, AppState>,
    payload: ProviderValidationPayload,
) -> Result<ProviderValidationResult, String> {
    validate_provider_connection(&state, payload).await
}

#[tauri::command]
fn start_osagent(
    window: WebviewWindow,
    state: State<AppState>,
    profile: Option<String>,
) -> Result<AgentStatus, String> {
    let running = *state.osagent_running.lock().unwrap();
    if running {
        return Err("OSAgent is already running".into());
    }

    let resolved_profile = profile.unwrap_or_else(|| "release".to_string());
    *state.run_profile.lock().unwrap() = resolved_profile.clone();
    let binary_path = get_osagent_path_for_profile(&resolved_profile);

    if !binary_path.exists() {
        let msg = format!(
            "osagent binary not found at {} — build it first",
            binary_path.display()
        );
        add_log(&state, "error", msg.clone());
        return Err(msg);
    }

    if compute_setup_state(&state).needs_setup {
        let msg = "Finish setup before starting OSAgent".to_string();
        add_log(&state, "warn", msg.clone());
        window.show().ok();
        window.set_focus().ok();
        return Err(msg);
    }

    if let Some(parent) = state.config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    add_log(
        &state,
        "info",
        format!(
            "Starting osagent ({}) from {}",
            resolved_profile,
            binary_path.display()
        ),
    );

    let log_dir = dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file_path = log_dir.join("launcher_output.log");

    let mut cmd = Command::new(&binary_path);
    cmd.arg("start")
        .arg("--config")
        .arg(&state.config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            *state.osagent_pid.lock().unwrap() = Some(pid);
            *state.osagent_running.lock().unwrap() = true;
            *state.osagent_process.lock().unwrap() = Some(child);

            add_log(&state, "info", format!("OSAgent started with PID: {}", pid));
            add_log(
                &state,
                "info",
                format!("Output log: {}", log_file_path.display()),
            );

            let handle_out = window.app_handle().clone();
            let log_file_clone = log_file_path.clone();
            if let Some(out) = stdout {
                std::thread::spawn(move || {
                    read_output_to_file(out, handle_out, log_file_clone);
                });
            }

            let handle_err = window.app_handle().clone();
            let log_file_clone = log_file_path.clone();
            if let Some(err) = stderr {
                std::thread::spawn(move || {
                    read_output_to_file(err, handle_err, log_file_clone);
                });
            }

            let _ = window.emit(
                "osagent-status-changed",
                AgentStatus {
                    running: true,
                    pid: Some(pid),
                    osagent_path: state.osagent_path.to_string_lossy().to_string(),
                    config_path: state.config_path.to_string_lossy().to_string(),
                },
            );

            update_tray_menu(window.app_handle(), true);

            Ok(get_status(state))
        }
        Err(e) => {
            let msg = format!("Failed to start OSAgent: {}", e);
            add_log(&state, "error", msg.clone());
            Err(msg)
        }
    }
}

#[tauri::command]
fn stop_osagent(window: WebviewWindow, state: State<AppState>) -> Result<AgentStatus, String> {
    let running = *state.osagent_running.lock().unwrap();
    if !running {
        return Err("OSAgent is not running".into());
    }

    add_log(&state, "info", "Stopping OSAgent...".into());

    {
        let mut process = state.osagent_process.lock().unwrap();
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    *state.osagent_pid.lock().unwrap() = None;
    *state.osagent_running.lock().unwrap() = false;

    add_log(&state, "info", "OSAgent stopped".into());

    let osagent_path = state.osagent_path.to_string_lossy().to_string();
    let config_path = state.config_path.to_string_lossy().to_string();

    let _ = window.emit(
        "osagent-status-changed",
        AgentStatus {
            running: false,
            pid: None,
            osagent_path,
            config_path,
        },
    );

    update_tray_menu(window.app_handle(), false);

    Ok(AgentStatus {
        running: false,
        pid: None,
        osagent_path: state.osagent_path.to_string_lossy().to_string(),
        config_path: state.config_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
fn restart_osagent(window: WebviewWindow, app_handle: AppHandle) -> Result<AgentStatus, String> {
    let state = app_handle.state::<AppState>();
    let profile = state.run_profile.lock().unwrap().clone();
    let _ = stop_osagent(window.clone(), state);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let state = app_handle.state::<AppState>();
    start_osagent(window, state, Some(profile))
}

#[tauri::command]
fn open_web_ui() {
    let _ = open::that("http://localhost:8765");
}

#[tauri::command]
fn hide_to_tray(window: WebviewWindow) {
    window.hide().ok();
}

#[tauri::command]
fn build_osagent(
    window: WebviewWindow,
    state: State<AppState>,
    profile: String,
) -> Result<String, String> {
    // Stop osagent if running to avoid file lock issues
    let running = *state.osagent_running.lock().unwrap();
    if running {
        add_log(
            &state,
            "info",
            "Stopping OSAgent before build...".to_string(),
        );
        let mut process = state.osagent_process.lock().unwrap();
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *state.osagent_pid.lock().unwrap() = None;
        *state.osagent_running.lock().unwrap() = false;
        let _ = window.emit(
            "osagent-status-changed",
            AgentStatus {
                running: false,
                pid: None,
                osagent_path: state.osagent_path.to_string_lossy().to_string(),
                config_path: state.config_path.to_string_lossy().to_string(),
            },
        );
        update_tray_menu(window.app_handle(), false);
        // Give the OS a moment to release the file lock
        std::thread::sleep(std::time::Duration::from_millis(500));
    }

    let osagent_dir = get_osagent_path_for_profile(&profile)
        .parent()
        .and_then(|p| p.parent())
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "Could not determine osagent directory".to_string())?;

    let log_file_path = dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("launcher_output.log");

    let start_msg = format!(
        "Building osagent ({}) in {}",
        profile,
        osagent_dir.display()
    );
    add_log(&state, "info", start_msg);
    *state.build_running.lock().unwrap() = true;

    let app_handle = window.app_handle().clone();
    let profile_clone = profile.clone();

    // Emit initial progress event so the frontend can show the progress section immediately.
    let _ = window.emit(
        "build-progress",
        BuildProgress {
            compiling: 0,
            current_crate: String::new(),
            warnings: 0,
            errors: 0,
            finished: false,
            success: false,
            profile: profile.clone(),
        },
    );

    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("cargo");
        if profile_clone == "debug" {
            cmd.args(["build"]);
        } else {
            cmd.args(["build", "--release"]);
        }
        cmd.current_dir(&osagent_dir);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Force the progress bar even though stderr is piped (not a TTY).
        // Disable color so we only get the raw progress text without color codes.
        cmd.env("CARGO_TERM_PROGRESS_WHEN", "always");
        cmd.env("CARGO_TERM_PROGRESS_WIDTH", "60");
        cmd.env("CARGO_TERM_COLOR", "never");

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let state = app_handle.state::<AppState>();
                add_log(&state, "error", format!("Failed to spawn cargo: {}", e));
                *state.build_running.lock().unwrap() = false;
                if let Some(win) = app_handle.get_webview_window("main") {
                    let _ = win.emit(
                        "build-progress",
                        BuildProgress {
                            compiling: 0,
                            current_crate: String::new(),
                            warnings: 0,
                            errors: 1,
                            finished: true,
                            success: false,
                            profile: profile_clone.clone(),
                        },
                    );
                }
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let app_handle_out = app_handle.clone();
        let log_file_out = log_file_path.clone();
        // Save JoinHandles so we can wait for readers to fully drain the pipe
        // before marking the build as done. child.wait() returning does NOT mean
        // the pipe buffers are empty — readers may still have data to read.
        let stdout_handle = stdout.map(|stdout| {
            std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for l in reader.lines().map_while(Result::ok) {
                    let trimmed = l.trim();
                    if !trimmed.is_empty() {
                        let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                        let state = app_handle_out.state::<AppState>();
                        add_log_to_state(&state.logs, "info", trimmed.to_string());
                        if let Ok(mut file) = std::fs::OpenOptions::new()
                            .create(true)
                            .append(true)
                            .open(&log_file_out)
                        {
                            use std::io::Write;
                            let _ = writeln!(file, "[{}] [INFO] {}", timestamp, trimmed);
                        }
                    }
                }
            })
        });

        let app_handle_err = app_handle.clone();
        let log_file_err = log_file_path.clone();
        let profile_for_err = profile_clone.clone();
        // Shared counters for emitting structured build-progress events.
        use std::sync::atomic::{AtomicU32, Ordering};
        let compiling_count = std::sync::Arc::new(AtomicU32::new(0));
        let warnings_count = std::sync::Arc::new(AtomicU32::new(0));
        let errors_count = std::sync::Arc::new(AtomicU32::new(0));
        let cc2 = compiling_count.clone();
        let wc2 = warnings_count.clone();
        let ec2 = errors_count.clone();
        let stderr_handle = stderr.map(|stderr| {
            std::thread::spawn(move || {
                use std::io::Read;
                let mut reader = std::io::BufReader::new(stderr);
                let mut buf = [0u8; 1];
                let mut line = String::new();

                fn process_line(
                    line: &str,
                    app_handle: &AppHandle,
                    cc: &std::sync::Arc<AtomicU32>,
                    wc: &std::sync::Arc<AtomicU32>,
                    ec: &std::sync::Arc<AtomicU32>,
                    profile: &str,
                    log_file: &std::path::Path,
                ) {
                    let clean = strip_ansi_codes(line);
                    let trimmed = clean.trim();
                    if trimmed.is_empty() {
                        return;
                    }
                    let level = if trimmed.starts_with("error") || trimmed.contains("error[") {
                        "error"
                    } else if trimmed.starts_with("warning") {
                        "warn"
                    } else {
                        "info"
                    };
                    // Parse "Compiling crate-name" events
                    if trimmed.starts_with("Compiling ") {
                        let n = cc.fetch_add(1, Ordering::Relaxed) + 1;
                        let crate_display = trimmed
                            .trim_start_matches("Compiling ")
                            .split_whitespace()
                            .next()
                            .unwrap_or("")
                            .to_string();
                        if let Some(win) = app_handle.get_webview_window("main") {
                            let _ = win.emit(
                                "build-progress",
                                BuildProgress {
                                    compiling: n,
                                    current_crate: crate_display.clone(),
                                    warnings: wc.load(Ordering::Relaxed),
                                    errors: ec.load(Ordering::Relaxed),
                                    finished: false,
                                    success: false,
                                    profile: profile.to_string(),
                                },
                            );
                        }
                    }
                    // Parse "Building [===>] 469/472: crate-name" progress bar
                    else if trimmed.starts_with("Building ") {
                        // Extract current/total like "469/472"
                        let progress_info = if let Some(bracket_end) = trimmed.find(']') {
                            let after_bracket = &trimmed[bracket_end + 1..].trim();
                            // Parse "469/472: crate-name"
                            if let Some(colon_pos) = after_bracket.find(':') {
                                let fraction = &after_bracket[..colon_pos].trim();
                                let crate_name = &after_bracket[colon_pos + 1..].trim();
                                // Remove trailing dots
                                let crate_name = crate_name
                                    .trim_end_matches('…')
                                    .trim_end_matches('.')
                                    .trim();
                                Some((fraction.to_string(), crate_name.to_string()))
                            } else {
                                None
                            }
                        } else {
                            None
                        };

                        if let Some((fraction, crate_name)) = progress_info {
                            // Parse "469/472" to get current count
                            let current: u32 = fraction
                                .split('/')
                                .next()
                                .and_then(|s| s.trim().parse().ok())
                                .unwrap_or(0);
                            if let Some(win) = app_handle.get_webview_window("main") {
                                let _ = win.emit(
                                    "build-progress",
                                    BuildProgress {
                                        compiling: current,
                                        current_crate: crate_name.clone(),
                                        warnings: wc.load(Ordering::Relaxed),
                                        errors: ec.load(Ordering::Relaxed),
                                        finished: false,
                                        success: false,
                                        profile: profile.to_string(),
                                    },
                                );
                            }
                        }
                    } else if trimmed.starts_with("Finished") || trimmed.starts_with("error:") {
                        // "Finished" → success; bare "error:" summary → failure
                        let success = trimmed.starts_with("Finished");
                        if !success {
                            ec.fetch_add(1, Ordering::Relaxed);
                        }
                        if let Some(win) = app_handle.get_webview_window("main") {
                            let _ = win.emit(
                                "build-progress",
                                BuildProgress {
                                    compiling: cc.load(Ordering::Relaxed),
                                    current_crate: String::new(),
                                    warnings: wc.load(Ordering::Relaxed),
                                    errors: ec.load(Ordering::Relaxed),
                                    finished: true,
                                    success,
                                    profile: profile.to_string(),
                                },
                            );
                        }
                    } else if trimmed.starts_with("warning") || trimmed.contains("warning[") {
                        wc.fetch_add(1, Ordering::Relaxed);
                    } else if trimmed.starts_with("error") || trimmed.contains("error[") {
                        ec.fetch_add(1, Ordering::Relaxed);
                    }
                    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                    let state = app_handle.state::<AppState>();
                    add_log_to_state(&state.logs, level, trimmed.to_string());
                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(log_file)
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            file,
                            "[{}] [{}] {}",
                            timestamp,
                            level.to_uppercase(),
                            trimmed
                        );
                    }
                }

                while let Ok(1) = reader.read(&mut buf) {
                    let b = buf[0];
                    if b == b'\r' || b == b'\n' {
                        if !line.is_empty() {
                            process_line(
                                &line,
                                &app_handle_err,
                                &cc2,
                                &wc2,
                                &ec2,
                                &profile_for_err,
                                &log_file_err,
                            );
                            line.clear();
                        }
                    } else {
                        line.push(b as char);
                    }
                }
                // Process any remaining content
                if !line.is_empty() {
                    process_line(
                        &line,
                        &app_handle_err,
                        &cc2,
                        &wc2,
                        &ec2,
                        &profile_for_err,
                        &log_file_err,
                    );
                }
            })
        });

        let exit_status = child.wait();

        // Wait for both readers to finish draining the pipe before we
        // write the completion log entry and clear build_running. This
        // guarantees the frontend's final poll sees all output.
        if let Some(h) = stdout_handle {
            let _ = h.join();
        }
        if let Some(h) = stderr_handle {
            let _ = h.join();
        }

        let state = app_handle.state::<AppState>();
        let success = matches!(&exit_status, Ok(s) if s.success());
        match &exit_status {
            Ok(s) if s.success() => {
                add_log(&state, "info", "Build completed successfully".to_string());
            }
            Ok(s) => {
                add_log(
                    &state,
                    "error",
                    format!("Build failed with code: {:?}", s.code()),
                );
            }
            Err(e) => {
                add_log(&state, "error", format!("Build process error: {}", e));
            }
        }
        // Emit a final progress event to ensure the frontend reaches 100% / error state
        // even if cargo didn't print a "Finished" or "error:" summary line.
        if let Some(win) = app_handle.get_webview_window("main") {
            let _ = win.emit(
                "build-progress",
                BuildProgress {
                    compiling: compiling_count.load(Ordering::Relaxed),
                    current_crate: String::new(),
                    warnings: warnings_count.load(Ordering::Relaxed),
                    errors: errors_count.load(Ordering::Relaxed),
                    finished: true,
                    success,
                    profile: profile_clone.clone(),
                },
            );
        }
        *state.build_running.lock().unwrap() = false;
    });

    Ok("Build started".to_string())
}

#[tauri::command]
fn show_window(window: WebviewWindow) {
    window.show().ok();
    window.set_focus().ok();
    window.unminimize().ok();
}

#[tauri::command]
fn minimize_window(window: WebviewWindow) {
    window.minimize().ok();
}

#[tauri::command]
fn exit_app(app: AppHandle, state: State<AppState>) {
    terminate_osagent_processes(&state);
    app.exit(0);
}

#[tauri::command]
fn check_osagent_path(state: State<AppState>) -> String {
    state.osagent_path.to_string_lossy().to_string()
}

#[tauri::command]
fn check_voice_status() -> VoiceStatus {
    let (whisper_installed, whisper_model) = check_whisper_installed();
    let (piper_installed, piper_voice) = check_piper_installed();
    VoiceStatus {
        whisper_installed,
        piper_installed,
        whisper_model,
        piper_voice,
    }
}

#[tauri::command]
async fn install_voice(
    window: WebviewWindow,
    payload: VoiceInstallPayload,
) -> Result<VoiceStatus, String> {
    let dir = get_voice_dir().ok_or_else(|| "Could not determine home directory".to_string())?;
    std::fs::create_dir_all(&dir).map_err(|e| format!("Failed to create voice dir: {}", e))?;

    if payload.install_whisper {
        let (binary_ok, _) = check_whisper_installed();
        if !binary_ok {
            let _ = window.emit(
                "voice-progress",
                VoiceProgress {
                    model_id: "whisper".to_string(),
                    stage: "downloading_binary".to_string(),
                    progress: 0.0,
                    message: "Downloading Whisper binary...".to_string(),
                },
            );

            #[cfg(target_os = "windows")]
            {
                let url = "https://github.com/ggml-org/whisper.cpp/releases/download/v1.8.3/whisper-bin-x64.zip";
                let archive = dir.join("whisper_archive.zip");
                stream_download(
                    url,
                    &archive,
                    &window,
                    "whisper",
                    "downloading_binary",
                    0.0,
                    0.25,
                )
                .await?;

                let _ = window.emit(
                    "voice-progress",
                    VoiceProgress {
                        model_id: "whisper".to_string(),
                        stage: "extracting".to_string(),
                        progress: 0.25,
                        message: "Extracting Whisper binary...".to_string(),
                    },
                );

                let extract_dir = dir.join("whisper_extract");
                std::fs::create_dir_all(&extract_dir).ok();
                extract_zip_powershell(&archive, &extract_dir)?;

                // whisper-bin-x64 zip extracts into a flat directory — find whisper.exe
                let binary_dest = dir.join("whisper.exe");
                // Try direct extraction
                let direct = extract_dir.join("whisper.exe");
                if direct.exists() {
                    std::fs::copy(&direct, &binary_dest).ok();
                } else {
                    // Search recursively
                    if let Ok(entries) = std::fs::read_dir(&extract_dir) {
                        for entry in entries.flatten() {
                            if entry.file_name() == "whisper.exe" {
                                std::fs::copy(entry.path(), &binary_dest).ok();
                                break;
                            }
                        }
                    }
                }
                // Copy DLLs too
                if let Ok(entries) = std::fs::read_dir(&extract_dir) {
                    for entry in entries.flatten() {
                        let name = entry.file_name().to_string_lossy().to_string();
                        if name.ends_with(".dll") {
                            let _ = std::fs::copy(entry.path(), dir.join(&name));
                        }
                    }
                }
                let _ = std::fs::remove_file(&archive);
                let _ = std::fs::remove_dir_all(&extract_dir);
            }

            #[cfg(target_os = "macos")]
            {
                return Err("Automatic Whisper binary installation is only supported on Windows. Please install whisper.cpp manually.".to_string());
            }

            #[cfg(target_os = "linux")]
            {
                return Err("Automatic Whisper binary installation is only supported on Windows. Please install whisper.cpp manually.".to_string());
            }
        }

        // Download model
        let model_id = if payload.whisper_model.is_empty() {
            "base"
        } else {
            &payload.whisper_model
        };
        let model_path = dir.join(format!("ggml-{}.bin", model_id));
        if !model_path.exists() {
            let _ = window.emit(
                "voice-progress",
                VoiceProgress {
                    model_id: "whisper".to_string(),
                    stage: "downloading_model".to_string(),
                    progress: 0.3,
                    message: format!("Downloading Whisper {} model...", model_id),
                },
            );
            let url = format!(
                "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
                model_id
            );
            stream_download(
                &url,
                &model_path,
                &window,
                "whisper",
                "downloading_model",
                0.3,
                0.65,
            )
            .await?;
        }

        let _ = window.emit(
            "voice-progress",
            VoiceProgress {
                model_id: "whisper".to_string(),
                stage: "complete".to_string(),
                progress: 0.95,
                message: "Whisper ready!".to_string(),
            },
        );
    }

    if payload.install_piper {
        let (binary_ok, _) = check_piper_installed();
        if !binary_ok {
            let _ = window.emit(
                "voice-progress",
                VoiceProgress {
                    model_id: "piper".to_string(),
                    stage: "downloading_binary".to_string(),
                    progress: 0.0,
                    message: "Downloading Piper binary...".to_string(),
                },
            );

            #[cfg(target_os = "windows")]
            {
                let url = "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_windows_amd64.zip";
                let archive = dir.join("piper_archive.zip");
                stream_download(
                    url,
                    &archive,
                    &window,
                    "piper",
                    "downloading_binary",
                    0.0,
                    0.25,
                )
                .await?;

                let _ = window.emit(
                    "voice-progress",
                    VoiceProgress {
                        model_id: "piper".to_string(),
                        stage: "extracting".to_string(),
                        progress: 0.25,
                        message: "Extracting Piper binary...".to_string(),
                    },
                );

                let extract_dir = dir.join("piper_extract");
                std::fs::create_dir_all(&extract_dir).ok();
                extract_zip_powershell(&archive, &extract_dir)?;

                let piper_inner = extract_dir.join("piper");
                let binary_src = piper_inner.join("piper.exe");
                let binary_dest = dir.join("piper.exe");
                if binary_src.exists() {
                    std::fs::copy(&binary_src, &binary_dest).ok();
                    // Copy DLLs and espeak-ng-data
                    if let Ok(entries) = std::fs::read_dir(&piper_inner) {
                        for entry in entries.flatten() {
                            let name = entry.file_name().to_string_lossy().to_string();
                            if name.ends_with(".dll") || name == "libtashkeel_model.ort" {
                                let _ = std::fs::copy(entry.path(), dir.join(&name));
                            }
                        }
                    }
                    let espeak_src = piper_inner.join("espeak-ng-data");
                    let espeak_dst = dir.join("espeak-ng-data");
                    if espeak_src.exists() {
                        let _ = copy_dir_recursive(&espeak_src, &espeak_dst);
                    }
                }
                let _ = std::fs::remove_file(&archive);
                let _ = std::fs::remove_dir_all(&extract_dir);
            }

            #[cfg(not(target_os = "windows"))]
            {
                let url = if cfg!(target_os = "macos") {
                    "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_macos_x64.tar.gz"
                } else {
                    "https://github.com/rhasspy/piper/releases/download/2023.11.14-2/piper_linux_x64.tar.gz"
                };
                let archive = dir.join("piper_archive.tar.gz");
                stream_download(
                    url,
                    &archive,
                    &window,
                    "piper",
                    "downloading_binary",
                    0.0,
                    0.25,
                )
                .await?;

                let extract_dir = dir.join("piper_extract");
                std::fs::create_dir_all(&extract_dir).ok();
                extract_tar_gz(&archive, &extract_dir)?;

                let piper_inner = extract_dir.join("piper");
                let binary_src = piper_inner.join("piper");
                let binary_dest = dir.join("piper");
                if binary_src.exists() {
                    std::fs::copy(&binary_src, &binary_dest).ok();
                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        let _ = std::fs::set_permissions(
                            &binary_dest,
                            std::fs::Permissions::from_mode(0o755),
                        );
                    }
                    let espeak_src = piper_inner.join("espeak-ng-data");
                    let espeak_dst = dir.join("espeak-ng-data");
                    if espeak_src.exists() {
                        let _ = copy_dir_recursive(&espeak_src, &espeak_dst);
                    }
                }
                let _ = std::fs::remove_file(&archive);
                let _ = std::fs::remove_dir_all(&extract_dir);
            }
        }

        // Download voice
        if !payload.piper_voice.is_empty() {
            let voice_path = dir.join(format!("{}.onnx", payload.piper_voice));
            if !voice_path.exists() {
                let _ = window.emit(
                    "voice-progress",
                    VoiceProgress {
                        model_id: "piper".to_string(),
                        stage: "downloading_voice".to_string(),
                        progress: 0.3,
                        message: format!("Downloading voice {}...", payload.piper_voice),
                    },
                );

                // Build HuggingFace URL from voice ID convention: lang_REGION-name-quality
                // e.g. en_US-libritts-high → en/en_US/libritts/high/en_US-libritts-high.onnx
                let voice_url = voice_id_to_hf_url(&payload.piper_voice);
                stream_download(
                    &voice_url,
                    &voice_path,
                    &window,
                    "piper",
                    "downloading_voice",
                    0.3,
                    0.65,
                )
                .await?;

                // Download JSON config
                let json_url = format!("{}.json", voice_url);
                let json_path = dir.join(format!("{}.onnx.json", payload.piper_voice));
                let _ = stream_download(
                    &json_url,
                    &json_path,
                    &window,
                    "piper",
                    "downloading_voice_config",
                    0.95,
                    0.04,
                )
                .await;
            }
        }

        let _ = window.emit(
            "voice-progress",
            VoiceProgress {
                model_id: "piper".to_string(),
                stage: "complete".to_string(),
                progress: 1.0,
                message: "Piper ready!".to_string(),
            },
        );
    }

    let (whisper_installed, whisper_model) = check_whisper_installed();
    let (piper_installed, piper_voice) = check_piper_installed();
    Ok(VoiceStatus {
        whisper_installed,
        piper_installed,
        whisper_model,
        piper_voice,
    })
}

fn voice_id_to_hf_url(voice_id: &str) -> String {
    // voice_id: e.g. "en_US-libritts-high" → lang=en, locale=en_US, name=libritts, quality=high
    let parts: Vec<&str> = voice_id.splitn(3, '-').collect();
    if parts.len() < 3 {
        return "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/libritts/high/en_US-libritts-high.onnx".to_string();
    }
    let locale = parts[0]; // e.g. "en_US"
    let name = parts[1]; // e.g. "libritts"
    let quality = parts[2]; // e.g. "high"
    let lang = locale.split('_').next().unwrap_or("en");
    format!(
        "https://huggingface.co/rhasspy/piper-voices/resolve/main/{}/{}/{}/{}/{}.onnx",
        lang, locale, name, quality, voice_id
    )
}

// --- Tray Helpers ---

fn update_tray_menu(app_handle: &AppHandle, running: bool) {
    let open_launcher = MenuItem::with_id(
        app_handle,
        "open_launcher",
        "Open Launcher",
        true,
        None::<&str>,
    )
    .unwrap();
    let open_ui =
        MenuItem::with_id(app_handle, "open_ui", "Open Web UI", true, None::<&str>).unwrap();
    let toggle = if running {
        MenuItem::with_id(
            app_handle,
            "stop_osagent",
            "Stop OSAgent",
            true,
            None::<&str>,
        )
        .unwrap()
    } else {
        MenuItem::with_id(
            app_handle,
            "start_osagent",
            "Start OSAgent",
            true,
            None::<&str>,
        )
        .unwrap()
    };
    let exit_item = MenuItem::with_id(app_handle, "exit", "Exit", true, None::<&str>).unwrap();
    let sep1 = PredefinedMenuItem::separator(app_handle).unwrap();
    let sep2 = PredefinedMenuItem::separator(app_handle).unwrap();

    if let Ok(menu) = Menu::with_items(
        app_handle,
        &[&open_launcher, &open_ui, &sep1, &toggle, &sep2, &exit_item],
    ) {
        if let Some(tray) = app_handle.tray_by_id("main-tray") {
            let _ = tray.set_menu(Some(menu));
        }
    }
}

// --- Output Reader ---

fn read_output_to_file<R: std::io::Read>(reader: R, app_handle: AppHandle, log_path: PathBuf) {
    let buf = BufReader::new(reader);
    for line in buf.lines() {
        match line {
            Ok(l) => {
                let trimmed = l.trim();
                if !trimmed.is_empty() {
                    let level = if trimmed.contains("ERROR")
                        || trimmed.contains("error")
                        || trimmed.contains("Error")
                    {
                        "error"
                    } else if trimmed.contains("WARN")
                        || trimmed.contains("warn")
                        || trimmed.contains("Warning")
                    {
                        "warn"
                    } else {
                        "info"
                    };

                    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                    let entry = LogEntry {
                        timestamp: timestamp.clone(),
                        level: level.to_string(),
                        message: trimmed.to_string(),
                    };

                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            file,
                            "[{}] [{}] {}",
                            timestamp,
                            level.to_uppercase(),
                            trimmed
                        );
                    }

                    let state = app_handle.state::<AppState>();
                    add_log_to_state(&state.logs, level, trimmed.to_string());

                    let _ = app_handle.emit("log-line", entry);
                }
            }
            Err(_) => break,
        }
    }
}

// --- Pending Update Application ---

#[derive(Debug, Clone, serde::Deserialize)]
struct LauncherPendingUpdate {
    tag: String,
    launcher_path: std::path::PathBuf,
    #[allow(dead_code)]
    created_at: chrono::DateTime<chrono::Utc>,
}

fn get_pending_update_path() -> Option<std::path::PathBuf> {
    dirs_next::home_dir().map(|h| h.join(".osagent").join("pending_update.json"))
}

fn spawn_updater_and_exit(launcher_path: &std::path::Path, new_launcher_path: &std::path::Path) -> bool {
    let current_exe = launcher_path.to_string_lossy().to_string();
    let new_exe = new_launcher_path.to_string_lossy().to_string();
    let cleanup_dir = new_launcher_path
        .parent()
        .map(|p| p.to_string_lossy().to_string())
        .unwrap_or_default();

    if let Some(updater_path) = get_embedded_updater_path() {
        info!("Using embedded updater: {}", updater_path.display());

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            use std::process::Command;
            const CREATE_NO_WINDOW: u32 = 0x08000000;
            const DETACHED_PROCESS: u32 = 0x00000008;

            let result = Command::new(&updater_path)
                .args([
                    "--pid",
                    &std::process::id().to_string(),
                    "--old",
                    &current_exe,
                    "--new",
                    &new_exe,
                    "--launch",
                    &current_exe,
                    "--cleanup",
                    &cleanup_dir,
                ])
                .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
                .spawn();

            match result {
                Ok(_) => {
                    info!("Updater spawned successfully, exiting launcher for update");
                    std::process::exit(0);
                }
                Err(e) => {
                    info!("Updater spawn failed: {}, falling back to bat script", e);
                }
            }
        }

        #[cfg(not(windows))]
        {
            use std::process::Command;

            let result = Command::new(&updater_path)
                .args([
                    "--pid",
                    &std::process::id().to_string(),
                    "--old",
                    &current_exe,
                    "--new",
                    &new_exe,
                    "--launch",
                    &current_exe,
                    "--cleanup",
                    &cleanup_dir,
                ])
                .spawn();

            match result {
                Ok(_) => {
                    info!("Updater spawned successfully, exiting launcher for update");
                    std::process::exit(0);
                }
                Err(e) => {
                    info!("Updater spawn failed: {}, falling back to script", e);
                }
            }
        }
    } else {
        info!("Embedded updater not available, falling back to legacy script");
    }

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        use std::process::Command;
        const CREATE_NO_WINDOW: u32 = 0x08000000;
        const DETACHED_PROCESS: u32 = 0x00000008;

        let bat_path = std::env::temp_dir().join("osagent-update.bat");
        let bat = format!(
            "@echo off\r\ntimeout /t 3 /nobreak >nul\r\ncopy /Y \"{new_exe}\" \"{current_exe}\"\r\ndel \"{new_exe}\"\r\nstart \"\" \"{current_exe}\"\r\ndel \"%~f0\"\r\n"
        );

        if let Err(e) = std::fs::write(&bat_path, bat) {
            info!("Failed to write updater bat: {}", e);
            return false;
        }

        info!("Spawning updater bat: {}", bat_path.display());

        let spawned = Command::new("cmd")
            .args(["/c", "start", "/min", "", bat_path.to_string_lossy().as_ref()])
            .creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn();

        match spawned {
            Ok(_) => {
                info!("Updater bat spawned, exiting launcher for update");
                std::process::exit(0);
            }
            Err(e) => {
                info!("Failed to spawn updater bat: {}", e);
                false
            }
        }
    }

    #[cfg(not(windows))]
    {
        info!("Auto-update not supported on this platform without embedded updater");
        false
    }
}

fn apply_pending_update_if_any() -> bool {
    let pending_path = match get_pending_update_path() {
        Some(p) => p,
        None => return false,
    };

    if !pending_path.exists() {
        return false;
    }

    let json = match std::fs::read_to_string(&pending_path) {
        Ok(j) => j,
        Err(e) => {
            info!("Failed to read pending update file: {}", e);
            return false;
        }
    };

    let pending: LauncherPendingUpdate = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(e) => {
            info!("Failed to parse pending update file: {}", e);
            let _ = std::fs::remove_file(&pending_path);
            return false;
        }
    };

    if !pending.launcher_path.exists() {
        info!("Staged binary does not exist: {}", pending.launcher_path.display());
        let _ = std::fs::remove_file(&pending_path);
        return false;
    }

    let launcher_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            info!("Failed to get current exe path: {}", e);
            return false;
        }
    };

    info!("Startup: pending update {} found, applying", pending.tag);
    let _ = std::fs::remove_file(&pending_path);
    spawn_updater_and_exit(&launcher_path, &pending.launcher_path)
}

// --- Process Monitor ---

fn start_process_monitor(app_handle: AppHandle) {
    std::thread::spawn(move || {
        info!("Starting process monitor thread");
        std::thread::sleep(std::time::Duration::from_secs(1));

        let state = app_handle.state::<AppState>();

        loop {
            // Check for pending update each iteration
            if check_and_apply_pending_update(&app_handle) {
                return;
            }

            // Monitor osagent process status
            {
                let mut running = state.osagent_running.lock().unwrap();
                if *running {
                    let mut process = state.osagent_process.lock().unwrap();
                    if let Some(ref mut child) = *process {
                        match child.try_wait() {
                            Ok(Some(status)) => {
                                let code = status.code().unwrap_or(-1);
                                add_log(
                                    &state,
                                    "warn",
                                    format!("OSAgent exited with code: {}", code),
                                );
                                *running = false;
                                state.osagent_pid.lock().unwrap().take();

                                let _ = app_handle.emit(
                                    "osagent-status-changed",
                                    AgentStatus {
                                        running: false,
                                        pid: None,
                                        osagent_path: state.osagent_path.to_string_lossy().to_string(),
                                        config_path: state.config_path.to_string_lossy().to_string(),
                                    },
                                );
                            }
                            Ok(None) => {}
                            Err(e) => {
                                add_log(&state, "error", format!("Failed to check process: {}", e));
                                *running = false;
                                state.osagent_pid.lock().unwrap().take();
                            }
                        }
                    }
                }
            }

            std::thread::sleep(std::time::Duration::from_secs(2));
        }
    });
}

fn check_and_apply_pending_update(app_handle: &AppHandle) -> bool {
    let pending_path = match get_pending_update_path() {
        Some(p) => p,
        None => return false,
    };

    if !pending_path.exists() {
        return false;
    }

    let state = app_handle.state::<AppState>();
    add_log(&state, "info", format!("Pending update file found: {}", pending_path.display()));

    let json = match std::fs::read_to_string(&pending_path) {
        Ok(j) => j,
        Err(e) => {
            add_log(&state, "error", format!("Failed to read pending update file: {}", e));
            return false;
        }
    };

    let pending: LauncherPendingUpdate = match serde_json::from_str(&json) {
        Ok(p) => p,
        Err(e) => {
            add_log(&state, "error", format!("Failed to parse pending update file: {}", e));
            let _ = std::fs::remove_file(&pending_path);
            return false;
        }
    };

    add_log(&state, "info", format!("Pending update: tag={}, binary={}", pending.tag, pending.launcher_path.display()));

    if !pending.launcher_path.exists() {
        add_log(&state, "error", format!("Staged binary missing: {}", pending.launcher_path.display()));
        let _ = std::fs::remove_file(&pending_path);
        return false;
    }

    let launcher_path = match std::env::current_exe() {
        Ok(p) => p,
        Err(e) => {
            add_log(&state, "error", format!("Failed to get current exe: {}", e));
            return false;
        }
    };

    add_log(&state, "info", format!("Applying update {}...", pending.tag));
    let _ = std::fs::remove_file(&pending_path);

    terminate_osagent_processes(&state);
    spawn_updater_and_exit(&launcher_path, &pending.launcher_path)
}

// --- Main Entry ---

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    if apply_pending_update_if_any() {
        info!("Update pending, launcher will restart shortly...");
        std::process::exit(0);
    }

    let osagent_path = get_osagent_path();
    let config_path = get_config_path();

    info!("OSAgent Launcher starting");
    info!("OSAgent path: {}", osagent_path.display());
    info!("Config path: {}", config_path.display());

    let app_state = AppState {
        osagent_process: Mutex::new(None),
        osagent_pid: Mutex::new(None),
        osagent_running: Mutex::new(false),
        build_running: Mutex::new(false),
        run_profile: Mutex::new("release".to_string()),
        osagent_path,
        config_path,
        logs: Mutex::new(Vec::new()),
    };

    tauri::Builder::default()
        .manage(app_state)
        .on_window_event(|window, event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                window.hide().ok();
                api.prevent_close();
            }
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_logs,
            get_build_running,
            get_binary_status,
            get_setup_state,
            get_setup_provider_catalog,
            start_setup_oauth,
            start_device_code_oauth,
            poll_device_code_oauth,
            browse_workspace_folder,
            save_setup_config,
            validate_setup_provider,
            start_osagent,
            stop_osagent,
            restart_osagent,
            open_web_ui,
            hide_to_tray,
            show_window,
            minimize_window,
            exit_app,
            check_osagent_path,
            build_osagent,
            check_voice_status,
            install_voice,
        ])
        .setup(|app| {
            // Build tray menu
            let open_launcher =
                MenuItem::with_id(app, "open_launcher", "Open Launcher", true, None::<&str>)?;
            let open_ui = MenuItem::with_id(app, "open_ui", "Open Web UI", true, None::<&str>)?;
            let start =
                MenuItem::with_id(app, "start_osagent", "Start OSAgent", true, None::<&str>)?;
            let exit_item = MenuItem::with_id(app, "exit", "Exit", true, None::<&str>)?;
            let sep1 = PredefinedMenuItem::separator(app)?;
            let sep2 = PredefinedMenuItem::separator(app)?;
            let menu = Menu::with_items(
                app,
                &[&open_launcher, &open_ui, &sep1, &start, &sep2, &exit_item],
            )?;

            let _tray = TrayIconBuilder::with_id("main-tray")
                .tooltip("OSAgent Launcher")
                .icon(app.default_window_icon().unwrap().clone())
                .menu(&menu)
                .show_menu_on_left_click(false)
                .on_tray_icon_event(|tray, event| match event {
                    TrayIconEvent::Click {
                        button: MouseButton::Left,
                        button_state: MouseButtonState::Up,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            if window.is_visible().unwrap_or(false) {
                                window.hide().ok();
                            } else {
                                window.show().ok();
                                window.set_focus().ok();
                            }
                        }
                    }
                    TrayIconEvent::DoubleClick {
                        button: MouseButton::Left,
                        ..
                    } => {
                        let app = tray.app_handle();
                        if let Some(window) = app.get_webview_window("main") {
                            window.show().ok();
                            window.set_focus().ok();
                        }
                    }
                    _ => {}
                })
                .on_menu_event(|app, event| match event.id.as_ref() {
                    "open_launcher" => {
                        if let Some(window) = app.get_webview_window("main") {
                            window.show().ok();
                            window.set_focus().ok();
                            window.unminimize().ok();
                        }
                    }
                    "open_ui" => {
                        open_web_ui();
                    }
                    "start_osagent" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = start_osagent(window, app.state(), None);
                        }
                    }
                    "stop_osagent" => {
                        if let Some(window) = app.get_webview_window("main") {
                            let _ = stop_osagent(window, app.state());
                        }
                    }
                    "exit" => {
                        let state = app.state::<AppState>();
                        terminate_osagent_processes(&state);
                        app.exit(0);
                    }
                    _ => {}
                })
                .build(app)?;

            start_process_monitor(app.handle().clone());

            let state = app.state::<AppState>();
            add_log(&state, "info", "OSAgent Launcher initialized".into());
            if compute_setup_state(&state).needs_setup {
                add_log(
                    &state,
                    "info",
                    "Launcher is ready to guide first-time setup".into(),
                );
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
