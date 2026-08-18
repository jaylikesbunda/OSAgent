//! Deferred catalog of low-frequency built-in tools.
//!
//! The same idea as the MCP catalog, applied to our own specialty tools:
//! weather, calendar, news, system status, memory/decision management,
//! goals, and the like stay out of every request and are loaded on demand
//! through `tool_search`. One extra round trip in exchange for keeping a
//! couple dozen schemas out of the prompt, and for keeping the model's
//! tool-choice space small enough to pick well.
//!
//! Activation appends schemas after the always-loaded native block, so a
//! discovery invalidates only the tail of the provider's cached prompt
//! prefix — exactly the contract the MCP catalog already maintains.

use crate::agent::provider::{ToolDefinition, ToolFunction};
use crate::mcp::search::{rank, SearchDocument};
use crate::tools::registry::{tool_prompt_description, Tool};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;
use std::sync::RwLock;
use tracing::warn;

/// Ceiling on native tools activated into one session. High enough to
/// never be the binding constraint in practice, low enough to bound
/// worst-case prompt growth if the model goes on a search spree.
const MAX_ACTIVATED: usize = 24;

/// One match from the catalog, carrying everything `tool_search` needs to
/// render the result and the registry needs to make it callable.
pub struct NativeMatch {
    pub name: String,
    pub tool: Arc<dyn Tool>,
}

struct NativeEntry {
    tool: Arc<dyn Tool>,
}

#[derive(Default)]
struct CatalogState {
    entries: Vec<NativeEntry>,
    documents: Vec<SearchDocument>,
    by_name: HashMap<String, usize>,
}

pub struct NativeToolCatalog {
    state: RwLock<CatalogState>,
    /// session_id -> tools activated in that session, in activation order.
    /// A fresh session starts with an empty set; activation never leaks
    /// across sessions. The empty string "" is the default/global bucket
    /// used by startup prompt building and callers without a session.
    activated: RwLock<HashMap<String, Vec<String>>>,
}

impl NativeToolCatalog {
    pub fn new() -> Self {
        Self {
            state: RwLock::new(CatalogState::default()),
            activated: RwLock::new(HashMap::new()),
        }
    }

    /// Index a deferred tool. The search text is the full prompt
    /// description (description + when-to-use + when-not-to-use +
    /// examples), so queries phrased the way the model would phrase them
    /// find the tool. Names are distinctive ("weather", "codesearch"),
    /// and name matches dominate the ranking, so collisions with the
    /// always-loaded core are not a concern.
    pub fn register(&self, tool: Arc<dyn Tool>) {
        let name = tool.name().to_string();
        let document = SearchDocument::build(
            "builtin",
            &name,
            None,
            Some(&tool_prompt_description(&tool)),
        );
        let mut state = self.state.write().unwrap();
        if state.by_name.contains_key(&name) {
            return;
        }
        let index = state.entries.len();
        state.documents.push(document);
        state.entries.push(NativeEntry { tool });
        state.by_name.insert(name, index);
    }

    pub fn is_empty(&self) -> bool {
        self.state.read().unwrap().entries.is_empty()
    }

    pub fn catalog_size(&self) -> usize {
        self.state.read().unwrap().entries.len()
    }

    pub fn contains(&self, name: &str) -> bool {
        self.state.read().unwrap().by_name.contains_key(name)
    }

    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        let state = self.state.read().unwrap();
        state
            .by_name
            .get(name)
            .and_then(|index| state.entries.get(*index))
            .map(|entry| entry.tool.clone())
    }

    /// Rank the deferred built-ins against `query`. The `select:` prefix
    /// is handled by `tool_search`, which owns the resolution across both
    /// catalogs; here we only rank.
    pub fn search(&self, query: &str, limit: usize) -> Vec<NativeMatch> {
        let state = self.state.read().unwrap();
        rank(&state.documents, query)
            .into_iter()
            .take(limit)
            .filter_map(|(index, _)| state.entries.get(index))
            .map(|entry| NativeMatch {
                name: entry.tool.name().to_string(),
                tool: entry.tool.clone(),
            })
            .collect()
    }

    /// Make tools callable for the rest of a session. Returns the names
    /// that were newly activated for that session. Activation order is
    /// preserved: activated definitions are appended to the tool array so
    /// an activation invalidates only the tail of the provider's cached
    /// prompt prefix, never the native tool block.
    pub fn activate(&self, session: &str, names: &[String]) -> Vec<String> {
        let state = self.state.read().unwrap();
        let mut activated = self.activated.write().unwrap();
        let bucket = activated.entry(session.to_string()).or_default();
        let mut added = Vec::new();
        for name in names {
            if !state.by_name.contains_key(name) || bucket.contains(name) {
                continue;
            }
            if bucket.len() >= MAX_ACTIVATED {
                warn!(
                    "Native deferred-tool activation cap ({}) reached; '{}' not activated",
                    MAX_ACTIVATED, name
                );
                break;
            }
            bucket.push(name.clone());
            added.push(name.clone());
        }
        added
    }

    pub fn activated_names(&self, session: &str) -> Vec<String> {
        self.activated
            .read()
            .unwrap()
            .get(session)
            .cloned()
            .unwrap_or_default()
    }

    /// Every activated tool across all sessions, deduplicated. For UI
    /// browsing, which is not session-scoped.
    pub fn all_activated_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        for bucket in self.activated.read().unwrap().values() {
            for name in bucket {
                if !names.contains(name) {
                    names.push(name.clone());
                }
            }
        }
        names
    }

    pub fn is_activated(&self, session: &str, name: &str) -> bool {
        self.activated
            .read()
            .unwrap()
            .get(session)
            .map(|bucket| bucket.iter().any(|activated| activated == name))
            .unwrap_or(false)
    }

    /// Definitions for tools activated in this session, in activation
    /// order.
    pub fn activated_definitions(&self, session: &str) -> Vec<ToolDefinition> {
        let state = self.state.read().unwrap();
        self.activated
            .read()
            .unwrap()
            .get(session)
            .into_iter()
            .flatten()
            .filter_map(|name| state.by_name.get(name))
            .filter_map(|index| state.entries.get(*index))
            .map(|entry| ToolDefinition {
                tool_type: "function".to_string(),
                function: ToolFunction {
                    name: entry.tool.name().to_string(),
                    description: tool_prompt_description(&entry.tool),
                    parameters: entry.tool.parameters(),
                },
            })
            .collect()
    }

    /// Drop all activation state for a session. Called when a session is
    /// deleted so its loaded tools do not linger in memory.
    pub fn prune_session(&self, session: &str) {
        self.activated.write().unwrap().remove(session);
    }

    /// The always-in-context table of contents. One line per deferred
    /// built-in — without it the model cannot know a capability exists
    /// and so never thinks to search for it.
    pub fn manifest(&self) -> Option<String> {
        let state = self.state.read().unwrap();
        if state.entries.is_empty() {
            return None;
        }
        let mut lines = Vec::new();
        for entry in &state.entries {
            lines.push(format!(
                "- {}: {}",
                entry.tool.name(),
                short_blurb(&entry.tool)
            ));
        }
        Some(format!(
            "# Deferred Built-in Tools\n\
             These built-in tools are NOT loaded yet. Call `tool_search` with a plain-language \
             query to load the ones you need, then call them normally.\n{}",
            lines.join("\n")
        ))
    }

    /// Search-result shape, mirroring the MCP catalog's so the model sees
    /// deferred built-ins and MCP tools uniformly.
    pub fn to_search_result(&self, name: &str) -> Option<Value> {
        self.get(name).map(|tool| {
            json!({
                "name": name,
                "server": "builtin",
                "description": tool_prompt_description(&tool),
                "read_only": false,
                "input_schema": tool.parameters(),
            })
        })
    }
}

/// One-line manifest blurb: the tool's when-to-use, capped. When a tool
/// has no when-to-use, its description's first sentence is used instead.
fn short_blurb(tool: &Arc<dyn Tool>) -> String {
    let when_to_use = tool.when_to_use().trim();
    let source = if !when_to_use.is_empty() && when_to_use != "See tool description" {
        when_to_use
    } else {
        tool.description().trim()
    };
    let mut blurb = source.split_whitespace().collect::<Vec<_>>().join(" ");
    blurb.truncate(120);
    if blurb.len() >= 120 {
        blurb.push('…');
    }
    blurb
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use std::sync::Arc;

    struct StubTool {
        name: &'static str,
        description: &'static str,
    }

    #[async_trait::async_trait]
    impl Tool for StubTool {
        fn name(&self) -> &str {
            self.name
        }
        fn description(&self) -> &str {
            self.description
        }
        fn parameters(&self) -> Value {
            json!({ "type": "object" })
        }
        async fn execute(&self, _args: Value) -> crate::error::Result<String> {
            Ok("ok".to_string())
        }
    }

    fn stub(name: &'static str, description: &'static str) -> Arc<dyn Tool> {
        Arc::new(StubTool { name, description })
    }

    fn catalog() -> Arc<NativeToolCatalog> {
        let catalog = Arc::new(NativeToolCatalog::new());
        catalog.register(stub(
            "weather",
            "Current conditions and forecast for a location",
        ));
        catalog.register(stub(
            "record_memory",
            "Persist a fact about the user for future sessions",
        ));
        catalog
    }

    #[test]
    fn registers_and_searches_by_plain_language() {
        let catalog = catalog();
        assert_eq!(catalog.catalog_size(), 2);
        assert!(catalog.contains("weather"));
        assert!(!catalog.contains("nope"));

        let hits = catalog.search("what is the weather like", 5);
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].name, "weather");
    }

    #[test]
    fn search_is_empty_for_unrelated_queries() {
        let catalog = catalog();
        assert!(catalog.search("kubernetes scaling", 5).is_empty());
    }

    #[test]
    fn activate_makes_definitions_available_once_per_session() {
        let catalog = catalog();
        assert!(catalog.activated_definitions("s1").is_empty());

        let added = catalog.activate("s1", &["weather".to_string(), "missing".to_string()]);
        assert_eq!(added, vec!["weather".to_string()]);

        let defs = catalog.activated_definitions("s1");
        assert_eq!(defs.len(), 1);
        assert_eq!(defs[0].function.name, "weather");
        assert!(defs[0].function.description.contains("forecast"));

        // Activating again in the same session is a no-op.
        let added_again = catalog.activate("s1", &["weather".to_string()]);
        assert!(added_again.is_empty());
        assert_eq!(catalog.activated_definitions("s1").len(), 1);

        // A different session starts clean: activation never leaks.
        assert!(catalog.activated_definitions("s2").is_empty());
        assert!(!catalog.is_activated("s2", "weather"));
        let added_s2 = catalog.activate("s2", &["record_memory".to_string()]);
        assert_eq!(added_s2, vec!["record_memory".to_string()]);
        assert_eq!(catalog.activated_definitions("s2").len(), 1);
        assert_eq!(catalog.activated_definitions("s1").len(), 1);

        // Pruning removes only the targeted session.
        catalog.prune_session("s1");
        assert!(catalog.activated_definitions("s1").is_empty());
        assert_eq!(catalog.activated_definitions("s2").len(), 1);
    }

    #[test]
    fn manifest_lists_every_deferred_tool() {
        let catalog = catalog();
        let manifest = catalog.manifest().expect("manifest");
        assert!(manifest.contains("weather"));
        assert!(manifest.contains("record_memory"));
        assert!(manifest.contains("tool_search"));
    }

    #[test]
    fn search_result_includes_schema_for_immediate_calls() {
        let catalog = catalog();
        let result = catalog.to_search_result("weather").expect("result");
        assert_eq!(result["server"], "builtin");
        assert_eq!(result["name"], "weather");
        assert!(result["description"].as_str().unwrap().contains("forecast"));
        assert!(result["input_schema"].is_object());
    }

    #[test]
    fn stub_config_unused_is_constructible() {
        let _config = Config::default();
    }
}
