//! Transports for talking to an MCP server: a child process over stdio,
//! or streamable HTTP.
//!
//! Both expose the same shape — send a JSON-RPC value, get frames back on
//! a channel — so `McpClient` doesn't branch on transport per call.

use crate::error::{OSAgentError, Result};
use crate::mcp::protocol::JsonRpcFrame;
use std::collections::HashMap;
use std::process::Stdio;
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin};
use tokio::sync::{mpsc, Mutex};
use tracing::{debug, warn};

pub enum Transport {
    Stdio(StdioTransport),
    Http(HttpTransport),
}

impl Transport {
    pub async fn send(&self, payload: serde_json::Value) -> Result<()> {
        match self {
            Self::Stdio(transport) => transport.send(payload).await,
            Self::Http(transport) => transport.send(payload).await,
        }
    }

    pub async fn shutdown(&self) {
        match self {
            Self::Stdio(transport) => transport.shutdown().await,
            Self::Http(_) => {}
        }
    }
}

/// Child process speaking line-delimited JSON-RPC on stdin/stdout.
pub struct StdioTransport {
    stdin: Mutex<ChildStdin>,
    child: Mutex<Child>,
}

impl StdioTransport {
    /// Spawn `command` and start pumping its stdout into `frames`.
    ///
    /// stderr is drained on its own task — MCP servers are chatty there,
    /// and a full stderr pipe deadlocks the child.
    pub async fn spawn(
        server: &str,
        command: &str,
        args: &[String],
        env: &HashMap<String, String>,
        cwd: Option<&str>,
        frames: mpsc::UnboundedSender<JsonRpcFrame>,
    ) -> Result<Self> {
        let mut builder = tokio::process::Command::new(command);
        builder
            .args(args)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped());

        for (key, value) in env {
            builder.env(key, value);
        }
        if let Some(dir) = cwd {
            builder.current_dir(shellexpand::tilde(dir).to_string());
        }

        // Without this the child survives an OSA crash and keeps a stale
        // stdin pipe open.
        #[cfg(unix)]
        builder.kill_on_drop(true);
        #[cfg(windows)]
        builder.kill_on_drop(true);

        let mut child = builder.spawn().map_err(|error| {
            OSAgentError::ToolExecution(format!(
                "MCP server '{}': failed to spawn '{}': {}",
                server, command, error
            ))
        })?;

        let stdin = child.stdin.take().ok_or_else(|| {
            OSAgentError::ToolExecution(format!("MCP server '{}': no stdin pipe", server))
        })?;
        let stdout = child.stdout.take().ok_or_else(|| {
            OSAgentError::ToolExecution(format!("MCP server '{}': no stdout pipe", server))
        })?;

        let stdout_server = server.to_string();
        tokio::spawn(async move {
            let mut lines = BufReader::new(stdout).lines();
            loop {
                match lines.next_line().await {
                    Ok(Some(line)) => {
                        let trimmed = line.trim();
                        if trimmed.is_empty() {
                            continue;
                        }
                        match serde_json::from_str::<JsonRpcFrame>(trimmed) {
                            Ok(frame) => {
                                if frames.send(frame).is_err() {
                                    break;
                                }
                            }
                            // Servers sometimes print banners to stdout
                            // before speaking protocol. Log, don't die.
                            Err(error) => debug!(
                                "MCP server '{}': ignoring non-JSON stdout line ({}): {}",
                                stdout_server, error, trimmed
                            ),
                        }
                    }
                    Ok(None) => break,
                    Err(error) => {
                        warn!("MCP server '{}': stdout read failed: {}", stdout_server, error);
                        break;
                    }
                }
            }
        });

        if let Some(stderr) = child.stderr.take() {
            let stderr_server = server.to_string();
            tokio::spawn(async move {
                let mut lines = BufReader::new(stderr).lines();
                while let Ok(Some(line)) = lines.next_line().await {
                    if !line.trim().is_empty() {
                        debug!("MCP server '{}' stderr: {}", stderr_server, line);
                    }
                }
            });
        }

        Ok(Self {
            stdin: Mutex::new(stdin),
            child: Mutex::new(child),
        })
    }

    async fn send(&self, payload: serde_json::Value) -> Result<()> {
        let mut line = serde_json::to_string(&payload)
            .map_err(|error| OSAgentError::Parse(error.to_string()))?;
        line.push('\n');

        let mut stdin = self.stdin.lock().await;
        stdin
            .write_all(line.as_bytes())
            .await
            .map_err(|error| OSAgentError::ToolExecution(format!("MCP write failed: {}", error)))?;
        stdin
            .flush()
            .await
            .map_err(|error| OSAgentError::ToolExecution(format!("MCP flush failed: {}", error)))?;
        Ok(())
    }

    async fn shutdown(&self) {
        let mut child = self.child.lock().await;
        let _ = child.start_kill();
    }
}

/// Streamable-HTTP transport: every request is a POST, and the reply is
/// either a single JSON body or an SSE stream of frames.
pub struct HttpTransport {
    client: reqwest::Client,
    url: String,
    headers: HashMap<String, String>,
    session_id: Arc<Mutex<Option<String>>>,
    frames: mpsc::UnboundedSender<JsonRpcFrame>,
}

impl HttpTransport {
    pub fn new(
        url: String,
        headers: HashMap<String, String>,
        timeout_seconds: u64,
        frames: mpsc::UnboundedSender<JsonRpcFrame>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(std::time::Duration::from_secs(timeout_seconds.max(1)))
            .build()?;
        Ok(Self {
            client,
            url,
            headers,
            session_id: Arc::new(Mutex::new(None)),
            frames,
        })
    }

    async fn send(&self, payload: serde_json::Value) -> Result<()> {
        let mut request = self
            .client
            .post(&self.url)
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", crate::mcp::protocol::PROTOCOL_VERSION);

        for (key, value) in &self.headers {
            request = request.header(key.as_str(), value.as_str());
        }
        if let Some(session) = self.session_id.lock().await.clone() {
            request = request.header("mcp-session-id", session);
        }

        let response = request.json(&payload).send().await?;

        // The server assigns a session on the initialize response and
        // expects it echoed on every subsequent request.
        if let Some(session) = response
            .headers()
            .get("mcp-session-id")
            .and_then(|value| value.to_str().ok())
        {
            *self.session_id.lock().await = Some(session.to_string());
        }

        let status = response.status();
        let content_type = response
            .headers()
            .get("content-type")
            .and_then(|value| value.to_str().ok())
            .unwrap_or("")
            .to_string();

        // 202 with no body is the correct answer to a notification.
        if status == reqwest::StatusCode::ACCEPTED {
            return Ok(());
        }

        let body = response.text().await?;
        if !status.is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "MCP HTTP request failed ({}): {}",
                status,
                body.chars().take(500).collect::<String>()
            )));
        }

        if content_type.contains("text/event-stream") {
            for frame in parse_sse_frames(&body) {
                let _ = self.frames.send(frame);
            }
        } else if !body.trim().is_empty() {
            match serde_json::from_str::<JsonRpcFrame>(&body) {
                Ok(frame) => {
                    let _ = self.frames.send(frame);
                }
                Err(error) => {
                    return Err(OSAgentError::Parse(format!(
                        "MCP HTTP response was not JSON-RPC ({}): {}",
                        error,
                        body.chars().take(300).collect::<String>()
                    )))
                }
            }
        }

        Ok(())
    }
}

/// Pull JSON-RPC frames out of an SSE body, ignoring comments, event
/// names, and any `data:` payload that isn't JSON-RPC.
fn parse_sse_frames(body: &str) -> Vec<JsonRpcFrame> {
    let mut frames = Vec::new();
    let mut data = String::new();

    for line in body.lines() {
        if let Some(rest) = line.strip_prefix("data:") {
            if !data.is_empty() {
                data.push('\n');
            }
            data.push_str(rest.trim_start());
        } else if line.trim().is_empty() && !data.is_empty() {
            if let Ok(frame) = serde_json::from_str::<JsonRpcFrame>(&data) {
                frames.push(frame);
            }
            data.clear();
        }
    }

    if !data.is_empty() {
        if let Ok(frame) = serde_json::from_str::<JsonRpcFrame>(&data) {
            frames.push(frame);
        }
    }

    frames
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_sse_stream_into_frames() {
        let body = ": ping\nevent: message\ndata: {\"id\":1,\"result\":{\"ok\":true}}\n\nevent: message\ndata: {\"id\":2,\"result\":{}}\n\n";
        let frames = parse_sse_frames(body);
        assert_eq!(frames.len(), 2);
        assert_eq!(frames[0].response_id(), Some(1));
        assert_eq!(frames[1].response_id(), Some(2));
    }

    #[test]
    fn parses_trailing_frame_without_blank_line() {
        let frames = parse_sse_frames("data: {\"id\":9,\"result\":{}}");
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].response_id(), Some(9));
    }

    #[test]
    fn ignores_non_jsonrpc_data_payloads() {
        assert!(parse_sse_frames("data: not json\n\n").is_empty());
    }
}
