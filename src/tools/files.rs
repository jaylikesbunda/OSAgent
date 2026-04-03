use crate::agent::instruction::{format_system_reminder, nearby_instruction_blocks};
use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::file_cache::FileReadCache;
use crate::tools::fuzzy_edit::{apply_replacement, fuzzy_find};
use crate::tools::guard::{ensure_relative_path_not_backups, path_touches_backups};
use crate::tools::output::path_touches_tool_outputs;
use crate::tools::registry::Tool;
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn workspace_is_read_only(config: &Config) -> bool {
    if let Some(workspace) = config.get_workspace_by_path(&config.agent.workspace) {
        if let Some((_, wp)) = config.get_workspace_for_path(&config.agent.workspace) {
            return !wp.permission.allows_writes();
        }
        return !workspace.permission.allows_writes();
    }

    false
}

fn path_is_in_workspace(path: &str, config: &Config) -> bool {
    config.is_path_in_workspace(path)
}

fn get_workspace_path_for(path: &str, config: &Config) -> Option<std::path::PathBuf> {
    config
        .get_workspace_for_path(path)
        .map(|(ws, wp)| std::path::PathBuf::from(&wp.path))
}

fn ensure_workspace(workspaces: &[PathBuf]) -> Result<()> {
    if workspaces.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "No workspace configured. Set a workspace path in settings.".to_string(),
        ));
    }
    Ok(())
}

pub struct ReadFileTool {
    workspaces: Vec<PathBuf>,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl ReadFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        let active_workspace = config.get_active_workspace();
        let workspaces: Vec<PathBuf> = active_workspace
            .paths
            .iter()
            .map(|wp| {
                let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                path.canonicalize().unwrap_or(path)
            })
            .collect();

        Self {
            workspaces,
            config,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = self.workspaces[0].join(path);
        let full_path_str = full_path.to_string_lossy().to_string();

        if !full_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "File not found: {}",
                path
            )));
        }

        if path_touches_tool_outputs(&full_path) && !path.starts_with(".osa_tool_outputs") {
            return Err(OSAgentError::ToolExecution(
                "Tool output files must be read by explicit .osa_tool_outputs path".to_string(),
            ));
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path_str, &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a file from the workspace directory with optional line ranges"
    }

    fn when_to_use(&self) -> &str {
        "Use after locating a specific file path and before making edits"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for broad discovery; use glob, grep, or list_files first"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Read a whole file".to_string(),
                input: json!({
                    "path": "src/main.rs"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Read a focused line range".to_string(),
                input: json!({
                    "path": "src/main.rs",
                    "start_line": 40,
                    "end_line": 90
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within workspace"
                },
                "start_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional 1-based start line for partial reads"
                },
                "end_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional 1-based end line for partial reads"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;
        let start_line = args["start_line"].as_u64().unwrap_or(1) as usize;
        let end_line_arg = args["end_line"].as_u64().map(|value| value as usize);

        let file_path = self.validate_path(path)?;

        let canonical = file_path
            .canonicalize()
            .unwrap_or_else(|_| file_path.clone());

        let wants_range = start_line != 1 || end_line_arg.is_some();

        if !wants_range {
            if let Some(entry) = self.cache.check_hit(&canonical) {
                return Ok(FileReadCache::format_stub(&entry, &file_path));
            }
        }

        let content = fs::read_to_string(&file_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;

        if content.is_empty() {
            return Ok("(empty file)".to_string());
        }

        let lines: Vec<&str> = content.lines().collect();
        let total_lines = lines.len();
        if total_lines == 0 {
            return Ok("(empty file)".to_string());
        }

        if !wants_range {
            self.cache.update(&canonical, &content);
        }

        let start_line = start_line.max(1);
        let end_line = end_line_arg.unwrap_or(total_lines).min(total_lines);

        if start_line > end_line {
            return Err(OSAgentError::ToolExecution(
                "start_line must be less than or equal to end_line".to_string(),
            ));
        }

        if start_line > total_lines {
            return Err(OSAgentError::ToolExecution(format!(
                "start_line {} is past end of file ({} lines)",
                start_line, total_lines
            )));
        }

        let mut output = lines[start_line - 1..end_line]
            .iter()
            .enumerate()
            .map(|(index, line)| format!("{}: {}", start_line + index, line))
            .collect::<Vec<_>>()
            .join("\n");

        if start_line != 1 || end_line != total_lines {
            output.push_str(&format!(
                "\n[showing lines {}-{} of {}]",
                start_line, end_line, total_lines
            ));
        }

        if let Some(reminder) =
            format_system_reminder(&nearby_instruction_blocks(&self.workspaces[0], &file_path))
        {
            output.push_str("\n\n");
            output.push_str(&reminder);
        }

        Ok(output)
    }
}

pub struct WriteFileTool {
    workspaces: Vec<PathBuf>,
    backup_dir: PathBuf,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl WriteFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspaces: Vec<PathBuf> = config
                .get_active_workspace()
                .paths
                .iter()
                .map(|wp| {
                    let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                    path.canonicalize().unwrap_or(path)
                })
                .collect();
            return Self {
                workspaces,
                backup_dir: PathBuf::new(),
                config,
                cache,
            };
        }

        let active_workspace = config.get_active_workspace();
        let mut workspaces: Vec<PathBuf> = Vec::new();
        let mut backup_dir = PathBuf::new();

        for (i, wp) in active_workspace.paths.iter().enumerate() {
            let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
            let canonical = path.canonicalize().unwrap_or(path.clone());

            if i == 0 {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                backup_dir = canonical.join(".osagent_backups");
                if !backup_dir.exists() {
                    let _ = fs::create_dir_all(&backup_dir);
                }
            }
            workspaces.push(canonical);
        }

        Self {
            workspaces,
            backup_dir,
            config,
            cache,
        }
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<Option<PathBuf>> {
        if !file_path.exists() {
            return Ok(None);
        }

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let backup_name = format!("{}_{}.bak", file_name, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(Some(backup_path))
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = self.workspaces[0].join(path);

        if let Some(parent) = full_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to create directory: {}", e))
                })?;
            }
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Write content to a file in the workspace directory"
    }

    fn when_to_use(&self) -> &str {
        "Use for creating new files or replacing most of a file with new content"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for small in-place edits; use edit_file or apply_patch instead"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within workspace"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; write operations are disabled".to_string(),
            ));
        }

        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let content = args["content"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'content' parameter".to_string())
        })?;

        let file_path = self.validate_path(path)?;

        let backup = self.create_backup(&file_path)?;

        fs::write(&file_path, content)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;

        if let Ok(canonical) = file_path.canonicalize() {
            self.cache.invalidate(&canonical);
        }

        let backup_msg = if let Some(backup_path) = backup {
            format!(" (backup created at {:?})", backup_path)
        } else {
            String::new()
        };

        Ok(format!("Successfully wrote to {}{}", path, backup_msg))
    }
}

pub struct EditFileTool {
    workspaces: Vec<PathBuf>,
    backup_dir: PathBuf,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl EditFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspaces: Vec<PathBuf> = config
                .get_active_workspace()
                .paths
                .iter()
                .map(|wp| {
                    let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                    path.canonicalize().unwrap_or(path)
                })
                .collect();
            return Self {
                workspaces,
                backup_dir: PathBuf::new(),
                config,
                cache,
            };
        }

        let active_workspace = config.get_active_workspace();
        let mut workspaces: Vec<PathBuf> = Vec::new();
        let mut backup_dir = PathBuf::new();

        for (i, wp) in active_workspace.paths.iter().enumerate() {
            let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
            let canonical = path.canonicalize().unwrap_or(path.clone());

            if i == 0 {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                backup_dir = canonical.join(".osagent_backups");
                if !backup_dir.exists() {
                    let _ = fs::create_dir_all(&backup_dir);
                }
            }
            workspaces.push(canonical);
        }

        Self {
            workspaces,
            backup_dir,
            config,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = self.workspaces[0].join(path);

        if !full_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "File not found: {}",
                path
            )));
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<PathBuf> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let backup_name = format!("{}_{}.bak", file_name, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(backup_path)
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Edit a file by replacing text with fuzzy matching (creates automatic backup)"
    }

    fn when_to_use(&self) -> &str {
        "Use for small text replacements; supports exact matching and fuzzy fallbacks (line-trimmed, whitespace-normalized, indentation-flexible, block-anchor, context-aware) for robust edits"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for multi-hunk edits, large rewrites, or when apply_patch is more appropriate"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within workspace"
                },
                "old_text": {
                    "type": "string",
                    "description": "Text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "Text to replace with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; edit operations are disabled".to_string(),
            ));
        }

        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let old_text = args["old_text"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'old_text' parameter".to_string())
        })?;

        let new_text = args["new_text"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'new_text' parameter".to_string())
        })?;

        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        if old_text.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "'old_text' cannot be empty".to_string(),
            ));
        }

        let file_path = self.validate_path(path)?;

        if let Ok(canonical) = file_path.canonicalize() {
            if let Some(entry) = self.cache.check(&canonical) {
                if let Ok(meta) = fs::metadata(&canonical) {
                    let current_mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if current_mtime != entry.mtime_secs {
                        return Err(OSAgentError::ToolExecution(
                            "File has been modified since last read. Re-read the file first with read_file, then retry the edit.".to_string(),
                        ));
                    }
                }
            }
        }

        let _backup_path = self.create_backup(&file_path)?;

        let content = fs::read_to_string(&file_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;

        if replace_all {
            let match_count = content.match_indices(old_text).count();
            if match_count == 0 {
                return Err(OSAgentError::ToolExecution(
                    "Text not found in file (exact match for replace_all)".to_string(),
                ));
            }
            let new_content = content.replace(old_text, new_text);
            fs::write(&file_path, new_content)
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;
            if let Ok(canonical) = file_path.canonicalize() {
                self.cache.invalidate(&canonical);
            }
            return Ok(format!(
                "Successfully edited {} ({} replacement{})",
                path,
                match_count,
                if match_count == 1 { "" } else { "s" }
            ));
        }

        let exact_count = content.match_indices(old_text).count();
        if exact_count == 1 {
            let new_content = content.replacen(old_text, new_text, 1);
            fs::write(&file_path, new_content)
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;
            if let Ok(canonical) = file_path.canonicalize() {
                self.cache.invalidate(&canonical);
            }
            return Ok(format!(
                "Successfully edited {} (1 replacement, exact match)",
                path
            ));
        }

        if exact_count > 1 {
            return Err(OSAgentError::ToolExecution(format!(
                "Text matched {} times; refine 'old_text', set replace_all=true, or use apply_patch",
                exact_count
            )));
        }

        let match_result = fuzzy_find(&content, old_text).ok_or_else(|| {
            OSAgentError::ToolExecution(
                "Text not found in file (tried exact, line-trimmed, whitespace-normalized, indentation-flexible, escape-normalized, trimmed-boundary, block-anchor, and context-aware matching)".to_string(),
            )
        })?;

        let new_content = apply_replacement(&content, &match_result, old_text, new_text);

        fs::write(&file_path, new_content)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;

        if let Ok(canonical) = file_path.canonicalize() {
            self.cache.invalidate(&canonical);
        }

        Ok(format!(
            "Successfully edited {} (1 replacement via {} matching, confidence: {:.0}%)",
            path,
            match_result.strategy,
            match_result.confidence * 100.0
        ))
    }
}

pub struct ListFilesTool {
    workspaces: Vec<PathBuf>,
    config: Config,
}

impl ListFilesTool {
    pub fn new(config: Config) -> Self {
        let active_workspace = config.get_active_workspace();
        let workspaces: Vec<PathBuf> = active_workspace
            .paths
            .iter()
            .map(|wp| {
                let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                path.canonicalize().unwrap_or(path)
            })
            .collect();

        Self { workspaces, config }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = if path.is_empty() || path == "." {
            self.workspaces[0].clone()
        } else {
            self.workspaces[0].join(path)
        };

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories in the workspace"
    }

    fn when_to_use(&self) -> &str {
        "Use for quick directory inspection when you need to understand the local file layout"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when you need content search or already know the exact file path"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to directory (default: workspace root)"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "List recursively (default: false)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().unwrap_or("");
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        let dir_path = self.validate_path(path)?;

        if !dir_path.exists() {
            return Err(OSAgentError::ToolExecution(
                "Directory not found".to_string(),
            ));
        }

        if !dir_path.is_dir() {
            return Err(OSAgentError::ToolExecution(
                "Path is not a directory".to_string(),
            ));
        }

        let mut results = Vec::new();

        fn list_dir(
            dir: &PathBuf,
            base: &PathBuf,
            results: &mut Vec<String>,
            recursive: bool,
        ) -> Result<()> {
            let entries = fs::read_dir(dir).map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to read directory: {}", e))
            })?;

            for entry in entries {
                let entry = entry.map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to read entry: {}", e))
                })?;
                let path = entry.path();
                let relative = path.strip_prefix(base).unwrap_or(&path);

                if path_touches_backups(relative) {
                    continue;
                }
                if path_touches_tool_outputs(relative) {
                    continue;
                }

                let type_str = if path.is_dir() { "DIR" } else { "FILE" };
                results.push(format!("[{}] {}", type_str, relative.display()));

                if recursive && path.is_dir() {
                    list_dir(&path, base, results, recursive)?;
                }
            }

            Ok(())
        }

        list_dir(&dir_path, &self.workspaces[0], &mut results, recursive)?;

        if results.is_empty() {
            Ok("Empty directory".to_string())
        } else {
            Ok(results.join("\n"))
        }
    }
}

pub struct DeleteFileTool {
    workspaces: Vec<PathBuf>,
    backup_dir: PathBuf,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl DeleteFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspaces: Vec<PathBuf> = config
                .get_active_workspace()
                .paths
                .iter()
                .map(|wp| {
                    let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                    path.canonicalize().unwrap_or(path)
                })
                .collect();
            return Self {
                workspaces,
                backup_dir: PathBuf::new(),
                config,
                cache,
            };
        }

        let active_workspace = config.get_active_workspace();
        let mut workspaces: Vec<PathBuf> = Vec::new();
        let mut backup_dir = PathBuf::new();

        for (i, wp) in active_workspace.paths.iter().enumerate() {
            let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
            let canonical = path.canonicalize().unwrap_or(path.clone());

            if i == 0 {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                backup_dir = canonical.join(".osagent_backups");
                if !backup_dir.exists() {
                    let _ = fs::create_dir_all(&backup_dir);
                }
            }
            workspaces.push(canonical);
        }

        Self {
            workspaces,
            backup_dir,
            config,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = self.workspaces[0].join(path);

        if !full_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "File not found: {}",
                path
            )));
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<PathBuf> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let backup_name = format!("{}_{}_DELETED.bak", file_name, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(backup_path)
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "Delete a file from the workspace (creates backup before deletion)"
    }

    fn when_to_use(&self) -> &str {
        "Use only when a file truly needs removal and the user requested or clearly implied that change"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for routine edits or when keeping history in-place is safer"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file to delete"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; delete operations are disabled".to_string(),
            ));
        }

        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let file_path = self.validate_path(path)?;

        let canonical = file_path.canonicalize().ok();

        let backup_path = self.create_backup(&file_path)?;

        fs::remove_file(&file_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to delete file: {}", e)))?;

        if let Some(canonical) = canonical {
            self.cache.invalidate(&canonical);
        }

        Ok(format!(
            "Successfully deleted {} (backup at {:?})",
            path, backup_path
        ))
    }
}
