use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::file_cache::FileReadCache;
use crate::tools::guard::ensure_relative_path_not_backups;
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn workspace_is_read_only(config: &Config) -> bool {
    if let Some(workspace) = config.get_workspace_by_path(&config.agent.workspace) {
        return !workspace.permission.allows_writes();
    }

    false
}

#[derive(Debug, Clone)]
enum PatchLine {
    Context(String),
    Remove(String),
    Add(String),
}

#[derive(Debug, Clone)]
struct PatchHunk {
    lines: Vec<PatchLine>,
}

#[derive(Debug, Clone)]
enum PatchOperation {
    Add {
        path: String,
        content: String,
    },
    Delete {
        path: String,
    },
    Update {
        path: String,
        move_to: Option<String>,
        hunks: Vec<PatchHunk>,
    },
}

pub struct ApplyPatchTool {
    workspace: PathBuf,
    backup_dir: PathBuf,
    cache: Arc<FileReadCache>,
}

impl ApplyPatchTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
            return Self {
                workspace,
                backup_dir: PathBuf::new(),
                cache,
            };
        }

        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        if !workspace.exists() {
            let _ = fs::create_dir_all(&workspace);
        }

        let canonical_workspace = workspace.canonicalize().unwrap_or(workspace);
        let backup_dir = canonical_workspace.join(".osagent_backups");
        if !backup_dir.exists() {
            let _ = fs::create_dir_all(&backup_dir);
        }

        Self {
            workspace: canonical_workspace,
            backup_dir,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;

        if path.trim().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Path cannot be empty".to_string(),
            ));
        }

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = self.workspace.join(path);
        if full_path.starts_with(&self.workspace) {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    fn ensure_parent_dir(path: &PathBuf) -> Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to create directory: {}", e))
            })?;
        }
        Ok(())
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<Option<PathBuf>> {
        if !file_path.exists() {
            return Ok(None);
        }

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("unknown");
        let backup_path = self
            .backup_dir
            .join(format!("{}_{}.bak", file_name, timestamp));

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(Some(backup_path))
    }

    fn normalize_text(text: &str) -> String {
        text.replace("\r\n", "\n").replace('\r', "\n")
    }

    fn split_lines(text: &str) -> Vec<String> {
        if text.is_empty() {
            return Vec::new();
        }

        Self::normalize_text(text)
            .split_terminator('\n')
            .map(|line| line.to_string())
            .collect()
    }

    fn parse_patch(&self, patch: &str) -> Result<Vec<PatchOperation>> {
        let normalized = Self::normalize_text(patch);
        let lines: Vec<&str> = normalized.split('\n').collect();

        if lines.first().copied() != Some("*** Begin Patch") {
            return Err(OSAgentError::ToolExecution(
                "Patch must start with '*** Begin Patch'".to_string(),
            ));
        }

        if lines.last().copied() != Some("*** End Patch") {
            return Err(OSAgentError::ToolExecution(
                "Patch must end with '*** End Patch'".to_string(),
            ));
        }

        let mut idx = 1usize;
        let mut operations = Vec::new();

        while idx + 1 < lines.len() {
            let line = lines[idx];
            if line.trim().is_empty() {
                idx += 1;
                continue;
            }

            if line == "*** End Patch" {
                break;
            }

            if let Some(path) = line.strip_prefix("*** Add File: ") {
                idx += 1;
                let mut content_lines = Vec::new();
                while idx < lines.len() {
                    let body_line = lines[idx];
                    if body_line.starts_with("*** ") {
                        break;
                    }

                    if let Some(rest) = body_line.strip_prefix('+') {
                        content_lines.push(rest.to_string());
                    } else {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Add file patches require '+' lines only: {}",
                            body_line
                        )));
                    }
                    idx += 1;
                }

                operations.push(PatchOperation::Add {
                    path: path.trim().to_string(),
                    content: content_lines.join("\n"),
                });
                continue;
            }

            if let Some(path) = line.strip_prefix("*** Delete File: ") {
                operations.push(PatchOperation::Delete {
                    path: path.trim().to_string(),
                });
                idx += 1;
                continue;
            }

            if let Some(path) = line.strip_prefix("*** Update File: ") {
                idx += 1;
                let mut move_to = None;
                if idx < lines.len() {
                    if let Some(target) = lines[idx].strip_prefix("*** Move to: ") {
                        move_to = Some(target.trim().to_string());
                        idx += 1;
                    }
                }

                let mut hunks = Vec::new();
                while idx < lines.len() {
                    let hunk_header = lines[idx];
                    if hunk_header.starts_with("*** ") {
                        break;
                    }

                    if hunk_header.trim().is_empty() {
                        idx += 1;
                        continue;
                    }

                    if !hunk_header.starts_with("@@") {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Expected hunk header starting with '@@', got '{}'",
                            hunk_header
                        )));
                    }

                    idx += 1;
                    let mut hunk_lines = Vec::new();
                    while idx < lines.len() {
                        let body_line = lines[idx];
                        if body_line.starts_with("@@") || body_line.starts_with("*** ") {
                            break;
                        }

                        match body_line.chars().next() {
                            Some(' ') => {
                                hunk_lines.push(PatchLine::Context(body_line[1..].to_string()))
                            }
                            Some('-') => {
                                hunk_lines.push(PatchLine::Remove(body_line[1..].to_string()))
                            }
                            Some('+') => {
                                hunk_lines.push(PatchLine::Add(body_line[1..].to_string()))
                            }
                            _ => {
                                return Err(OSAgentError::ToolExecution(format!(
                                    "Invalid patch line '{}'. Use space, '+', or '-'.",
                                    body_line
                                )))
                            }
                        }

                        idx += 1;
                    }

                    if hunk_lines.is_empty() {
                        return Err(OSAgentError::ToolExecution(
                            "Patch hunk cannot be empty".to_string(),
                        ));
                    }

                    hunks.push(PatchHunk { lines: hunk_lines });
                }

                if hunks.is_empty() {
                    return Err(OSAgentError::ToolExecution(format!(
                        "Update patch for '{}' must include at least one hunk",
                        path.trim()
                    )));
                }

                operations.push(PatchOperation::Update {
                    path: path.trim().to_string(),
                    move_to,
                    hunks,
                });
                continue;
            }

            return Err(OSAgentError::ToolExecution(format!(
                "Unknown patch header '{}'",
                line
            )));
        }

        if operations.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Patch did not contain any file operations".to_string(),
            ));
        }

        Ok(operations)
    }

    fn find_hunk_start(
        original_lines: &[String],
        search_start: usize,
        expected: &[String],
    ) -> Option<usize> {
        if expected.is_empty() {
            return Some(search_start.min(original_lines.len()));
        }

        let max_start = original_lines.len().checked_sub(expected.len())?;
        for start in search_start..=max_start {
            if original_lines[start..start + expected.len()] == *expected {
                return Some(start);
            }
        }

        None
    }

    fn apply_hunks(&self, original: &str, hunks: &[PatchHunk]) -> Result<String> {
        let eol = if original.contains("\r\n") {
            "\r\n"
        } else {
            "\n"
        };
        let had_trailing_newline = original.ends_with('\n');
        let original_lines = Self::split_lines(original);
        let mut result = Vec::new();
        let mut cursor = 0usize;

        for hunk in hunks {
            let expected: Vec<String> = hunk
                .lines
                .iter()
                .filter_map(|line| match line {
                    PatchLine::Add(_) => None,
                    PatchLine::Context(value) | PatchLine::Remove(value) => Some(value.clone()),
                })
                .collect();

            let start =
                Self::find_hunk_start(&original_lines, cursor, &expected).ok_or_else(|| {
                    OSAgentError::ToolExecution(
                        "Failed to locate patch hunk in target file".to_string(),
                    )
                })?;

            result.extend_from_slice(&original_lines[cursor..start]);

            let mut source_idx = start;
            for line in &hunk.lines {
                match line {
                    PatchLine::Context(value) => {
                        if original_lines.get(source_idx).map(String::as_str)
                            != Some(value.as_str())
                        {
                            return Err(OSAgentError::ToolExecution(
                                "Patch context did not match file contents".to_string(),
                            ));
                        }
                        result.push(value.clone());
                        source_idx += 1;
                    }
                    PatchLine::Remove(value) => {
                        if original_lines.get(source_idx).map(String::as_str)
                            != Some(value.as_str())
                        {
                            return Err(OSAgentError::ToolExecution(
                                "Patch removal did not match file contents".to_string(),
                            ));
                        }
                        source_idx += 1;
                    }
                    PatchLine::Add(value) => result.push(value.clone()),
                }
            }

            cursor = source_idx;
        }

        result.extend_from_slice(&original_lines[cursor..]);

        let mut output = result.join(eol);
        if had_trailing_newline && !output.is_empty() {
            output.push_str(eol);
        }
        Ok(output)
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply structured multi-file patches with add, update, move, and delete operations"
    }

    fn when_to_use(&self) -> &str {
        "Use for precise multi-hunk edits, coordinated changes across files, or when exact replacements are too fragile"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for simple one-off replacements that edit_file can handle safely"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![ToolExample {
            description: "Update a file with a diff-style patch".to_string(),
            input: json!({
                "patch": "*** Begin Patch\n*** Update File: src/main.rs\n@@\n-fn old() {}\n+fn new() {}\n*** End Patch"
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "patch": {
                    "type": "string",
                    "description": "Patch text using the custom format with *** Begin Patch / *** End Patch envelopes"
                }
            },
            "required": ["patch"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; patch operations are disabled".to_string(),
            ));
        }

        let patch = args["patch"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'patch' parameter".to_string()))?;

        let operations = self.parse_patch(patch)?;
        let mut results = Vec::new();

        for operation in operations {
            match operation {
                PatchOperation::Add { path, content } => {
                    let file_path = self.validate_path(&path)?;
                    if file_path.exists() {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Cannot add '{}': file already exists",
                            path
                        )));
                    }

                    Self::ensure_parent_dir(&file_path)?;
                    fs::write(&file_path, content).map_err(|e| {
                        OSAgentError::ToolExecution(format!("Failed to write file: {}", e))
                    })?;
                    if let Ok(canonical) = file_path.canonicalize() {
                        self.cache.invalidate(&canonical);
                    }
                    results.push(format!("Added {}", path));
                }
                PatchOperation::Delete { path } => {
                    let file_path = self.validate_path(&path)?;
                    if !file_path.exists() {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Cannot delete '{}': file does not exist",
                            path
                        )));
                    }

                    let canonical = file_path.canonicalize().ok();
                    let _ = self.create_backup(&file_path)?;
                    fs::remove_file(&file_path).map_err(|e| {
                        OSAgentError::ToolExecution(format!("Failed to delete file: {}", e))
                    })?;
                    if let Some(canonical) = canonical {
                        self.cache.invalidate(&canonical);
                    }
                    results.push(format!("Deleted {}", path));
                }
                PatchOperation::Update {
                    path,
                    move_to,
                    hunks,
                } => {
                    let source_path = self.validate_path(&path)?;
                    if !source_path.exists() {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Cannot update '{}': file does not exist",
                            path
                        )));
                    }

                    let original = fs::read_to_string(&source_path).map_err(|e| {
                        OSAgentError::ToolExecution(format!("Failed to read file: {}", e))
                    })?;
                    let updated = self.apply_hunks(&original, &hunks)?;
                    let target_path = if let Some(target) = &move_to {
                        self.validate_path(target)?
                    } else {
                        source_path.clone()
                    };

                    let _ = self.create_backup(&source_path)?;
                    if target_path != source_path {
                        Self::ensure_parent_dir(&target_path)?;
                        fs::write(&target_path, updated).map_err(|e| {
                            OSAgentError::ToolExecution(format!(
                                "Failed to write moved file: {}",
                                e
                            ))
                        })?;
                        fs::remove_file(&source_path).map_err(|e| {
                            OSAgentError::ToolExecution(format!(
                                "Failed to remove original file after move: {}",
                                e
                            ))
                        })?;
                        if let Ok(canonical) = source_path.canonicalize() {
                            self.cache.invalidate(&canonical);
                        }
                        if let Ok(canonical) = target_path.canonicalize() {
                            self.cache.invalidate(&canonical);
                        }
                        results.push(format!(
                            "Updated {} and moved to {}",
                            path,
                            move_to.unwrap()
                        ));
                    } else {
                        fs::write(&source_path, updated).map_err(|e| {
                            OSAgentError::ToolExecution(format!("Failed to write file: {}", e))
                        })?;
                        if let Ok(canonical) = source_path.canonicalize() {
                            self.cache.invalidate(&canonical);
                        }
                        results.push(format!("Updated {}", path));
                    }
                }
            }
        }

        Ok(results.join("\n"))
    }
}
