use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::lsp::client::LspClient;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::sync::Arc;

pub struct LspTool {
    client: Arc<LspClient>,
    config: Arc<tokio::sync::RwLock<Config>>,
}

impl LspTool {
    pub fn new(config: Config) -> Self {
        let client = LspClient::new(config.lsp.servers.clone());
        Self {
            client: Arc::new(client),
            config: Arc::new(tokio::sync::RwLock::new(config)),
        }
    }
}

#[async_trait]
impl Tool for LspTool {
    fn name(&self) -> &str {
        "lsp"
    }

    fn description(&self) -> &str {
        "Language Server Protocol operations for code navigation"
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to navigate to definitions, find references, or get code information"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "operation": {
                    "type": "string",
                    "enum": [
                        "goToDefinition",
                        "findReferences",
                        "hover",
                        "documentSymbol",
                        "workspaceSymbol"
                    ],
                    "description": "The LSP operation to perform"
                },
                "file_path": {
                    "type": "string",
                    "description": "The absolute or relative path to the file"
                },
                "line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "The line number (1-based)"
                },
                "character": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "The character position (1-based)"
                }
            },
            "required": ["operation", "file_path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let operation = args["operation"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing operation".to_string()))?;

        let file_path = args["file_path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing file_path".to_string()))?;

        let line = args["line"].as_u64().unwrap_or(1) as u32;
        let character = args["character"].as_u64().unwrap_or(1) as u32;

        let config = self.config.read().await;
        let workspace_path = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        drop(config);

        let file_abs = if PathBuf::from(file_path).is_absolute() {
            PathBuf::from(file_path)
        } else {
            workspace_path.join(file_path)
        };

        let file_str = file_abs.to_string_lossy().to_string();

        let result = match operation {
            "goToDefinition" => {
                self.client
                    .goto_definition(&file_str, line, character, &workspace_path)
                    .await?
            }
            "findReferences" => {
                self.client
                    .find_references(&file_str, line, character, &workspace_path)
                    .await?
            }
            "hover" => {
                self.client
                    .hover(&file_str, line, character, &workspace_path)
                    .await?
            }
            "documentSymbol" => {
                self.client
                    .document_symbol(&file_str, &workspace_path)
                    .await?
            }
            "workspaceSymbol" => {
                let query = args.get("query").and_then(|v| v.as_str()).unwrap_or("");
                self.client.workspace_symbol(query, &workspace_path).await?
            }
            _ => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Unknown operation: {}",
                    operation
                )))
            }
        };

        let output = serde_json::to_string_pretty(&result)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;

        Ok(format!(
            "{} {}:{}:{}\n{}",
            operation, file_path, line, character, output
        ))
    }
}
