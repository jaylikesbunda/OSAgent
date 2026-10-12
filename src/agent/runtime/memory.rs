//! Memory + decision-memory facades
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub async fn save_config(&self, config_path: &std::path::Path) -> Result<()> {
        let cfg = self.config.read().await.clone();
        cfg.save(config_path)
    }

    pub fn decision_memory_status(&self) -> crate::agent::decision_memory::DecisionMemoryStatus {
        self.decision_memory.status()
    }

    pub fn memory_status(&self) -> MemoryStatus {
        self.memory_store.status()
    }

    pub async fn list_memories(&self) -> Result<Vec<MemoryEntry>> {
        self.memory_store.list().await
    }

    pub async fn add_memory(
        &self,
        title: String,
        content: String,
        tags: Vec<String>,
        category: Option<MemoryCategory>,
        scope: MemoryScope,
        workspace_id: Option<String>,
        confirmed: bool,
        source: String,
    ) -> Result<MemoryEntry> {
        self.memory_store
            .add(
                title,
                content,
                tags,
                category,
                scope,
                workspace_id,
                confirmed,
                source,
            )
            .await
    }

    pub async fn update_memory(
        &self,
        id: &str,
        title: Option<String>,
        content: Option<String>,
        tags: Option<Vec<String>>,
        category: Option<MemoryCategory>,
        scope: Option<MemoryScope>,
        workspace_id: Option<String>,
        confirmed: Option<bool>,
    ) -> Result<MemoryEntry> {
        self.memory_store
            .update(
                id,
                title,
                content,
                tags,
                category,
                scope,
                workspace_id,
                confirmed,
            )
            .await
    }

    pub async fn delete_memory(&self, id: &str) -> Result<bool> {
        self.memory_store.delete(id).await
    }

    pub async fn list_memory_suggestions(&self) -> Result<Vec<MemorySuggestion>> {
        self.memory_store.list_suggestions().await
    }

    pub async fn approve_memory_suggestion(&self, id: &str, actor: String) -> Result<MemoryEntry> {
        self.memory_store.approve_suggestion(id, actor).await
    }

    pub async fn reject_memory_suggestion(
        &self,
        id: &str,
        actor: String,
        note: Option<String>,
    ) -> Result<bool> {
        self.memory_store.reject_suggestion(id, actor, note).await
    }

    pub async fn list_decisions(
        &self,
    ) -> Result<Vec<crate::agent::decision_memory::DecisionEntry>> {
        self.decision_memory.list().await
    }

    pub async fn delete_decision(&self, id: &str) -> Result<bool> {
        self.decision_memory.delete(id, "user".to_string()).await
    }

    pub async fn list_decision_suggestions(&self) -> Result<Vec<DecisionSuggestion>> {
        self.decision_memory.list_suggestions().await
    }

    pub async fn approve_decision_suggestion(
        &self,
        id: &str,
        actor: String,
    ) -> Result<crate::agent::decision_memory::DecisionEntry> {
        self.decision_memory.approve_suggestion(id, actor).await
    }

    pub async fn reject_decision_suggestion(
        &self,
        id: &str,
        actor: String,
        note: Option<String>,
    ) -> Result<bool> {
        self.decision_memory
            .reject_suggestion(id, actor, note)
            .await
    }
}
