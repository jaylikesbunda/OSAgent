use crate::error::{OSAgentError, Result};
use crate::storage::{ArchivedMessage, SessionSearchHit, SqliteStorage};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use chrono::{DateTime, Utc};
use serde_json::{json, Value};
use std::sync::Arc;

/// Per-message content cap before a line is cut short.
const MAX_MESSAGE_CHARS: usize = 500;
/// Total output target for one `read` call. The global tool-result
/// truncation layer is the backstop; this keeps results useful instead of
/// clipped mid-sentence.
const READ_BUDGET_CHARS: usize = 12_000;
/// Total output target for one `search` call.
const SEARCH_BUDGET_CHARS: usize = 4_000;
/// Default / maximum page sizes per action.
const READ_DEFAULT_LIMIT: usize = 40;
const READ_MAX_LIMIT: usize = 200;
const SEARCH_DEFAULT_LIMIT: usize = 10;
const SEARCH_MAX_LIMIT: usize = 50;
const LIST_DEFAULT_LIMIT: usize = 25;
const LIST_MAX_LIMIT: usize = 50;

pub struct SessionsTool {
    storage: Arc<SqliteStorage>,
}

impl SessionsTool {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self { storage }
    }

    fn session_name(metadata: &Value) -> Option<String> {
        metadata
            .get("name")
            .and_then(|value| value.as_str())
            .map(str::to_string)
            .filter(|name| !name.trim().is_empty())
    }

    fn short_id(id: &str) -> String {
        id.chars().take(8).collect()
    }

    fn format_timestamp(timestamp: DateTime<Utc>) -> String {
        timestamp.format("%Y-%m-%d %H:%M UTC").to_string()
    }

    /// Cut long message bodies down to one display line budget.
    fn clip(text: &str) -> String {
        if text.chars().count() <= MAX_MESSAGE_CHARS {
            return text.to_string();
        }
        let cut: String = text.chars().take(MAX_MESSAGE_CHARS).collect();
        format!("{} …[truncated]", cut)
    }
}

/// What the runtime permission gate needs to know about a `sessions` call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionAccessScope {
    /// No conversation content is involved (`list`), or the call is malformed
    /// and will fail validation anyway — no prompt needed.
    Ungated,
    /// The caller's own session. Always allowed.
    Exempt,
    /// Another conversation: gate this resource string
    /// (`session://<id>`, or `session://*` for broad search).
    Resource(String),
}

/// Pure mapping from tool arguments to an access scope, shared by the
/// runtime gate so both sides agree on what triggers a prompt.
pub fn resolve_access_scope(
    caller_session_id: &str,
    action: &str,
    target_session_id: Option<&str>,
) -> SessionAccessScope {
    match action {
        "list" => SessionAccessScope::Ungated,
        "search" => SessionAccessScope::Resource("session://*".to_string()),
        _ => match target_session_id {
            Some(target) if target == caller_session_id => SessionAccessScope::Exempt,
            Some(target) => SessionAccessScope::Resource(format!("session://{}", target)),
            None => SessionAccessScope::Ungated,
        },
    }
}

/// One renderable entry in a merged live+archived timeline.
struct TimelineEntry {
    timestamp: DateTime<Utc>,
    role: String,
    content: String,
    archived: bool,
}

fn format_timeline_line(entry: &TimelineEntry) -> String {
    let marker = if entry.archived {
        "[archived] "
    } else {
        ""
    };
    format!(
        "[{}] {}{}: {}",
        SessionsTool::format_timestamp(entry.timestamp),
        marker,
        entry.role,
        SessionsTool::clip(&entry.content),
    )
}

#[async_trait]
impl Tool for SessionsTool {
    fn name(&self) -> &str {
        "sessions"
    }

    fn description(&self) -> &str {
        "List, read, and search your other conversations (sessions), including \
content that was compacted out of context. Read-only."
    }

    fn when_to_use(&self) -> &str {
        "Use when the user refers to a past conversation ('what did we decide \
about X yesterday?') or you need context from another session. Search first \
to locate content, then read a specific window"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for the current session (you already have its history), \
for files on disk, or to modify other sessions — this tool cannot write"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "List recent conversations".to_string(),
                input: json!({ "action": "list" }),
            },
            ToolExample {
                description: "Read the tail of another conversation".to_string(),
                input: json!({
                    "action": "read",
                    "target_session_id": "a1b2c3d4-1234-5678-90ab-cdef12345678"
                }),
            },
            ToolExample {
                description: "Search across all conversations".to_string(),
                input: json!({
                    "action": "search",
                    "query": "deploy checklist",
                    "limit": 5
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["list", "read", "search"],
                    "description": "list: show recent conversations (metadata only). read: messages of one conversation. search: full-text across all conversations, including compacted content."
                },
                "target_session_id": {
                    "type": "string",
                    "description": "Session id from `list` (for action=read). Reading your own session or your own subagent sessions needs no approval; others trigger a permission prompt unless pre-approved."
                },
                "query": {
                    "type": "string",
                    "description": "Search text (for action=search)"
                },
                "limit": {
                    "type": "integer",
                    "description": "Max items returned. Defaults: list 25, read 40 messages, search 10 hits."
                },
                "offset_from_end": {
                    "type": "integer",
                    "description": "For action=read: skip this many messages before the returned window, counting back from the newest (0 = most recent)."
                },
                "role_filter": {
                    "type": "string",
                    "enum": ["user", "assistant", "tool"],
                    "description": "For action=read: only return messages with this role."
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let action = args["action"].as_str().unwrap_or("read");

        match action {
            "list" => self.run_list(args),
            "read" => self.run_read(args).await,
            "search" => self.run_search(args).await,
            other => Err(OSAgentError::ToolExecution(format!(
                "Unknown action '{}'. Expected list, read, or search.",
                other
            ))),
        }
    }
}

impl SessionsTool {
    fn run_list(&self, args: Value) -> Result<String> {
        let limit = args["limit"]
            .as_u64()
            .map(|value| value.min(LIST_MAX_LIMIT as u64) as usize)
            .unwrap_or(LIST_DEFAULT_LIMIT);

        let mut summaries = self.storage.list_session_summaries()?;
        summaries.sort_by(|a, b| b.updated_at.cmp(&a.updated_at));
        summaries.truncate(limit);

        if summaries.is_empty() {
            return Ok("No sessions found.".to_string());
        }

        let total = self.storage.get_session_count()?;
        let mut lines = vec![format!(
            "{} conversations (showing newest {}):",
            total,
            summaries.len()
        )];
        for summary in summaries {
            let name = Self::session_name(&summary.metadata)
                .map(|name| format!("\"{}\" ", name))
                .unwrap_or_default();
            let kind = if summary.agent_type == "primary" {
                String::new()
            } else {
                format!(" type={}", summary.agent_type)
            };
            lines.push(format!(
                "- {} {}model={}{} status={} updated={}",
                Self::short_id(&summary.id),
                name,
                summary.model,
                kind,
                summary.task_status,
                Self::format_timestamp(summary.updated_at),
            ));
        }
        lines.push(format!(
            "Pass a full session id from `/api/sessions` output or Settings → \
Sessions; ids shown here are shortened prefixes. Use action=read with \
target_session_id to inspect one."
        ));
        Ok(lines.join("\n"))
    }

    async fn run_read(&self, args: Value) -> Result<String> {
        let Some(target) = args["target_session_id"].as_str() else {
            return Err(OSAgentError::ToolExecution(
                "action=read requires target_session_id (get ids from action=list)".to_string(),
            ));
        };

        let limit = args["limit"]
            .as_u64()
            .map(|value| value.clamp(1, READ_MAX_LIMIT as u64) as usize)
            .unwrap_or(READ_DEFAULT_LIMIT);
        let offset_from_end = args["offset_from_end"].as_u64().unwrap_or(0) as usize;
        let role_filter = args["role_filter"].as_str();

        // Resolve a shortened id prefix against stored sessions so callers
        // can reuse the ids `list` prints.
        let resolved = self.resolve_session_ref(target)?;
        let Some(session) = self.storage.get_session(&resolved)? else {
            return Err(OSAgentError::ToolExecution(format!(
                "Session '{}' not found. Use action=list to see available conversations.",
                target
            )));
        };

        // Live transcript plus everything earlier compactions removed,
        // merged into one chronological timeline.
        let archived = self.storage.get_archived_messages(&resolved, 1_000)?;
        let mut timeline: Vec<TimelineEntry> = session
            .messages
            .iter()
            .filter(|message| !matches!(message.role.as_str(), "system"))
            .map(|message| TimelineEntry {
                timestamp: message.timestamp,
                role: message.role.clone(),
                content: message.content.clone(),
                archived: false,
            })
            .collect();
        for ArchivedMessage {
            role,
            content,
            timestamp,
            ..
        } in archived
        {
            timeline.push(TimelineEntry {
                timestamp,
                role,
                content,
                archived: true,
            });
        }
        timeline.sort_by(|a, b| a.timestamp.cmp(&b.timestamp));

        let filtered_total = timeline.len();
        if let Some(role) = role_filter {
            timeline.retain(|entry| entry.role == role);
        }

        let window_end = timeline.len().saturating_sub(offset_from_end);
        let window_start = window_end.saturating_sub(limit);

        if window_start >= window_end && offset_from_end > 0 {
            return Ok(format!(
                "Nothing to show: {}/{} matching messages lie before the requested \
offset_from_end={} (lower it to see more).",
                timeline.len(),
                filtered_total,
                offset_from_end
            ));
        }
        if timeline.is_empty() {
            return Ok(format!(
                "No matching messages in session '{}'.{}",
                Self::short_id(&resolved),
                role_filter
                    .map(|role| format!(" (role filter: {})", role))
                    .unwrap_or_default(),
            ));
        }

        let name = Self::session_name(&session.metadata)
            .map(|name| format!(" \"{}\"", name))
            .unwrap_or_default();
        let mut lines = vec![format!(
            "Conversation {}{} — messages {}..{} of {}:{}",
            Self::short_id(&resolved),
            name,
            window_start,
            window_end,
            filtered_total,
            role_filter
                .map(|role| format!(" (role filter: {})", role))
                .unwrap_or_default(),
        )];

        let mut used = 0usize;
        let mut omitted = 0usize;
        for entry in &timeline[window_start..window_end] {
            let line = format_timeline_line(entry);
            if used + line.len() > READ_BUDGET_CHARS {
                omitted = window_end - window_start - lines.len() + 1;
                break;
            }
            used += line.len();
            lines.push(line);
        }
        if omitted > 0 || window_end < filtered_total || offset_from_end > 0 {
            lines.push(format!(
                "… [windowed view: use offset_from_end/limit/role_filter to page through]"
            ));
        }

        Ok(lines.join("\n"))
    }

    async fn run_search(&self, args: Value) -> Result<String> {
        let Some(query) = args["query"].as_str().map(str::trim).filter(|q| !q.is_empty())
        else {
            return Err(OSAgentError::ToolExecution(
                "action=search requires query".to_string(),
            ));
        };
        let limit = args["limit"]
            .as_u64()
            .map(|value| value.clamp(1, SEARCH_MAX_LIMIT as u64) as usize)
            .unwrap_or(SEARCH_DEFAULT_LIMIT);

        // Half the budget goes to live transcripts, half to the compaction
        // archive, then the merged list is trimmed to `limit`.
        let per_source = limit.div_ceil(2).max(1);
        let mut hits: Vec<SessionSearchHit> = self.storage.search_messages(query, per_source)?;
        hits.extend(self.storage.search_archived_messages(query, per_source)?);
        hits.truncate(limit);

        if hits.is_empty() {
            return Ok(format!("No matches for \"{}\".", query));
        }

        let mut lines = vec![format!("Matches for \"{}\":", query)];
        let mut used = 0usize;
        for hit in hits {
            let archive_note = if hit.archived { " [archived]" } else { "" };
            let line = format!(
                "- {} seq={} [{}]{} {}",
                Self::short_id(&hit.session_id),
                hit.seq,
                hit.role,
                archive_note,
                SessionsTool::clip(&hit.snippet),
            );
            if used + line.len() > SEARCH_BUDGET_CHARS {
                break;
            }
            used += line.len();
            lines.push(line);
        }
        lines.push(
            "Use action=read with the session id (full id needed) to see the \
surrounding conversation."
                .to_string(),
        );
        Ok(lines.join("\n"))
    }

    /// Accepts a full session id or an unambiguous prefix printed by
    /// `action=list`. Ambiguous or unknown references surface as errors.
    fn resolve_session_ref(&self, reference: &str) -> Result<String> {
        let reference = reference.trim();
        if reference.is_empty() {
            return Ok(String::new());
        }
        if reference.len() >= 36 || !reference.chars().all(|c| c.is_ascii_hexdigit() || c == '-') {
            return Ok(reference.to_string());
        }

        let matches: Vec<String> = self
            .storage
            .list_sessions()?
            .into_iter()
            .map(|session| session.id)
            .filter(|id| id.starts_with(reference))
            .collect();
        match matches.len() {
            1 => Ok(matches.into_iter().next().expect("one match")),
            0 => Ok(reference.to_string()),
            _ => Err(OSAgentError::ToolExecution(format!(
                "'{}' matches {} sessions; pass a longer prefix.",
                reference,
                matches.len()
            ))),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn msg(role: &str, content: &str, minutes_ago: i64) -> crate::storage::Message {
        crate::storage::Message {
            role: role.to_string(),
            content: content.to_string(),
            thinking: None,
            timestamp: Utc::now() - chrono::Duration::minutes(minutes_ago),
            tool_calls: None,
            tool_call_id: None,
            metadata: serde_json::json!({}),
            tokens: None,
            images: Vec::new(),
        }
    }

    fn storage() -> Arc<SqliteStorage> {
        Arc::new(SqliteStorage::new_in_memory().expect("in-memory storage"))
    }

    #[test]
    fn scope_resolution_matches_plan_matrix() {
        let caller = "11111111-1111-1111-1111-111111111111";
        assert_eq!(
            resolve_access_scope(caller, "list", None),
            SessionAccessScope::Ungated
        );
        assert_eq!(
            resolve_access_scope(caller, "search", None),
            SessionAccessScope::Resource("session://*".to_string())
        );
        assert_eq!(
            resolve_access_scope(caller, "read", Some(caller)),
            SessionAccessScope::Exempt
        );
        assert_eq!(
            resolve_access_scope(caller, "read", Some("22222222-2222-2222-2222-222222222222")),
            SessionAccessScope::Resource("session://22222222-2222-2222-2222-222222222222".to_string())
        );
        assert_eq!(
            resolve_access_scope(caller, "read", None),
            SessionAccessScope::Ungated
        );
    }

    #[test]
    fn clips_long_content() {
        let long = "x".repeat(MAX_MESSAGE_CHARS + 100);
        let clipped = SessionsTool::clip(&long);
        assert!(clipped.ends_with("…[truncated]"));
        assert!(clipped.chars().count() < long.chars().count());
        assert_eq!(SessionsTool::clip("short"), "short");
    }

    #[tokio::test]
    async fn list_reports_recent_conversations() {
        let storage = storage();
        storage
            .create_session("m".to_string(), "p".to_string(), Some("Named".to_string()))
            .expect("create");
        let tool = SessionsTool::new(storage);
        let output = tool.execute(json!({"action": "list"})).await.unwrap();
        assert!(output.contains("Named"));
        assert!(output.contains("status=active"));
    }

    #[tokio::test]
    async fn read_requires_target_and_merges_archived_history() {
        let storage = storage();
        let mut session = storage
            .create_session("m".to_string(), "p".to_string(), None)
            .expect("create");
        let old_user = "the launch code is 4815";
        session.messages.push(msg("user", old_user, 120));
        session.messages.push(msg("assistant", "acknowledged", 119));
        storage.update_session(&session).expect("persist");

        // Simulate compaction archiving the first message away, then shrink
        // the live transcript like update_session would after compaction.
        let archived_count = storage
            .archive_messages(&session.id, &session.messages[..1])
            .expect("archive");
        assert_eq!(archived_count, 1);
        session.messages.remove(0);
        storage.update_session(&session).expect("shrink");

        let tool = SessionsTool::new(storage.clone());

        // Missing target is a clean validation error, not a panic.
        let missing = tool.execute(json!({"action": "read"})).await;
        assert!(missing.is_err());

        let output = tool
            .execute(json!({
                "action": "read",
                "target_session_id": &session.id,
                "role_filter": "assistant"
            }))
            .await
            .unwrap();
        assert!(output.contains("acknowledged"));
        assert!(!output.contains(old_user));

        let output = tool
            .execute(json!({"action": "read", "target_session_id": &session.id}))
            .await
            .unwrap();
        assert!(output.contains("[archived] user"));
        assert!(output.contains("launch code"));

        // Search finds the archived snippet and labels it.
        let hits = storage.search_messages("launch code", 5).expect("live search");
        assert!(hits.is_empty(), "compacted rows leave the live transcript");
        let output = tool
            .execute(json!({"action": "search", "query": "launch code"}))
            .await
            .unwrap();
        assert!(output.contains("[archived]"));
        assert!(output.contains("launch code"));
    }

    #[test]
    fn archive_is_idempotent_per_batch() {
        let storage = storage();
        let mut session = storage
            .create_session("m".to_string(), "p".to_string(), None)
            .expect("create");
        session.messages.push(msg("user", "hello there", 60));
        storage.update_session(&session).expect("persist");

        let first = storage
            .archive_messages(&session.id, &session.messages)
            .expect("first archive");
        let replay = storage
            .archive_messages(&session.id, &session.messages)
            .expect("replayed archive");
        assert_eq!(first, 1);
        assert_eq!(replay, 0, "same batch must not duplicate rows");
        assert_eq!(
            storage
                .get_archived_messages(&session.id, 10)
                .expect("read back")
                .len(),
            1
        );
    }
}
