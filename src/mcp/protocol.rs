//! Minimal MCP (Model Context Protocol) wire types.
//!
//! Only the surface OSA actually uses is modelled: initialization, tool
//! discovery, and tool invocation. Notifications and resource/prompt
//! primitives are accepted and ignored rather than rejected, so a server
//! that speaks more of the protocol than we do still works.

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// Protocol revision OSA advertises during `initialize`.
pub const PROTOCOL_VERSION: &str = "2025-06-18";

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcRequest {
    pub jsonrpc: &'static str,
    pub id: i64,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcRequest {
    pub fn new(id: i64, method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            id,
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct JsonRpcNotification {
    pub jsonrpc: &'static str,
    pub method: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub params: Option<Value>,
}

impl JsonRpcNotification {
    pub fn new(method: impl Into<String>, params: Option<Value>) -> Self {
        Self {
            jsonrpc: "2.0",
            method: method.into(),
            params,
        }
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcError {
    pub code: i64,
    pub message: String,
    #[serde(default)]
    pub data: Option<Value>,
}

/// A frame read off a transport. Servers interleave responses to our
/// requests with their own requests and notifications; only the first
/// carries an `id` we issued.
#[derive(Debug, Clone, Deserialize)]
pub struct JsonRpcFrame {
    #[serde(default)]
    pub id: Option<Value>,
    #[serde(default)]
    pub method: Option<String>,
    #[serde(default)]
    pub result: Option<Value>,
    #[serde(default)]
    pub error: Option<JsonRpcError>,
}

impl JsonRpcFrame {
    /// Numeric request id, when this frame answers something we sent.
    pub fn response_id(&self) -> Option<i64> {
        match self.id.as_ref()? {
            Value::Number(number) => number.as_i64(),
            Value::String(text) => text.parse().ok(),
            _ => None,
        }
    }

    pub fn is_response(&self) -> bool {
        self.method.is_none() && self.id.is_some()
    }
}

/// `initialize` params. Capabilities are deliberately sparse: OSA is a
/// pure client and exposes no roots, sampling, or elicitation.
pub fn initialize_params(client_name: &str, client_version: &str) -> Value {
    json!({
        "protocolVersion": PROTOCOL_VERSION,
        "capabilities": {},
        "clientInfo": {
            "name": client_name,
            "version": client_version,
        }
    })
}

/// One tool as advertised by a server's `tools/list`.
#[derive(Debug, Clone, Deserialize)]
pub struct McpToolSpec {
    pub name: String,
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default, rename = "inputSchema")]
    pub input_schema: Option<Value>,
    #[serde(default)]
    pub annotations: Option<McpToolAnnotations>,
}

#[derive(Debug, Clone, Default, Deserialize)]
pub struct McpToolAnnotations {
    #[serde(default)]
    pub title: Option<String>,
    #[serde(default, rename = "readOnlyHint")]
    pub read_only_hint: Option<bool>,
    #[serde(default, rename = "destructiveHint")]
    pub destructive_hint: Option<bool>,
    #[serde(default, rename = "idempotentHint")]
    pub idempotent_hint: Option<bool>,
    #[serde(default, rename = "openWorldHint")]
    pub open_world_hint: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ToolsListResult {
    #[serde(default)]
    pub tools: Vec<McpToolSpec>,
    #[serde(default, rename = "nextCursor")]
    pub next_cursor: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct InitializeResult {
    #[serde(default, rename = "protocolVersion")]
    pub protocol_version: Option<String>,
    #[serde(default, rename = "serverInfo")]
    pub server_info: Option<ServerInfo>,
    #[serde(default)]
    pub capabilities: Option<Value>,
    #[serde(default)]
    pub instructions: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ServerInfo {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub version: Option<String>,
}

/// Flatten a `tools/call` result into text.
///
/// MCP returns a content array of typed blocks; the model only ever sees
/// text, so non-text blocks are summarized rather than dropped silently.
pub fn flatten_tool_content(result: &Value) -> String {
    let Some(blocks) = result.get("content").and_then(|value| value.as_array()) else {
        // Some servers answer with `structuredContent` only.
        if let Some(structured) = result.get("structuredContent") {
            return serde_json::to_string_pretty(structured).unwrap_or_default();
        }
        return serde_json::to_string_pretty(result).unwrap_or_default();
    };

    let mut parts: Vec<String> = Vec::new();
    for block in blocks {
        match block.get("type").and_then(|value| value.as_str()) {
            Some("text") => {
                if let Some(text) = block.get("text").and_then(|value| value.as_str()) {
                    parts.push(text.to_string());
                }
            }
            Some("image") => {
                let mime = block
                    .get("mimeType")
                    .and_then(|value| value.as_str())
                    .unwrap_or("image");
                parts.push(format!("[image content: {}]", mime));
            }
            Some("audio") => parts.push("[audio content]".to_string()),
            Some("resource_link") => {
                let uri = block
                    .get("uri")
                    .and_then(|value| value.as_str())
                    .unwrap_or("");
                parts.push(format!("[resource: {}]", uri));
            }
            Some("resource") => {
                if let Some(resource) = block.get("resource") {
                    if let Some(text) = resource.get("text").and_then(|value| value.as_str()) {
                        parts.push(text.to_string());
                    } else {
                        let uri = resource
                            .get("uri")
                            .and_then(|value| value.as_str())
                            .unwrap_or("");
                        parts.push(format!("[embedded resource: {}]", uri));
                    }
                }
            }
            _ => parts.push(serde_json::to_string(block).unwrap_or_default()),
        }
    }

    if parts.is_empty() {
        return "(no content)".to_string();
    }
    parts.join("\n")
}

/// Whether the server flagged this result as an error for the model to
/// recover from (distinct from a protocol-level JSON-RPC error).
pub fn is_tool_error(result: &Value) -> bool {
    result
        .get("isError")
        .and_then(|value| value.as_bool())
        .unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn flattens_text_blocks() {
        let result = json!({"content": [{"type": "text", "text": "hello"}, {"type": "text", "text": "world"}]});
        assert_eq!(flatten_tool_content(&result), "hello\nworld");
    }

    #[test]
    fn falls_back_to_structured_content() {
        let result = json!({"structuredContent": {"count": 2}});
        assert!(flatten_tool_content(&result).contains("\"count\""));
    }

    #[test]
    fn parses_string_response_ids() {
        let frame: JsonRpcFrame = serde_json::from_value(json!({"id": "7", "result": {}})).unwrap();
        assert_eq!(frame.response_id(), Some(7));
        assert!(frame.is_response());
    }

    #[test]
    fn server_initiated_requests_are_not_responses() {
        let frame: JsonRpcFrame =
            serde_json::from_value(json!({"id": 1, "method": "roots/list"})).unwrap();
        assert!(!frame.is_response());
    }
}
