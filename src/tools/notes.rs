use crate::error::Result;
use crate::storage::models::WorkingNotes;
use crate::storage::SqliteStorage;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

pub struct UpdateNotesTool {
    storage: Arc<SqliteStorage>,
    max_chars: usize,
}

impl UpdateNotesTool {
    pub fn new(storage: Arc<SqliteStorage>, max_chars: usize) -> Self {
        Self { storage, max_chars }
    }
}

#[async_trait]
impl Tool for UpdateNotesTool {
    fn name(&self) -> &str {
        "update_notes"
    }

    fn description(&self) -> &str {
        "Update the session's rolling working-notes handoff (goal, decisions, files touched, what was tried, errors/fixes, next step). Keeps a fresh agent able to continue instantly after compaction."
    }

    fn when_to_use(&self) -> &str {
        "Use after each meaningful step to keep the handoff note current"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for trivial chatter with no task state to record"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "notes": {
                    "type": "string",
                    "description": "Full replacement text of the working notes (under 2000 chars)"
                }
            },
            "required": ["notes"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let session_id = args["session_id"].as_str().unwrap_or("default");
        let mut notes = args["notes"].as_str().unwrap_or("").trim().to_string();
        if notes.is_empty() {
            return Ok("Working notes unchanged (empty notes ignored).".to_string());
        }
        if notes.chars().count() > self.max_chars {
            notes = notes.chars().take(self.max_chars).collect();
        }
        let mut session = match self.storage.get_session(session_id)? {
            Some(session) => session,
            None => return Ok("Session not found; notes not saved.".to_string()),
        };
        let iteration = args["iteration"].as_u64().unwrap_or(0) as usize;
        let mut state = session.context_state.unwrap_or_default();
        state.working_notes = Some(WorkingNotes {
            text: notes,
            updated_at: Some(chrono::Utc::now()),
            updated_iteration: iteration,
            verified: false,
        });
        session.context_state = Some(state);
        self.storage.update_session(&session)?;
        Ok("Working notes updated.".to_string())
    }
}
