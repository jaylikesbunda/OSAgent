use crate::error::{OSAgentError, Result};
use crate::storage::models::*;
use chrono::Utc;
use rusqlite::params;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

#[derive(Clone)]
pub struct SqliteStorage {
    conn: Arc<Mutex<rusqlite::Connection>>,
}

impl SqliteStorage {
    fn queued_message_from_row(row: &rusqlite::Row<'_>) -> rusqlite::Result<QueuedMessage> {
        let status: String = row.get(4)?;
        let dispatched_at_ts: Option<i64> = row.get(8)?;
        Ok(QueuedMessage {
            id: row.get(0)?,
            session_id: row.get(1)?,
            client_message_id: row.get(2)?,
            content: row.get(3)?,
            status: QueuedMessageStatus::from_str(&status),
            position: row.get(5)?,
            created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(6)?, 0)
                .unwrap_or_else(Utc::now),
            updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(7)?, 0)
                .unwrap_or_else(Utc::now),
            dispatched_at: dispatched_at_ts.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
        })
    }

    fn apply_pragmas(conn: &rusqlite::Connection) -> Result<()> {
        conn.execute_batch(
            "PRAGMA journal_mode = WAL;
             PRAGMA synchronous = NORMAL;
             PRAGMA busy_timeout = 5000;
             PRAGMA cache_size = -64000;
             PRAGMA temp_store = MEMORY;",
        )
        .map_err(OSAgentError::Storage)?;
        Ok(())
    }

    pub fn new(database_path: &str) -> Result<Self> {
        let path = PathBuf::from(shellexpand::tilde(database_path).to_string());
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|_| {
                OSAgentError::Storage(rusqlite::Error::InvalidPath(parent.to_path_buf()))
            })?;
        }

        let conn = rusqlite::Connection::open(&path).map_err(OSAgentError::Storage)?;
        Self::apply_pragmas(&conn)?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.run_migrations()?;
        Ok(storage)
    }

    pub fn new_in_memory() -> Result<Self> {
        let conn = rusqlite::Connection::open_in_memory().map_err(OSAgentError::Storage)?;
        Self::apply_pragmas(&conn)?;
        let storage = Self {
            conn: Arc::new(Mutex::new(conn)),
        };
        storage.run_migrations()?;
        Ok(storage)
    }

    fn with_conn<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        let guard = self
            .conn
            .lock()
            .map_err(|_| OSAgentError::Unknown("db mutex poisoned".to_string()))?;
        f(&guard)
    }

    fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        let mut guard = self
            .conn
            .lock()
            .map_err(|_| OSAgentError::Unknown("db mutex poisoned".to_string()))?;
        f(&mut guard)
    }

    fn run_migrations(&self) -> Result<()> {
        self.with_conn(|conn| {
            let table_exists: bool = conn
                .query_row(
                    "SELECT COUNT(*) > 0 FROM sqlite_master WHERE type='table' AND name='sessions'",
                    [],
                    |row| row.get(0),
                )
                .unwrap_or(false);

            if table_exists {
                let has_parent_id: bool = conn
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM PRAGMA_table_info('sessions') WHERE name='parent_id'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);

                if !has_parent_id {
                    conn.execute("ALTER TABLE sessions ADD COLUMN parent_id TEXT", [])?;
                    conn.execute("ALTER TABLE sessions ADD COLUMN agent_type TEXT NOT NULL DEFAULT 'primary'", [])?;
                    conn.execute("ALTER TABLE sessions ADD COLUMN task_status TEXT NOT NULL DEFAULT 'active'", [])?;
                    conn.execute("CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_id)", [])?;
                }

                let has_context_state: bool = conn
                    .query_row(
                        "SELECT COUNT(*) > 0 FROM PRAGMA_table_info('sessions') WHERE name='context_state'",
                        [],
                        |row| row.get(0),
                    )
                    .unwrap_or(false);

                if !has_context_state {
                    conn.execute("ALTER TABLE sessions ADD COLUMN context_state BLOB", [])?;
                }
            } else {
                conn.execute_batch(
                    r#"
                    CREATE TABLE sessions (
                        id TEXT PRIMARY KEY,
                        created_at INTEGER NOT NULL,
                        updated_at INTEGER NOT NULL,
                        model TEXT NOT NULL,
                        provider TEXT NOT NULL,
                        messages BLOB NOT NULL,
                        metadata BLOB,
                        parent_id TEXT REFERENCES sessions(id) ON DELETE CASCADE,
                        agent_type TEXT NOT NULL DEFAULT 'primary',
                        task_status TEXT NOT NULL DEFAULT 'active',
                        context_state BLOB
                    );
                    CREATE INDEX IF NOT EXISTS idx_sessions_parent ON sessions(parent_id);
                    "#,
                )?;
            }

            conn.execute_batch(
                r#"
                CREATE TABLE IF NOT EXISTS checkpoints (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    created_at INTEGER NOT NULL,
                    state BLOB NOT NULL,
                    tool_name TEXT,
                    tool_input TEXT,
                    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS audit_log (
                    id TEXT PRIMARY KEY,
                    timestamp INTEGER NOT NULL,
                    session_id TEXT NOT NULL,
                    tool TEXT NOT NULL,
                    input TEXT NOT NULL,
                    output TEXT NOT NULL,
                    duration_ms INTEGER NOT NULL,
                    user TEXT NOT NULL
                );

                CREATE INDEX IF NOT EXISTS idx_sessions_created ON sessions(created_at);
                CREATE INDEX IF NOT EXISTS idx_checkpoints_session ON checkpoints(session_id);
                CREATE INDEX IF NOT EXISTS idx_audit_timestamp ON audit_log(timestamp);
                CREATE INDEX IF NOT EXISTS idx_audit_session ON audit_log(session_id);
                CREATE TABLE IF NOT EXISTS tasks (
                    id TEXT PRIMARY KEY,
                    description TEXT NOT NULL,
                    status TEXT NOT NULL,
                    parent_id TEXT,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    metadata BLOB
                );

                CREATE TABLE IF NOT EXISTS session_events (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    event_type TEXT NOT NULL,
                    timestamp INTEGER NOT NULL,
                    data BLOB NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE TABLE IF NOT EXISTS file_snapshots (
                    id TEXT PRIMARY KEY,
                    snapshot_id TEXT NOT NULL,
                    session_id TEXT NOT NULL,
                    tool_name TEXT NOT NULL,
                    path TEXT NOT NULL,
                    existed INTEGER NOT NULL,
                    content BLOB,
                    created_at INTEGER NOT NULL,
                    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_session_events_session ON session_events(session_id, timestamp);
                CREATE INDEX IF NOT EXISTS idx_file_snapshots_session ON file_snapshots(session_id, created_at);
                CREATE INDEX IF NOT EXISTS idx_file_snapshots_snapshot ON file_snapshots(snapshot_id);

                CREATE TABLE IF NOT EXISTS todo_items (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    priority TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL
                );

                CREATE TABLE IF NOT EXISTS queued_messages (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    client_message_id TEXT NOT NULL,
                    content TEXT NOT NULL,
                    status TEXT NOT NULL,
                    position INTEGER NOT NULL,
                    created_at INTEGER NOT NULL,
                    updated_at INTEGER NOT NULL,
                    dispatched_at INTEGER
                );

                CREATE INDEX IF NOT EXISTS idx_todo_session ON todo_items(session_id, position);
                CREATE INDEX IF NOT EXISTS idx_queued_messages_session ON queued_messages(session_id, position);
                CREATE UNIQUE INDEX IF NOT EXISTS idx_queued_messages_client ON queued_messages(session_id, client_message_id);

                CREATE TABLE IF NOT EXISTS subagent_tasks (
                    id TEXT PRIMARY KEY,
                    session_id TEXT NOT NULL,
                    parent_session_id TEXT NOT NULL,
                    description TEXT NOT NULL,
                    prompt TEXT NOT NULL,
                    agent_type TEXT NOT NULL,
                    status TEXT NOT NULL,
                    tool_count INTEGER NOT NULL DEFAULT 0,
                    result TEXT,
                    created_at INTEGER NOT NULL,
                    completed_at INTEGER,
                    FOREIGN KEY (session_id) REFERENCES sessions(id) ON DELETE CASCADE,
                    FOREIGN KEY (parent_session_id) REFERENCES sessions(id) ON DELETE CASCADE
                );

                CREATE INDEX IF NOT EXISTS idx_subagent_parent ON subagent_tasks(parent_session_id);
                CREATE INDEX IF NOT EXISTS idx_subagent_status ON subagent_tasks(status, created_at);
                "#,
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn create_session(
        &self,
        model: String,
        provider: String,
        name: Option<String>,
    ) -> Result<Session> {
        let session = Session::new(model, provider, name);
        let messages_bytes = serde_json::to_vec(&session.messages)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;
        let metadata_bytes = serde_json::to_vec(&session.metadata)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;
        let context_state_bytes: Option<Vec<u8>> = session
            .context_state
            .as_ref()
            .map(|cs| serde_json::to_vec(cs).unwrap_or_default());

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at, model, provider, messages, metadata, parent_id, agent_type, task_status, context_state) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    session.id,
                    session.created_at.timestamp(),
                    session.updated_at.timestamp(),
                    session.model,
                    session.provider,
                    messages_bytes,
                    metadata_bytes,
                    session.parent_id.as_ref(),
                    session.agent_type,
                    session.task_status,
                    context_state_bytes,
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(session)
        })
    }

    pub fn create_subagent_session(
        &self,
        parent_id: String,
        model: String,
        provider: String,
        agent_type: String,
    ) -> Result<Session> {
        let session = Session::new_subagent(parent_id, model, provider, agent_type);
        let messages_bytes = serde_json::to_vec(&session.messages)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;
        let metadata_bytes = serde_json::to_vec(&session.metadata)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO sessions (id, created_at, updated_at, model, provider, messages, metadata, parent_id, agent_type, task_status) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10)",
                params![
                    session.id,
                    session.created_at.timestamp(),
                    session.updated_at.timestamp(),
                    session.model,
                    session.provider,
                    messages_bytes,
                    metadata_bytes,
                    session.parent_id.as_ref(),
                    session.agent_type.clone(),
                    session.task_status,
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(session)
        })
    }

    pub fn get_session(&self, id: &str) -> Result<Option<Session>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, created_at, updated_at, model, provider, messages, metadata, parent_id, agent_type, task_status, context_state FROM sessions WHERE id = ?1")
                .map_err(OSAgentError::Storage)?;

            let result = stmt.query_row(params![id], |row| {
                let messages_bytes: Vec<u8> = row.get(5)?;
                let metadata_bytes: Vec<u8> = row.get(6)?;
                let context_state_bytes: Option<Vec<u8>> = row.get(10)?;
                let context_state = context_state_bytes
                    .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                Ok(Session {
                    id: row.get(0)?,
                    created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(1)?, 0)
                        .unwrap_or_else(Utc::now),
                    updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                        .unwrap_or_else(Utc::now),
                    model: row.get(3)?,
                    provider: row.get(4)?,
                    messages: serde_json::from_slice(&messages_bytes).unwrap_or_default(),
                    metadata: serde_json::from_slice(&metadata_bytes)
                        .unwrap_or_else(|_| serde_json::json!({})),
                    parent_id: row.get(7)?,
                    agent_type: row.get(8)?,
                    task_status: row.get(9)?,
                    context_state,
                })
            });

            match result {
                Ok(session) => Ok(Some(session)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(OSAgentError::Storage(e)),
            }
        })
    }

    pub fn update_session(&self, session: &Session) -> Result<()> {
        let messages_bytes = serde_json::to_vec(&session.messages)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;
        let metadata_bytes = serde_json::to_vec(&session.metadata)
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;
        let context_state_bytes: Option<Vec<u8>> = session
            .context_state
            .as_ref()
            .map(|cs| serde_json::to_vec(cs).unwrap_or_default());

        self.with_conn(|conn| {
            conn.execute(
                "UPDATE sessions SET updated_at = ?1, messages = ?2, metadata = ?3, task_status = ?4, context_state = ?5 WHERE id = ?6",
                params![
                    Utc::now().timestamp(),
                    messages_bytes,
                    metadata_bytes,
                    session.task_status.clone(),
                    context_state_bytes,
                    session.id
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn list_sessions(&self) -> Result<Vec<Session>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, created_at, updated_at, model, provider, messages, metadata, parent_id, agent_type, task_status, context_state FROM sessions ORDER BY created_at DESC")
                .map_err(OSAgentError::Storage)?;
            let sessions = stmt
                .query_map([], |row| {
                    let messages_bytes: Vec<u8> = row.get(5)?;
                    let metadata_bytes: Vec<u8> = row.get(6)?;
                    let context_state_bytes: Option<Vec<u8>> = row.get(10)?;
                    let context_state = context_state_bytes
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                    Ok(Session {
                        id: row.get(0)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(1)?, 0)
                            .unwrap_or_else(Utc::now),
                        updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                            .unwrap_or_else(Utc::now),
                        model: row.get(3)?,
                        provider: row.get(4)?,
                        messages: serde_json::from_slice(&messages_bytes).unwrap_or_default(),
                        metadata: serde_json::from_slice(&metadata_bytes)
                            .unwrap_or_else(|_| serde_json::json!({})),
                        parent_id: row.get(7)?,
                        agent_type: row.get(8)?,
                        task_status: row.get(9)?,
                        context_state,
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(sessions)
        })
    }

    pub fn get_session_count(&self) -> Result<i64> {
        self.with_conn(|conn| {
            let count: i64 = conn
                .query_row("SELECT COUNT(*) FROM sessions", [], |row| row.get(0))
                .map_err(OSAgentError::Storage)?;
            Ok(count)
        })
    }

    pub fn get_child_sessions(&self, parent_id: &str) -> Result<Vec<Session>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, created_at, updated_at, model, provider, messages, metadata, parent_id, agent_type, task_status, context_state FROM sessions WHERE parent_id = ?1 ORDER BY created_at DESC")
                .map_err(OSAgentError::Storage)?;
            let sessions = stmt
                .query_map(params![parent_id], |row| {
                    let messages_bytes: Vec<u8> = row.get(5)?;
                    let metadata_bytes: Vec<u8> = row.get(6)?;
                    let context_state_bytes: Option<Vec<u8>> = row.get(10)?;
                    let context_state = context_state_bytes
                        .and_then(|bytes| serde_json::from_slice(&bytes).ok());
                    Ok(Session {
                        id: row.get(0)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(1)?, 0)
                            .unwrap_or_else(Utc::now),
                        updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                            .unwrap_or_else(Utc::now),
                        model: row.get(3)?,
                        provider: row.get(4)?,
                        messages: serde_json::from_slice(&messages_bytes).unwrap_or_default(),
                        metadata: serde_json::from_slice(&metadata_bytes)
                            .unwrap_or_else(|_| serde_json::json!({})),
                        parent_id: row.get(7)?,
                        agent_type: row.get(8)?,
                        task_status: row.get(9)?,
                        context_state,
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(sessions)
        })
    }

    pub fn delete_session(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM sessions WHERE id = ?1", params![id])
                .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn delete_all_sessions(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM sessions", [])
                .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn clear_all_tasks(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM tasks", [])
                .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn clear_all_todo_items(&self) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM todo_items", [])
                .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn create_checkpoint(
        &self,
        session_id: &str,
        state: Vec<u8>,
        tool_name: Option<String>,
        tool_input: Option<String>,
    ) -> Result<Checkpoint> {
        let checkpoint = Checkpoint {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            created_at: Utc::now(),
            state,
            tool_name,
            tool_input,
        };
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO checkpoints (id, session_id, created_at, state, tool_name, tool_input) VALUES (?1, ?2, ?3, ?4, ?5, ?6)",
                params![
                    checkpoint.id,
                    checkpoint.session_id,
                    checkpoint.created_at.timestamp(),
                    checkpoint.state,
                    checkpoint.tool_name,
                    checkpoint.tool_input,
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(checkpoint)
        })
    }

    pub fn get_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, session_id, created_at, state, tool_name, tool_input FROM checkpoints WHERE id = ?1")
                .map_err(OSAgentError::Storage)?;
            let result = stmt.query_row(params![id], |row| {
                Ok(Checkpoint {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                        .unwrap_or_else(Utc::now),
                    state: row.get(3)?,
                    tool_name: row.get(4)?,
                    tool_input: row.get(5)?,
                })
            });
            match result {
                Ok(checkpoint) => Ok(Some(checkpoint)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(OSAgentError::Storage(e)),
            }
        })
    }

    pub fn list_checkpoints(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, session_id, created_at, state, tool_name, tool_input FROM checkpoints WHERE session_id = ?1 ORDER BY created_at DESC")
                .map_err(OSAgentError::Storage)?;
            let checkpoints = stmt
                .query_map(params![session_id], |row| {
                    Ok(Checkpoint {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(2)?, 0)
                            .unwrap_or_else(Utc::now),
                        state: row.get(3)?,
                        tool_name: row.get(4)?,
                        tool_input: row.get(5)?,
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(checkpoints)
        })
    }

    pub fn log_audit(&self, entry: AuditEntry) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO audit_log (id, timestamp, session_id, tool, input, output, duration_ms, user) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                params![
                    entry.id,
                    entry.timestamp.timestamp(),
                    entry.session_id,
                    entry.tool,
                    entry.input,
                    entry.output,
                    entry.duration_ms,
                    entry.user,
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn list_audit(&self, limit: usize, offset: usize) -> Result<Vec<AuditEntry>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, timestamp, session_id, tool, input, output, duration_ms, user FROM audit_log ORDER BY timestamp DESC LIMIT ?1 OFFSET ?2")
                .map_err(OSAgentError::Storage)?;
            let entries = stmt
                .query_map(params![limit, offset], |row| {
                    Ok(AuditEntry {
                        id: row.get(0)?,
                        timestamp: chrono::DateTime::from_timestamp(row.get::<_, i64>(1)?, 0)
                            .unwrap_or_else(Utc::now),
                        session_id: row.get(2)?,
                        tool: row.get(3)?,
                        input: row.get(4)?,
                        output: row.get(5)?,
                        duration_ms: row.get(6)?,
                        user: row.get(7)?,
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(entries)
        })
    }

    pub fn append_session_event(
        &self,
        session_id: &str,
        event_type: &str,
        data: serde_json::Value,
    ) -> Result<StoredSessionEvent> {
        let event = StoredSessionEvent {
            id: Uuid::new_v4().to_string(),
            session_id: session_id.to_string(),
            event_type: event_type.to_string(),
            timestamp: Utc::now(),
            data,
        };

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO session_events (id, session_id, event_type, timestamp, data) VALUES (?1, ?2, ?3, ?4, ?5)",
                params![
                    event.id,
                    event.session_id,
                    event.event_type,
                    event.timestamp.timestamp(),
                    serde_json::to_vec(&event.data).unwrap(),
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(event)
        })
    }

    pub fn list_session_events(&self, session_id: &str) -> Result<Vec<StoredSessionEvent>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, session_id, event_type, timestamp, data FROM session_events WHERE session_id = ?1 ORDER BY timestamp ASC",
                )
                .map_err(OSAgentError::Storage)?;

            let items = stmt
                .query_map(params![session_id], |row| {
                    Ok(StoredSessionEvent {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        event_type: row.get(2)?,
                        timestamp: chrono::DateTime::from_timestamp(row.get::<_, i64>(3)?, 0)
                            .unwrap_or_else(Utc::now),
                        data: serde_json::from_slice(&row.get::<_, Vec<u8>>(4)?)
                            .unwrap_or_else(|_| serde_json::json!({})),
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(items)
        })
    }

    pub fn create_file_snapshot_records(
        &self,
        session_id: &str,
        snapshot_id: &str,
        tool_name: &str,
        records: &[FileSnapshotRecord],
    ) -> Result<()> {
        self.with_conn(|conn| {
            for record in records {
                conn.execute(
                    "INSERT INTO file_snapshots (id, snapshot_id, session_id, tool_name, path, existed, content, created_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)",
                    params![
                        record.id,
                        snapshot_id,
                        session_id,
                        tool_name,
                        record.path,
                        if record.existed { 1 } else { 0 },
                        record.content,
                        record.created_at.timestamp(),
                    ],
                )
                .map_err(OSAgentError::Storage)?;
            }
            Ok(())
        })
    }

    pub fn list_file_snapshot_summaries(
        &self,
        session_id: &str,
    ) -> Result<Vec<FileSnapshotSummary>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT snapshot_id, session_id, tool_name, path, created_at FROM file_snapshots WHERE session_id = ?1 ORDER BY created_at DESC, path ASC",
                )
                .map_err(OSAgentError::Storage)?;

            let rows = stmt
                .query_map(params![session_id], |row| {
                    Ok((
                        row.get::<_, String>(0)?,
                        row.get::<_, String>(1)?,
                        row.get::<_, String>(2)?,
                        row.get::<_, String>(3)?,
                        row.get::<_, i64>(4)?,
                    ))
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;

            let mut grouped: Vec<FileSnapshotSummary> = Vec::new();
            for (snapshot_id, session_id, tool_name, path, created_at) in rows {
                if let Some(existing) = grouped.iter_mut().find(|item| item.snapshot_id == snapshot_id) {
                    existing.paths.push(path);
                    continue;
                }

                grouped.push(FileSnapshotSummary {
                    snapshot_id,
                    session_id,
                    tool_name,
                    created_at: chrono::DateTime::from_timestamp(created_at, 0)
                        .unwrap_or_else(Utc::now),
                    paths: vec![path],
                });
            }

            Ok(grouped)
        })
    }

    pub fn list_file_snapshot_records(
        &self,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<Vec<FileSnapshotRecord>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, snapshot_id, session_id, tool_name, path, existed, content, created_at FROM file_snapshots WHERE session_id = ?1 AND snapshot_id = ?2 ORDER BY path ASC",
                )
                .map_err(OSAgentError::Storage)?;

            let items = stmt
                .query_map(params![session_id, snapshot_id], |row| {
                    Ok(FileSnapshotRecord {
                        id: row.get(0)?,
                        snapshot_id: row.get(1)?,
                        session_id: row.get(2)?,
                        tool_name: row.get(3)?,
                        path: row.get(4)?,
                        existed: row.get::<_, i64>(5)? != 0,
                        content: row.get(6)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(7)?, 0)
                            .unwrap_or_else(Utc::now),
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(items)
        })
    }

    pub fn create_task(
        &self,
        description: String,
        parent_id: Option<String>,
    ) -> Result<crate::tools::task::Task> {
        use crate::tools::task::Task;

        let task = Task::new(description, parent_id);

        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO tasks (id, description, status, parent_id, created_at, updated_at, metadata)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                params![
                    task.id,
                    task.description,
                    serde_json::to_string(&task.status).unwrap(),
                    task.parent_id,
                    task.created_at.timestamp(),
                    task.updated_at.timestamp(),
                    serde_json::to_vec(&task.metadata).unwrap(),
                ],
            ).map_err(OSAgentError::Storage)?;

            Ok(task)
        })
    }

    pub fn update_task_status(
        &self,
        task_id: &str,
        status: crate::tools::task::TaskStatus,
    ) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE tasks SET status = ?1, updated_at = ?2 WHERE id = ?3",
                params![
                    serde_json::to_string(&status).unwrap(),
                    Utc::now().timestamp(),
                    task_id,
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn list_tasks(&self, parent_id: Option<&str>) -> Result<Vec<crate::tools::task::Task>> {
        use crate::tools::task::{Task, TaskStatus};

        self.with_conn(|conn| {
            let query = if parent_id.is_some() {
                "SELECT id, description, status, parent_id, created_at, updated_at, metadata 
                 FROM tasks WHERE parent_id = ?1 ORDER BY created_at"
            } else {
                "SELECT id, description, status, parent_id, created_at, updated_at, metadata 
                 FROM tasks ORDER BY created_at"
            };

            let mut stmt = conn.prepare_cached(query).map_err(OSAgentError::Storage)?;

            let tasks: Vec<Task> = if let Some(pid) = parent_id {
                stmt.query_map(params![pid], |row| {
                    Ok(Task {
                        id: row.get(0)?,
                        description: row.get(1)?,
                        status: serde_json::from_str(&row.get::<_, String>(2)?)
                            .unwrap_or(TaskStatus::Pending),
                        parent_id: row.get(3)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(4)?, 0)
                            .unwrap_or_else(Utc::now),
                        updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(5)?, 0)
                            .unwrap_or_else(Utc::now),
                        metadata: serde_json::from_slice(&row.get::<_, Vec<u8>>(6)?)
                            .unwrap_or(serde_json::json!({})),
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?
            } else {
                stmt.query_map([], |row| {
                    Ok(Task {
                        id: row.get(0)?,
                        description: row.get(1)?,
                        status: serde_json::from_str(&row.get::<_, String>(2)?)
                            .unwrap_or(TaskStatus::Pending),
                        parent_id: row.get(3)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(4)?, 0)
                            .unwrap_or_else(Utc::now),
                        updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(5)?, 0)
                            .unwrap_or_else(Utc::now),
                        metadata: serde_json::from_slice(&row.get::<_, Vec<u8>>(6)?)
                            .unwrap_or(serde_json::json!({})),
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?
            };

            Ok(tasks)
        })
    }

    pub fn create_subtasks(
        &self,
        task_id: &str,
        descriptions: Vec<String>,
    ) -> Result<Vec<crate::tools::task::Task>> {
        use crate::tools::task::Task;

        let mut tasks = Vec::new();

        for desc in descriptions {
            let task = Task::new(desc, Some(task_id.to_string()));

            self.with_conn(|conn| {
                conn.execute(
                    "INSERT INTO tasks (id, description, status, parent_id, created_at, updated_at, metadata)
                     VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
                    params![
                        task.id,
                        task.description,
                        serde_json::to_string(&task.status).unwrap(),
                        task.parent_id,
                        task.created_at.timestamp(),
                        task.updated_at.timestamp(),
                        serde_json::to_vec(&task.metadata).unwrap(),
                    ],
                ).map_err(OSAgentError::Storage)?;

                Ok(())
            })?;

            tasks.push(task);
        }

        Ok(tasks)
    }

    pub fn delete_task(&self, task_id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM tasks WHERE id = ?1 OR parent_id = ?1",
                params![task_id],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn upsert_todo_item(
        &self,
        id: &str,
        session_id: &str,
        content: &str,
        status: crate::tools::todo::TodoStatus,
        priority: crate::tools::todo::TodoPriority,
        position: i32,
    ) -> Result<()> {
        let now = Utc::now();
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO todo_items (id, session_id, content, status, priority, position, created_at, updated_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)
                 ON CONFLICT(id) DO UPDATE SET 
                    content = excluded.content,
                    status = excluded.status,
                    priority = excluded.priority,
                    position = excluded.position,
                    updated_at = excluded.updated_at",
                params![
                    id,
                    session_id,
                    content,
                    status.as_str(),
                    priority.as_str(),
                    position,
                    now.timestamp(),
                    now.timestamp(),
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn list_todo_items(&self, session_id: &str) -> Result<Vec<crate::tools::todo::TodoItem>> {
        use crate::tools::todo::{TodoItem, TodoPriority, TodoStatus};

        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, session_id, content, status, priority, position, created_at, updated_at
                     FROM todo_items WHERE session_id = ?1 ORDER BY position ASC",
                )
                .map_err(OSAgentError::Storage)?;

            let items = stmt
                .query_map(params![session_id], |row| {
                    let status_str: String = row.get(3)?;
                    let priority_str: String = row.get(4)?;
                    Ok(TodoItem {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        content: row.get(2)?,
                        status: TodoStatus::from_str(&status_str).unwrap_or(TodoStatus::Pending),
                        priority: TodoPriority::from_str(&priority_str).unwrap_or(TodoPriority::Medium),
                        position: row.get(5)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(6)?, 0)
                            .unwrap_or_else(Utc::now),
                        updated_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(7)?, 0)
                            .unwrap_or_else(Utc::now),
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;

            Ok(items)
        })
    }

    pub fn clear_todo_items(&self, session_id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "DELETE FROM todo_items WHERE session_id = ?1",
                params![session_id],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn enqueue_message(
        &self,
        session_id: &str,
        client_message_id: &str,
        content: &str,
    ) -> Result<(QueuedMessage, bool)> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(OSAgentError::Storage)?;

            let mut existing_stmt = tx
                .prepare_cached(
                    "SELECT id, session_id, client_message_id, content, status, position, created_at, updated_at, dispatched_at
                     FROM queued_messages WHERE session_id = ?1 AND client_message_id = ?2 LIMIT 1",
                )
                .map_err(OSAgentError::Storage)?;

            let existing = match existing_stmt.query_row(params![session_id, client_message_id], Self::queued_message_from_row) {
                Ok(item) => Some(item),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(error) => return Err(OSAgentError::Storage(error)),
            };
            drop(existing_stmt);

            if let Some(item) = existing {
                tx.commit().map_err(OSAgentError::Storage)?;
                return Ok((item, false));
            }

            let next_position: i64 = tx
                .query_row(
                    "SELECT COALESCE(MAX(position), 0) + 1 FROM queued_messages WHERE session_id = ?1",
                    params![session_id],
                    |row| row.get(0),
                )
                .map_err(OSAgentError::Storage)?;

            let now = Utc::now();
            let item = QueuedMessage {
                id: Uuid::new_v4().to_string(),
                session_id: session_id.to_string(),
                client_message_id: client_message_id.to_string(),
                content: content.to_string(),
                status: QueuedMessageStatus::Pending,
                position: next_position,
                created_at: now,
                updated_at: now,
                dispatched_at: None,
            };

            tx.execute(
                "INSERT INTO queued_messages (id, session_id, client_message_id, content, status, position, created_at, updated_at, dispatched_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    &item.id,
                    &item.session_id,
                    &item.client_message_id,
                    &item.content,
                    item.status.as_str(),
                    item.position,
                    item.created_at.timestamp(),
                    item.updated_at.timestamp(),
                    item.dispatched_at.map(|dt| dt.timestamp()),
                ],
            )
            .map_err(OSAgentError::Storage)?;

            tx.commit().map_err(OSAgentError::Storage)?;
            Ok((item, true))
        })
    }

    pub fn list_queued_messages(&self, session_id: &str) -> Result<Vec<QueuedMessage>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached(
                    "SELECT id, session_id, client_message_id, content, status, position, created_at, updated_at, dispatched_at
                     FROM queued_messages
                     WHERE session_id = ?1
                     ORDER BY position ASC, created_at ASC",
                )
                .map_err(OSAgentError::Storage)?;

            let items = stmt
                .query_map(params![session_id], Self::queued_message_from_row)
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;

            Ok(items)
        })
    }

    pub fn claim_next_queued_message(&self, session_id: &str) -> Result<Option<QueuedMessage>> {
        self.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(OSAgentError::Storage)?;

            let select_sql = "SELECT id, session_id, client_message_id, content, status, position, created_at, updated_at, dispatched_at
                              FROM queued_messages
                              WHERE session_id = ?1 AND status = ?2
                              ORDER BY position ASC, created_at ASC
                              LIMIT 1";

            let mut stmt = tx.prepare_cached(select_sql).map_err(OSAgentError::Storage)?;
            let dispatching = match stmt.query_row(
                params![session_id, QueuedMessageStatus::Dispatching.as_str()],
                Self::queued_message_from_row,
            ) {
                Ok(item) => Some(item),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(error) => return Err(OSAgentError::Storage(error)),
            };
            drop(stmt);

            if let Some(item) = dispatching {
                tx.commit().map_err(OSAgentError::Storage)?;
                return Ok(Some(item));
            }

            let mut stmt = tx.prepare_cached(select_sql).map_err(OSAgentError::Storage)?;
            let pending = match stmt.query_row(
                params![session_id, QueuedMessageStatus::Pending.as_str()],
                Self::queued_message_from_row,
            ) {
                Ok(item) => Some(item),
                Err(rusqlite::Error::QueryReturnedNoRows) => None,
                Err(error) => return Err(OSAgentError::Storage(error)),
            };
            drop(stmt);

            let Some(mut item) = pending else {
                tx.commit().map_err(OSAgentError::Storage)?;
                return Ok(None);
            };

            let now = Utc::now();
            tx.execute(
                "UPDATE queued_messages
                 SET status = ?1, updated_at = ?2, dispatched_at = COALESCE(dispatched_at, ?2)
                 WHERE id = ?3",
                params![
                    QueuedMessageStatus::Dispatching.as_str(),
                    now.timestamp(),
                    &item.id,
                ],
            )
            .map_err(OSAgentError::Storage)?;

            item.status = QueuedMessageStatus::Dispatching;
            item.updated_at = now;
            item.dispatched_at = Some(now);

            tx.commit().map_err(OSAgentError::Storage)?;
            Ok(Some(item))
        })
    }

    pub fn delete_queued_message(&self, id: &str) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute("DELETE FROM queued_messages WHERE id = ?1", params![id])
                .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn create_subagent_task(&self, task: &SubagentTask) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "INSERT INTO subagent_tasks (id, session_id, parent_session_id, description, prompt, agent_type, status, tool_count, result, created_at, completed_at)
                 VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10, ?11)",
                params![
                    task.id,
                    task.session_id,
                    task.parent_session_id,
                    task.description,
                    task.prompt,
                    task.agent_type,
                    task.status,
                    task.tool_count,
                    task.result.as_ref(),
                    task.created_at.timestamp(),
                    task.completed_at.map(|dt| dt.timestamp()),
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn update_subagent_task(&self, task: &SubagentTask) -> Result<()> {
        self.with_conn(|conn| {
            conn.execute(
                "UPDATE subagent_tasks SET status = ?1, tool_count = ?2, result = ?3, completed_at = ?4 WHERE id = ?5",
                params![
                    task.status.clone(),
                    task.tool_count,
                    task.result.as_ref(),
                    task.completed_at.map(|dt| dt.timestamp()),
                    task.id.clone(),
                ],
            )
            .map_err(OSAgentError::Storage)?;
            Ok(())
        })
    }

    pub fn get_subagent_task(&self, id: &str) -> Result<Option<SubagentTask>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, session_id, parent_session_id, description, prompt, agent_type, status, tool_count, result, created_at, completed_at FROM subagent_tasks WHERE id = ?1")
                .map_err(OSAgentError::Storage)?;

            let result = stmt.query_row(params![id], |row| {
                let completed_at: Option<i64> = row.get(10)?;
                Ok(SubagentTask {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    parent_session_id: row.get(2)?,
                    description: row.get(3)?,
                    prompt: row.get(4)?,
                    agent_type: row.get(5)?,
                    status: row.get(6)?,
                    tool_count: row.get(7)?,
                    result: row.get(8)?,
                    created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(9)?, 0)
                        .unwrap_or_else(Utc::now),
                    completed_at: completed_at.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
                })
            });

            match result {
                Ok(task) => Ok(Some(task)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(OSAgentError::Storage(e)),
            }
        })
    }

    pub fn get_subagent_task_by_session(&self, session_id: &str) -> Result<Option<SubagentTask>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, session_id, parent_session_id, description, prompt, agent_type, status, tool_count, result, created_at, completed_at FROM subagent_tasks WHERE session_id = ?1")
                .map_err(OSAgentError::Storage)?;

            let result = stmt.query_row(params![session_id], |row| {
                let completed_at: Option<i64> = row.get(10)?;
                Ok(SubagentTask {
                    id: row.get(0)?,
                    session_id: row.get(1)?,
                    parent_session_id: row.get(2)?,
                    description: row.get(3)?,
                    prompt: row.get(4)?,
                    agent_type: row.get(5)?,
                    status: row.get(6)?,
                    tool_count: row.get(7)?,
                    result: row.get(8)?,
                    created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(9)?, 0)
                        .unwrap_or_else(Utc::now),
                    completed_at: completed_at.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
                })
            });

            match result {
                Ok(task) => Ok(Some(task)),
                Err(rusqlite::Error::QueryReturnedNoRows) => Ok(None),
                Err(e) => Err(OSAgentError::Storage(e)),
            }
        })
    }

    pub fn list_subagent_tasks(&self, parent_session_id: &str) -> Result<Vec<SubagentTask>> {
        self.with_conn(|conn| {
            let mut stmt = conn
                .prepare_cached("SELECT id, session_id, parent_session_id, description, prompt, agent_type, status, tool_count, result, created_at, completed_at FROM subagent_tasks WHERE parent_session_id = ?1 ORDER BY created_at DESC")
                .map_err(OSAgentError::Storage)?;

            let tasks = stmt
                .query_map(params![parent_session_id], |row| {
                    let completed_at: Option<i64> = row.get(10)?;
                    Ok(SubagentTask {
                        id: row.get(0)?,
                        session_id: row.get(1)?,
                        parent_session_id: row.get(2)?,
                        description: row.get(3)?,
                        prompt: row.get(4)?,
                        agent_type: row.get(5)?,
                        status: row.get(6)?,
                        tool_count: row.get(7)?,
                        result: row.get(8)?,
                        created_at: chrono::DateTime::from_timestamp(row.get::<_, i64>(9)?, 0)
                            .unwrap_or_else(Utc::now),
                        completed_at: completed_at.and_then(|ts| chrono::DateTime::from_timestamp(ts, 0)),
                    })
                })
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;
            Ok(tasks)
        })
    }

    pub fn cleanup_completed_subagents(&self, days: i64) -> Result<usize> {
        let cutoff = Utc::now() - chrono::Duration::days(days);
        self.with_conn_mut(|conn| {
            let tx = conn.transaction().map_err(OSAgentError::Storage)?;

            let mut stmt = tx
                .prepare_cached(
                    "SELECT session_id FROM subagent_tasks WHERE status = 'completed' AND completed_at < ?1",
                )
                .map_err(OSAgentError::Storage)?;

            let session_ids: Vec<String> = stmt
                .query_map(params![cutoff.timestamp()], |row| row.get(0))
                .map_err(OSAgentError::Storage)?
                .collect::<std::result::Result<Vec<_>, _>>()
                .map_err(OSAgentError::Storage)?;

            drop(stmt);

            for session_id in &session_ids {
                tx.execute("DELETE FROM sessions WHERE id = ?1", params![session_id])
                    .map_err(OSAgentError::Storage)?;
            }

            tx.commit().map_err(OSAgentError::Storage)?;
            Ok(session_ids.len())
        })
    }
}
