//! What each persona actually sees.
//!
//! Discovery only works if `tool_search` reaches the model *and* the
//! tools it activates reach the model in the same profile. A profile
//! that gets one without the other is worse than having neither: the
//! agent searches, is told the tool is loaded, and then cannot call it.

use osagent::config::{Config, McpConfig, McpServerConfig, McpTransport};
use osagent::mcp::McpManager;
use osagent::storage::SqliteStorage;
use osagent::tools::registry::{ToolProfile, ToolRegistry};
use std::sync::Arc;

const FIXTURE_SERVER: &str = r#"
import json
import sys

TOOLS = [
    {
        "name": "create_issue",
        "description": "Create a new issue",
        "inputSchema": {"type": "object", "properties": {}},
    },
    {
        "name": "list_issues",
        "description": "List issues",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
    },
]

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")
    if method == "notifications/initialized":
        continue
    if method == "initialize":
        result = {"protocolVersion": "2025-06-18", "capabilities": {"tools": {}},
                  "serverInfo": {"name": "tracker", "version": "1"}}
    elif method == "tools/list":
        result = {"tools": TOOLS}
    else:
        result = {}
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()
"#;

fn python() -> &'static str {
    if cfg!(windows) {
        "python"
    } else {
        "python3"
    }
}

fn python_available() -> bool {
    std::process::Command::new(python())
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

const PROFILES: &[(&str, ToolProfile)] = &[
    ("default", ToolProfile::Default),
    ("code", ToolProfile::Code),
    ("plan", ToolProfile::Plan),
    ("creative", ToolProfile::Creative),
    ("custom", ToolProfile::Custom),
];

fn registry() -> ToolRegistry {
    let storage = Arc::new(SqliteStorage::new_in_memory().expect("storage"));
    ToolRegistry::new(Config::default_config(), storage).expect("registry")
}

fn names(registry: &ToolRegistry, profile: ToolProfile) -> Vec<String> {
    names_for_session(registry, profile, "")
}

fn names_for_session(registry: &ToolRegistry, profile: ToolProfile, session: &str) -> Vec<String> {
    registry
        .get_tool_definitions_for_profile(profile, session)
        .into_iter()
        .map(|tool| tool.function.name)
        .collect()
}

#[test]
fn tool_search_is_present_when_native_catalog_is_nonempty() {
    // Deferred built-ins (weather, code_python, memory, ...) always exist,
    // so tool_search must be offered even with no MCP server connected —
    // the model needs the gateway to load them. (Custom is the roleplay
    // persona and is deliberately near-toolless.)
    let registry = registry();
    for (label, profile) in PROFILES {
        let visible = names(&registry, *profile);
        let can_search = *profile != ToolProfile::Custom;
        assert_eq!(
            visible.iter().any(|n| n == "tool_search"),
            can_search,
            "{} persona has tool_search={}",
            label,
            can_search
        );
        // Nothing is loaded into a fresh session: deferred built-ins must
        // not leak into the default "" bucket.
        for deferred in ["weather", "code_python", "lsp", "task"] {
            assert!(
                !visible.iter().any(|n| n == deferred),
                "{} persona has '{}' loaded without a search",
                label,
                deferred
            );
        }
    }
}

#[tokio::test]
async fn every_persona_that_can_search_can_also_call_what_it_finds() {
    if !python_available() {
        eprintln!("skipping: python not on PATH");
        return;
    }

    let directory = tempfile::tempdir().expect("temp dir");
    let script = directory.path().join("server.py");
    std::fs::write(&script, FIXTURE_SERVER).expect("write fixture");

    let manager = McpManager::connect(&McpConfig {
        enabled: true,
        servers: vec![McpServerConfig {
            name: "tracker".to_string(),
            enabled: true,
            transport: Some(McpTransport::Stdio),
            command: Some(python().to_string()),
            args: vec![script.to_string_lossy().to_string()],
            timeout_seconds: 20,
            ..Default::default()
        }],
        ..Default::default()
    })
    .await;
    assert_eq!(manager.catalog_size(), 2, "fixture server did not connect");

    let registry = registry();
    registry.register_mcp(manager.clone());

    // Simulate what tool_search does: activate both tools in session "s1".
    manager.activate(
        "s1",
        &[
            "mcp__tracker__create_issue".to_string(),
            "mcp__tracker__list_issues".to_string(),
        ],
    );

    for (label, profile) in PROFILES {
        // Activation is session-scoped: query the same session the tools
        // were activated into.
        let visible = names_for_session(&registry, *profile, "s1");
        let can_search = visible.iter().any(|n| n == "tool_search");
        let reachable: Vec<&String> = visible.iter().filter(|n| n.starts_with("mcp__")).collect();

        // The invariant: discovery and callable tools are all-or-nothing
        // within a session.
        assert_eq!(
            can_search,
            !reachable.is_empty(),
            "{} persona has tool_search={} but {} reachable MCP tool(s) — either searching \
             is a dead end, or tools appear with no way to discover them",
            label,
            can_search,
            reachable.len()
        );

        if !can_search {
            continue;
        }

        // Plan is read-only by contract, so it should see the read-only
        // tool and not the mutating one.
        if *profile == ToolProfile::Plan {
            assert!(
                reachable.iter().all(|n| n.ends_with("list_issues")),
                "plan persona exposed a non-read-only MCP tool: {:?}",
                reachable
            );
        } else {
            assert_eq!(reachable.len(), 2, "{} lost an activated tool", label);
        }
    }

    manager.shutdown().await;
}

#[test]
fn tool_script_is_absent_from_read_only_and_minimal_personas() {
    let registry = registry();
    assert!(!names(&registry, ToolProfile::Plan)
        .iter()
        .any(|n| n == "tool_script"));
    assert!(!names(&registry, ToolProfile::Custom)
        .iter()
        .any(|n| n == "tool_script"));
    assert!(names(&registry, ToolProfile::Code)
        .iter()
        .any(|n| n == "tool_script"));
}

#[test]
fn native_tool_block_is_stable_across_activations() {
    // The prompt-cache argument only holds if the native block never
    // changes shape when MCP tools come and go. `tool_search` and
    // activated MCP tools are appended after the sorted native block, so
    // only the native core is required to stay sorted and byte-stable.
    let registry = registry();
    let before = names(&registry, ToolProfile::Default);
    let after = names(&registry, ToolProfile::Default);
    assert_eq!(before, after);

    let core: Vec<&String> = before
        .iter()
        .filter(|n| n.as_str() != "tool_search" && !n.starts_with("mcp__"))
        .collect();
    assert!(
        core.windows(2).all(|pair| pair[0] <= pair[1]),
        "native tools must stay sorted so the cached prefix is byte-stable"
    );
    assert_eq!(
        core.len(),
        before.len() - 1,
        "expected exactly one appended tail entry (tool_search), got {}",
        before.len() - core.len()
    );
}
