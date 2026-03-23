use crate::error::{OSAgentError, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

pub struct LspTransport {
    process: Child,
    request_id: Arc<RwLock<u64>>,
    pending: Arc<RwLock<HashMap<u64, mpsc::Sender<Value>>>>,
}

impl LspTransport {
    pub fn spawn(command: &str, args: &[&str], cwd: &Path) -> Result<Self> {
        let mut process = Command::new(command)
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .map_err(|e| OSAgentError::Unknown(format!("Failed to spawn LSP server: {}", e)))?;

        let stdout = process.stdout.take().ok_or_else(|| {
            OSAgentError::Unknown("Failed to take stdout from LSP process".to_string())
        })?;

        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            let mut buffer = String::new();
            loop {
                buffer.clear();
                match reader.read_line(&mut buffer) {
                    Ok(0) => break,
                    Ok(_) => {
                        let line = buffer.trim();
                        if line.starts_with("Content-Length:") {
                            continue;
                        }
                        if line.is_empty() {
                            let mut body = String::new();
                            if let Ok(n) = reader.read_line(&mut body) {
                                if n > 0 {
                                    eprintln!("[LSP] {}", body.trim());
                                }
                            }
                        }
                    }
                    Err(_) => break,
                }
            }
        });

        Ok(Self {
            process,
            request_id: Arc::new(RwLock::new(0)),
            pending: Arc::new(RwLock::new(HashMap::new())),
        })
    }

    pub async fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        let id = {
            let mut counter = self.request_id.write().await;
            *counter += 1;
            *counter
        };

        let request = serde_json::json!({
            "jsonrpc": "2.0",
            "id": id,
            "method": method,
            "params": params
        });

        let request_str =
            serde_json::to_string(&request).map_err(|e| OSAgentError::Parse(e.to_string()))?;

        let message = format!(
            "Content-Length: {}\r\n\r\n{}",
            request_str.len(),
            request_str
        );

        let mut stdin = self
            .process
            .stdin
            .take()
            .ok_or_else(|| OSAgentError::Unknown("LSP stdin not available".to_string()))?;

        stdin
            .write_all(message.as_bytes())
            .map_err(OSAgentError::Io)?;

        self.process.stdin = Some(stdin);

        tokio::time::sleep(Duration::from_millis(100)).await;

        Ok(serde_json::json!({
            "id": id,
            "result": serde_json::Value::Null
        }))
    }

    pub fn is_running(&mut self) -> bool {
        self.process.try_wait().ok().flatten().is_none()
    }

    pub fn kill(&mut self) {
        let _ = self.process.kill();
    }
}

impl Drop for LspTransport {
    fn drop(&mut self) {
        self.kill();
    }
}
