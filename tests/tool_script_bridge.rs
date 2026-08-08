//! End-to-end coverage for the `tool_script` bridge.
//!
//! The bridge is the one place where a script gets to call the agent's
//! tools directly, so these tests care about two things equally: that it
//! works, and that it cannot be used to reach a tool the script never
//! declared.

use osagent::config::Config;
use osagent::storage::SqliteStorage;
use osagent::tools::registry::ToolRegistry;
use osagent::tools::tool_script::{run_script, ScriptContext};
use serde_json::json;
use std::sync::Arc;

fn python_available() -> bool {
    let program = if cfg!(windows) { "python" } else { "python3" };
    std::process::Command::new(program)
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

fn node_available() -> bool {
    std::process::Command::new("node")
        .arg("--version")
        .output()
        .map(|output| output.status.success())
        .unwrap_or(false)
}

struct Harness {
    _workspace: tempfile::TempDir,
    context_registry: Arc<ToolRegistry>,
    config: Config,
    workspace_path: String,
}

fn harness() -> Harness {
    let workspace = tempfile::tempdir().expect("workspace");
    let workspace_path = workspace.path().to_string_lossy().to_string();

    std::fs::write(workspace.path().join("hello.txt"), "line one\nline two\n").expect("seed file");

    let mut config = Config::default_config();
    config.agent.workspace = workspace_path.clone();
    config.ensure_workspace_defaults();

    let storage = Arc::new(SqliteStorage::new_in_memory().expect("storage"));
    let registry = ToolRegistry::new(config.clone(), storage).expect("registry");

    Harness {
        _workspace: workspace,
        context_registry: Arc::new(registry),
        config,
        workspace_path,
    }
}

fn context(harness: &Harness) -> ScriptContext {
    ScriptContext {
        registry: harness.context_registry.clone(),
        config: harness.config.clone(),
        workspace_path: harness.workspace_path.clone(),
        event_bus: None,
        session_id: "test-session".to_string(),
    }
}

#[tokio::test]
async fn python_script_calls_a_tool_through_the_bridge() {
    if !python_available() {
        eprintln!("skipping: python not on PATH");
        return;
    }
    let harness = harness();

    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "code": "content = str(tools.read_file(filePath='hello.txt'))\nprint('OK', 'line one' in content and 'line two' in content)",
        }),
    )
    .await
    .expect("script should run");

    assert!(
        result.output.contains("OK True"),
        "script output was: {}",
        result.output
    );
    assert_eq!(result.metadata["tool_calls"], 1);
    assert_eq!(result.metadata["exit_code"], 0);
}

#[tokio::test]
async fn node_script_calls_a_tool_through_the_bridge() {
    if !node_available() {
        eprintln!("skipping: node not on PATH");
        return;
    }
    let harness = harness();

    let result = run_script(
        context(&harness),
        &json!({
            "language": "node",
            "uses": ["read_file"],
            "code": "const content = await tools.read_file({ filePath: 'hello.txt' });\nconsole.log('CHARS', String(content).length > 0);",
        }),
    )
    .await
    .expect("script should run");

    assert!(
        result.output.contains("CHARS true"),
        "script output was: {}",
        result.output
    );
    assert_eq!(result.metadata["tool_calls"], 1);
}

#[tokio::test]
async fn intermediate_results_never_reach_the_output() {
    if !python_available() {
        return;
    }
    let harness = harness();

    // The script reads a file whose contents would be noise in the
    // transcript and prints only a derived number. If the bridge ever
    // leaked raw tool output into the result, this would catch it.
    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "code": "content = str(tools.read_file(filePath='hello.txt'))\nprint(len(content.splitlines()))",
        }),
    )
    .await
    .expect("script should run");

    assert!(
        !result.output.contains("line one"),
        "raw tool output leaked into the result"
    );
}

#[tokio::test]
async fn undeclared_tools_are_rejected_at_the_bridge() {
    if !python_available() {
        return;
    }
    let harness = harness();

    // `write_file` exists and is permitted, but the script declared only
    // `read_file`. The declaration is the security boundary.
    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "code": "import osa\ntry:\n    osa.call('write_file', {'path': 'evil.txt', 'content': 'x'})\n    print('BREACH')\nexcept osa.ToolError as error:\n    print('BLOCKED', error)",
        }),
    )
    .await
    .expect("script should run");

    assert!(
        !result.output.contains("BREACH"),
        "undeclared tool was reachable"
    );
    assert!(result.output.contains("BLOCKED"), "got: {}", result.output);
    assert!(
        !harness._workspace.path().join("evil.txt").exists(),
        "undeclared tool actually wrote a file"
    );
}

#[tokio::test]
async fn a_bad_token_is_refused() {
    if !python_available() {
        return;
    }
    let harness = harness();

    // Simulates another process on loopback trying to drive the bridge
    // while a script is running.
    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "code": r#"
import json, os, socket
connection = socket.create_connection(("127.0.0.1", int(os.environ["OSA_BRIDGE_PORT"])))
connection.sendall((json.dumps({"token": "wrong", "tool": "read_file", "arguments": {"filePath": "hello.txt"}}) + "\n").encode())
print("RESPONSE", connection.recv(4096).decode().strip())
"#,
        }),
    )
    .await
    .expect("script should run");

    assert!(
        result.output.contains("unauthorized"),
        "bridge accepted a bad token: {}",
        result.output
    );
}

#[tokio::test]
async fn control_plane_tools_cannot_be_declared() {
    let harness = harness();

    let error = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["subagent"],
            "code": "print('hi')",
        }),
    )
    .await
    .expect_err("declaring a control-plane tool must fail");

    assert!(error.to_string().contains("subagent"));
}

#[tokio::test]
async fn unknown_tools_fail_before_the_script_runs() {
    let harness = harness();

    let error = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["mcp__nope__missing"],
            "code": "print('hi')",
        }),
    )
    .await
    .expect_err("unknown tool must fail");

    assert!(error.to_string().contains("tool_search"), "got: {}", error);
}

#[tokio::test]
async fn scripts_must_declare_something() {
    let harness = harness();

    let error = run_script(
        context(&harness),
        &json!({"language": "python", "uses": [], "code": "print('hi')"}),
    )
    .await
    .expect_err("empty 'uses' must fail");

    assert!(error.to_string().contains("uses"));
}

#[tokio::test]
async fn script_errors_surface_with_their_traceback() {
    if !python_available() {
        return;
    }
    let harness = harness();

    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "code": "raise ValueError('deliberate')",
        }),
    )
    .await
    .expect("a failing script is a result, not a transport error");

    assert_ne!(result.metadata["exit_code"], 0);
    assert!(
        result.output.contains("deliberate"),
        "got: {}",
        result.output
    );
}

#[tokio::test]
async fn a_hanging_script_is_terminated() {
    if !python_available() {
        return;
    }
    let harness = harness();

    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "timeout_seconds": 1,
            "code": "import time\ntime.sleep(30)",
        }),
    )
    .await
    .expect("timeout is a result, not an error");

    assert_eq!(result.metadata["timed_out"], true);
    assert!(result.output.contains("exceeded"));
}

#[tokio::test]
async fn tool_failures_are_catchable_inside_the_script() {
    if !python_available() {
        return;
    }
    let harness = harness();

    let result = run_script(
        context(&harness),
        &json!({
            "language": "python",
            "uses": ["read_file"],
            "code": "import osa\ntry:\n    tools.read_file(filePath='does-not-exist.txt')\n    print('UNEXPECTED')\nexcept osa.ToolError:\n    print('HANDLED')",
        }),
    )
    .await
    .expect("script should run");

    assert!(result.output.contains("HANDLED"), "got: {}", result.output);
}
