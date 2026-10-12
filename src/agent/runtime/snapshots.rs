//! File snapshots, diffs, and revert
//! Extracted verbatim from runtime.rs; behavior-preserving split.
use super::*;

impl AgentRuntime {
    pub(super) fn snapshot_candidate_paths(
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Vec<String> {
        match tool_name {
            "write_file" | "edit_file" | "delete_file" => args["path"]
                .as_str()
                .map(|path| vec![path.to_string()])
                .unwrap_or_default(),
            "apply_patch" => args["patch"]
                .as_str()
                .map(Self::patch_paths_for_snapshot)
                .unwrap_or_default(),
            _ => Vec::new(),
        }
    }

    pub(super) fn patch_paths_for_snapshot(patch: &str) -> Vec<String> {
        patch
            .lines()
            .filter_map(|line| {
                [
                    "*** Add File: ",
                    "*** Delete File: ",
                    "*** Update File: ",
                    "*** Move to: ",
                ]
                .iter()
                .find_map(|prefix| line.strip_prefix(prefix))
                .map(|path| path.trim().to_string())
            })
            .collect()
    }

    pub(super) fn workspace_root(workspace: &WorkspaceConfig) -> std::path::PathBuf {
        std::path::PathBuf::from(shellexpand::tilde(&workspace.resolved_path()).to_string())
    }

    pub(super) fn snapshot_allowed_path(path: &str) -> bool {
        if path.trim().is_empty() {
            return false;
        }
        if ensure_relative_path_not_backups(path).is_err() {
            return false;
        }
        let joined = std::path::Path::new(path);
        !path_touches_tool_outputs(joined)
    }

    pub(super) fn truncate_diff_text(input: &str, limit_chars: usize) -> String {
        if input.chars().count() <= limit_chars {
            return input.to_string();
        }
        let mut out = String::new();
        for ch in input.chars().take(limit_chars) {
            out.push(ch);
        }
        out
    }

    pub(super) fn build_file_diff_metadata(
        &self,
        session_id: &str,
        snapshot_id: &str,
        workspace: &WorkspaceConfig,
    ) -> Result<serde_json::Value> {
        let records = self
            .storage
            .list_file_snapshot_records(session_id, snapshot_id)?;
        let root = Self::workspace_root(workspace);

        let mut files = Vec::new();
        for record in records {
            let abs_path = root.join(&record.path);
            let new_content_bytes = std::fs::read(&abs_path).ok();

            let old_content = record
                .content
                .as_ref()
                .map(|bytes| Self::truncate_diff_text(&String::from_utf8_lossy(bytes), 50_000));
            let new_content = new_content_bytes
                .as_ref()
                .map(|bytes| Self::truncate_diff_text(&String::from_utf8_lossy(bytes), 50_000));

            let status = if !record.existed && new_content.is_some() {
                "added"
            } else if record.existed && new_content.is_none() {
                "deleted"
            } else {
                "modified"
            };

            files.push(serde_json::json!({
                "path": record.path,
                "status": status,
                "old_content": old_content,
                "new_content": new_content,
            }));
        }

        Ok(serde_json::json!({ "diff_files": files }))
    }

    pub(super) fn capture_file_snapshots(
        &self,
        session_id: &str,
        workspace: &WorkspaceConfig,
        tool_name: &str,
        args: &serde_json::Value,
    ) -> Result<Option<String>> {
        let mut paths = Self::snapshot_candidate_paths(tool_name, args)
            .into_iter()
            .filter(|path| Self::snapshot_allowed_path(path))
            .collect::<Vec<_>>();
        paths.sort();
        paths.dedup();

        if paths.is_empty() {
            return Ok(None);
        }

        let root = Self::workspace_root(workspace);
        let snapshot_id = Uuid::new_v4().to_string();
        let now = Utc::now();
        let records = paths
            .into_iter()
            .map(|path| {
                let full_path = root.join(&path);
                let content = std::fs::read(&full_path).ok();
                crate::storage::FileSnapshotRecord {
                    id: Uuid::new_v4().to_string(),
                    snapshot_id: snapshot_id.clone(),
                    session_id: session_id.to_string(),
                    tool_name: tool_name.to_string(),
                    path,
                    existed: full_path.exists(),
                    content,
                    created_at: now,
                }
            })
            .collect::<Vec<_>>();

        self.storage
            .create_file_snapshot_records(session_id, &snapshot_id, tool_name, &records)?;
        Ok(Some(snapshot_id))
    }

    pub(super) async fn revert_snapshot_for_session(
        &self,
        session_id: &str,
        snapshot_id: &str,
    ) -> Result<crate::storage::FileSnapshotSummary> {
        let session = self
            .session_manager
            .get_session(session_id)
            .await?
            .ok_or_else(|| OSAgentError::Session("Session not found".to_string()))?;
        let cfg = self.config.read().await;
        let workspace = Self::resolve_workspace_for_session(&session, &cfg)
            .unwrap_or_else(|| cfg.get_active_workspace());
        drop(cfg);

        let records = self
            .storage
            .list_file_snapshot_records(session_id, snapshot_id)?;
        if records.is_empty() {
            return Err(OSAgentError::Session(format!(
                "Snapshot '{}' not found for session {}",
                snapshot_id, session_id
            )));
        }

        let root = Self::workspace_root(&workspace);
        for record in &records {
            let path = root.join(&record.path);
            if record.existed {
                if let Some(parent) = path.parent() {
                    std::fs::create_dir_all(parent)?;
                }
                std::fs::write(&path, record.content.clone().unwrap_or_default())?;
            } else if path.exists() {
                let _ = std::fs::remove_file(&path);
            }
        }

        let summary = self
            .storage
            .list_file_snapshot_summaries(session_id)?
            .into_iter()
            .find(|item| item.snapshot_id == snapshot_id)
            .ok_or_else(|| {
                OSAgentError::Session("Snapshot summary missing after restore".to_string())
            })?;
        Ok(summary)
    }
}
