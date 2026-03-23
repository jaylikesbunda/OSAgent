use crate::error::Result;
use crate::storage::{Session, SqliteStorage};

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

    pub async fn list_sessions(&self) -> Result<Vec<Session>> {
        self.storage.list_sessions()
    }

    pub async fn delete_session(&self, id: &str) -> Result<()> {
        self.storage.delete_session(id)
    }

    pub async fn delete_all_sessions(&self) -> Result<()> {
        self.storage.delete_all_sessions()
    }
}
