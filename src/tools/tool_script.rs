//! `tool_script` — drive many tools from one sandboxed script.
//!
//! The context win here is bigger than the one from deferring schemas.
//! When a script fetches 200 records, filters to 3, and passes those to
//! another tool, only what it prints reaches the conversation; the 200
//! never exist as far as the model is concerned. Control flow, retries,
//! and dependent calls come free, which the JSON-array `batch` tool
//! cannot express.
//!
//! Because the script calls the registry directly, it bypasses the
//! runtime's per-call permission gate. That gate is reinstated here: a
//! script may only touch tools it declared in `uses`, the declaration is
//! what the user approves when the `tool_script` call itself is gated,
//! and denied tools stay denied regardless of what was declared.

use crate::agent::events::{AgentEvent, EventBus};
use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::permission::PermissionAction;
use crate::tools::registry::{Tool, ToolExample, ToolRegistry, ToolResult};
use async_trait::async_trait;
use serde::Deserialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::{TcpListener, TcpStream};
use tokio::time::timeout;
use tracing::{debug, warn};
use uuid::Uuid;

/// Tools a script may never call, whatever it declares.
///
/// These are control-plane tools the runtime interprets itself (asking
/// the user, spawning agents, switching persona). Reaching them through
/// the bridge would either deadlock or silently skip the runtime
/// handling that gives them meaning.
const BRIDGE_BLOCKED_TOOLS: &[&str] = &[
    "tool_script",
    "batch",
    "question",
    "subagent",
    "coordinator",
    "persona",
    "plan_exit",
    "task",
    "schedule",
];

/// Ceiling on bridge calls from one script. A runaway loop should fail
/// loudly and cheaply rather than hammer a remote API until the timeout.
const MAX_BRIDGE_CALLS: usize = 250;

/// Output past this is truncated before it reaches the model — the point
/// of the tool is that the script summarizes, not that it dumps.
const MAX_OUTPUT_BYTES: usize = 64 * 1024;

const DEFAULT_TIMEOUT_SECONDS: u64 = 120;
const MAX_TIMEOUT_SECONDS: u64 = 600;

pub struct ToolScriptTool;

impl ToolScriptTool {
    pub fn new() -> Self {
        Self
    }
}

impl Default for ToolScriptTool {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Tool for ToolScriptTool {
    fn name(&self) -> &str {
        "tool_script"
    }

    fn description(&self) -> &str {
        "Run a Python or JavaScript script that calls other tools through an `osa` helper. \
         Only what the script prints enters the conversation, so use it to fetch, join, filter, \
         or loop over large results without paying context for the intermediate data."
    }

    fn when_to_use(&self) -> &str {
        "Use when a task needs several dependent tool calls, a loop over many items, or when \
         intermediate results would be large and only a summary matters"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for a single tool call, for independent read-only calls (use batch), or \
         when you need to inspect each intermediate result before deciding what to do next"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "Summarize many records without loading them into context".to_string(),
            input: json!({
                "language": "python",
                "uses": ["mcp__linear__list_issues"],
                "code": "issues = osa.tools.linear.list_issues(team='core')\nstale = [i for i in issues if i['state'] == 'backlog']\nprint(f'{len(stale)} stale of {len(issues)}')"
            }),
        }]
    }

    fn parameters(&self) -> Value {
        // Static on purpose. Listing the currently activated MCP tools
        // here would be friendlier, but this schema sits in the native
        // tool block, which is kept byte-stable so activating a tool
        // only invalidates the tail of the provider's cached prompt
        // prefix. A hint that changes on every activation would rewrite
        // the whole block instead.
        let hint = "Every tool this script will call, by exact name. Calls to anything not \
                    listed here are rejected. MCP tool names come from tool_search results.";

        json!({
            "type": "object",
            "properties": {
                "language": {
                    "type": "string",
                    "enum": ["python", "node"],
                    "description": "Interpreter to run the script with."
                },
                "code": {
                    "type": "string",
                    "description": "Script source. Call tools via `osa.tools.<tool>(...)`; for \
                                    MCP tools that is `osa.tools.<server>.<tool>(...)`. Arguments \
                                    are passed as keyword arguments (Python) or a single object \
                                    (JavaScript). Print only the summary you want to see."
                },
                "uses": {
                    "type": "array",
                    "items": {"type": "string"},
                    "minItems": 1,
                    "description": hint
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": MAX_TIMEOUT_SECONDS,
                    "description": "Wall-clock limit for the whole script."
                }
            },
            "required": ["language", "code", "uses"]
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        Err(OSAgentError::ToolExecution(
            "The tool_script tool is handled by the OSA runtime and should not be executed directly"
                .to_string(),
        ))
    }
}

#[derive(Debug, Deserialize)]
struct BridgeRequest {
    token: String,
    tool: String,
    #[serde(default)]
    arguments: Value,
}

/// What the runtime needs to run a script. Kept separate from the tool
/// definition because execution needs the registry, which the registry
/// cannot hand to a tool it owns.
pub struct ScriptContext {
    pub registry: Arc<ToolRegistry>,
    pub config: Config,
    pub workspace_path: String,
    pub event_bus: Option<EventBus>,
    pub session_id: String,
}

/// Execute a `tool_script` call. Called by the runtime, mirroring how
/// `batch` is intercepted.
pub async fn run_script(context: ScriptContext, args: &Value) -> Result<ToolResult> {
    let language = args
        .get("language")
        .and_then(|value| value.as_str())
        .unwrap_or("python")
        .to_lowercase();
    if language != "python" && language != "node" {
        return Err(OSAgentError::ToolExecution(format!(
            "Unsupported script language '{}'. Use \"python\" or \"node\".",
            language
        )));
    }

    let code = args
        .get("code")
        .and_then(|value| value.as_str())
        .ok_or_else(|| OSAgentError::ToolExecution("Missing 'code' parameter".to_string()))?;

    let declared: Vec<String> = args
        .get("uses")
        .and_then(|value| value.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|item| item.as_str().map(|name| name.trim().to_string()))
                .filter(|name| !name.is_empty())
                .collect()
        })
        .unwrap_or_default();

    if declared.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "'uses' must list every tool the script will call. Scripts run with no tool access \
             by default."
                .to_string(),
        ));
    }

    let allowlist = build_allowlist(&context, &declared)?;

    let timeout_seconds = args
        .get("timeout_seconds")
        .and_then(|value| value.as_u64())
        .unwrap_or(DEFAULT_TIMEOUT_SECONDS)
        .clamp(1, MAX_TIMEOUT_SECONDS);

    // Bind before spawning so the port is known when the stub is written.
    let listener = TcpListener::bind("127.0.0.1:0")
        .await
        .map_err(|error| OSAgentError::ToolExecution(format!("Bridge bind failed: {}", error)))?;
    let port = listener
        .local_addr()
        .map_err(|error| OSAgentError::ToolExecution(error.to_string()))?
        .port();
    let token = Uuid::new_v4().to_string();

    let call_count = Arc::new(AtomicUsize::new(0));
    let bridge = BridgeState {
        registry: context.registry.clone(),
        workspace_path: context.workspace_path.clone(),
        allowlist: allowlist.clone(),
        token: token.clone(),
        call_count: call_count.clone(),
        event_bus: context.event_bus.clone(),
        session_id: context.session_id.clone(),
    };
    let bridge_task = tokio::spawn(serve_bridge(listener, Arc::new(bridge)));

    let outcome = run_interpreter(
        &language,
        code,
        &allowlist,
        &context.workspace_path,
        port,
        &token,
        timeout_seconds,
    )
    .await;

    bridge_task.abort();

    let calls = call_count.load(Ordering::Relaxed);
    let ScriptOutcome {
        stdout,
        stderr,
        exit_code,
        timed_out,
    } = outcome?;

    let mut output = String::new();
    if timed_out {
        output.push_str(&format!(
            "Script exceeded its {}s limit and was terminated.\n",
            timeout_seconds
        ));
    }
    if !stdout.trim().is_empty() {
        output.push_str(stdout.trim_end());
        output.push('\n');
    }
    if !stderr.trim().is_empty() {
        output.push_str("\n--- stderr ---\n");
        output.push_str(stderr.trim_end());
        output.push('\n');
    }
    if output.trim().is_empty() {
        output.push_str("(script produced no output)\n");
    }
    if exit_code != 0 && !timed_out {
        output.push_str(&format!("\nScript exited with code {}.\n", exit_code));
    }

    let truncated = truncate_output(&output);
    let success = exit_code == 0 && !timed_out;

    Ok(ToolResult {
        output: truncated,
        title: Some(format!(
            "{} script · {} tool call(s)",
            language,
            calls
        )),
        metadata: json!({
            "language": language,
            "tool_calls": calls,
            "exit_code": exit_code,
            "timed_out": timed_out,
            "declared_tools": declared,
            "success": success,
        }),
        attachments: Vec::new(),
    })
}

/// Resolve the declared tool names into the set the bridge will honor.
///
/// Three gates, all of which must pass: the tool must exist, config must
/// not have denied it, and the permission evaluator must not deny it.
/// An unknown or blocked name fails the whole call rather than being
/// dropped — a script that silently loses a tool fails confusingly later.
fn build_allowlist(context: &ScriptContext, declared: &[String]) -> Result<HashSet<String>> {
    let manager = context.registry.mcp_manager();
    let mut allowed = HashSet::new();

    for name in declared {
        if BRIDGE_BLOCKED_TOOLS.contains(&name.as_str()) {
            return Err(OSAgentError::ToolExecution(format!(
                "'{}' cannot be called from a script; call it directly instead.",
                name
            )));
        }

        let exists = if ToolRegistry::is_mcp_tool(name) {
            manager
                .as_ref()
                .map(|manager| manager.is_known(name))
                .unwrap_or(false)
        } else {
            context.registry.has_tool(name)
        };
        if !exists {
            return Err(OSAgentError::ToolExecution(format!(
                "Unknown tool '{}' in 'uses'. For MCP tools, run tool_search first so the name \
                 is known.",
                name
            )));
        }

        if !context.registry.is_allowed(name) {
            return Err(OSAgentError::ToolNotAllowed(name.clone()));
        }

        if context
            .config
            .evaluate_permission_rule(name, &context.workspace_path)
            == Some(PermissionAction::Deny)
        {
            return Err(OSAgentError::ToolExecution(format!(
                "Tool '{}' is denied by a permission rule and cannot be used from a script.",
                name
            )));
        }

        allowed.insert(name.clone());
    }

    Ok(allowed)
}

struct BridgeState {
    registry: Arc<ToolRegistry>,
    workspace_path: String,
    allowlist: HashSet<String>,
    token: String,
    call_count: Arc<AtomicUsize>,
    event_bus: Option<EventBus>,
    session_id: String,
}

/// Accept bridge connections for as long as the script runs. One
/// connection per tool call keeps framing trivial and lets the script
/// side be a dozen lines of stdlib in either language.
async fn serve_bridge(listener: TcpListener, state: Arc<BridgeState>) {
    loop {
        let Ok((stream, _)) = listener.accept().await else {
            break;
        };
        let state = state.clone();
        tokio::spawn(async move {
            if let Err(error) = handle_bridge_connection(stream, state).await {
                debug!("tool_script bridge connection ended: {}", error);
            }
        });
    }
}

async fn handle_bridge_connection(stream: TcpStream, state: Arc<BridgeState>) -> Result<()> {
    let (read_half, mut write_half) = stream.into_split();
    let mut reader = BufReader::new(read_half);
    let mut line = String::new();

    if reader.read_line(&mut line).await? == 0 {
        return Ok(());
    }

    let response = match serde_json::from_str::<BridgeRequest>(line.trim()) {
        Ok(request) => dispatch(&state, request).await,
        Err(error) => json!({"ok": false, "error": format!("Malformed bridge request: {}", error)}),
    };

    let mut payload = serde_json::to_string(&response).unwrap_or_else(|_| {
        "{\"ok\":false,\"error\":\"failed to serialize response\"}".to_string()
    });
    payload.push('\n');
    write_half.write_all(payload.as_bytes()).await?;
    write_half.flush().await?;
    Ok(())
}

async fn dispatch(state: &BridgeState, request: BridgeRequest) -> Value {
    // The token stops anything else on the loopback interface from
    // driving the agent's tools while a script is running.
    if request.token != state.token {
        warn!("tool_script bridge rejected a request with a bad token");
        return json!({"ok": false, "error": "unauthorized"});
    }

    if !state.allowlist.contains(&request.tool) {
        return json!({
            "ok": false,
            "error": format!(
                "Tool '{}' was not declared in 'uses' and is unavailable to this script.",
                request.tool
            )
        });
    }

    let count = state.call_count.fetch_add(1, Ordering::Relaxed) + 1;
    if count > MAX_BRIDGE_CALLS {
        return json!({
            "ok": false,
            "error": format!("Script exceeded the {} tool-call limit.", MAX_BRIDGE_CALLS)
        });
    }

    let call_id = format!("script-{}", Uuid::new_v4());
    if let Some(bus) = state.event_bus.as_ref() {
        bus.emit(AgentEvent::ToolStart {
            session_id: state.session_id.clone(),
            sequence: 0,
            tool_call_id: call_id.clone(),
            tool_name: request.tool.clone(),
            arguments: request.arguments.clone(),
            message_index: -1,
            timestamp: SystemTime::now(),
        });
    }

    let started = Instant::now();
    let result = state
        .registry
        .execute_in_workspace_result(
            &request.tool,
            request.arguments.clone(),
            Some(state.workspace_path.clone()),
        )
        .await;
    let duration_ms = started.elapsed().as_millis() as u64;

    let (success, output) = match &result {
        Ok(value) => (true, value.output.clone()),
        Err(error) => (false, error.to_string()),
    };

    // Every bridge call shows up in the UI exactly like a direct call.
    // Scripts must not be a way to act unobserved.
    if let Some(bus) = state.event_bus.as_ref() {
        bus.emit(AgentEvent::ToolComplete {
            session_id: state.session_id.clone(),
            sequence: 0,
            tool_call_id: call_id,
            tool_name: request.tool.clone(),
            success,
            output: output.clone(),
            title: result.as_ref().ok().and_then(|value| value.title.clone()),
            metadata: Some(json!({"via": "tool_script"})),
            duration_ms,
            timestamp: SystemTime::now(),
        });
    }

    match result {
        Ok(value) => json!({"ok": true, "output": value.output}),
        Err(error) => json!({"ok": false, "error": error.to_string()}),
    }
}

struct ScriptOutcome {
    stdout: String,
    stderr: String,
    exit_code: i32,
    timed_out: bool,
}

async fn run_interpreter(
    language: &str,
    code: &str,
    allowlist: &HashSet<String>,
    workspace_path: &str,
    port: u16,
    token: &str,
    timeout_seconds: u64,
) -> Result<ScriptOutcome> {
    let directory = tempfile::tempdir().map_err(|error| {
        OSAgentError::ToolExecution(format!("Failed to create script directory: {}", error))
    })?;

    let (stub_name, entry_name, program) = match language {
        "python" => ("osa.py", "script.py", "python"),
        _ => ("osa.js", "script.js", "node"),
    };

    std::fs::write(
        directory.path().join(stub_name),
        match language {
            "python" => python_stub(allowlist),
            _ => node_stub(allowlist),
        },
    )?;
    std::fs::write(
        directory.path().join(entry_name),
        match language {
            "python" => format!("import osa\nfrom osa import tools\n\n{}\n", code),
            _ => format!(
                "const osa = require('./osa.js');\nconst tools = osa.tools;\n\
                 (async () => {{\n{}\n}})().catch(error => {{ \
                 console.error(error && error.stack ? error.stack : String(error)); \
                 process.exitCode = 1; }});\n",
                code
            ),
        },
    )?;

    let interpreter = if language == "python" && cfg!(windows) {
        "python"
    } else if language == "python" {
        "python3"
    } else {
        program
    };

    let script_path = directory.path().join(entry_name);
    let script_dir = directory.path().to_path_buf();
    let workspace = shellexpand::tilde(workspace_path).to_string();
    let token = token.to_string();

    let child = tokio::task::spawn_blocking(move || {
        std::process::Command::new(interpreter)
            .arg(&script_path)
            .current_dir(if std::path::Path::new(&workspace).is_dir() {
                std::path::PathBuf::from(&workspace)
            } else {
                script_dir.clone()
            })
            .env("OSA_BRIDGE_PORT", port.to_string())
            .env("OSA_BRIDGE_TOKEN", token)
            .env("PYTHONPATH", &script_dir)
            .env("NODE_PATH", &script_dir)
            .env("PYTHONDONTWRITEBYTECODE", "1")
            .output()
    });

    match timeout(Duration::from_secs(timeout_seconds), child).await {
        Ok(joined) => {
            let output = joined
                .map_err(|error| OSAgentError::ToolExecution(error.to_string()))?
                .map_err(|error| {
                    OSAgentError::ToolExecution(format!(
                        "Failed to run {}: {}. Is it installed and on PATH?",
                        interpreter, error
                    ))
                })?;
            Ok(ScriptOutcome {
                stdout: String::from_utf8_lossy(&output.stdout).to_string(),
                stderr: String::from_utf8_lossy(&output.stderr).to_string(),
                exit_code: output.status.code().unwrap_or(-1),
                timed_out: false,
            })
        }
        Err(_) => Ok(ScriptOutcome {
            stdout: String::new(),
            stderr: String::new(),
            exit_code: -1,
            timed_out: true,
        }),
    }
}

/// Map declared tool names onto the dotted paths a script will use.
///
/// `read_file` becomes `tools.read_file`; `mcp__linear__create_issue`
/// becomes `tools.linear.create_issue`. Building this in Rust rather
/// than guessing at runtime means an unknown attribute fails immediately
/// with a list of what does exist.
fn namespace_map(allowlist: &HashSet<String>) -> (Vec<(String, String)>, HashMap<String, Vec<(String, String)>>) {
    let mut flat: Vec<(String, String)> = Vec::new();
    let mut nested: HashMap<String, Vec<(String, String)>> = HashMap::new();

    let mut names: Vec<&String> = allowlist.iter().collect();
    names.sort();

    for name in names {
        if let Some(rest) = name.strip_prefix(crate::mcp::MCP_TOOL_PREFIX) {
            if let Some((server, tool)) = rest.split_once("__") {
                nested
                    .entry(server.to_string())
                    .or_default()
                    .push((tool.to_string(), name.clone()));
                continue;
            }
        }
        flat.push((name.clone(), name.clone()));
    }

    (flat, nested)
}

fn python_stub(allowlist: &HashSet<String>) -> String {
    let (flat, nested) = namespace_map(allowlist);
    let flat_json = serde_json::to_string(
        &flat
            .iter()
            .map(|(alias, name)| (alias.clone(), name.clone()))
            .collect::<HashMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());
    let nested_json = serde_json::to_string(
        &nested
            .iter()
            .map(|(server, tools)| {
                (
                    server.clone(),
                    tools.iter().cloned().collect::<HashMap<String, String>>(),
                )
            })
            .collect::<HashMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());

    format!(
        r#"# Bridge to OSA's tools. Generated per script run.
import json
import os
import socket

_PORT = int(os.environ["OSA_BRIDGE_PORT"])
_TOKEN = os.environ["OSA_BRIDGE_TOKEN"]
_FLAT = json.loads(r'''{flat}''')
_NESTED = json.loads(r'''{nested}''')


class ToolError(RuntimeError):
    """A tool call failed. Catch this to handle failures in-script."""


def call(tool, arguments=None):
    """Call a tool by its full name. Returns parsed JSON when the tool
    returns JSON, otherwise the raw string."""
    payload = json.dumps({{
        "token": _TOKEN,
        "tool": tool,
        "arguments": arguments or {{}},
    }}) + "\n"

    connection = socket.create_connection(("127.0.0.1", _PORT))
    try:
        connection.sendall(payload.encode("utf-8"))
        chunks = []
        while True:
            chunk = connection.recv(65536)
            if not chunk:
                break
            chunks.append(chunk)
            if chunks[-1].endswith(b"\n"):
                break
    finally:
        connection.close()

    response = json.loads(b"".join(chunks).decode("utf-8"))
    if not response.get("ok"):
        raise ToolError(response.get("error", "unknown tool error"))

    output = response.get("output", "")
    try:
        return json.loads(output)
    except (ValueError, TypeError):
        return output


class _Namespace(object):
    def __init__(self, name, mapping):
        self._name = name
        self._mapping = mapping

    def __getattr__(self, attribute):
        if attribute in self._mapping:
            target = self._mapping[attribute]
            if isinstance(target, dict):
                return _Namespace(attribute, target)
            return lambda **kwargs: call(target, kwargs)
        raise AttributeError(
            "'%s' is not available. Declared tools: %s"
            % (attribute, ", ".join(sorted(self._mapping)))
        )

    def __dir__(self):
        return sorted(self._mapping)


_ROOT = dict(_FLAT)
_ROOT.update(_NESTED)
tools = _Namespace("tools", _ROOT)
"#,
        flat = flat_json,
        nested = nested_json
    )
}

fn node_stub(allowlist: &HashSet<String>) -> String {
    let (flat, nested) = namespace_map(allowlist);
    let flat_json = serde_json::to_string(
        &flat
            .iter()
            .map(|(alias, name)| (alias.clone(), name.clone()))
            .collect::<HashMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());
    let nested_json = serde_json::to_string(
        &nested
            .iter()
            .map(|(server, tools)| {
                (
                    server.clone(),
                    tools.iter().cloned().collect::<HashMap<String, String>>(),
                )
            })
            .collect::<HashMap<_, _>>(),
    )
    .unwrap_or_else(|_| "{}".to_string());

    format!(
        r#"// Bridge to OSA's tools. Generated per script run.
const net = require('net');

const PORT = parseInt(process.env.OSA_BRIDGE_PORT, 10);
const TOKEN = process.env.OSA_BRIDGE_TOKEN;
const FLAT = {flat};
const NESTED = {nested};

class ToolError extends Error {{}}

function call(tool, args) {{
  return new Promise((resolve, reject) => {{
    const socket = net.createConnection({{ port: PORT, host: '127.0.0.1' }});
    let buffer = '';
    socket.on('connect', () => {{
      socket.write(JSON.stringify({{ token: TOKEN, tool, arguments: args || {{}} }}) + '\n');
    }});
    socket.on('data', (chunk) => {{
      buffer += chunk.toString('utf8');
      if (buffer.includes('\n')) socket.end();
    }});
    socket.on('error', reject);
    socket.on('close', () => {{
      let response;
      try {{
        response = JSON.parse(buffer);
      }} catch (error) {{
        reject(new ToolError('Malformed bridge response: ' + buffer));
        return;
      }}
      if (!response.ok) {{
        reject(new ToolError(response.error || 'unknown tool error'));
        return;
      }}
      try {{
        resolve(JSON.parse(response.output));
      }} catch (error) {{
        resolve(response.output);
      }}
    }});
  }});
}}

function build(mapping) {{
  const namespace = {{}};
  for (const [key, target] of Object.entries(mapping)) {{
    if (typeof target === 'string') {{
      namespace[key] = (args) => call(target, args);
    }} else {{
      namespace[key] = build(target);
    }}
  }}
  return namespace;
}}

const tools = build(Object.assign({{}}, FLAT, NESTED));

module.exports = {{ call, tools, ToolError }};
"#,
        flat = flat_json,
        nested = nested_json
    )
}

fn truncate_output(output: &str) -> String {
    if output.len() <= MAX_OUTPUT_BYTES {
        return output.to_string();
    }
    let mut end = MAX_OUTPUT_BYTES;
    while end > 0 && !output.is_char_boundary(end) {
        end -= 1;
    }
    format!(
        "{}\n\n[output truncated at {} bytes — have the script print less]",
        &output[..end],
        MAX_OUTPUT_BYTES
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    fn allowlist(names: &[&str]) -> HashSet<String> {
        names.iter().map(|name| name.to_string()).collect()
    }

    #[test]
    fn mcp_tools_become_nested_namespaces() {
        let (flat, nested) = namespace_map(&allowlist(&[
            "read_file",
            "mcp__linear__create_issue",
            "mcp__linear__list_issues",
        ]));

        assert_eq!(flat, vec![("read_file".to_string(), "read_file".to_string())]);
        let linear = nested.get("linear").expect("linear namespace");
        assert_eq!(linear.len(), 2);
        assert!(linear
            .iter()
            .any(|(alias, name)| alias == "create_issue" && name == "mcp__linear__create_issue"));
    }

    #[test]
    fn generated_python_stub_embeds_only_declared_tools() {
        let stub = python_stub(&allowlist(&["read_file", "mcp__linear__create_issue"]));
        assert!(stub.contains("read_file"));
        assert!(stub.contains("create_issue"));
        assert!(!stub.contains("delete_file"));
    }

    #[test]
    fn generated_node_stub_is_syntactically_plausible() {
        let stub = node_stub(&allowlist(&["mcp__slack__post_message"]));
        assert!(stub.contains("module.exports"));
        assert!(stub.contains("post_message"));
        assert!(stub.contains("OSA_BRIDGE_TOKEN"));
    }

    #[test]
    fn output_truncation_respects_char_boundaries() {
        let long = "é".repeat(MAX_OUTPUT_BYTES);
        let truncated = truncate_output(&long);
        assert!(truncated.contains("[output truncated"));
    }

    #[test]
    fn short_output_is_untouched() {
        assert_eq!(truncate_output("hello"), "hello");
    }

    #[test]
    fn control_plane_tools_are_blocked_by_name() {
        for tool in ["tool_script", "question", "subagent"] {
            assert!(BRIDGE_BLOCKED_TOOLS.contains(&tool));
        }
    }
}
