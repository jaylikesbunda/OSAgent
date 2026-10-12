//! Queued messages: CRUD and manual dispatch
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn enqueue_message(
        &self,
        session_id: &str,
        client_message_id: &str,
        content: &str,
        images: &[MessageImage],
        attachment_context: Option<&str>,
        attachments: &[MessageAttachment],
    ) -> Result<(QueuedMessage, bool)> {
        if self.get_session(session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }
        self.storage.enqueue_message(
            session_id,
            client_message_id,
            content,
            images,
            attachment_context,
            attachments,
        )
    }

    pub async fn list_queued_messages(&self, session_id: &str) -> Result<Vec<QueuedMessage>> {
        self.storage.list_queued_messages(session_id)
    }

    /// "Send now": interrupt whatever the session is doing and run a specific
    /// queued message next. The current run is cancelled the same way a Stop
    /// click cancels it; the chosen message is promoted to the front of the
    /// queue, so whichever path ends the current run dispatches it next. When
    /// the session is idle the queue is dispatched immediately.
    pub async fn send_queued_message_now(
        &self,
        session_id: &str,
        queue_entry_id: &str,
        user: &str,
    ) -> Result<bool> {
        if self.get_session(session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }
        let promoted = self
            .storage
            .promote_queued_message(session_id, queue_entry_id)?;
        if !promoted {
            return Ok(false);
        }
        self.resume_session_queue(session_id);

        if self.is_session_busy(session_id) {
            // Stop the active run (and its subagents) the same way a Stop click
            // does; whichever path ends the current run re-dispatches the queue
            // once the run guard drops, which will claim the promoted message.
            self.cancel_session(session_id);
            self.cancel_subagents_for_parent(session_id).await;
        } else {
            let runtime = self.self_arc.get().and_then(|weak| weak.upgrade());
            if let Some(runtime) = runtime {
                let session_id = session_id.to_string();
                let user = user.to_string();
                tokio::spawn(async move {
                    runtime.dispatch_queued_messages(&session_id, &user);
                });
            }
        }
        Ok(true)
    }

    /// Cancel (remove) a queued message. Returns false when the row does not
    /// exist or belongs to another session.
    pub async fn cancel_queued_message(
        &self,
        session_id: &str,
        queue_entry_id: &str,
    ) -> Result<bool> {
        if self.get_session(session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }
        self.storage
            .delete_session_queued_message(session_id, queue_entry_id)
    }

    /// Replace the text of a pending queued message, keeping its id, position
    /// and attachments. Returns false when the row is missing, belongs to
    /// another session, or is already dispatching.
    pub async fn edit_queued_message(
        &self,
        session_id: &str,
        queue_entry_id: &str,
        content: &str,
    ) -> Result<bool> {
        if self.get_session(session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }
        if content.trim().is_empty() {
            return Err(OSAgentError::Session(
                "Queued message content cannot be empty".to_string(),
            ));
        }
        self.storage
            .update_session_queued_message_content(session_id, queue_entry_id, content)
    }

    /// Reorder pending queued messages to match the given id sequence.
    pub async fn reorder_queued_messages(&self, session_id: &str, ids: &[String]) -> Result<()> {
        if self.get_session(session_id).await?.is_none() {
            return Err(OSAgentError::Session("Session not found".to_string()));
        }
        self.storage
            .reorder_session_queued_messages(session_id, ids)
    }
}
