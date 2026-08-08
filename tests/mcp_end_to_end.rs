//! End-to-end coverage against a real MCP server process.
//!
//! The unit tests cover ranking, naming, and activation with a
//! hand-built catalog. What they cannot prove is that the client
//! actually speaks the protocol — handshake, pagination, error frames,
//! content flattening — against a process on the other end of a pipe.
//! This does, using a fixture server written in stdlib Python.

use osagent::config::{McpConfig, McpServerConfig, McpTransport};
use osagent::mcp::McpManager;

/// A minimal but honest MCP server: real JSON-RPC framing, a paginated
/// `tools/list`, a working tool, a tool that reports `isError`, and one
/// that returns structured content only.
const FIXTURE_SERVER: &str = r#"
import json
import sys

TOOLS_PAGE_ONE = [
    {
        "name": "create_issue",
        "description": "Create a new issue in a project tracker",
        "inputSchema": {
            "type": "object",
            "properties": {"title": {"type": "string"}},
            "required": ["title"],
        },
    },
    {
        "name": "listProjects",
        "description": "List every project the user can see",
        "inputSchema": {"type": "object", "properties": {}},
        "annotations": {"readOnlyHint": True},
    },
]

TOOLS_PAGE_TWO = [
    {
        "name": "delete_project",
        "description": "Permanently delete a project",
        "inputSchema": {"type": "object", "properties": {"id": {"type": "string"}}},
        "annotations": {"destructiveHint": True},
    },
]


def respond(request_id, result):
    sys.stdout.write(json.dumps({"jsonrpc": "2.0", "id": request_id, "result": result}) + "\n")
    sys.stdout.flush()


def fail(request_id, code, message):
    sys.stdout.write(
        json.dumps({"jsonrpc": "2.0", "id": request_id, "error": {"code": code, "message": message}})
        + "\n"
    )
    sys.stdout.flush()


# Banner on stdout before the protocol starts: real servers do this and
# the client must not choke on it.
sys.stdout.write("starting fixture server\n")
sys.stdout.flush()

for line in sys.stdin:
    line = line.strip()
    if not line:
        continue
    message = json.loads(line)
    method = message.get("method")
    request_id = message.get("id")

    if method == "initialize":
        respond(request_id, {
            "protocolVersion": "2025-06-18",
            "capabilities": {"tools": {}},
            "serverInfo": {"name": "fixture", "version": "1.2.3"},
            "instructions": "Fixture server for tracking issues and projects.",
        })
    elif method == "notifications/initialized":
        continue
    elif method == "tools/list":
        cursor = (message.get("params") or {}).get("cursor")
        if cursor == "page2":
            respond(request_id, {"tools": TOOLS_PAGE_TWO})
        else:
            respond(request_id, {"tools": TOOLS_PAGE_ONE, "nextCursor": "page2"})
    elif method == "tools/call":
        params = message.get("params") or {}
        name = params.get("name")
        arguments = params.get("arguments") or {}
        if name == "create_issue":
            respond(request_id, {
                "content": [{"type": "text", "text": "created: " + arguments.get("title", "")}]
            })
        elif name == "listProjects":
            respond(request_id, {"structuredContent": {"projects": ["core", "infra"]}})
        elif name == "delete_project":
            respond(request_id, {
                "content": [{"type": "text", "text": "refusing to delete"}],
                "isError": True,
            })
        else:
            fail(request_id, -32602, "unknown tool: " + str(name))
    else:
        fail(request_id, -32601, "method not found: " + str(method))
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

struct Fixture {
    _directory: tempfile::TempDir,
    config: McpConfig,
}

fn fixture() -> Fixture {
    let directory = tempfile::tempdir().expect("temp dir");
    let script = directory.path().join("fixture_server.py");
    std::fs::write(&script, FIXTURE_SERVER).expect("write fixture");

    let config = McpConfig {
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
    };

    Fixture {
        _directory: directory,
        config,
    }
}

#[tokio::test]
async fn connects_and_indexes_every_page_of_tools() {
    if !python_available() {
        eprintln!("skipping: python not on PATH");
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    // Three tools across two pages proves cursor pagination works.
    assert_eq!(manager.catalog_size(), 3, "expected both pages of tools");

    let summaries = manager.server_summaries();
    assert_eq!(summaries.len(), 1);
    assert!(summaries[0].connected, "error: {:?}", summaries[0].error);
    assert!(
        summaries[0].blurb.contains("Fixture server"),
        "blurb should fall back to server instructions, got: {}",
        summaries[0].blurb
    );

    manager.shutdown().await;
}

#[tokio::test]
async fn tools_stay_out_of_context_until_searched() {
    if !python_available() {
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    // The whole premise: a connected server costs zero tool schemas.
    assert!(
        manager.activated_definitions().is_empty(),
        "connecting a server must not load any schemas"
    );

    // The manifest is what the model actually sees.
    let manifest = manager.manifest_prompt().expect("manifest");
    assert!(manifest.contains("tracker"));
    assert!(manifest.contains("3 tools"));

    let matches = manager.search("create issue", 5);
    assert_eq!(matches[0].tool, "create_issue");

    manager.activate(&[matches[0].qualified_name.clone()]);
    let definitions = manager.activated_definitions();
    assert_eq!(definitions.len(), 1);
    assert_eq!(definitions[0].function.name, "mcp__tracker__create_issue");
    assert_eq!(definitions[0].function.parameters["type"], "object");
    assert!(definitions[0].function.parameters["properties"]
        .get("title")
        .is_some());

    manager.shutdown().await;
}

#[tokio::test]
async fn calls_a_tool_and_flattens_its_content() {
    if !python_available() {
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    let (output, is_error) = manager
        .call(
            "mcp__tracker__create_issue",
            serde_json::json!({"title": "bridge leaks"}),
        )
        .await
        .expect("call should succeed");

    assert_eq!(output, "created: bridge leaks");
    assert!(!is_error);

    // Calling a catalogued tool activates it, so the model can keep
    // using it without searching again.
    assert!(manager.is_activated("mcp__tracker__create_issue"));

    manager.shutdown().await;
}

#[tokio::test]
async fn structured_content_survives_flattening() {
    if !python_available() {
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    let (output, is_error) = manager
        .call("mcp__tracker__listProjects", serde_json::json!({}))
        .await
        .expect("call should succeed");

    assert!(!is_error);
    assert!(output.contains("core"), "got: {}", output);

    manager.shutdown().await;
}

#[tokio::test]
async fn tool_level_errors_are_reported_not_swallowed() {
    if !python_available() {
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    let (output, is_error) = manager
        .call(
            "mcp__tracker__delete_project",
            serde_json::json!({"id": "1"}),
        )
        .await
        .expect("transport should succeed even when the tool fails");

    assert!(
        is_error,
        "server set isError and the client must surface it"
    );
    assert!(output.contains("refusing"));

    manager.shutdown().await;
}

#[tokio::test]
async fn protocol_errors_become_rust_errors() {
    if !python_available() {
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    let result = manager
        .call("mcp__tracker__does_not_exist", serde_json::json!({}))
        .await;
    assert!(result.is_err(), "unknown tools must not silently succeed");

    manager.shutdown().await;
}

#[tokio::test]
async fn read_only_and_destructive_hints_are_preserved() {
    if !python_available() {
        return;
    }
    let fixture = fixture();
    let manager = McpManager::connect(&fixture.config).await;

    let list = manager.entry("mcp__tracker__listProjects").expect("entry");
    assert!(list.read_only, "readOnlyHint must survive into the catalog");

    let delete = manager
        .entry("mcp__tracker__delete_project")
        .expect("entry");
    assert!(delete.destructive);
    assert!(!delete.read_only);
    assert!(delete
        .to_definition()
        .function
        .description
        .contains("destructive"));

    manager.shutdown().await;
}

#[tokio::test]
async fn always_active_tools_skip_the_search_round_trip() {
    if !python_available() {
        return;
    }
    let mut fixture = fixture();
    fixture.config.servers[0].always_active = vec!["create_issue".to_string()];

    let manager = McpManager::connect(&fixture.config).await;
    assert_eq!(
        manager.activated_names(),
        vec!["mcp__tracker__create_issue".to_string()]
    );

    manager.shutdown().await;
}

#[tokio::test]
async fn a_broken_server_does_not_take_down_the_others() {
    if !python_available() {
        return;
    }
    let mut fixture = fixture();
    let working = fixture.config.servers[0].clone();
    fixture.config.servers = vec![
        McpServerConfig {
            name: "broken".to_string(),
            enabled: true,
            command: Some("definitely-not-a-real-binary-9f3a".to_string()),
            timeout_seconds: 5,
            ..Default::default()
        },
        working,
    ];

    let manager = McpManager::connect(&fixture.config).await;

    // The working server still landed.
    assert_eq!(manager.catalog_size(), 3);

    let summaries = manager.server_summaries();
    let broken = summaries
        .iter()
        .find(|summary| summary.name == "broken")
        .expect("broken server should still be listed");
    assert!(!broken.connected);
    assert!(broken.error.is_some());

    // And the failure is visible to the model rather than silent.
    let manifest = manager.manifest_prompt().expect("manifest");
    assert!(manifest.contains("unavailable"));

    manager.shutdown().await;
}

#[tokio::test]
async fn disabled_servers_are_never_started() {
    if !python_available() {
        return;
    }
    let mut fixture = fixture();
    fixture.config.servers[0].enabled = false;

    let manager = McpManager::connect(&fixture.config).await;
    assert_eq!(manager.catalog_size(), 0);
    assert!(manager.server_summaries().is_empty());
}

#[tokio::test]
async fn validation_rejects_unusable_server_definitions() {
    let missing_command = McpServerConfig {
        name: "x".to_string(),
        transport: Some(McpTransport::Stdio),
        ..Default::default()
    };
    assert!(missing_command.validate().is_err());

    let bad_url = McpServerConfig {
        name: "x".to_string(),
        transport: Some(McpTransport::Http),
        url: Some("ftp://example.com".to_string()),
        ..Default::default()
    };
    assert!(bad_url.validate().is_err());

    let bad_name = McpServerConfig {
        name: "my server".to_string(),
        command: Some("echo".to_string()),
        ..Default::default()
    };
    assert!(bad_name.validate().is_err());

    let fine = McpServerConfig {
        name: "linear".to_string(),
        command: Some("npx".to_string()),
        args: vec!["-y".to_string(), "mcp-remote".to_string()],
        ..Default::default()
    };
    assert!(fine.validate().is_ok());
}
