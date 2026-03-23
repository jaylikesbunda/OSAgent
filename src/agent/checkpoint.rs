use crate::error::Result;
use crate::storage::Session;
use crate::storage::{Checkpoint, SqliteStorage};

pub struct CheckpointManager {
    storage: SqliteStorage,
}

impl CheckpointManager {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }

    pub async fn create_checkpoint(
        &self,
        session: &Session,
        tool_name: Option<String>,
        tool_input: Option<String>,
    ) -> Result<Checkpoint> {
        let state = serde_json::to_vec(&session)
            .map_err(|e| crate::error::OSAgentError::Parse(e.to_string()))?;

        self.storage
            .create_checkpoint(&session.id, state, tool_name, tool_input)
    }

    #[allow(dead_code)]
    pub async fn get_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        self.storage.get_checkpoint(id)
    }

    pub async fn list_checkpoints(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        self.storage.list_checkpoints(session_id)
    }

    pub async fn rollback(&self, checkpoint_id: &str) -> Result<Session> {
        let checkpoint = self.storage.get_checkpoint(checkpoint_id)?.ok_or_else(|| {
            crate::error::OSAgentError::Session("Checkpoint not found".to_string())
        })?;

        let session: Session = serde_json::from_slice(&checkpoint.state)
            .map_err(|e| crate::error::OSAgentError::Parse(e.to_string()))?;

        self.storage.update_session(&session)?;

        Ok(session)
    }
}
