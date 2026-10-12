//! Subagent facades (see SubagentManager)
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn get_subagent_status(
        &self,
        session_id: &str,
    ) -> Result<(Session, crate::storage::SubagentTask, bool)> {
        let session = self
            .storage
            .get_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;

        let task = self
            .storage
            .get_subagent_task_by_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Subagent task not found".to_string()))?;

        let is_running = self.subagent_manager.is_subagent_running(session_id);

        Ok((session, task, is_running))
    }

    pub async fn get_subagent_result(
        &self,
        session_id: &str,
    ) -> Result<crate::storage::SubagentTask> {
        self.storage
            .get_subagent_task_by_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Subagent task not found".to_string()))
    }

    pub async fn cancel_subagent(&self, session_id: &str) -> Result<bool> {
        self.subagent_manager.cancel_subagent(session_id).await
    }

    /// Resume a child session from the web UI or another control surface.
    /// The child keeps its complete message history; only the continuation
    /// prompt is appended and a new task record is created for the run.
    pub async fn resume_subagent(
        &self,
        session_id: &str,
        prompt: Option<String>,
        background: bool,
    ) -> Result<String> {
        let session = self
            .storage
            .get_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Subagent session not found".to_string()))?;
        let parent_session_id = session.parent_id.clone().ok_or_else(|| {
            OSAgentError::Session(
                "Session is not a subagent and cannot be resumed here".to_string(),
            )
        })?;
        let task = self
            .storage
            .get_subagent_task_by_session(session_id)?
            .ok_or_else(|| OSAgentError::Session("Subagent task not found".to_string()))?;
        let continuation = prompt
            .filter(|value| !value.trim().is_empty())
            .unwrap_or_else(|| {
                format!(
                    "Continue the previous task from the existing session history. Original task:\n{}",
                    task.prompt
                )
            });

        self.subagent_manager
            .resume_subagent(
                parent_session_id,
                session_id.to_string(),
                task.description,
                continuation,
                session.agent_type,
                background,
            )
            .await
    }

    pub async fn cleanup_completed_subagents(&self, days: i64) -> Result<usize> {
        self.subagent_manager.cleanup_completed(days).await
    }

    pub async fn list_subagent_tasks(
        &self,
        parent_session_id: &str,
    ) -> Result<Vec<crate::storage::SubagentTask>> {
        self.storage.list_subagent_tasks(parent_session_id)
    }

    pub fn is_any_subagent_running(&self, parent_session_id: &str) -> bool {
        self.subagent_manager
            .is_any_running_for_parent(parent_session_id)
    }
}
