use crate::config::LspServerConfig;
use crate::error::{OSAgentError, Result};
use crate::lsp::transport::LspTransport;
use dashmap::DashMap;
use serde_json::Value;
use std::collections::HashMap;
use std::path::Path;
use std::sync::Arc;
use tokio::sync::RwLock;

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

        let transport = LspTransport::spawn(
            &server_info.1.command,
            &server_info
                .1
                .args
                .iter()
                .map(|s| s.as_str())
                .collect::<Vec<_>>(),
            workspace,
        )?;

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
}
