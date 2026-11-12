//! `agents`: structured communication between agents in a conversation tree
//! (a chat and its subagents) and between Global chat and regular chats.
//!
//! The schema lives here; execution needs the live runtime (session tree,
//! run state, permissions), so the primary turn loop and the subagent loop
//! intercept calls before they reach [`Tool::execute`].

use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use serde_json::{json, Value};

#[derive(Default)]
pub struct AgentsTool;

#[async_trait]
impl Tool for AgentsTool {
    fn name(&self) -> &str {
        "agents"
    }

    fn description(&self) -> &str {
        "Communicate with other agents. list: agents in this conversation tree and their state. send: deliver a message; a running agent sees it before its next model request, an idle one on its next turn. Never starts a turn. followup: give an agent a new task; starts a turn if it is idle. start_chat (Global chat only): open a new regular chat in a workspace and give it a task; it appears in the sidebar and starts working. A subagent's final answer reaches its parent automatically. Agents cannot wait for replies: never poll or repeat a message."
    }

    fn when_to_use(&self) -> &str {
        "coordinating parallel subagents, giving a finished subagent more work, sharing a finding a parent or sibling needs now, or (from Global chat) starting a new chat or session for a task"
    }

    fn when_not_to_use(&self) -> &str {
        "repeating your final answer, acknowledgements, chatter, or polling for status"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "See your tree".into(),
                input: json!({"action": "list"}),
            },
            ToolExample {
                description: "Tell a running sibling about a finding".into(),
                input: json!({"action": "send", "target": "3f2a…", "message": "Auth tokens are created in src/auth/token.rs; skip that file."}),
            },
            ToolExample {
                description: "Give a finished subagent another task".into(),
                input: json!({"action": "followup", "target": "3f2a…", "message": "Also check token expiry and report the file and line."}),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {"type": "string", "enum": ["list", "send", "followup", "start_chat"]},
                "target": {
                    "type": "string",
                    "description": "Agent session id from list, or parent, root, or global. Global chat may name a regular chat by its full id."
                },
                "message": {"type": "string", "description": "Self-contained text for the recipient (for start_chat, the new chat's task)."},
                "workspace_id": {"type": "string", "description": "start_chat: configured workspace for the new chat; defaults to the active workspace."},
                "title": {"type": "string", "description": "start_chat: short sidebar title."},
                "report_back": {"type": "boolean", "description": "start_chat: ask the new chat to send its result back (default true)."}
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        Err(OSAgentError::ToolExecution(
            "The agents tool is available only inside an agent run.".into(),
        ))
    }
}
