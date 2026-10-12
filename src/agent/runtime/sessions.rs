//! Session store facades: CRUD, search, history, todos, feedback, events
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.session_manager.get_session(id).await
    }

    pub async fn get_child_sessions(&self, parent_id: &str) -> Result<Vec<Session>> {
        self.storage.get_child_sessions(parent_id)
    }

    pub async fn get_parent_session(&self, session_id: &str) -> Result<Option<Session>> {
        let session = self.storage.get_session(session_id)?;
        if let Some(s) = session {
            if let Some(parent_id) = &s.parent_id {
                return self.storage.get_session(parent_id);
            }
        }
        Ok(None)
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.session_manager.list_sessions().await
    }

    pub async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>> {
        self.session_manager.list_session_summaries().await
    }

    pub async fn search_messages(
        &self,
        query: &str,
        limit: usize,
    ) -> Result<Vec<crate::storage::SessionSearchHit>> {
        self.storage.search_messages(query, limit)
    }

    /// Drops messages from `from` onward so the next turn re-runs from that
    /// point. Refuses while a turn is in flight, since the running turn holds
    /// its own copy of the transcript and would write it straight back.
    pub async fn truncate_session_messages(&self, session_id: &str, from: usize) -> Result<()> {
        if self.active_runs.contains_key(session_id) {
            return Err(OSAgentError::Session(
                "Cannot edit history while the agent is running. Stop it first.".to_string(),
            ));
        }

        let session_lock = self.get_session_lock(session_id);
        let _lock_guard = session_lock.lock().await;
        self.storage.truncate_session_messages(session_id, from)
    }

    pub async fn list_session_history(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::StoredSessionEvent>> {
        self.storage.list_session_events(session_id)
    }

    /// Pre-compaction messages archived by context compaction, oldest first.
    /// The web UI merges these back into the transcript so a compacted chat
    /// still looks the way it did, with the summary card at the boundary.
    pub fn get_session_archive(&self, session_id: &str) -> Result<Vec<Message>> {
        self.storage
            .get_archived_session_messages(session_id, 10_000)
    }

    pub async fn append_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<crate::storage::StoredSessionEvent> {
        self.storage
            .append_session_event(session_id, event_type, data)
    }

    pub async fn list_file_snapshots(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::FileSnapshotSummary>> {
        self.storage.list_file_snapshot_summaries(session_id)
    }

    pub async fn list_todo_items(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::tools::todo::TodoItem>> {
        self.storage.list_todo_items(session_id)
    }

    pub async fn list_message_feedback(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::MessageFeedback>> {
        self.storage.list_message_feedback(session_id)
    }

    pub async fn put_message_feedback(
        &self,
        session_id: &str,
        seq: i64,
        rating: &str,
        note: Option<&str>,
        if_version: Option<&str>,
    ) -> Result<crate::storage::FeedbackPutOutcome> {
        self.storage
            .put_message_feedback(session_id, seq, rating, note, if_version)
    }

    pub async fn delete_message_feedback(&self, session_id: &str, seq: i64) -> Result<bool> {
        self.storage.delete_message_feedback(session_id, seq)
    }

    pub async fn revert_file_snapshot(
        &self,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<crate::storage::FileSnapshotSummary> {
        let summary = self
            .revert_snapshot_for_session(session_id, snapshot_id)
            .await?;

        if let Some(mut session) = self.session_manager.get_session(session_id).await? {
            self.record_session_event(
                &mut session,
                "snapshot_revert",
                serde_json::json!({
                    "snapshot_id": snapshot_id,
                    "paths": summary.paths.clone(),
                }),
            )?;
            self.session_manager.update_session(&session).await?;
        }

        Ok(summary)
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        // Cancel any in-progress operation
        self.cancel_session(id);
        // Cancel any child subagents
        self.subagent_manager.cancel_all_for_parent(id).await;
        // Clean up session locks and cancellation notifiers
        self.session_locks.remove(id);
        self.session_cancellation.remove(id);
        // Drop the session's tool activation so its loaded tools do not
        // linger in memory or leak into future sessions.
        self.tool_registry.prune_session_tools(id);
        self.session_manager.delete_session(id).await
    }

    pub async fn update_session(&self, session: &Session) -> Result<()> {
        self.session_manager.update_session(session).await
    }

    /// Rename a session without rewriting its transcript or metadata blob, so
    /// renaming a chat mid-turn does not race the running turn's persistence.
    pub async fn update_session_title(&self, id: &str, title: &str) -> Result<()> {
        self.session_manager.update_session_title(id, title).await
    }

    /// Auto-naming variant: only applies when the session is still untitled,
    /// so a generated title cannot clobber a user's rename.
    pub async fn auto_name_session_title(&self, id: &str, title: &str) -> Result<()> {
        self.session_manager
            .update_session_title_if_untitled(id, title)
            .await
    }

    pub async fn set_session_metadata_value(
        &self,
        id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        self.session_manager
            .set_session_metadata_value(id, key, value)
            .await
    }

    pub async fn set_discord_community_profile(
        &self,
        session_id: &str,
        context: String,
    ) -> Result<()> {
        let mut session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session(format!("Session not found: {session_id}")))?;
        let metadata = session.metadata.as_object_mut().ok_or_else(|| {
            OSAgentError::Session("Session metadata is not an object".to_string())
        })?;
        metadata.insert(
            "discord_community".to_string(),
            serde_json::Value::Bool(true),
        );
        metadata.insert(
            "discord_community_context".to_string(),
            serde_json::Value::String(context),
        );
        self.session_manager.update_session(&session).await
    }

    pub async fn delete_all_sessions(&self) -> Result<()> {
        self.session_manager.delete_all_sessions().await
    }
}
