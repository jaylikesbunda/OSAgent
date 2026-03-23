use crate::error::{OSAgentError, Result};
use crate::workflow::types::*;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use uuid::Uuid;

pub struct WorkflowDb {
    db_path: PathBuf,
}

impl WorkflowDb {
    pub fn new(db_path: PathBuf) -> Self {
        Self { db_path }
    }

    pub fn init_tables(&self) -> Result<()> {
        let conn =
            rusqlite::Connection::open(&self.db_path).map_err(|e| OSAgentError::Storage(e))?;

        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS workflows (
                id TEXT PRIMARY KEY,
                name TEXT NOT NULL,
                description TEXT,
                current_version INTEGER DEFAULT 1,
                created_at TEXT DEFAULT (datetime('now')),
                updated_at TEXT DEFAULT (datetime('now'))
            )
            "#,
            [],
        )?;

        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS workflow_versions (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                version INTEGER NOT NULL,
                graph_json TEXT NOT NULL,
                created_at TEXT DEFAULT (datetime('now')),
                FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE,
                UNIQUE(workflow_id, version)
            )
            "#,
            [],
        )?;

        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS workflow_runs (
                id TEXT PRIMARY KEY,
                workflow_id TEXT NOT NULL,
                workflow_version INTEGER NOT NULL,
                status TEXT NOT NULL DEFAULT 'running',
                started_at TEXT DEFAULT (datetime('now')),
                completed_at TEXT,
                error_message TEXT,
                FOREIGN KEY (workflow_id) REFERENCES workflows(id) ON DELETE CASCADE
            )
            "#,
            [],
        )?;

        conn.execute(
            r#"
            CREATE TABLE IF NOT EXISTS workflow_node_logs (
                id TEXT PRIMARY KEY,
                run_id TEXT NOT NULL,
                node_id TEXT NOT NULL,
                node_type TEXT NOT NULL,
                status TEXT NOT NULL,
                input_json TEXT,
                output_json TEXT,
                started_at TEXT DEFAULT (datetime('now')),
                completed_at TEXT,
                FOREIGN KEY (run_id) REFERENCES workflow_runs(id) ON DELETE CASCADE
            )
            "#,
            [],
        )?;

        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_workflow_versions ON workflow_versions(workflow_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_workflow_runs ON workflow_runs(workflow_id)",
            [],
        )?;
        conn.execute(
            "CREATE INDEX IF NOT EXISTS idx_workflow_node_logs ON workflow_node_logs(run_id)",
            [],
        )?;

        Ok(())
    }

    fn with_conn<T>(&self, f: impl FnOnce(&rusqlite::Connection) -> Result<T>) -> Result<T> {
        let conn =
            rusqlite::Connection::open(&self.db_path).map_err(|e| OSAgentError::Storage(e))?;
        f(&conn)
    }

    fn with_conn_mut<T>(
        &self,
        f: impl FnOnce(&mut rusqlite::Connection) -> Result<T>,
    ) -> Result<T> {
        let mut conn =
            rusqlite::Connection::open(&self.db_path).map_err(|e| OSAgentError::Storage(e))?;
        f(&mut conn)
    }

    pub fn create_workflow(&self, workflow: &Workflow) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO workflows (id, name, description, current_version) VALUES (?, ?, ?, ?)",
                rusqlite::params![workflow.id, workflow.name, workflow.description, workflow.current_version],
            )?;
            Ok(())
        })
    }

    pub fn get_workflow(&self, id: &str) -> Result<Option<Workflow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, name, description, current_version, created_at, updated_at FROM workflows WHERE id = ?")?;
            let mut rows = stmt.query(rusqlite::params![id])?;

            if let Some(row) = rows.next()? {
                Ok(Some(Workflow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    current_version: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn list_workflows(&self) -> Result<Vec<Workflow>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, name, description, current_version, created_at, updated_at FROM workflows ORDER BY updated_at DESC")?;
            let mut rows = stmt.query([])?;

            let mut workflows = Vec::new();
            while let Some(row) = rows.next()? {
                workflows.push(Workflow {
                    id: row.get(0)?,
                    name: row.get(1)?,
                    description: row.get(2)?,
                    current_version: row.get(3)?,
                    created_at: row.get(4)?,
                    updated_at: row.get(5)?,
                });
            }
            Ok(workflows)
        })
    }

    pub fn update_workflow(&self, id: &str, new_version: i32) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE workflows SET current_version = ?, updated_at = datetime('now') WHERE id = ?",
                rusqlite::params![new_version, id],
            )?;
            Ok(())
        })
    }

    pub fn delete_workflow(&self, id: &str) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute("DELETE FROM workflows WHERE id = ?", rusqlite::params![id])?;
            Ok(())
        })
    }

    pub fn create_version(&self, version: &WorkflowVersion) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO workflow_versions (id, workflow_id, version, graph_json) VALUES (?, ?, ?, ?)",
                rusqlite::params![version.id, version.workflow_id, version.version, version.graph_json],
            )?;
            Ok(())
        })
    }

    pub fn get_version(&self, workflow_id: &str, version: i32) -> Result<Option<WorkflowVersion>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, workflow_id, version, graph_json, created_at FROM workflow_versions WHERE workflow_id = ? AND version = ?")?;
            let mut rows = stmt.query(rusqlite::params![workflow_id, version])?;

            if let Some(row) = rows.next()? {
                Ok(Some(WorkflowVersion {
                    id: row.get(0)?,
                    workflow_id: row.get(1)?,
                    version: row.get(2)?,
                    graph_json: row.get(3)?,
                    created_at: row.get(4)?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn list_versions(&self, workflow_id: &str) -> Result<Vec<WorkflowVersion>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, workflow_id, version, graph_json, created_at FROM workflow_versions WHERE workflow_id = ? ORDER BY version DESC")?;
            let mut rows = stmt.query(rusqlite::params![workflow_id])?;

            let mut versions = Vec::new();
            while let Some(row) = rows.next()? {
                versions.push(WorkflowVersion {
                    id: row.get(0)?,
                    workflow_id: row.get(1)?,
                    version: row.get(2)?,
                    graph_json: row.get(3)?,
                    created_at: row.get(4)?,
                });
            }
            Ok(versions)
        })
    }

    pub fn create_run(&self, run: &WorkflowRun) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO workflow_runs (id, workflow_id, workflow_version, status) VALUES (?, ?, ?, ?)",
                rusqlite::params![run.id, run.workflow_id, run.workflow_version, run.status],
            )?;
            Ok(())
        })
    }

    pub fn update_run_status(
        &self,
        run_id: &str,
        status: &str,
        error_message: Option<&str>,
    ) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE workflow_runs SET status = ?, completed_at = datetime('now'), error_message = ? WHERE id = ?",
                rusqlite::params![status, error_message, run_id],
            )?;
            Ok(())
        })
    }

    pub fn get_run(&self, run_id: &str) -> Result<Option<WorkflowRun>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, workflow_id, workflow_version, status, started_at, completed_at, error_message FROM workflow_runs WHERE id = ?")?;
            let mut rows = stmt.query(rusqlite::params![run_id])?;

            if let Some(row) = rows.next()? {
                Ok(Some(WorkflowRun {
                    id: row.get(0)?,
                    workflow_id: row.get(1)?,
                    workflow_version: row.get(2)?,
                    status: row.get(3)?,
                    started_at: row.get(4)?,
                    completed_at: row.get(5)?,
                    error_message: row.get(6)?,
                }))
            } else {
                Ok(None)
            }
        })
    }

    pub fn list_runs(&self, workflow_id: &str) -> Result<Vec<WorkflowRun>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, workflow_id, workflow_version, status, started_at, completed_at, error_message FROM workflow_runs WHERE workflow_id = ? ORDER BY started_at DESC")?;
            let mut rows = stmt.query(rusqlite::params![workflow_id])?;

            let mut runs = Vec::new();
            while let Some(row) = rows.next()? {
                runs.push(WorkflowRun {
                    id: row.get(0)?,
                    workflow_id: row.get(1)?,
                    workflow_version: row.get(2)?,
                    status: row.get(3)?,
                    started_at: row.get(4)?,
                    completed_at: row.get(5)?,
                    error_message: row.get(6)?,
                });
            }
            Ok(runs)
        })
    }

    pub fn create_node_log(&self, log: &NodeLog) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "INSERT INTO workflow_node_logs (id, run_id, node_id, node_type, status, input_json, output_json) VALUES (?, ?, ?, ?, ?, ?, ?)",
                rusqlite::params![log.id, log.run_id, log.node_id, log.node_type, log.status, log.input_json, log.output_json],
            )?;
            Ok(())
        })
    }

    pub fn update_node_log(&self, id: &str, status: &str, output_json: Option<&str>) -> Result<()> {
        self.with_conn_mut(|conn| {
            conn.execute(
                "UPDATE workflow_node_logs SET status = ?, completed_at = datetime('now'), output_json = ? WHERE id = ?",
                rusqlite::params![status, output_json, id],
            )?;
            Ok(())
        })
    }

    pub fn get_node_logs(&self, run_id: &str) -> Result<Vec<NodeLog>> {
        self.with_conn(|conn| {
            let mut stmt = conn.prepare("SELECT id, run_id, node_id, node_type, status, input_json, output_json, started_at, completed_at FROM workflow_node_logs WHERE run_id = ? ORDER BY started_at")?;
            let mut rows = stmt.query(rusqlite::params![run_id])?;

            let mut logs = Vec::new();
            while let Some(row) = rows.next()? {
                logs.push(NodeLog {
                    id: row.get(0)?,
                    run_id: row.get(1)?,
                    node_id: row.get(2)?,
                    node_type: row.get(3)?,
                    status: row.get(4)?,
                    input_json: row.get(5)?,
                    output_json: row.get(6)?,
                    started_at: row.get(7)?,
                    completed_at: row.get(8)?,
                });
            }
            Ok(logs)
        })
    }
}
