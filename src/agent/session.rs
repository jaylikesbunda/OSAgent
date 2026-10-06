use crate::error::Result;
use crate::storage::{Session, SessionSummary, SqliteStorage};

#[derive(Clone)]
pub struct SessionManager {
    storage: SqliteStorage,
}

impl SessionManager {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }

    pub async fn create_session(
        &self,
        model: String,
        provider: String,
        name: Option<String>,
    ) -> Result<Session> {
        self.storage.create_session(model, provider, name)
    }

    pub async fn get_session_count(&self) -> Result<i64> {
        self.storage.get_session_count()
    }

    pub async fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.storage.get_session(id)
    }

    pub async fn update_session(&self, session: &Session) -> Result<()> {
        self.storage.update_session(session)
    }

    pub async fn update_session_title(&self, id: &str, title: &str) -> Result<()> {
        self.storage.update_session_title(id, title)
    }

    pub async fn update_session_title_if_untitled(&self, id: &str, title: &str) -> Result<()> {
        self.storage.update_session_title_if_untitled(id, title)
    }

    pub async fn set_session_metadata_value(
        &self,
        id: &str,
        key: &str,
        value: serde_json::Value,
    ) -> Result<()> {
        self.storage.set_session_metadata_value(id, key, value)
    }

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.storage.list_sessions()
    }

    pub async fn list_session_summaries(&self) -> Result<Vec<SessionSummary>> {
        self.storage.list_session_summaries()
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        self.storage.delete_session(id)
    }

    pub async fn delete_all_sessions(&self) -> Result<()> {
        self.storage.delete_all_sessions()
    }
}
