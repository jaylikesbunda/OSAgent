use crate::error::OSAgentError;
use crate::skills::config::{parse_frontmatter, ConfigField, MaskedValue, SkillConfigStore};
use crate::skills::installer::{InstallResult, SkillInstaller};
use crate::skills::store::{SkillInfo, SkillStore};
use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Command;
use std::sync::Arc;
use tracing::{info, warn};

pub struct SkillService {
    store: Arc<SkillStore>,
    installer: Arc<SkillInstaller>,
    config_store: Arc<SkillConfigStore>,
}

impl SkillService {
    pub fn new() -> Self {
        let store = Arc::new(SkillStore::new());
        let installer = Arc::new(SkillInstaller::new());
        let config_store = Arc::new(SkillConfigStore::new(
            crate::skills::config::get_config_base_dir(),
        ));

        Self {
            store,
            installer,
            config_store,
        }
    }

    pub fn list_skills(&self) -> Result<Vec<SkillInfo>, OSAgentError> {
        self.store
            .list_skills()
            .map_err(|e| OSAgentError::Unknown(format!("Failed to list skills: {}", e)))
    }

    pub fn get_skill(&self, name: &str) -> Result<SkillInfo, OSAgentError> {
        self.store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill: {}", e)))
    }

    pub fn get_skill_content(&self, name: &str) -> Result<String, OSAgentError> {
        self.store
            .get_skill_content(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill content: {}", e)))
    }

    pub fn install_skill(&self, bundle_data: &[u8]) -> Result<InstallResult, OSAgentError> {
        self.installer.install_from_bundle(bundle_data)
    }

    pub fn uninstall_skill(&self, name: &str) -> Result<(), OSAgentError> {
        self.installer.uninstall(name)
    }

    pub fn upgrade_skill(
        &self,
        name: &str,
        bundle_data: &[u8],
    ) -> Result<InstallResult, OSAgentError> {
        self.installer.upgrade_from_bundle(name, bundle_data)
    }

    pub fn export_skill(&self, name: &str) -> Result<Vec<u8>, OSAgentError> {
        self.installer.export_skill(name)
    }

    pub fn get_config(&self, name: &str) -> Result<HashMap<String, MaskedValue>, OSAgentError> {
        self.config_store
            .get_masked_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get config: {}", e)))
    }

    pub fn save_config(
        &self,
        name: &str,
        settings: HashMap<String, String>,
    ) -> Result<(), OSAgentError> {
        let mut config = self
            .config_store
            .load_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to load config: {}", e)))?;

        for (key, value) in settings {
            if !value.is_empty() {
                config.settings.insert(key, value);
            } else {
                config.settings.remove(&key);
            }
        }

        self.config_store
            .save_config(name, &config)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to save config: {}", e)))
    }

    pub fn delete_config_value(&self, name: &str, key: &str) -> Result<(), OSAgentError> {
        let mut config = self
            .config_store
            .load_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to load config: {}", e)))?;
        config.settings.remove(key);
        self.config_store
            .save_config(name, &config)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to save config: {}", e)))
    }

    pub fn set_skill_enabled(&self, name: &str, enabled: bool) -> Result<(), OSAgentError> {
        let mut config = self
            .config_store
            .load_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to load config: {}", e)))?;
        config.enabled = enabled;
        self.config_store
            .save_config(name, &config)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to save config: {}", e)))
    }

    pub fn is_skill_enabled(&self, name: &str) -> bool {
        self.config_store
            .load_config(name)
            .map(|c| c.enabled)
            .unwrap_or(true)
    }

    pub fn get_skill_env(&self, name: &str) -> Result<HashMap<String, String>, OSAgentError> {
        if !self.is_skill_enabled(name) {
            return Ok(HashMap::new());
        }
        self.store
            .get_env_for_skill(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get env: {}", e)))
    }

    pub fn skill_exists(&self, name: &str) -> bool {
        self.store.skill_exists(name)
    }

    pub fn reload_skill(&self, name: &str) -> Result<SkillInfo, OSAgentError> {
        if !self.store.skill_exists(name) {
            return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
        }
        self.store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill: {}", e)))
    }

    pub fn reload_all(&self) -> Result<Vec<SkillInfo>, OSAgentError> {
        self.store
            .list_skills()
            .map_err(|e| OSAgentError::Unknown(format!("Failed to list skills: {}", e)))
    }

    pub async fn authorize_skill(&self, name: &str) -> Result<String, OSAgentError> {
        let skill_md_path = self.store.skill_skill_md_path(name);
        let content = std::fs::read_to_string(&skill_md_path).map_err(|e| {
            OSAgentError::Unknown(format!("Failed to read SKILL.md for '{}': {}", name, e))
        })?;

        let schema = parse_frontmatter(&content).ok_or_else(|| {
            OSAgentError::Unknown(format!("Skill '{}' has no valid frontmatter", name))
        })?;

        let token_refresh = schema.token_refresh.as_ref().ok_or_else(|| {
            OSAgentError::Unknown(format!(
                "Skill '{}' has no token_refresh block — authorize is not supported",
                name
            ))
        })?;

        if token_refresh.supports_native_oauth() {
            self.authorize_native(name, token_refresh).await
        } else {
            let authorize_action = schema
                .actions
                .iter()
                .find(|a| a.name == "authorize")
                .ok_or_else(|| {
                    OSAgentError::Unknown(format!(
                        "Skill '{}' has no authorize_url and no 'authorize' action",
                        name
                    ))
                })?;
            self.authorize_via_script(name, authorize_action, token_refresh)
                .await
        }
    }

    async fn authorize_native(
        &self,
        name: &str,
        tr: &crate::skills::config::SkillTokenRefreshSchema,
    ) -> Result<String, OSAgentError> {
        let config = self.config_store.load_config(name).unwrap_or_default();

        let client_id = config.settings.get(&tr.client_id_field).cloned().ok_or_else(|| {
            OSAgentError::Unknown(format!(
                "{} not configured for skill '{}'",
                tr.client_id_field, name
            ))
        })?;
        let client_secret = if tr.client_secret_field.is_empty() {
            String::new()
        } else {
            config.settings.get(&tr.client_secret_field).cloned().ok_or_else(|| {
                OSAgentError::Unknown(format!(
                    "{} not configured for skill '{}'",
                    tr.client_secret_field, name
                ))
            })?
        };

        let authorize_url = tr.authorize_url.as_ref().unwrap();
        let redirect_uri = tr.redirect_uri();
        let port = tr.callback_port;

        let mut auth_url = format!(
            "{}?client_id={}&response_type=code&redirect_uri={}",
            authorize_url,
            urlencoding::encode(&client_id),
            urlencoding::encode(&redirect_uri),
        );
        if let Some(scopes) = &tr.scopes {
            auth_url.push_str("&scope=");
            auth_url.push_str(&urlencoding::encode(scopes));
        }

        info!("Starting native OAuth for '{}' on port {}", name, port);

        let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", port))
            .await
            .map_err(|e| {
                OSAgentError::Unknown(format!("Failed to listen on port {}: {}", port, e))
            })?;

        info!("Opening browser for '{}': {}", name, auth_url);
        open_browser(&auth_url)?;

        let (stream, _) = tokio::time::timeout(
            std::time::Duration::from_secs(120),
            listener.accept(),
        )
        .await
        .map_err(|_| {
            OSAgentError::Unknown("Timed out waiting for authorization callback".into())
        })?
        .map_err(|e| {
            OSAgentError::Unknown(format!("Failed to accept callback connection: {}", e))
        })?;

        let request_bytes = tokio::time::timeout(
            std::time::Duration::from_secs(10),
            read_http_request(stream),
        )
        .await
        .map_err(|_| {
            OSAgentError::Unknown("Timed out reading callback request".into())
        })??;

        let (auth_code, error_param) = parse_callback_params(&request_bytes);

        let response_html = if auth_code.is_some() {
            "<html><body style=\"font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;background:#121212;color:#fff\"><div style=\"text-align:center\"><h1 style=\"color:#1db954\">Authorized!</h1><p>You can close this window.</p></div></body></html>"
        } else {
            "<html><body style=\"font-family:sans-serif;display:flex;align-items:center;justify-content:center;height:100vh;background:#121212;color:#fff\"><div style=\"text-align:center\"><h1 style=\"color:#e74c3c\">Failed</h1><p>Authorization was denied or failed.</p></div></body></html>"
        };

        send_http_response(request_bytes.stream, response_html).await?;

        if let Some(err) = error_param {
            return Err(OSAgentError::Unknown(format!(
                "Authorization denied: {}", err
            )));
        }
        let code = auth_code.ok_or_else(|| {
            OSAgentError::Unknown("No authorization code received from callback".into())
        })?;

        info!("Exchanging authorization code for tokens for '{}'", name);

        let mut body: HashMap<String, String> = HashMap::new();
        body.insert("grant_type".to_string(), "authorization_code".to_string());
        body.insert("code".to_string(), code);
        body.insert("redirect_uri".to_string(), redirect_uri);
        body.insert("client_id".to_string(), client_id);
        if !client_secret.is_empty() {
            body.insert("client_secret".to_string(), client_secret);
        }

        let client = reqwest::Client::new();
        let response = client
            .post(&tr.token_url)
            .form(&body)
            .send()
            .await
            .map_err(|e| {
                OSAgentError::Unknown(format!("Token exchange request failed: {}", e))
            })?;

        let status = response.status();
        let response_text = response.text().await.map_err(|e| {
            OSAgentError::Unknown(format!("Failed to read token response: {}", e))
        })?;

        if !status.is_success() {
            return Err(OSAgentError::Unknown(format!(
                "Token exchange failed (HTTP {}): {}",
                status, response_text
            )));
        }

        let tokens: serde_json::Value = serde_json::from_str(&response_text).map_err(|e| {
            OSAgentError::Unknown(format!("Token response is not valid JSON: {}", e))
        })?;

        self.save_tokens(name, &tokens, &tr.refresh_token_field, &tr.access_token_field)
    }

    async fn authorize_via_script(
        &self,
        name: &str,
        authorize_action: &crate::skills::config::SkillActionSchema,
        tr: &crate::skills::config::SkillTokenRefreshSchema,
    ) -> Result<String, OSAgentError> {
        let runner = match &authorize_action.runner {
            crate::skills::config::SkillActionRunner::Script { script, args } => {
                (script.clone(), args.clone())
            }
            _ => {
                return Err(OSAgentError::Unknown(format!(
                    "Authorize action for skill '{}' is not a script action",
                    name
                )));
            }
        };

        let (script, args) = runner;
        let skill_dir = self.store.skill_dir(name);
        let sep = std::path::MAIN_SEPARATOR.to_string();
        let normalized_script = script.replace('/', sep.as_str());
        let script_path = skill_dir.join(&normalized_script);
        if !script_path.exists() {
            return Err(OSAgentError::Unknown(format!(
                "Authorize script not found: {}",
                script_path.to_string_lossy()
            )));
        }

        let skill_dir = skill_dir.canonicalize().map_err(|e| {
            OSAgentError::Unknown(format!("Failed to resolve skill directory: {}", e))
        })?;
        let script_path = script_path.canonicalize().map_err(|e| {
            OSAgentError::Unknown(format!("Failed to resolve authorize script path: {}", e))
        })?;

        let config = self.config_store.load_config(name).unwrap_or_default();
        let env: HashMap<String, String> = config.settings;
        let refresh_token_field = tr.refresh_token_field.clone();
        let access_token_field = tr.access_token_field.clone();

        let output = tokio::task::spawn_blocking(move || {
            let sep = std::path::MAIN_SEPARATOR.to_string();
            let script_path_str = script_path.to_string_lossy().replace('/', sep.as_str());
            let script_path_for_cmd = std::path::PathBuf::from(&script_path_str);
            let extension = script_path_for_cmd
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            let mut command = match extension.as_str() {
                "ps1" => {
                    let mut cmd = Command::new(if cfg!(windows) { "powershell" } else { "pwsh" });
                    cmd.arg("-NoProfile")
                        .arg("-ExecutionPolicy")
                        .arg("Bypass")
                        .arg("-File")
                        .arg(&script_path_str);
                    cmd
                }
                "sh" => {
                    let mut cmd = Command::new("sh");
                    cmd.arg(&script_path_str);
                    cmd
                }
                "py" => {
                    let mut cmd = Command::new("python");
                    cmd.arg(&script_path_str);
                    cmd
                }
                "js" => {
                    let mut cmd = Command::new("node");
                    cmd.arg(&script_path_str);
                    cmd
                }
                _ => Command::new(&script_path_str),
            };

            command.current_dir(&skill_dir);
            command.args(&args);
            command.envs(&env);

            command.output()
        })
        .await
        .map_err(|e| OSAgentError::Unknown(format!("Authorize script task failed: {}", e)))?
        .map_err(|e| OSAgentError::Unknown(format!("Failed to execute authorize script: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let msg = if !stderr.is_empty() { &stderr } else { &stdout };
            return Err(OSAgentError::Unknown(format!(
                "Authorize script failed: {}",
                msg
            )));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        info!("Authorize script stdout for '{}': {} bytes", name, stdout.len());

        let json_line = stdout
            .lines()
            .filter_map(|line| {
                let trimmed = line.trim();
                if trimmed.starts_with('{') && trimmed.ends_with('}') {
                    serde_json::from_str::<serde_json::Value>(trimmed).ok().map(|_| trimmed)
                } else {
                    None
                }
            })
            .next_back();

        let json_str = json_line.ok_or_else(|| {
            warn!("No JSON line found in authorize output for '{}'. Raw output:\n{}", name, stdout);
            OSAgentError::Unknown(format!(
                "Authorize script for '{}' did not return a valid JSON object. Output:\n{}",
                name, stdout
            ))
        })?;

        let tokens: serde_json::Value = serde_json::from_str(json_str).map_err(|e| {
            OSAgentError::Unknown(format!("Failed to parse authorize output JSON: {}", e))
        })?;

        if let Some(err) = tokens.get("error").and_then(|v| v.as_str()) {
            return Err(OSAgentError::Unknown(format!(
                "Authorize script returned error: {}", err
            )));
        }

        self.save_tokens(name, &tokens, &refresh_token_field, &access_token_field)
    }

    fn save_tokens(
        &self,
        name: &str,
        tokens: &serde_json::Value,
        refresh_token_field: &str,
        access_token_field: &str,
    ) -> Result<String, OSAgentError> {
        let mut saved_keys = Vec::new();
        let mut config = self.config_store.load_config(name).unwrap_or_default();

        if let Some(rt) = tokens.get("refresh_token").and_then(|v| v.as_str()) {
            info!("Saving refresh_token to config key '{}' for skill '{}'", refresh_token_field, name);
            config.settings.insert(refresh_token_field.to_string(), rt.to_string());
            saved_keys.push(refresh_token_field.to_string());
        } else {
            warn!("No refresh_token found in authorize output for skill '{}'", name);
        }
        if let Some(at) = tokens.get("access_token").and_then(|v| v.as_str()) {
            info!("Saving access_token to config key '{}' for skill '{}'", access_token_field, name);
            config.settings.insert(access_token_field.to_string(), at.to_string());
            saved_keys.push(access_token_field.to_string());
        } else {
            warn!("No access_token found in authorize output for skill '{}'", name);
        }

        if saved_keys.is_empty() {
            return Err(OSAgentError::Unknown(
                "Authorize returned JSON but contained no refresh_token or access_token fields".to_string()
            ));
        }

        self.config_store.save_config(name, &config).map_err(|e| {
            OSAgentError::Unknown(format!("Failed to save tokens: {}", e))
        })?;

        let verify = self.config_store.load_config(name).map_err(|e| {
            OSAgentError::Unknown(format!("Saved tokens but failed to verify ({}). Try authorizing again.", e))
        })?;
        for key in &saved_keys {
            if !verify.settings.contains_key(key) {
                return Err(OSAgentError::Unknown(format!(
                    "Token '{}' was saved but not found on verification. Config file may be corrupted.",
                    key
                )));
            }
        }

        info!("Tokens saved and verified for skill '{}' ({} keys)", name, saved_keys.len());
        Ok("Authorization successful! Tokens saved automatically.".to_string())
    }
}

impl Default for SkillService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SkillDetails {
    #[serde(flatten)]
    pub info: SkillInfo,
    pub config: HashMap<String, MaskedValue>,
    pub config_schema: Vec<ConfigField>,
    pub content: String,
    pub has_authorize: bool,
}

impl SkillService {
    pub fn get_skill_details(&self, name: &str) -> Result<SkillDetails, OSAgentError> {
        let info = self
            .store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill info: {}", e)))?;
        let config = self.get_config(name)?;
        let config_schema = info.config_schema.clone();
        let content = self
            .store
            .get_skill_content(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get content: {}", e)))?;

        let has_authorize = parse_frontmatter(&content)
            .map(|schema| {
                schema.token_refresh.as_ref().map(|tr| tr.supports_native_oauth()).unwrap_or(false)
                    || schema.actions.iter().any(|a| a.name == "authorize")
            })
            .unwrap_or(false);

        Ok(SkillDetails {
            info,
            config,
            config_schema,
            content,
            has_authorize,
        })
    }
}

struct CallbackRequest {
    stream: tokio::net::TcpStream,
    path: String,
}

async fn read_http_request(mut stream: tokio::net::TcpStream) -> Result<CallbackRequest, OSAgentError> {
    use tokio::io::AsyncReadExt;
    let mut buf = vec![0u8; 8192];
    let n = stream.read(&mut buf).await.map_err(|e| {
        OSAgentError::Unknown(format!("Failed to read callback request: {}", e))
    })?;
    let request = String::from_utf8_lossy(&buf[..n]).to_string();
    let path = request
        .lines()
        .next()
        .and_then(|line| line.split_whitespace().nth(1))
        .unwrap_or("/")
        .to_string();
    Ok(CallbackRequest { stream, path })
}

fn parse_callback_params(req: &CallbackRequest) -> (Option<String>, Option<String>) {
    let code = regex_on_path(&req.path, r"[?&]code=([^&\s]+)");
    let error = regex_on_path(&req.path, r"[?&]error=([^&\s]+)");
    (code, error)
}

fn regex_on_path(path: &str, pattern: &str) -> Option<String> {
    regex::Regex::new(pattern)
        .ok()
        .and_then(|re| re.captures(path))
        .and_then(|caps| caps.get(1).map(|m| m.as_str().to_string()))
}

async fn send_http_response(
    mut stream: tokio::net::TcpStream,
    html: &str,
) -> Result<(), OSAgentError> {
    use tokio::io::AsyncWriteExt;
    let body = html.as_bytes();
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Type: text/html; charset=utf-8\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    );
    stream.write_all(response.as_bytes()).await.map_err(|e| {
        OSAgentError::Unknown(format!("Failed to send callback response: {}", e))
    })?;
    stream.write_all(body).await.map_err(|e| {
        OSAgentError::Unknown(format!("Failed to send callback body: {}", e))
    })?;
    let _ = stream.shutdown().await;
    Ok(())
}

fn open_browser(url: &str) -> Result<(), OSAgentError> {
    let result = if cfg!(target_os = "windows") {
        Command::new("rundll32")
            .args(["url.dll,FileProtocolHandler", url])
            .spawn()
    } else if cfg!(target_os = "macos") {
        Command::new("open").arg(url).spawn()
    } else {
        Command::new("xdg-open").arg(url).spawn()
    };
    result.map_err(|e| {
        OSAgentError::Unknown(format!("Failed to open browser: {}", e))
    })?;
    Ok(())
}
