//! MCP server management facades
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    /// Configured servers joined with their live connection state, for
    /// the settings UI. Servers that failed to start appear here with
    /// their error rather than silently missing.
    pub async fn mcp_status(&self) -> serde_json::Value {
        let config = self.config.read().await.clone();
        let manager = self.tool_registry.mcp_manager();
        let summaries = manager
            .as_ref()
            .map(|manager| manager.server_summaries())
            .unwrap_or_default();

        let servers: Vec<serde_json::Value> = config
            .mcp
            .servers
            .iter()
            .map(|server| {
                let live = summaries.iter().find(|summary| summary.name == server.name);
                serde_json::json!({
                    "name": server.name,
                    "enabled": server.enabled,
                    "transport": match server.transport_kind() {
                        crate::config::McpTransport::Http => "http",
                        crate::config::McpTransport::Stdio => "stdio",
                    },
                    "command": server.command,
                    "args": server.args,
                    "env": server.env,
                    "cwd": server.cwd,
                    "url": server.url,
                    "headers_keys": server.headers.keys().collect::<Vec<_>>(),
                    "timeout_seconds": server.timeout_seconds,
                    "description": server.description,
                    "always_active": server.always_active,
                    "connected": live.map(|summary| summary.connected).unwrap_or(false),
                    "tool_count": live.map(|summary| summary.tool_count).unwrap_or(0),
                    "blurb": live.map(|summary| summary.blurb.clone()),
                    "error": live.and_then(|summary| summary.error.clone()),
                })
            })
            .collect();

        serde_json::json!({
            "enabled": config.mcp.enabled,
            "connecting": manager.is_none() && !config.mcp.servers.is_empty(),
            "catalog_size": manager.as_ref().map(|m| m.catalog_size()).unwrap_or(0),
            "activated": manager.as_ref().map(|m| m.all_activated_names()).unwrap_or_default(),
            "max_activated_tools": config.mcp.max_activated_tools,
            "servers": servers,
        })
    }

    /// The catalog, for browsing in the UI. Never enters the model's
    /// context — that is the entire point of the deferred design.
    pub async fn mcp_catalog(&self, server: Option<String>) -> Vec<serde_json::Value> {
        let Some(manager) = self.tool_registry.mcp_manager() else {
            return Vec::new();
        };
        let activated = manager.all_activated_names();
        manager
            .entries()
            .into_iter()
            .filter(|entry| {
                server
                    .as_ref()
                    .map(|filter| &entry.server == filter)
                    .unwrap_or(true)
            })
            .map(|entry| {
                serde_json::json!({
                    "name": entry.qualified_name,
                    "server": entry.server,
                    "tool": entry.tool,
                    "description": entry.description,
                    "read_only": entry.read_only,
                    "activated": activated.contains(&entry.qualified_name),
                })
            })
            .collect()
    }

    /// Add or replace a server by name, persist it, and reconnect.
    ///
    /// Validation runs before anything is written so a typo in the UI
    /// surfaces as a message rather than a dead server in the config.
    pub async fn upsert_mcp_server(&self, server: crate::config::McpServerConfig) -> Result<()> {
        server.validate()?;

        {
            let mut config = self.config.write().await;
            match config
                .mcp
                .servers
                .iter_mut()
                .find(|existing| existing.name == server.name)
            {
                Some(existing) => *existing = server,
                None => config.mcp.servers.push(server),
            }
            config.save_to_current_path()?;
        }

        self.reload_mcp_servers().await
    }

    pub async fn remove_mcp_server(&self, name: &str) -> Result<()> {
        {
            let mut config = self.config.write().await;
            let before = config.mcp.servers.len();
            config.mcp.servers.retain(|server| server.name != name);
            if config.mcp.servers.len() == before {
                return Err(OSAgentError::Config(format!(
                    "No MCP server named '{}'",
                    name
                )));
            }
            config.save_to_current_path()?;
        }

        self.reload_mcp_servers().await
    }

    pub async fn set_mcp_server_enabled(&self, name: &str, enabled: bool) -> Result<()> {
        {
            let mut config = self.config.write().await;
            let server = config
                .mcp
                .servers
                .iter_mut()
                .find(|server| server.name == name)
                .ok_or_else(|| OSAgentError::Config(format!("No MCP server named '{}'", name)))?;
            server.enabled = enabled;
            config.save_to_current_path()?;
        }

        self.reload_mcp_servers().await
    }

    /// Tear down the current MCP connections and rebuild from config.
    ///
    /// Synchronous by design: the UI needs the connection result to show
    /// whether the server the user just added actually works.
    pub async fn reload_mcp_servers(&self) -> Result<()> {
        let mcp_config = self.config.read().await.mcp.clone();
        let manager = crate::mcp::McpManager::connect(&mcp_config).await;
        if let Some(previous) = self.tool_registry.register_mcp(manager) {
            previous.shutdown().await;
        }
        Ok(())
    }

    /// Connect to a candidate server without saving it, so the UI can
    /// offer a "test connection" button before the user commits.
    pub async fn test_mcp_server(
        &self,
        server: crate::config::McpServerConfig,
    ) -> Result<serde_json::Value> {
        server.validate()?;

        let probe = crate::config::McpConfig {
            enabled: true,
            servers: vec![server.clone()],
            ..Default::default()
        };
        let manager = crate::mcp::McpManager::connect(&probe).await;
        let summary = manager
            .server_summaries()
            .into_iter()
            .next()
            .ok_or_else(|| OSAgentError::Config("Server did not report a status".to_string()))?;
        let sample: Vec<String> = manager
            .search(&server.name, 8)
            .into_iter()
            .map(|entry| entry.tool)
            .collect();
        manager.shutdown().await;

        Ok(serde_json::json!({
            "connected": summary.connected,
            "tool_count": summary.tool_count,
            "blurb": summary.blurb,
            "error": summary.error,
            "sample_tools": sample,
        }))
    }
}
