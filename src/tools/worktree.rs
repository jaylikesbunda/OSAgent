//! Git worktrees as agent-callable tools (`worktree_create` / `worktree_list`
//! / `worktree_remove`), following OpenCode's tool shape. The agent spins up
//! isolated checkouts for parallel exploration: each worktree is an
//! independent working tree on its own branch, so subagents can read/analyze
//! (or experiment) without stepping on each other's edits.
//!
//! The created directory can be used anywhere a path is accepted (workdir for
//! bash/process, or added as a workspace root) — the tool output says so
//! explicitly so the model routes follow-up work there.

use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolExample, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::{Path, PathBuf};
use std::process::Stdio;

/// Where worktrees land: a `.worktrees` folder next to the repository, so
/// they live beside the repo (like OpenCode's sibling worktrees) without
/// polluting the repository itself.
fn worktree_base_dir(repo_root: &Path) -> PathBuf {
    repo_root
        .parent()
        .map(|parent| {
            parent.join(format!(
                ".worktrees-{}",
                repo_root.file_name().unwrap_or_default().to_string_lossy()
            ))
        })
        .unwrap_or_else(|| repo_root.join(".worktrees"))
}

fn sanitize_name(name: &str) -> Result<String> {
    let trimmed = name.trim();
    if trimmed.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "worktree name cannot be empty".to_string(),
        ));
    }
    if trimmed.len() > 64 {
        return Err(OSAgentError::ToolExecution(
            "worktree name too long (max 64 chars)".to_string(),
        ));
    }
    let cleaned: String = trimmed
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' || ch == '.' {
                ch
            } else {
                '-'
            }
        })
        .collect();
    let cleaned = cleaned.trim_matches(['.', '-']).to_string();
    if cleaned.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "worktree name must contain letters, digits, '-', '_' or '.'".to_string(),
        ));
    }
    Ok(cleaned)
}

fn run_git(cwd: &Path, args: &[&str]) -> Result<String> {
    let output = std::process::Command::new("git")
        .args(args)
        .current_dir(cwd)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .output()
        .map_err(|e| OSAgentError::ToolExecution(format!("failed to run git: {}", e)))?;
    if !output.status.success() {
        return Err(OSAgentError::ToolExecution(format!(
            "git {} failed: {}",
            args.join(" "),
            String::from_utf8_lossy(&output.stderr).trim()
        )));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_string())
}

fn repo_root(start: &Path) -> Result<PathBuf> {
    let root = run_git(start, &["rev-parse", "--show-toplevel"])?;
    Ok(PathBuf::from(root))
}

fn git_available() -> bool {
    std::process::Command::new("git")
        .arg("--version")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .map(|status| status.success())
        .unwrap_or(false)
}

fn ensure_git_available() -> Result<()> {
    if git_available() {
        Ok(())
    } else {
        Err(OSAgentError::ToolExecution(
            "git is not installed or not on PATH".to_string(),
        ))
    }
}

// ---------------------------------------------------------------------------
// worktree_create
// ---------------------------------------------------------------------------

pub struct WorktreeCreateTool {
    workspace: PathBuf,
}

impl WorktreeCreateTool {
    pub fn new(config: Config) -> Self {
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        Self { workspace }
    }

    fn resolved_start(&self, base: Option<&str>) -> Result<PathBuf> {
        match base {
            Some(base) => {
                let path = PathBuf::from(shellexpand::tilde(base).to_string());
                if path.exists() {
                    Ok(path)
                } else {
                    Err(OSAgentError::ToolExecution(format!(
                        "base directory does not exist: {}",
                        base
                    )))
                }
            }
            None => Ok(self.workspace.clone()),
        }
    }
}

#[async_trait]
impl Tool for WorktreeCreateTool {
    fn name(&self) -> &str {
        "worktree_create"
    }

    fn description(&self) -> &str {
        "Create an isolated git worktree at `<repo>/.worktrees-<name>` with its own branch, so parallel work cannot collide. Returns the directory - use it as a bash `workdir`, or add it as a workspace root for full tool access."
    }

    fn when_to_use(&self) -> &str {
        "an experiment must not disturb the main checkout, or parallel subagents"
    }

    fn when_not_to_use(&self) -> &str {
        "ordinary edits in the current workspace"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "create a worktree named 'auth-probe'".to_string(),
            input: json!({"name": "auth-probe"}),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "Short name for the worktree (used for the directory and branch)."
                },
                "base": {
                    "type": "string",
                    "description": "Optional repository directory to create the worktree from. Defaults to the workspace."
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        ensure_git_available()?;
        let name =
            sanitize_name(args["name"].as_str().ok_or_else(|| {
                OSAgentError::ToolExecution("Missing 'name' parameter".to_string())
            })?)?;
        let start = self.resolved_start(args["base"].as_str())?;
        let repo = repo_root(&start)?;

        let base_dir = worktree_base_dir(&repo);
        std::fs::create_dir_all(&base_dir).map_err(|e| {
            OSAgentError::ToolExecution(format!("cannot create worktree base dir: {}", e))
        })?;
        let directory = base_dir.join(&name);

        if directory.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "worktree directory already exists: {} (use worktree_list / worktree_remove)",
                directory.display()
            )));
        }

        // Branch name mirrors the worktree name; `git worktree add -b` fails
        // if the branch exists, which is the honest outcome (names are unique
        // per worktree base dir anyway).
        run_git(
            &repo,
            &[
                "worktree",
                "add",
                "-b",
                &name,
                &directory.to_string_lossy(),
                "HEAD",
            ],
        )?;

        Ok(format!(
            "Created git worktree '{}' on branch '{}'\nDirectory: {}\nUse this directory as workdir for bash/process, or add it as a workspace root to run full tooling there. Remove it with worktree_remove when done.",
            name,
            name,
            directory.display()
        ))
    }
}

// ---------------------------------------------------------------------------
// worktree_list
// ---------------------------------------------------------------------------

pub struct WorktreeListTool {
    workspace: PathBuf,
}

impl WorktreeListTool {
    pub fn new(config: Config) -> Self {
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        Self { workspace }
    }
}

/// One entry of OpenCode's `worktree_list` shape: `directory` + optional
/// `strategy`.
#[derive(serde::Serialize)]
struct WorktreeEntry {
    directory: String,
    strategy: String,
}

#[async_trait]
impl Tool for WorktreeListTool {
    fn name(&self) -> &str {
        "worktree_list"
    }

    fn description(&self) -> &str {
        "List the repository git worktrees, including the main checkout, with each directory and its strategy."
    }

    fn when_to_use(&self) -> &str {
        "before creating a worktree, or to find one again"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "base": {
                    "type": "string",
                    "description": "Optional repository directory. Defaults to the workspace."
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        ensure_git_available()?;
        let start = match args["base"].as_str() {
            Some(base) => PathBuf::from(shellexpand::tilde(base).to_string()),
            None => self.workspace.clone(),
        };
        let repo = repo_root(&start)?;

        let output = run_git(&repo, &["worktree", "list", "--porcelain"])?;
        let mut entries: Vec<WorktreeEntry> = Vec::new();
        let mut current_dir: Option<String> = None;
        let mut current_branch: Option<String> = None;
        let mut flush = |entries: &mut Vec<WorktreeEntry>,
                         current_dir: &mut Option<String>,
                         current_branch: &mut Option<String>| {
            if let Some(directory) = current_dir.take() {
                entries.push(WorktreeEntry {
                    strategy: current_branch
                        .take()
                        .unwrap_or_else(|| "detached".to_string()),
                    directory,
                });
            }
        };

        for line in output.lines() {
            if let Some(path) = line.strip_prefix("worktree ") {
                flush(&mut entries, &mut current_dir, &mut current_branch);
                current_dir = Some(path.to_string());
            } else if let Some(branch) = line.strip_prefix("branch ") {
                current_branch = Some(branch.trim_start_matches("refs/heads/").to_string());
            }
        }
        flush(&mut entries, &mut current_dir, &mut current_branch);

        if entries.is_empty() {
            return Ok("No git worktrees found.".to_string());
        }
        let listed = entries
            .iter()
            .map(|entry| format!("{} ({})", entry.directory, entry.strategy))
            .collect::<Vec<_>>()
            .join("\n");
        Ok(format!(
            "Git worktrees:\n{}\n\nJSON: {}",
            listed,
            serde_json::to_string(&entries).unwrap_or_default()
        ))
    }
}

// ---------------------------------------------------------------------------
// worktree_remove
// ---------------------------------------------------------------------------

pub struct WorktreeRemoveTool {
    workspace: PathBuf,
}

impl WorktreeRemoveTool {
    pub fn new(config: Config) -> Self {
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        Self { workspace }
    }
}

#[async_trait]
impl Tool for WorktreeRemoveTool {
    fn name(&self) -> &str {
        "worktree_remove"
    }

    fn description(&self) -> &str {
        "Remove a git worktree by absolute `directory` from worktree_create or worktree_list. Modified or untracked files block it unless `force` discards them - only force when the user confirmed those changes can be lost."
    }

    fn when_to_use(&self) -> &str {
        "cleaning up an exploration whose findings are merged or copied back"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "remove a worktree after finishing the investigation".to_string(),
            input: json!({"directory": "C:/code/.worktrees-myrepo/auth-probe"}),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "directory": {
                    "type": "string",
                    "description": "Absolute path of the worktree to remove."
                },
                "force": {
                    "type": "boolean",
                    "description": "Discard modified and untracked files. Only set this when the user has confirmed forced removal."
                }
            },
            "required": ["directory"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        ensure_git_available()?;
        let directory = args["directory"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'directory' parameter".to_string())
        })?;
        let directory = PathBuf::from(shellexpand::tilde(directory).to_string());
        if !directory.is_absolute() {
            return Err(OSAgentError::ToolExecution(
                "directory must be an absolute path (see worktree_list)".to_string(),
            ));
        }
        if !directory.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "worktree directory does not exist: {}",
                directory.display()
            )));
        }

        let start = self.workspace.clone();
        let repo = repo_root(&start).unwrap_or_else(|_| directory.clone());

        let force = args["force"].as_bool().unwrap_or(false);
        let directory_str = directory.to_string_lossy().to_string();
        let mut git_args = vec!["worktree", "remove"];
        if force {
            git_args.push("--force");
        }
        git_args.push(&directory_str);
        run_git(&repo, &git_args)?;

        // Prune so the repo stops tracking the removed worktree.
        let _ = run_git(&repo, &["worktree", "prune"]);

        Ok(format!(
            "Removed git worktree at {}{}",
            directory.display(),
            if force { " (forced)" } else { "" }
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_repo() -> tempfile::TempDir {
        let dir = tempfile::tempdir().expect("tempdir");
        run_git(dir.path(), &["init", "-q"]).expect("git init");
        run_git(dir.path(), &["config", "user.email", "osa@test"]).expect("email");
        run_git(dir.path(), &["config", "user.name", "OSA Test"]).expect("name");
        std::fs::write(dir.path().join("readme.txt"), "hello").unwrap();
        run_git(dir.path(), &["add", "."]).expect("add");
        run_git(dir.path(), &["commit", "-qm", "init"]).expect("commit");
        dir
    }

    fn tool_config(workspace: &Path) -> Config {
        let mut config = Config::default();
        config.agent.workspace = workspace.to_string_lossy().to_string();
        config
    }

    #[tokio::test]
    async fn worktree_roundtrip_create_list_remove() {
        let repo = init_repo();
        let config = tool_config(repo.path());

        let create = WorktreeCreateTool::new(config.clone());
        let result = create
            .execute(json!({"name": "probe one"}))
            .await
            .expect("create");
        assert!(
            result.contains("Created git worktree 'probe-one'"),
            "{}",
            result
        );

        let list = WorktreeListTool::new(config.clone());
        let listed = list.execute(json!({})).await.expect("list");
        assert!(listed.contains("probe-one"), "{}", listed);

        // The worktree directory exists and is on its own branch.
        let base = worktree_base_dir(&repo_root(repo.path()).unwrap());
        let directory = base.join("probe-one");
        assert!(directory.exists());
        let branch = run_git(&directory, &["rev-parse", "--abbrev-ref", "HEAD"]).unwrap();
        assert_eq!(branch, "probe-one");

        // Duplicate names are refused.
        let duplicate = create.execute(json!({"name": "probe-one"})).await;
        assert!(duplicate.is_err());

        let remove = WorktreeRemoveTool::new(config.clone());
        let removed = remove
            .execute(json!({"directory": directory.to_string_lossy()}))
            .await
            .expect("remove");
        assert!(removed.contains("Removed"), "{}", removed);
        assert!(!directory.exists());

        let listed = list.execute(json!({})).await.expect("list");
        assert!(!listed.contains("probe-one"), "{}", listed);
    }

    #[tokio::test]
    async fn name_sanitizing_and_validation() {
        assert_eq!(sanitize_name("auth probe").unwrap(), "auth-probe");
        assert_eq!(sanitize_name("ok_name.1").unwrap(), "ok_name.1");
        assert!(sanitize_name("").is_err());
        assert!(sanitize_name("///").is_err());
        assert!(sanitize_name(&"x".repeat(100)).is_err());
    }

    #[tokio::test]
    async fn remove_rejects_relative_paths() {
        let repo = init_repo();
        let remove = WorktreeRemoveTool::new(tool_config(repo.path()));
        let result = remove.execute(json!({"directory": "relative/path"})).await;
        assert!(result.is_err());
    }
}
