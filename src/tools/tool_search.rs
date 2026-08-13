//! `tool_search` — the entry point to the deferred MCP catalog.
//!
//! Searching returns full JSON schemas and activates the matches, so the
//! very next assistant turn can call them directly. That two-step is the
//! whole trade: one extra round trip in exchange for keeping hundreds of
//! tool schemas out of every request.

use crate::error::Result;
use crate::mcp::McpHandle;
use crate::tools::registry::{Tool, ToolExample, ToolOutcome, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};

pub struct ToolSearchTool {
    mcp: McpHandle,
    default_limit: usize,
}

impl ToolSearchTool {
    pub fn new(mcp: McpHandle, default_limit: usize) -> Self {
        Self {
            mcp,
            default_limit: default_limit.max(1),
        }
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Find and load tools from connected MCP servers. Their schemas are not in context until \
         you search for them; matches returned here become callable immediately by name."
    }

    fn when_to_use(&self) -> &str {
        "Use when the task needs a capability listed under 'Connected MCP Servers' that you do \
         not already have a loaded tool for, or to reload a schema with select:<tool_name>"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for built-in tools (files, bash, search, web) — those are always loaded"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Find tools for filing a bug".to_string(),
                input: json!({ "query": "create issue ticket" }),
            },
            ToolExample {
                description: "Reload a specific schema by name".to_string(),
                input: json!({ "query": "select:mcp__linear__create_issue" }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Plain-language description of the capability you need \
                                    (\"post a slack message\"), or \"select:name1,name2\" to \
                                    fetch exact tools by name."
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 25,
                    "description": "Maximum tools to return. Keep this small; every result \
                                    adds a full JSON schema to the conversation."
                },
                "server": {
                    "type": "string",
                    "description": "Restrict results to one MCP server by name."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_result(args).await.map(|result| result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let query = args
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if query.is_empty() {
            return Ok(ToolResult::new(
                "tool_search requires a non-empty 'query'. Describe the capability you need, \
                 e.g. \"send a slack message\".",
            ));
        }

        let Some(manager) = self.mcp.get() else {
            return Ok(ToolResult::new(
                "No MCP servers are connected, so there are no searchable tools. \
                 Every available tool is already loaded.",
            ));
        };

        if manager.is_empty() {
            return Ok(ToolResult::new(
                "No MCP servers are connected, so there are no searchable tools. \
                 Every available tool is already loaded.",
            ));
        }

        let limit = args
            .get("limit")
            .and_then(|value| value.as_u64())
            .map(|value| value.clamp(1, 25) as usize)
            .unwrap_or(self.default_limit);
        let server_filter = args
            .get("server")
            .and_then(|value| value.as_str())
            .map(|value| value.trim().to_lowercase())
            .filter(|value| !value.is_empty());

        // Over-fetch before filtering by server, or a server filter can
        // starve results that ranked just outside the limit.
        let fetch = if server_filter.is_some() {
            (limit * 6).min(120)
        } else {
            limit
        };

        let mut matches = manager.search(&query, fetch);
        if let Some(ref server) = server_filter {
            matches.retain(|entry| entry.server.to_lowercase() == *server);
        }
        matches.truncate(limit);

        if matches.is_empty() {
            let servers = manager
                .server_summaries()
                .into_iter()
                .filter(|summary| summary.connected)
                .map(|summary| format!("{} ({} tools)", summary.name, summary.tool_count))
                .collect::<Vec<_>>()
                .join(", ");
            return Ok(ToolResult::new(format!(
                "No tools matched \"{}\". Connected servers: {}. Try different wording, or a \
                 broader single keyword.",
                query,
                if servers.is_empty() {
                    "none".to_string()
                } else {
                    servers
                }
            )));
        }

        let names: Vec<String> = matches
            .iter()
            .map(|entry| entry.qualified_name.clone())
            .collect();
        let newly_activated = manager.activate(&names);

        let payload: Vec<Value> = matches
            .iter()
            .map(|entry| entry.to_search_result())
            .collect();
        let rendered = serde_json::to_string_pretty(&payload).unwrap_or_default();

        let activation_note = if newly_activated.is_empty() {
            "All matching tools were already loaded.".to_string()
        } else {
            format!(
                "Loaded {} tool(s); they are now callable by name.",
                newly_activated.len()
            )
        };

        Ok(ToolResult {
            output: format!(
                "{}\n\n{}\n\nCall them directly by their `name`, or drive several at once with \
                 `tool_script` to keep intermediate results out of the conversation.",
                activation_note, rendered
            ),
            outcome: ToolOutcome::Success,
            title: Some(format!("{} tool(s) for \"{}\"", matches.len(), query)),
            metadata: json!({
                "query": query,
                "matched": names,
                "activated": newly_activated,
            }),
            attachments: Vec::new(),
        })
    }
}
