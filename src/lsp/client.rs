use crate::config::LspServerConfig;
use crate::error::{OSAgentError, Result};
use crate::lsp::transport::LspTransport;
use dashmap::DashMap;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
pub struct LspDiagnostic {
    pub line: u32,
    pub character: u32,
    pub end_line: u32,
    pub end_character: u32,
    pub severity: u32,
    pub code: Option<String>,
    pub source: Option<String>,
    pub message: String,
}

pub struct LspClient {
    clients: Arc<DashMap<String, Arc<RwLock<Option<LspTransport>>>>>,
    servers: Arc<HashMap<String, LspServerConfig>>,
}

impl LspClient {
    pub fn new(servers: HashMap<String, LspServerConfig>) -> Self {
        Self {
            clients: Arc::new(DashMap::new()),
            servers: Arc::new(servers),
        }
    }

    fn get_server_for_file(file_path: &str) -> Option<(String, LspServerConfig)> {
        let path = Path::new(file_path);
        let ext = path.extension()?.to_str()?;

        match ext {
            "rs" => Some((
                "rust".to_string(),
                LspServerConfig {
                    command: "rust-analyzer".to_string(),
                    args: vec![],
                    root_markers: vec!["Cargo.toml".to_string(), "rust-project.json".to_string()],
                },
            )),
            "ts" | "tsx" | "js" | "jsx" => Some((
                "typescript".to_string(),
                LspServerConfig {
                    command: "typescript-language-server".to_string(),
                    args: vec!["--stdio".to_string()],
                    root_markers: vec!["package.json".to_string(), "tsconfig.json".to_string()],
                },
            )),
            "py" => Some((
                "python".to_string(),
                LspServerConfig {
                    command: "pylsp".to_string(),
                    args: vec![],
                    root_markers: vec![
                        "pyproject.toml".to_string(),
                        "setup.py".to_string(),
                        "requirements.txt".to_string(),
                    ],
                },
            )),
            "go" => Some((
                "go".to_string(),
                LspServerConfig {
                    command: "gopls".to_string(),
                    args: vec![],
                    root_markers: vec!["go.mod".to_string()],
                },
            )),
            "java" => Some((
                "java".to_string(),
                LspServerConfig {
                    command: "jdtls".to_string(),
                    args: vec![],
                    root_markers: vec!["pom.xml".to_string(), "build.gradle".to_string()],
                },
            )),
            _ => None,
        }
    }

    pub async fn get_or_create_client(
        &self,
        file_path: &str,
        workspace: &Path,
    ) -> Result<Arc<RwLock<Option<LspTransport>>>> {
        let server_info = Self::get_server_for_file(file_path)
            .or_else(|| {
                self.servers
                    .iter()
                    .next()
                    .map(|(k, v)| (k.clone(), v.clone()))
            })
            .ok_or_else(|| {
                OSAgentError::ToolExecution(
                    "No LSP server available for this file type".to_string(),
                )
            })?;

        let key = server_info.0.clone();

        if let Some(client) = self.clients.get(&key) {
            return Ok(client.clone());
        }

        if !command_available(&server_info.1.command) {
            return Err(OSAgentError::ToolExecution(format!(
                "LSP server '{}' not found on PATH",
                server_info.1.command
            )));
        }

        let mut transport = LspTransport::spawn(
            &server_info.1.command,
            &server_info
                .1
                .args
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            workspace,
        )?;

        let init_params = serde_json::json!({
            "processId": null,
            "rootUri": format!("file://{}", workspace.display()),
            "capabilities": {}
        });
        match transport.request("initialize", init_params).await {
            Ok(resp) => {
                if resp.get("error").is_some() {
                    eprintln!("[LSP] initialize error: {}", resp["error"]);
                }
            }
            Err(e) => {
                eprintln!("[LSP] initialize failed: {}", e);
            }
        }
        let _ = transport.notify("initialized", serde_json::json!({})).await;

        let client = Arc::new(RwLock::new(Some(transport)));
        self.clients.insert(key, client.clone());

        Ok(client)
    }

    pub async fn goto_definition(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
        workspace: &Path,
    ) -> Result<Value> {
        let client = self.get_or_create_client(file_path, workspace).await?;
        let mut guard = client.write().await;

        if let Some(ref mut transport) = *guard {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": format!("file://{}", file_path)
                },
                "position": {
                    "line": line,
                    "character": character
                }
            });
            transport.request("textDocument/definition", params).await
        } else {
            Err(OSAgentError::ToolExecution(
                "LSP client not initialized".to_string(),
            ))
        }
    }

    pub async fn find_references(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
        workspace: &Path,
    ) -> Result<Value> {
        let client = self.get_or_create_client(file_path, workspace).await?;
        let mut guard = client.write().await;

        if let Some(ref mut transport) = *guard {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": format!("file://{}", file_path)
                },
                "position": {
                    "line": line,
                    "character": character
                }
            });
            transport.request("textDocument/references", params).await
        } else {
            Err(OSAgentError::ToolExecution(
                "LSP client not initialized".to_string(),
            ))
        }
    }

    pub async fn hover(
        &self,
        file_path: &str,
        line: u32,
        character: u32,
        workspace: &Path,
    ) -> Result<Value> {
        let client = self.get_or_create_client(file_path, workspace).await?;
        let mut guard = client.write().await;

        if let Some(ref mut transport) = *guard {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": format!("file://{}", file_path)
                },
                "position": {
                    "line": line,
                    "character": character
                }
            });
            transport.request("textDocument/hover", params).await
        } else {
            Err(OSAgentError::ToolExecution(
                "LSP client not initialized".to_string(),
            ))
        }
    }

    pub async fn document_symbol(&self, file_path: &str, workspace: &Path) -> Result<Value> {
        let client = self.get_or_create_client(file_path, workspace).await?;
        let mut guard = client.write().await;

        if let Some(ref mut transport) = *guard {
            let params = serde_json::json!({
                "textDocument": {
                    "uri": format!("file://{}", file_path)
                }
            });
            transport
                .request("textDocument/documentSymbol", params)
                .await
        } else {
            Err(OSAgentError::ToolExecution(
                "LSP client not initialized".to_string(),
            ))
        }
    }

    pub async fn workspace_symbol(&self, query: &str, workspace: &Path) -> Result<Value> {
        let client = self.get_or_create_client("dummy.rs", workspace).await?;
        let mut guard = client.write().await;

        if let Some(ref mut transport) = *guard {
            let params = serde_json::json!({
                "query": query
            });
            transport.request("workspace/symbol", params).await
        } else {
            Err(OSAgentError::ToolExecution(
                "LSP client not initialized".to_string(),
            ))
        }
    }

    pub async fn shutdown(&self) -> Result<()> {
        for entry in self.clients.iter() {
            if let Some(ref mut transport) = *entry.value().write().await {
                transport.kill();
            }
        }
        self.clients.clear();
        Ok(())
    }

    pub async fn diagnostics(&self, file_path: &str, workspace: &Path) -> Vec<LspDiagnostic> {
        let client = match self.get_or_create_client(file_path, workspace).await {
            Ok(client) => client,
            Err(_) => return Vec::new(),
        };

        let uri = format!("file://{}", file_path);
        let mut guard = client.write().await;
        let Some(ref mut transport) = *guard else {
            return Vec::new();
        };

        let mut diagnostics: Vec<LspDiagnostic> = Vec::new();

        let params = serde_json::json!({
            "textDocument": { "uri": uri.clone() }
        });
        match transport.request("textDocument/diagnostic", params).await {
            Ok(resp) => {
                if resp.get("error").is_none() {
                    if let Some(result) = resp.get("result") {
                        let items: Vec<Value> = match result {
                            Value::Array(items) => items.clone(),
                            Value::Object(map) => map
                                .get("items")
                                .and_then(|v| v.as_array())
                                .cloned()
                                .unwrap_or_default(),
                            _ => Vec::new(),
                        };
                        if !result.is_null() || !items.is_empty() {
                            diagnostics = parse_diagnostics(items);
                        }
                    }
                }
            }
            Err(e) => {
                eprintln!("[LSP] textDocument/diagnostic request failed: {}", e);
            }
        }

        if diagnostics.is_empty() {
            if let Some(pushed) = transport.pushed_diagnostics(&uri) {
                if let Some(items) = pushed.get("diagnostics").and_then(|v| v.as_array()) {
                    diagnostics = parse_diagnostics(items.clone());
                }
            }
        }

        diagnostics
    }
}

fn command_available(command: &str) -> bool {
    let path_env = std::env::var_os("PATH").unwrap_or_default();
    let has_extension =
        command.ends_with(".exe") || command.ends_with(".cmd") || command.ends_with(".bat");
    std::env::split_paths(&path_env).any(|dir| {
        let candidate = dir.join(command);
        candidate.is_file()
            || (!has_extension && cfg!(windows) && dir.join(format!("{}.exe", command)).is_file())
    })
}

fn parse_diagnostics(items: Vec<Value>) -> Vec<LspDiagnostic> {
    items
        .iter()
        .filter_map(|item| {
            let range = item.get("range")?;
            let start = range.get("start")?;
            let end = range.get("end")?;
            let line = start.get("line")?.as_u64()?;
            let character = start.get("character")?.as_u64()?;
            Some(LspDiagnostic {
                line: line as u32,
                character: character as u32,
                end_line: end.get("line").and_then(|v| v.as_u64()).unwrap_or(line) as u32,
                end_character: end
                    .get("character")
                    .and_then(|v| v.as_u64())
                    .unwrap_or(character) as u32,
                severity: item.get("severity").and_then(|v| v.as_u64()).unwrap_or(1) as u32,
                code: item.get("code").map(|v| match v {
                    Value::String(s) => s.clone(),
                    other => other.to_string(),
                }),
                source: item
                    .get("source")
                    .and_then(|v| v.as_str())
                    .map(String::from),
                message: item
                    .get("message")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string(),
            })
        })
        .collect()
}
