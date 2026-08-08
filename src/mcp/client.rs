//! One connected MCP server: request/response correlation, handshake,
//! tool listing, and tool invocation.

use crate::config::McpServerConfig;
use crate::error::{OSAgentError, Result};
use crate::mcp::protocol::{
    flatten_tool_content, initialize_params, is_tool_error, InitializeResult, JsonRpcFrame,
    JsonRpcNotification, JsonRpcRequest, McpToolSpec, ToolsListResult,
};
use crate::mcp::transport::{HttpTransport, StdioTransport, Transport};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use tokio::sync::{mpsc, oneshot};
use tokio::time::{timeout, Duration};
use tracing::{debug, warn};

/// Cap on `tools/list` pagination, so a server that returns a cursor
/// forever can't spin us.
const MAX_TOOL_PAGES: usize = 20;

pub struct McpClient {
    name: String,
    transport: Transport,
    next_id: AtomicI64,
    pending: Arc<StdMutex<HashMap<i64, oneshot::Sender<StdResult>>>>,
    request_timeout: Duration,
    healthy: AtomicBool,
    instructions: StdMutex<Option<String>>,
    server_version: StdMutex<Option<String>>,
}

type StdResult = std::result::Result<Value, String>;

impl McpClient {
    /// Connect, handshake, and leave the client ready for `list_tools`.
    pub async fn connect(config: &McpServerConfig) -> Result<Self> {
        let (frame_tx, frame_rx) = mpsc::unbounded_channel::<JsonRpcFrame>();

        let transport = match config.transport_kind() {
            crate::config::McpTransport::Stdio => {
                let command = config.command.as_deref().ok_or_else(|| {
                    OSAgentError::Config(format!(
                        "MCP server '{}': stdio transport requires a command",
                        config.name
                    ))
                })?;
                Transport::Stdio(Box::new(
                    StdioTransport::spawn(
                        &config.name,
                        command,
                        &config.args,
                        &config.env,
                        config.cwd.as_deref(),
                        frame_tx,
                    )
                    .await?,
                ))
            }
            crate::config::McpTransport::Http => {
                let url = config.url.as_deref().ok_or_else(|| {
                    OSAgentError::Config(format!(
                        "MCP server '{}': http transport requires a url",
                        config.name
                    ))
                })?;
                Transport::Http(HttpTransport::new(
                    url.to_string(),
                    config.headers.clone(),
                    config.timeout_seconds,
                    frame_tx,
                )?)
            }
        };

        let pending: Arc<StdMutex<HashMap<i64, oneshot::Sender<StdResult>>>> =
            Arc::new(StdMutex::new(HashMap::new()));

        Self::spawn_dispatcher(config.name.clone(), frame_rx, pending.clone());

        let client = Self {
            name: config.name.clone(),
            transport,
            next_id: AtomicI64::new(1),
            pending,
            request_timeout: Duration::from_secs(config.timeout_seconds.max(1)),
            healthy: AtomicBool::new(true),
            instructions: StdMutex::new(None),
            server_version: StdMutex::new(None),
        };

        client.handshake().await?;
        Ok(client)
    }

    /// Route incoming frames to whoever is awaiting that request id.
    ///
    /// Server-initiated requests are answered with "method not found"
    /// rather than ignored — a server that blocks on a roots/sampling
    /// request would otherwise hang forever.
    fn spawn_dispatcher(
        server: String,
        mut frames: mpsc::UnboundedReceiver<JsonRpcFrame>,
        pending: Arc<StdMutex<HashMap<i64, oneshot::Sender<StdResult>>>>,
    ) {
        tokio::spawn(async move {
            while let Some(frame) = frames.recv().await {
                if !frame.is_response() {
                    if let Some(method) = frame.method.as_deref() {
                        debug!("MCP server '{}': unhandled inbound {}", server, method);
                    }
                    continue;
                }

                let Some(id) = frame.response_id() else {
                    continue;
                };
                let sender = pending.lock().unwrap().remove(&id);
                let Some(sender) = sender else {
                    debug!("MCP server '{}': response for unknown id {}", server, id);
                    continue;
                };

                let payload = match (frame.result, frame.error) {
                    (_, Some(error)) => Err(format!("{} (code {})", error.message, error.code)),
                    (Some(result), None) => Ok(result),
                    (None, None) => Ok(Value::Null),
                };
                let _ = sender.send(payload);
            }

            // Transport closed: fail every in-flight request instead of
            // leaving callers to hit their individual timeouts.
            let waiters: Vec<_> = pending.lock().unwrap().drain().map(|(_, tx)| tx).collect();
            for waiter in waiters {
                let _ = waiter.send(Err("MCP server connection closed".to_string()));
            }
        });
    }

    async fn handshake(&self) -> Result<()> {
        let result = self
            .request(
                "initialize",
                Some(initialize_params("osagent", env!("CARGO_PKG_VERSION"))),
            )
            .await?;

        if let Ok(parsed) = serde_json::from_value::<InitializeResult>(result) {
            if let Some(instructions) = parsed.instructions {
                *self.instructions.lock().unwrap() = Some(instructions);
            }
            if let Some(info) = parsed.server_info {
                *self.server_version.lock().unwrap() = info.version;
            }
        }

        self.notify("notifications/initialized", None).await?;
        Ok(())
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn instructions(&self) -> Option<String> {
        self.instructions.lock().unwrap().clone()
    }

    pub fn is_healthy(&self) -> bool {
        self.healthy.load(Ordering::Relaxed)
    }

    /// Every tool the server advertises, following pagination cursors.
    pub async fn list_tools(&self) -> Result<Vec<McpToolSpec>> {
        let mut tools = Vec::new();
        let mut cursor: Option<String> = None;

        for _ in 0..MAX_TOOL_PAGES {
            let params = cursor
                .as_ref()
                .map(|cursor| json!({ "cursor": cursor }))
                .or(Some(json!({})));
            let result = self.request("tools/list", params).await?;
            let page: ToolsListResult = serde_json::from_value(result).map_err(|error| {
                OSAgentError::Parse(format!(
                    "MCP server '{}': malformed tools/list response: {}",
                    self.name, error
                ))
            })?;

            tools.extend(page.tools);
            match page.next_cursor {
                Some(next) if !next.is_empty() => cursor = Some(next),
                _ => return Ok(tools),
            }
        }

        warn!(
            "MCP server '{}': stopped paginating tools/list after {} pages",
            self.name, MAX_TOOL_PAGES
        );
        Ok(tools)
    }

    /// Invoke a tool. Returns flattened text plus whether the server
    /// marked the result as an error.
    pub async fn call_tool(&self, tool: &str, arguments: Value) -> Result<(String, bool)> {
        let arguments = if arguments.is_null() {
            json!({})
        } else {
            arguments
        };
        let result = self
            .request(
                "tools/call",
                Some(json!({ "name": tool, "arguments": arguments })),
            )
            .await?;

        Ok((flatten_tool_content(&result), is_tool_error(&result)))
    }

    async fn request(&self, method: &str, params: Option<Value>) -> Result<Value> {
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (tx, rx) = oneshot::channel();
        self.pending.lock().unwrap().insert(id, tx);

        let payload = serde_json::to_value(JsonRpcRequest::new(id, method, params))
            .map_err(|error| OSAgentError::Parse(error.to_string()))?;

        if let Err(error) = self.transport.send(payload).await {
            self.pending.lock().unwrap().remove(&id);
            self.healthy.store(false, Ordering::Relaxed);
            return Err(error);
        }

        match timeout(self.request_timeout, rx).await {
            Ok(Ok(Ok(value))) => Ok(value),
            Ok(Ok(Err(message))) => Err(OSAgentError::ToolExecution(format!(
                "MCP server '{}' returned an error for {}: {}",
                self.name, method, message
            ))),
            Ok(Err(_)) => {
                self.healthy.store(false, Ordering::Relaxed);
                Err(OSAgentError::ToolExecution(format!(
                    "MCP server '{}' dropped the response to {}",
                    self.name, method
                )))
            }
            Err(_) => {
                self.pending.lock().unwrap().remove(&id);
                Err(OSAgentError::ToolExecution(format!(
                    "MCP server '{}' timed out after {}s on {}",
                    self.name,
                    self.request_timeout.as_secs(),
                    method
                )))
            }
        }
    }

    async fn notify(&self, method: &str, params: Option<Value>) -> Result<()> {
        let payload = serde_json::to_value(JsonRpcNotification::new(method, params))
            .map_err(|error| OSAgentError::Parse(error.to_string()))?;
        self.transport.send(payload).await
    }

    pub async fn shutdown(&self) {
        self.transport.shutdown().await;
    }
}
