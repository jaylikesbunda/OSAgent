use crate::error::{OSAgentError, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::sync::{Arc, RwLock as StdRwLock};
use std::time::Duration;
use tokio::sync::{mpsc, RwLock};

pub struct LspTransport {
    process: Child,
    request_id: Arc<RwLock<u64>>,
    pending: Arc<RwLock<HashMap<u64, mpsc::Sender<Value>>>>,
    publish: Arc<StdRwLock<HashMap<String, Value>>>,
}

const LSP_RESPONSE_TIMEOUT: Duration = Duration::from_secs(15);

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

        let pending = Arc::new(RwLock::new(HashMap::<u64, mpsc::Sender<Value>>::new()));
        let publish = Arc::new(StdRwLock::new(HashMap::<String, Value>::new()));
        let pending_reader = pending.clone();
        let publish_reader = publish.clone();

        std::thread::spawn(move || {
            let mut reader = BufReader::new(stdout);
            while let Some(body) = read_frame(&mut reader) {
                match serde_json::from_str::<Value>(&body) {
                    Ok(message) => {
                        if let Some(id) = message.get("id").and_then(|v| v.as_u64()) {
                            if let Some(sender) = pending_reader.blocking_write().remove(&id) {
                                let _ = sender.try_send(message);
                            }
                        } else if message.get("method").and_then(|m| m.as_str())
                            == Some("textDocument/publishDiagnostics")
                        {
                            if let Some(params) = message.get("params") {
                                if let Some(uri) = params.get("uri").and_then(|u| u.as_str()) {
                                    publish_reader
                                        .write()
                                        .unwrap()
                                        .insert(uri.to_string(), params.clone());
                                }
                            }
                        } else {
                            eprintln!("[LSP notification] {}", body.trim());
                        }
                    }
                    Err(_) => {
                        eprintln!("[LSP parse error] {}", body.trim());
                    }
                }
            }
        });

        Ok(Self {
            process,
            request_id: Arc::new(RwLock::new(0)),
            pending,
            publish,
        })
    }

    pub async fn notify(&mut self, method: &str, params: Value) -> Result<()> {
        let request = serde_json::json!({
            "jsonrpc": "2.0",
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
        Ok(())
    }

    pub fn pushed_diagnostics(&self, uri: &str) -> Option<Value> {
        self.publish.read().unwrap().get(uri).cloned()
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

        let (tx, mut rx) = mpsc::channel::<Value>(1);
        {
            let mut pending = self.pending.write().await;
            pending.insert(id, tx);
        }

        let mut stdin = self
            .process
            .stdin
            .take()
            .ok_or_else(|| OSAgentError::Unknown("LSP stdin not available".to_string()))?;

        stdin
            .write_all(message.as_bytes())
            .map_err(OSAgentError::Io)?;

        self.process.stdin = Some(stdin);

        tokio::select! {
            maybe = rx.recv() => {
                match maybe {
                    Some(response) => Ok(response),
                    None => Err(OSAgentError::Unknown(
                        "LSP channel closed before response".to_string(),
                    )),
                }
            }
            _ = tokio::time::sleep(LSP_RESPONSE_TIMEOUT) => {
                self.pending.write().await.remove(&id);
                Err(OSAgentError::Unknown(format!(
                    "LSP request '{}' timed out after {}s",
                    method,
                    LSP_RESPONSE_TIMEOUT.as_secs()
                )))
            }
        }
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

fn read_frame(reader: &mut BufReader<std::process::ChildStdout>) -> Option<String> {
    let mut content_length: Option<usize> = None;
    loop {
        let mut line = String::new();
        match reader.read_line(&mut line) {
            Ok(0) => return None,
            Ok(_) => {
                let line = line.trim();
                if line.is_empty() {
                    break;
                }
                if let Some(value) = line.strip_prefix("Content-Length:") {
                    content_length = value.trim().parse::<usize>().ok();
                }
            }
            Err(_) => return None,
        }
    }

    let length = content_length?;
    let mut body_bytes = vec![0u8; length];
    let mut read_total = 0usize;
    while read_total < length {
        match reader.read(&mut body_bytes[read_total..]) {
            Ok(0) => return None,
            Ok(n) => read_total += n,
            Err(_) => return None,
        }
    }

    String::from_utf8(body_bytes).ok()
}
