//! `tool_search` — the entry point to the deferred tool catalogs.
//!
//! Searching returns full JSON schemas and activates the matches, so the
//! very next assistant turn can call them directly. That two-step is the
//! whole trade: one extra round trip in exchange for keeping schemas for
//! low-frequency built-ins (weather, calendar, memory management, ...)
//! and for every MCP server tool out of every request.
//!
//! The always-loaded native core (files, bash, search, web, edits) is not
//! searchable here — those tools are in context on every turn by design.

use crate::error::Result;
use crate::mcp::McpHandle;
use crate::tools::native_catalog::NativeToolCatalog;
use crate::tools::registry::{tool_prompt_description, Tool, ToolExample, ToolOutcome, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct ToolSearchTool {
    mcp: McpHandle,
    native: Arc<NativeToolCatalog>,
    default_limit: usize,
}

impl ToolSearchTool {
    pub fn new(mcp: McpHandle, native: Arc<NativeToolCatalog>, default_limit: usize) -> Self {
        Self {
            mcp,
            native,
            default_limit: default_limit.max(1),
        }
    }

    fn resolve_select(&self, session: &str, names: &[String]) -> (Vec<Value>, Vec<String>, usize) {
        let mut payload = Vec::new();
        let mut native_activate = Vec::new();
        let mut mcp_parts = Vec::new();

        for name in names {
            if self.native.contains(name) {
                native_activate.push(name.clone());
                if let Some(result) = self.native.to_search_result(name) {
                    payload.push(result);
                }
            } else {
                mcp_parts.push(name.clone());
            }
        }

        let mut newly_activated = self.native.activate(session, &native_activate);

        if !mcp_parts.is_empty() {
            if let Some(manager) = self.mcp.get() {
                let entries = manager.search(&format!("select:{}", mcp_parts.join(",")), 25);
                let qualified: Vec<String> = entries
                    .iter()
                    .map(|entry| entry.qualified_name.clone())
                    .collect();
                newly_activated.extend(manager.activate(session, &qualified));
                payload.extend(entries.iter().map(|entry| entry.to_search_result()));
            }
        }

        (payload, newly_activated, names.len())
    }

    fn finish(
        &self,
        payload: Vec<Value>,
        activation_note: String,
        count: usize,
    ) -> Result<ToolResult> {
        let rendered = serde_json::to_string_pretty(&payload).unwrap_or_default();
        Ok(ToolResult {
            output: format!(
                "{}\n\n{}\n\nCall them directly by their `name`, or drive several at once with \
                 `tool_script` to keep intermediate results out of the conversation.",
                activation_note, rendered
            ),
            outcome: ToolOutcome::Success,
            title: Some(format!("{} tool(s) loaded", count)),
            metadata: json!({
                "matched": payload.iter().filter_map(|v| v.get("name")).filter_map(|v| v.as_str()).collect::<Vec<_>>(),
            }),
            attachments: Vec::new(),
        })
    }
}

#[async_trait]
impl Tool for ToolSearchTool {
    fn name(&self) -> &str {
        "tool_search"
    }

    fn description(&self) -> &str {
        "Find and load tools that are not loaded by default. Core tools (files, edit, bash, search, web) are always loaded; this searches the deferred catalogs: low-frequency built-ins (weather, calendar, news, system status, memory, decisions, goals, ...) and connected MCP server tools. Their schemas are not in context until you search for them; matches returned here become callable immediately by name."
    }

    fn when_to_use(&self) -> &str {
        "Use when the task needs a capability listed in the prompt's tool manifest (deferred built-ins or MCP servers) that you do not already have loaded, or to reload a schema with select:<tool_name>"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for core tools (files, bash, search, web, edits) — those are always loaded"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Find tools for filing a bug".to_string(),
                input: json!({ "query": "create issue ticket" }),
            },
            ToolExample {
                description: "Reload a specific schema by name".to_string(),
                input: json!({ "query": "select:weather,record_memory" }),
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
                                    (\"post a slack message\", \"check the weather\"), or \
                                    \"select:name1,name2\" to fetch exact tools by name."
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
                    "description": "Restrict results to one MCP server by name (built-in tools are not affected)."
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.execute_result(args).await.map(|result| result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        // The runtime injects session_id into every tool call. Activation
        // is session-scoped: tools loaded here stay loaded only for this
        // session, never for new ones.
        let session = args
            .get("session_id")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .to_string();

        let query = args
            .get("query")
            .and_then(|value| value.as_str())
            .unwrap_or("")
            .trim()
            .to_string();

        if query.is_empty() {
            return Ok(ToolResult::new(
                "tool_search requires a non-empty 'query'. Describe the capability you need, \
                 e.g. \"check the weather\" or \"send a slack message\".",
            ));
        }

        let has_native = !self.native.is_empty();
        let has_mcp = self
            .mcp
            .get()
            .map(|manager| !manager.is_empty())
            .unwrap_or(false);
        if !has_native && !has_mcp {
            return Ok(ToolResult::new(
                "No deferred tools are available — every available tool is already loaded.",
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

        // `select:` resolves exact names across both catalogs.
        if let Some(list) = query.strip_prefix("select:") {
            let names: Vec<String> = list
                .split(',')
                .map(|name| name.trim().to_string())
                .filter(|name| !name.is_empty())
                .collect();
            if names.is_empty() {
                return Ok(ToolResult::new(
                    "select: requires at least one tool name, e.g. select:weather,record_memory",
                ));
            }
            let (payload, newly_activated, all_requested) = self.resolve_select(&session, &names);
            if payload.is_empty() {
                return Ok(ToolResult::new(
                    "No catalog entries matched the requested names. Available deferred \
                     built-ins are listed in the prompt manifest; MCP servers under \
                     'Connected MCP Servers'."
                        .to_string(),
                ));
            }
            let activation_note = if newly_activated.is_empty() {
                "All requested tools were already loaded.".to_string()
            } else {
                format!(
                    "Loaded {} tool(s): {}; they are now callable by name.",
                    newly_activated.len(),
                    newly_activated.join(", ")
                )
            };
            return self.finish(payload, activation_note, all_requested);
        }

        // A server filter is MCP-only, so it hands the whole budget to
        // the MCP catalog.
        let native_limit = if server_filter.is_some() { 0 } else { limit };
        let native_matches = if native_limit > 0 {
            self.native.search(&query, native_limit)
        } else {
            Vec::new()
        };

        let mcp_limit = limit.saturating_sub(native_matches.len()).max(1);
        let mcp_matches = if let Some(manager) = self.mcp.get() {
            if let Some(ref server) = server_filter {
                // Over-fetch before filtering by server, or a server filter can
                // starve results that ranked just outside the limit.
                let fetch = (limit * 6).min(120);
                let mut matches = manager.search(&query, fetch);
                matches.retain(|entry| entry.server.to_lowercase() == *server);
                matches.truncate(mcp_limit);
                matches
            } else {
                manager.search(&query, mcp_limit)
            }
        } else {
            Vec::new()
        };

        let native_names: Vec<String> = native_matches.iter().map(|m| m.name.clone()).collect();
        let mcp_names: Vec<String> = mcp_matches
            .iter()
            .map(|entry| entry.qualified_name.clone())
            .collect();

        let newly_activated = {
            let mut added = self.native.activate(&session, &native_names);
            if let Some(manager) = self.mcp.get() {
                added.extend(manager.activate(&session, &mcp_names));
            }
            added
        };

        if native_matches.is_empty() && mcp_matches.is_empty() {
            return Ok(ToolResult::new(format!(
                "No tools matched \"{}\". Try different wording, a broader single keyword, or \
                 select:<name> to load a tool you already know the name of.",
                query
            )));
        }

        let mut payload: Vec<Value> = Vec::with_capacity(native_matches.len() + mcp_matches.len());
        for m in native_matches {
            payload.push(json!({
                "name": m.name,
                "server": "builtin",
                "description": tool_prompt_description(&m.tool),
                "read_only": false,
                "input_schema": m.tool.parameters(),
            }));
        }
        for entry in mcp_matches {
            payload.push(entry.to_search_result());
        }

        let activation_note = if newly_activated.is_empty() {
            "All matching tools were already loaded.".to_string()
        } else {
            format!(
                "Loaded {} tool(s); they are now callable by name.",
                newly_activated.len()
            )
        };

        let count = payload.len();
        self.finish(payload, activation_note, count)
    }
}
