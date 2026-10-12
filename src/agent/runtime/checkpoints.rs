//! Checkpoint/rollback facades (see CheckpointManager)
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn list_checkpoints(
        &self,
        session_id: &str,
    ) -> Result<Vec<crate::storage::Checkpoint>> {
        self.checkpoint_manager.list_checkpoints(session_id).await
    }

    pub async fn create_checkpoint(&self, session_id: &str) -> Result<crate::storage::Checkpoint> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;
        let cfg = self.config.read().await;
        let workspace = Self::resolve_workspace_for_session(&session, &cfg)
            .unwrap_or_else(|| cfg.get_active_workspace());
        drop(cfg);

        self.checkpoint_manager
            .create_checkpoint(
                &session,
                Some("manual".to_string()),
                None,
                Some(workspace.resolved_path()),
            )
            .await
    }

    pub async fn get_checkpoint_diffs(
        &self,
        checkpoint_id: &str,
    ) -> Result<Vec<crate::storage::CheckpointDiff>> {
        self.checkpoint_manager
            .checkpoint_diffs(checkpoint_id)
            .await
    }

    pub async fn list_checkpoint_changed_files(
        &self,
        checkpoint_id: &str,
    ) -> Result<Vec<std::path::PathBuf>> {
        self.checkpoint_manager
            .list_changed_files(checkpoint_id)
            .await
    }

    pub async fn compute_checkpoint_diff(
        &self,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
    ) -> Result<String> {
        self.checkpoint_manager
            .compute_diff(from_checkpoint_id, to_checkpoint_id)
            .await
    }

    pub async fn rollback(&self, checkpoint_id: &str) -> Result<Session> {
        self.checkpoint_manager.rollback(checkpoint_id).await
    }
}
