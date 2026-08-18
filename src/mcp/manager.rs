//! The deferred tool catalog.
//!
//! Every MCP tool is indexed here but none are handed to the model up
//! front. The model sees a one-line-per-server manifest, searches the
//! catalog with `tool_search`, and only then do full JSON schemas enter
//! the request. This keeps a 200-tool MCP setup at roughly the context
//! cost of a paragraph.

use crate::agent::provider::{ToolDefinition, ToolFunction};
use crate::config::{McpConfig, McpServerConfig};
use crate::error::{OSAgentError, Result};
use crate::mcp::client::McpClient;
use crate::mcp::search::{rank, SearchDocument};
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, RwLock};
use tracing::{info, warn};

/// Prefix that marks a tool as belonging to an MCP server. The registry
/// routes on it, so it must not collide with any native tool name.
pub const MCP_TOOL_PREFIX: &str = "mcp__";

/// Upper bound on a single tool description entering the request. Some
/// servers ship multi-KB prose per tool; past this it's noise.
const MAX_DESCRIPTION_CHARS: usize = 1500;

/// Cap on how many tools a single server may contribute, so one
/// misconfigured server can't dominate search results.
const MAX_TOOLS_PER_SERVER: usize = 400;

#[derive(Debug, Clone)]
pub struct CatalogEntry {
    pub qualified_name: String,
    pub server: String,
    pub tool: String,
    pub title: Option<String>,
    pub description: String,
    pub input_schema: Value,
    pub read_only: bool,
    pub destructive: bool,
}

impl CatalogEntry {
    pub fn to_definition(&self) -> ToolDefinition {
        let mut description = self.description.clone();
        if description.chars().count() > MAX_DESCRIPTION_CHARS {
            description = description.chars().take(MAX_DESCRIPTION_CHARS).collect();
            description.push_str("… (truncated)");
        }
        if self.read_only {
            description.push_str(" [read-only]");
        } else if self.destructive {
            description.push_str(" [destructive: may delete or overwrite data]");
        }

        ToolDefinition {
            tool_type: "function".to_string(),
            function: ToolFunction {
                name: self.qualified_name.clone(),
                description: format!("({} MCP) {}", self.server, description.trim()),
                parameters: normalize_schema(&self.input_schema),
            },
        }
    }

    /// Compact form for search results — schema included, since the
    /// point of searching is to learn how to call the tool.
    pub fn to_search_result(&self) -> Value {
        json!({
            "name": self.qualified_name,
            "server": self.server,
            "description": self.description,
            "read_only": self.read_only,
            "input_schema": normalize_schema(&self.input_schema),
        })
    }
}

/// Providers reject tool schemas that aren't `type: object`, but MCP
/// servers sometimes omit the wrapper or send `null`.
fn normalize_schema(schema: &Value) -> Value {
    match schema {
        Value::Object(map) if map.contains_key("type") => schema.clone(),
        Value::Object(map) if map.contains_key("properties") => {
            let mut object = map.clone();
            object.insert("type".to_string(), json!("object"));
            Value::Object(object)
        }
        _ => json!({ "type": "object", "properties": {} }),
    }
}

#[derive(Debug, Clone)]
pub struct ServerSummary {
    pub name: String,
    pub blurb: String,
    pub tool_count: usize,
    pub connected: bool,
    pub error: Option<String>,
}

#[derive(Default)]
struct CatalogState {
    entries: Vec<CatalogEntry>,
    documents: Vec<SearchDocument>,
    by_name: HashMap<String, usize>,
    servers: Vec<ServerSummary>,
}

pub struct McpManager {
    clients: RwLock<HashMap<String, Arc<McpClient>>>,
    state: RwLock<CatalogState>,
    /// Tools preloaded from config `always_active`, available to every
    /// session from the first turn (skips the search round trip).
    preloaded: RwLock<Vec<String>>,
    /// session_id -> tools activated in that session, in activation order.
    /// Activation order is preserved: activated definitions are appended
    /// to the tool array so an activation invalidates only the tail of
    /// the provider's cached prompt prefix, never the native tool block.
    /// A fresh session starts with an empty set; activation never leaks
    /// across sessions. The empty string "" is the default/global bucket.
    activated: RwLock<HashMap<String, Vec<String>>>,
    max_activated: usize,
}

impl McpManager {
    /// Connect to every enabled server. A server that fails to start is
    /// recorded and skipped — one broken entry in the config must not
    /// take down the agent.
    pub async fn connect(config: &McpConfig) -> Arc<Self> {
        let manager = Arc::new(Self {
            clients: RwLock::new(HashMap::new()),
            state: RwLock::new(CatalogState::default()),
            preloaded: RwLock::new(Vec::new()),
            activated: RwLock::new(HashMap::new()),
            max_activated: config.max_activated_tools,
        });

        if !config.enabled {
            return manager;
        }

        let mut summaries: Vec<ServerSummary> = Vec::new();
        let mut entries: Vec<CatalogEntry> = Vec::new();
        let mut taken: HashSet<String> = HashSet::new();

        for server in config.servers.iter().filter(|server| server.enabled) {
            match Self::connect_one(server).await {
                Ok((client, specs)) => {
                    let mut server_entries = Vec::new();
                    for spec in specs.into_iter().take(MAX_TOOLS_PER_SERVER) {
                        let qualified = qualify(&server.name, &spec.name, &mut taken);
                        server_entries.push(CatalogEntry {
                            qualified_name: qualified,
                            server: server.name.clone(),
                            tool: spec.name.clone(),
                            title: spec.title.clone().or_else(|| {
                                spec.annotations.as_ref().and_then(|a| a.title.clone())
                            }),
                            description: spec.description.clone().unwrap_or_default(),
                            read_only: spec
                                .annotations
                                .as_ref()
                                .and_then(|a| a.read_only_hint)
                                .unwrap_or(false),
                            destructive: spec
                                .annotations
                                .as_ref()
                                .and_then(|a| a.destructive_hint)
                                .unwrap_or(false),
                            input_schema: spec.input_schema.clone().unwrap_or(Value::Null),
                        });
                    }

                    info!(
                        "MCP server '{}' connected with {} tools",
                        server.name,
                        server_entries.len()
                    );
                    summaries.push(ServerSummary {
                        name: server.name.clone(),
                        blurb: derive_blurb(
                            server,
                            client.instructions().as_deref(),
                            &server_entries,
                        ),
                        tool_count: server_entries.len(),
                        connected: true,
                        error: None,
                    });
                    entries.extend(server_entries);
                    manager
                        .clients
                        .write()
                        .unwrap()
                        .insert(server.name.clone(), client);
                }
                Err(error) => {
                    warn!("MCP server '{}' unavailable: {}", server.name, error);
                    summaries.push(ServerSummary {
                        name: server.name.clone(),
                        blurb: server.description.clone().unwrap_or_default(),
                        tool_count: 0,
                        connected: false,
                        error: Some(error.to_string()),
                    });
                }
            }
        }

        manager.install_catalog(entries, summaries);

        // Pre-activation: servers can name tools the agent should have
        // from turn one, skipping the search round trip for the paths a
        // user hits every session.
        let preload: Vec<String> = config
            .servers
            .iter()
            .filter(|server| server.enabled)
            .flat_map(|server| {
                server
                    .always_active
                    .iter()
                    .map(move |tool| qualified_name(&server.name, tool))
            })
            .collect();
        if !preload.is_empty() {
            manager.preload(&preload);
        }

        manager
    }

    async fn connect_one(
        server: &McpServerConfig,
    ) -> Result<(Arc<McpClient>, Vec<crate::mcp::protocol::McpToolSpec>)> {
        let client = Arc::new(McpClient::connect(server).await?);
        let tools = client.list_tools().await?;
        Ok((client, tools))
    }

    fn install_catalog(&self, entries: Vec<CatalogEntry>, servers: Vec<ServerSummary>) {
        let documents = entries
            .iter()
            .map(|entry| {
                SearchDocument::build(
                    &entry.server,
                    &entry.tool,
                    entry.title.as_deref(),
                    Some(&entry.description),
                )
            })
            .collect();
        let by_name = entries
            .iter()
            .enumerate()
            .map(|(index, entry)| (entry.qualified_name.clone(), index))
            .collect();

        let mut state = self.state.write().unwrap();
        state.entries = entries;
        state.documents = documents;
        state.by_name = by_name;
        state.servers = servers;
    }

    pub fn is_empty(&self) -> bool {
        self.state.read().unwrap().entries.is_empty()
    }

    pub fn catalog_size(&self) -> usize {
        self.state.read().unwrap().entries.len()
    }

    /// The whole catalog, for UI browsing. This never reaches the model
    /// — deferring these is the entire point.
    pub fn entries(&self) -> Vec<CatalogEntry> {
        self.state.read().unwrap().entries.clone()
    }

    pub fn server_summaries(&self) -> Vec<ServerSummary> {
        self.state.read().unwrap().servers.clone()
    }

    pub fn is_known(&self, qualified_name: &str) -> bool {
        self.state
            .read()
            .unwrap()
            .by_name
            .contains_key(qualified_name)
    }

    pub fn entry(&self, qualified_name: &str) -> Option<CatalogEntry> {
        let state = self.state.read().unwrap();
        state
            .by_name
            .get(qualified_name)
            .and_then(|index| state.entries.get(*index))
            .cloned()
    }

    /// Search the catalog. `select:a,b,c` bypasses ranking and fetches
    /// exact names, which is how the model re-fetches a schema it saw
    /// earlier without guessing at query wording.
    pub fn search(&self, query: &str, limit: usize) -> Vec<CatalogEntry> {
        let state = self.state.read().unwrap();

        if let Some(list) = query.trim().strip_prefix("select:") {
            return list
                .split(',')
                .map(|name| name.trim())
                .filter(|name| !name.is_empty())
                .filter_map(|name| {
                    let key = if name.starts_with(MCP_TOOL_PREFIX) {
                        name.to_string()
                    } else {
                        format!("{}{}", MCP_TOOL_PREFIX, name)
                    };
                    state
                        .by_name
                        .get(&key)
                        .or_else(|| state.by_name.get(name))
                        .and_then(|index| state.entries.get(*index))
                        .cloned()
                })
                .collect();
        }

        rank(&state.documents, query)
            .into_iter()
            .take(limit)
            .filter_map(|(index, _)| state.entries.get(index).cloned())
            .collect()
    }

    /// Preload tools for every session (config `always_active`). These are
    /// available from the first turn and are not session-scoped.
    pub fn preload(&self, names: &[String]) {
        let state = self.state.read().unwrap();
        let mut preloaded = self.preloaded.write().unwrap();
        for name in names {
            if state.by_name.contains_key(name) && !preloaded.contains(name) {
                preloaded.push(name.clone());
            }
        }
    }

    /// Make tools callable for the rest of a session. Returns the names
    /// that were newly activated for that session.
    pub fn activate(&self, session: &str, names: &[String]) -> Vec<String> {
        let state = self.state.read().unwrap();
        let mut activated = self.activated.write().unwrap();
        let bucket = activated.entry(session.to_string()).or_default();
        let mut added = Vec::new();

        for name in names {
            if !state.by_name.contains_key(name) || bucket.contains(name) {
                continue;
            }
            if bucket.len() >= self.max_activated {
                warn!(
                    "MCP activation cap ({}) reached; '{}' not activated",
                    self.max_activated, name
                );
                break;
            }
            bucket.push(name.clone());
            added.push(name.clone());
        }

        added
    }

    pub fn activated_names(&self, session: &str) -> Vec<String> {
        let mut names = self.preloaded.read().unwrap().clone();
        if let Some(bucket) = self.activated.read().unwrap().get(session) {
            names.extend(bucket.iter().cloned());
        }
        names
    }

    /// Every activated tool across all sessions, deduplicated. For UI
    /// browsing (Settings > MCP Servers), which is not session-scoped.
    pub fn all_activated_names(&self) -> Vec<String> {
        let mut names = self.preloaded.read().unwrap().clone();
        for bucket in self.activated.read().unwrap().values() {
            for name in bucket {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    pub fn is_activated(&self, session: &str, qualified_name: &str) -> bool {
        let preloaded = self.preloaded.read().unwrap();
        if preloaded.iter().any(|name| name == qualified_name) {
            return true;
        }
        self.activated
            .read()
            .unwrap()
            .get(session)
            .map(|bucket| bucket.iter().any(|name| name == qualified_name))
            .unwrap_or(false)
    }

    /// Definitions for preloaded and session-activated tools, in
    /// preload-then-activation order.
    pub fn activated_definitions(&self, session: &str) -> Vec<ToolDefinition> {
        let state = self.state.read().unwrap();
        let mut definitions = Vec::new();
        for name in self.preloaded.read().unwrap().iter() {
            if let Some(index) = state.by_name.get(name) {
                if let Some(entry) = state.entries.get(*index) {
                    definitions.push(entry.to_definition());
                }
            }
        }
        for name in self
            .activated
            .read()
            .unwrap()
            .get(session)
            .into_iter()
            .flatten()
        {
            if let Some(index) = state.by_name.get(name) {
                if let Some(entry) = state.entries.get(*index) {
                    definitions.push(entry.to_definition());
                }
            }
        }
        definitions
    }

    /// Drop all activation state for a session. Called when a session is
    /// deleted so its loaded tools do not linger in memory.
    pub fn prune_session(&self, session: &str) {
        self.activated.write().unwrap().remove(session);
    }

    /// The always-in-context table of contents. One line per server —
    /// without it the model cannot know a capability exists and so never
    /// thinks to search for it.
    pub fn manifest_prompt(&self) -> Option<String> {
        let state = self.state.read().unwrap();
        if state.servers.is_empty() {
            return None;
        }

        let mut lines = Vec::new();
        for server in &state.servers {
            if !server.connected {
                lines.push(format!(
                    "- {}: unavailable ({})",
                    server.name,
                    server
                        .error
                        .as_deref()
                        .unwrap_or("failed to connect")
                        .chars()
                        .take(120)
                        .collect::<String>()
                ));
                continue;
            }
            lines.push(format!(
                "- {} ({} tools): {}",
                server.name, server.tool_count, server.blurb
            ));
        }

        Some(format!(
            "# Connected MCP Servers\n\
             These servers' tools are NOT loaded yet. Call `tool_search` with a plain-language \
             query to load the ones you need, then call them normally. Use \
             `tool_search` with `select:<name>` to reload a specific schema.\n{}",
            lines.join("\n")
        ))
    }

    /// Invoke an MCP tool. Calling a catalogued-but-inactive tool works
    /// and activates it: the model sometimes remembers a name from an
    /// earlier search whose schema has since been dropped, and failing
    /// that call would be pure friction.
    pub async fn call(
        &self,
        session: &str,
        qualified_name: &str,
        arguments: Value,
    ) -> Result<(String, bool)> {
        let entry = self.entry(qualified_name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!(
                "Unknown MCP tool '{}'. Use tool_search to find available tools.",
                qualified_name
            ))
        })?;

        if !self.is_activated(session, qualified_name) {
            self.activate(session, &[qualified_name.to_string()]);
        }

        let client = self
            .clients
            .read()
            .unwrap()
            .get(&entry.server)
            .cloned()
            .ok_or_else(|| {
                OSAgentError::ToolExecution(format!(
                    "MCP server '{}' is not connected",
                    entry.server
                ))
            })?;

        client.call_tool(&entry.tool, arguments).await
    }

    pub async fn shutdown(&self) {
        let clients: Vec<Arc<McpClient>> = self.clients.read().unwrap().values().cloned().collect();
        for client in clients {
            client.shutdown().await;
        }
    }
}

/// A late-bound slot for the manager.
///
/// The tool registry is built synchronously at startup but MCP servers
/// connect asynchronously and may reconnect when the user edits them in
/// the UI. Tools hold a handle rather than the manager itself so they
/// see reconnections without the registry being rebuilt.
#[derive(Clone, Default)]
pub struct McpHandle {
    inner: Arc<RwLock<Option<Arc<McpManager>>>>,
}

impl McpHandle {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn get(&self) -> Option<Arc<McpManager>> {
        self.inner.read().unwrap().clone()
    }

    /// Swap in a new manager, returning the previous one so the caller
    /// can shut its child processes down.
    pub fn set(&self, manager: Option<Arc<McpManager>>) -> Option<Arc<McpManager>> {
        std::mem::replace(&mut *self.inner.write().unwrap(), manager)
    }
}

/// `mcp__server__tool`, sanitized to the character set providers accept
/// for tool names and de-duplicated against names already taken.
fn qualified_name(server: &str, tool: &str) -> String {
    format!(
        "{}{}__{}",
        MCP_TOOL_PREFIX,
        sanitize(server),
        sanitize(tool)
    )
}

fn qualify(server: &str, tool: &str, taken: &mut HashSet<String>) -> String {
    let mut name = qualified_name(server, tool);

    // Provider tool-name limit is 64 characters; keep the tail, which is
    // the distinctive part.
    if name.len() > 64 {
        let overflow = name.len() - 64;
        let sanitized_tool = sanitize(tool);
        let keep = sanitized_tool.len().saturating_sub(overflow);
        name = format!(
            "{}{}__{}",
            MCP_TOOL_PREFIX,
            sanitize(server),
            &sanitized_tool[sanitized_tool.len() - keep.max(1)..]
        );
        name.truncate(64);
    }

    if taken.contains(&name) {
        for suffix in 2..1000 {
            let candidate = format!("{}_{}", name.trim_end_matches('_'), suffix);
            if !taken.contains(&candidate) {
                name = candidate;
                break;
            }
        }
    }

    taken.insert(name.clone());
    name
}

fn sanitize(text: &str) -> String {
    let cleaned: String = text
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || character == '_' || character == '-' {
                character
            } else {
                '_'
            }
        })
        .collect();
    if cleaned.is_empty() {
        "unnamed".to_string()
    } else {
        cleaned
    }
}

/// A server's one-line capability summary.
///
/// Config wins, then the server's own `instructions`, then a synthesized
/// line built from the most common tokens across its tool names. The
/// fallback matters: a server with no self-description would otherwise
/// be invisible in the manifest and never get searched.
fn derive_blurb(
    server: &McpServerConfig,
    instructions: Option<&str>,
    entries: &[CatalogEntry],
) -> String {
    if let Some(description) = server.description.as_ref().filter(|d| !d.trim().is_empty()) {
        return one_line(description, 160);
    }

    if let Some(instructions) = instructions.filter(|text| !text.trim().is_empty()) {
        return one_line(instructions, 160);
    }

    let mut counts: HashMap<String, usize> = HashMap::new();
    for entry in entries {
        for token in crate::mcp::search::tokenize(&entry.tool) {
            // Verbs are shared by every server; nouns identify it.
            if matches!(
                token.as_str(),
                "get" | "list" | "create" | "update" | "delete" | "set" | "add" | "search" | "read"
            ) {
                continue;
            }
            *counts.entry(token).or_insert(0) += 1;
        }
    }

    let mut ranked: Vec<(String, usize)> = counts.into_iter().collect();
    ranked.sort_by(|left, right| right.1.cmp(&left.1).then(left.0.cmp(&right.0)));
    let topics: Vec<String> = ranked.into_iter().take(6).map(|(token, _)| token).collect();

    if topics.is_empty() {
        format!("{} tools", server.name)
    } else {
        topics.join(", ")
    }
}

fn one_line(text: &str, max_chars: usize) -> String {
    let normalized = text.split_whitespace().collect::<Vec<_>>().join(" ");
    if normalized.chars().count() <= max_chars {
        return normalized;
    }
    let mut truncated: String = normalized.chars().take(max_chars).collect();
    truncated.push('…');
    truncated
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(server: &str, tool: &str, description: &str) -> CatalogEntry {
        CatalogEntry {
            qualified_name: qualified_name(server, tool),
            server: server.to_string(),
            tool: tool.to_string(),
            title: None,
            description: description.to_string(),
            input_schema: json!({"type": "object", "properties": {"id": {"type": "string"}}}),
            read_only: false,
            destructive: false,
        }
    }

    fn manager_with(entries: Vec<CatalogEntry>) -> McpManager {
        let manager = McpManager {
            clients: RwLock::new(HashMap::new()),
            state: RwLock::new(CatalogState::default()),
            preloaded: RwLock::new(Vec::new()),
            activated: RwLock::new(HashMap::new()),
            max_activated: 64,
        };
        let servers = vec![ServerSummary {
            name: "linear".to_string(),
            blurb: "issues".to_string(),
            tool_count: entries.len(),
            connected: true,
            error: None,
        }];
        manager.install_catalog(entries, servers);
        manager
    }

    #[test]
    fn qualified_names_are_provider_safe() {
        let mut taken = HashSet::new();
        let name = qualify("my server!", "do/thing", &mut taken);
        assert_eq!(name, "mcp__my_server___do_thing");
        assert!(name.len() <= 64);
        assert!(name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-'));
    }

    #[test]
    fn colliding_names_get_distinct_suffixes() {
        let mut taken = HashSet::new();
        let first = qualify("srv", "tool", &mut taken);
        let second = qualify("srv", "tool", &mut taken);
        assert_ne!(first, second);
    }

    #[test]
    fn long_names_are_truncated_to_the_provider_limit() {
        let mut taken = HashSet::new();
        let name = qualify("server", &"x".repeat(120), &mut taken);
        assert!(name.len() <= 64, "got {} chars", name.len());
    }

    #[test]
    fn nothing_is_activated_until_asked() {
        let manager = manager_with(vec![entry("linear", "create_issue", "Create an issue")]);
        assert!(manager.activated_definitions("s1").is_empty());
        assert_eq!(manager.catalog_size(), 1);
    }

    #[test]
    fn search_activates_nothing_by_itself() {
        let manager = manager_with(vec![entry("linear", "create_issue", "Create an issue")]);
        assert_eq!(manager.search("create issue", 5).len(), 1);
        assert!(manager.activated_names("s1").is_empty());
    }

    #[test]
    fn activation_is_append_only_idempotent_and_session_scoped() {
        let manager = manager_with(vec![
            entry("linear", "create_issue", "Create an issue"),
            entry("linear", "list_issues", "List issues"),
        ]);
        let first = qualified_name("linear", "create_issue");
        let second = qualified_name("linear", "list_issues");

        manager.activate("s1", std::slice::from_ref(&first));
        manager.activate("s1", &[first.clone(), second.clone()]);

        assert_eq!(
            manager.activated_names("s1"),
            vec![first.clone(), second.clone()]
        );

        // A different session starts clean: activation never leaks.
        assert!(manager.activated_names("s2").is_empty());
        assert!(!manager.is_activated("s2", &first));
        manager.activate("s2", &[second.clone()]);
        assert_eq!(manager.activated_names("s2"), vec![second.clone()]);
        assert_eq!(manager.activated_definitions("s1").len(), 2);
        assert_eq!(manager.activated_definitions("s2").len(), 1);

        // Pruning removes only the targeted session.
        manager.prune_session("s1");
        assert!(manager.activated_names("s1").is_empty());
        assert_eq!(manager.activated_definitions("s2").len(), 1);
    }

    #[test]
    fn select_syntax_fetches_exact_names_with_or_without_prefix() {
        let manager = manager_with(vec![entry("linear", "create_issue", "Create an issue")]);
        assert_eq!(manager.search("select:linear__create_issue", 5).len(), 1);
        assert_eq!(
            manager.search("select:mcp__linear__create_issue", 5).len(),
            1
        );
    }

    #[test]
    fn preloaded_tools_are_available_to_every_session() {
        let manager = manager_with(vec![entry("linear", "create_issue", "Create an issue")]);
        let name = qualified_name("linear", "create_issue");
        manager.preload(&[name.clone()]);

        assert!(manager.is_activated("s1", &name));
        assert!(manager.is_activated("s2", &name));
        assert_eq!(manager.activated_definitions("s1").len(), 1);
        assert_eq!(manager.activated_definitions("s2").len(), 1);
    }

    #[test]
    fn activation_respects_the_cap() {
        let manager = McpManager {
            clients: RwLock::new(HashMap::new()),
            state: RwLock::new(CatalogState::default()),
            preloaded: RwLock::new(Vec::new()),
            activated: RwLock::new(HashMap::new()),
            max_activated: 1,
        };
        manager.install_catalog(
            vec![entry("linear", "a", "a"), entry("linear", "b", "b")],
            Vec::new(),
        );
        manager.activate(
            "s1",
            &[qualified_name("linear", "a"), qualified_name("linear", "b")],
        );
        assert_eq!(manager.activated_names("s1").len(), 1);
    }

    #[test]
    fn schemas_without_a_type_are_normalized() {
        let normalized = normalize_schema(&json!({"properties": {"id": {"type": "string"}}}));
        assert_eq!(normalized["type"], "object");
        let normalized = normalize_schema(&Value::Null);
        assert_eq!(normalized["type"], "object");
    }

    #[test]
    fn blurb_falls_back_to_tool_name_topics() {
        let server = McpServerConfig {
            name: "gh".to_string(),
            ..Default::default()
        };
        let entries = vec![
            entry("gh", "list_pull_requests", ""),
            entry("gh", "create_pull_request", ""),
            entry("gh", "get_pull_request", ""),
        ];
        let blurb = derive_blurb(&server, None, &entries);
        assert!(blurb.contains("pull"), "got: {}", blurb);
    }
}
