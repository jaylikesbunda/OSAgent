use crate::error::{OSAgentError, Result};
use crate::storage::Session;
use crate::storage::{Checkpoint, CheckpointDiff, SqliteStorage};
use crate::tools::output::path_touches_tool_outputs;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::process::Command;
use tracing::warn;
use uuid::Uuid;

fn shadow_git_dir(workspace: &Path) -> PathBuf {
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    let canonical = workspace
        .canonicalize()
        .unwrap_or_else(|_| workspace.to_path_buf());
    let mut hasher = Sha256::new();
    hasher.update(canonical.to_string_lossy().as_bytes());
    let hash = format!("{:x}", hasher.finalize());
    PathBuf::from(home)
        .join(".osagent")
        .join("checkpoints")
        .join(&hash[..16])
}

pub struct CheckpointManager {
    storage: SqliteStorage,
}

impl CheckpointManager {
    pub fn new(storage: SqliteStorage) -> Self {
        Self { storage }
    }

    fn ensure_shadow_repo(&self, workspace: &Path) -> Result<PathBuf> {
        let git_dir = shadow_git_dir(workspace);
        if !git_dir.exists() {
            std::fs::create_dir_all(&git_dir).map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to create shadow git dir: {}", e))
            })?;
            self.run_shadow_git(&git_dir, workspace, &["init"])?;
        }
        let gitignore = workspace.join(".gitignore");
        let needed = match std::fs::read_to_string(&gitignore) {
            Ok(content) => !content.contains(crate::tools::output::TOOL_OUTPUT_DIR_NAME),
            Err(_) => true,
        };
        if needed {
            let line = format!("\n{}\n", crate::tools::output::TOOL_OUTPUT_DIR_NAME);
            let _ = std::fs::OpenOptions::new()
                .create(true)
                .append(true)
                .open(&gitignore)
                .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
        }
        Ok(git_dir)
    }

    pub async fn create_checkpoint(
        &self,
        session: &Session,
        tool_name: Option<String>,
        tool_input: Option<String>,
        workspace_path: Option<String>,
    ) -> Result<Checkpoint> {
        let state = serde_json::to_vec(&session)
            .map_err(|e| crate::error::OSAgentError::Parse(e.to_string()))?;

        let mut git_commit = None;
        if let Some(path) = workspace_path.as_deref() {
            match self.create_git_checkpoint(path, session.id.as_str()).await {
                Ok(commit) => {
                    git_commit = commit;
                }
                Err(error) => {
                    warn!("Failed to create git checkpoint: {}", error);
                }
            }
        }

        let checkpoint = self.storage.create_checkpoint(
            &session.id,
            state,
            tool_name,
            tool_input,
            git_commit.clone(),
            workspace_path.clone(),
        )?;

        if let (Some(path), Some(hash)) = (workspace_path.as_deref(), git_commit.as_deref()) {
            if let Err(error) = self.persist_checkpoint_diffs(path, hash, &checkpoint.id) {
                warn!("Failed to persist checkpoint diffs: {}", error);
            }
        }

        Ok(checkpoint)
    }

    async fn create_git_checkpoint(
        &self,
        workspace_path: &str,
        session_id: &str,
    ) -> Result<Option<String>> {
        let workspace = PathBuf::from(workspace_path);
        if !workspace.exists() {
            return Ok(None);
        }

        let git_dir = self.ensure_shadow_repo(&workspace)?;
        self.run_shadow_git(&git_dir, &workspace, &["add", "-A"])?;

        let checkpoint_label = format!("checkpoint {}", session_id);
        let commit_output = self.run_shadow_git(
            &git_dir,
            &workspace,
            &["commit", "--allow-empty", "-m", checkpoint_label.as_str()],
        )?;

        if commit_output.contains("nothing to commit") {
            return Ok(None);
        }

        let head = self.run_shadow_git(&git_dir, &workspace, &["rev-parse", "HEAD"])?;
        let head = head.trim();
        if head.is_empty() {
            return Ok(None);
        }

        Ok(Some(head.to_string()))
    }

    fn run_shadow_git(
        &self,
        git_dir: &Path,
        work_tree: &Path,
        args: &[&str],
    ) -> Result<String> {
        let full_args = {
            let mut a = vec![
                format!("--git-dir={}", git_dir.to_string_lossy()),
                format!("--work-tree={}", work_tree.to_string_lossy()),
            ];
            a.extend(args.iter().map(|s| s.to_string()));
            a
        };

        let output = Command::new("git")
            .args(&full_args)
            .output()
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to run git command: {}", e)))?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            return Err(OSAgentError::ToolExecution(if stderr.is_empty() {
                format!("Git command failed: git {}", args.join(" "))
            } else {
                format!("Git command failed: {}", stderr)
            }));
        }

        Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
    }

    fn persist_checkpoint_diffs(
        &self,
        workspace_path: &str,
        git_commit: &str,
        checkpoint_id: &str,
    ) -> Result<()> {
        let workspace = PathBuf::from(workspace_path);
        let git_dir = shadow_git_dir(&workspace);

        let parent = self.run_shadow_git(&git_dir, &workspace, &["rev-parse", &format!("{}^", git_commit)]);
        let base = match parent {
            Ok(value) if !value.trim().is_empty() => value,
            _ => "4b825dc642cb6eb9a060e54bf8d69288fbee4904".to_string(),
        };

        let name_status = self.run_shadow_git(
            &git_dir,
            &workspace,
            &["diff", "--name-status", base.as_str(), git_commit],
        )?;
        let mut checkpoint_diffs = Vec::new();

        for line in name_status.lines() {
            if line.trim().is_empty() {
                continue;
            }
            let mut parts = line.splitn(2, '\t');
            let status_raw = parts.next().unwrap_or("M").trim();
            let path = parts.next().unwrap_or("").trim();
            if path.is_empty() {
                continue;
            }
            if path_touches_tool_outputs(Path::new(path)) {
                continue;
            }
            let diff_output = self.run_shadow_git(
                &git_dir,
                &workspace,
                &["diff", "--unified=3", base.as_str(), git_commit, "--", path],
            )?;
            let status = if status_raw.starts_with('A') {
                "added"
            } else if status_raw.starts_with('D') {
                "deleted"
            } else {
                "modified"
            };

            checkpoint_diffs.push(CheckpointDiff {
                id: Uuid::new_v4().to_string(),
                checkpoint_id: checkpoint_id.to_string(),
                path: path.to_string(),
                diff: diff_output,
                status: status.to_string(),
            });
        }

        self.storage
            .replace_checkpoint_diffs(checkpoint_id, checkpoint_diffs.as_slice())
    }

    #[allow(dead_code)]
    pub async fn get_checkpoint(&self, id: &str) -> Result<Option<Checkpoint>> {
        self.storage.get_checkpoint(id)
    }

    pub async fn list_checkpoints(&self, session_id: &str) -> Result<Vec<Checkpoint>> {
        self.storage.list_checkpoints(session_id)
    }

    pub async fn list_changed_files(&self, checkpoint_id: &str) -> Result<Vec<PathBuf>> {
        let diffs = self.storage.list_checkpoint_diffs(checkpoint_id)?;
        Ok(diffs.into_iter().map(|item| PathBuf::from(item.path)).collect())
    }

    pub async fn checkpoint_diffs(&self, checkpoint_id: &str) -> Result<Vec<CheckpointDiff>> {
        self.storage.list_checkpoint_diffs(checkpoint_id)
    }

    pub async fn compute_diff(
        &self,
        from_checkpoint_id: &str,
        to_checkpoint_id: &str,
    ) -> Result<String> {
        let from = self
            .storage
            .get_checkpoint(from_checkpoint_id)?
            .ok_or_else(|| OSAgentError::Session("Source checkpoint not found".to_string()))?;
        let to = self
            .storage
            .get_checkpoint(to_checkpoint_id)?
            .ok_or_else(|| OSAgentError::Session("Target checkpoint not found".to_string()))?;

        let workspace = to
            .workspace_path
            .as_ref()
            .or(from.workspace_path.as_ref())
            .ok_or_else(|| OSAgentError::Session("Checkpoint has no workspace path".to_string()))?;

        let workspace_path = Path::new(workspace);
        let git_dir = shadow_git_dir(workspace_path);

        let from_commit = from
            .git_commit
            .as_ref()
            .ok_or_else(|| OSAgentError::Session("Source checkpoint has no git commit".to_string()))?;
        let to_commit = to
            .git_commit
            .as_ref()
            .ok_or_else(|| OSAgentError::Session("Target checkpoint has no git commit".to_string()))?;

        self.run_shadow_git(
            &git_dir,
            workspace_path,
            &["diff", "--unified=3", from_commit, to_commit],
        )
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
