use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::guard::{ensure_relative_path_not_backups, path_touches_backups};
use crate::tools::output::{maybe_store_large_output, path_touches_tool_outputs};
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;
use tokio::time::timeout;
use walkdir::WalkDir;

fn compile_file_matcher(pattern: Option<&str>) -> Result<Option<GlobMatcher>> {
    match pattern.map(str::trim).filter(|value| !value.is_empty()) {
        Some(pattern) => {
            let glob = Glob::new(pattern).map_err(|e| {
                OSAgentError::ToolExecution(format!("Invalid glob pattern '{}': {}", pattern, e))
            })?;
            Ok(Some(glob.compile_matcher()))
        }
        None => Ok(None),
    }
}

fn path_matches(matcher: Option<&GlobMatcher>, relative_path: &Path) -> bool {
    match matcher {
        Some(matcher) => matcher.is_match(relative_path),
        None => true,
    }
}

fn discouraged_path_penalty(relative_path: &Path) -> usize {
    let mut penalty = 0usize;

    for component in relative_path.components() {
        let part = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        penalty += match part.as_str() {
            "build" | "target" | "node_modules" => 120,
            "dist" | "out" | ".cache" | "build.cache" => 90,
            ".git" | ".idea" | ".vscode" => 60,
            _ => 0,
        };
    }

    if let Some(name) = relative_path.file_name().and_then(|name| name.to_str()) {
        let lower = name.to_ascii_lowercase();
        if lower.ends_with(".o")
            || lower.ends_with(".obj")
            || lower.ends_with(".idx")
            || lower.ends_with(".a")
            || lower.ends_with(".so")
            || lower.ends_with(".dll")
            || lower.ends_with(".exe")
        {
            penalty += 60;
        }
    }

    penalty
}

fn path_sort_key(relative_path: &Path) -> (usize, usize, String) {
    (
        discouraged_path_penalty(relative_path),
        relative_path.components().count(),
        relative_path.display().to_string(),
    )
}

pub struct GrepTool {
    workspace: PathBuf,
    writable: bool,
    timeout_seconds: u64,
}

impl GrepTool {
    pub fn new(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        let timeout_seconds = config.tools.grep.timeout_seconds;

        if !workspace.exists() {
            let _ = fs::create_dir_all(&workspace);
        }

        let canonical_workspace = workspace.canonicalize().unwrap_or(workspace);

        Self {
            workspace: canonical_workspace,
            writable,
            timeout_seconds,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = if path.is_empty() || path == "." {
            self.workspace.clone()
        } else {
            self.workspace.join(path)
        };

        if full_path.starts_with(&self.workspace) {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for GrepTool {
    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with regular expressions and optional glob filtering, preferring source/config files over build artifacts"
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to find text or code patterns across files; start with focused paths or file patterns when possible"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when you only need file names or a full file read, and avoid broad generated/build trees unless they are specifically relevant"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Search all Rust files for TODO".to_string(),
                input: json!({
                    "pattern": "TODO",
                    "file_pattern": "**/*.rs"
                }),
            },
            ToolExample {
                description: "Case-insensitive search".to_string(),
                input: json!({
                    "pattern": "error",
                    "case_sensitive": false
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Regular expression pattern to search for"
                },
                "path": {
                    "type": "string",
                    "description": "Relative path to search in (default: workspace root)"
                },
                "file_pattern": {
                    "type": "string",
                    "description": "Glob pattern to filter files (for example '**/*.rs' or 'src/**/*.py')"
                },
                "case_sensitive": {
                    "type": "boolean",
                    "description": "Case sensitive search (default: true)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let pattern_str = args["pattern"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'pattern' parameter".to_string())
        })?;

        let path = args["path"].as_str().unwrap_or(".");
        let file_pattern = args["file_pattern"].as_str();
        let case_sensitive = args["case_sensitive"].as_bool().unwrap_or(true);

        let search_path = self.validate_path(path)?;
        let matcher = compile_file_matcher(file_pattern)?;

        let pattern_str_owned = pattern_str.to_string();
        let case_sensitive_owned = case_sensitive;
        let workspace = self.workspace.clone();
        let timeout_seconds = self.timeout_seconds;

        let result = timeout(
            Duration::from_secs(timeout_seconds),
            tokio::task::spawn_blocking(move || {
                let pattern = match if case_sensitive_owned {
                    Regex::new(&pattern_str_owned)
                } else {
                    Regex::new(&format!("(?i){}", pattern_str_owned))
                } {
                    Ok(p) => p,
                    Err(e) => {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Invalid regex pattern: {}",
                            e
                        )))
                    }
                };

                let mut matches: Vec<((usize, usize, String), String)> = Vec::new();

                for entry in WalkDir::new(&search_path)
                    .into_iter()
                    .filter_map(|entry| entry.ok())
                {
                    let entry_path = entry.path();
                    if !entry.file_type().is_file() {
                        continue;
                    }

                    let relative_path = entry_path.strip_prefix(&workspace).unwrap_or(entry_path);
                    if path_touches_backups(relative_path) {
                        continue;
                    }
                    if path_touches_tool_outputs(relative_path)
                        && search_path != workspace.join(".osa_tool_outputs")
                    {
                        continue;
                    }
                    if !path_matches(matcher.as_ref(), relative_path) {
                        continue;
                    }

                    let Ok(content) = fs::read_to_string(entry_path) else {
                        continue;
                    };

                    for (line_num, line) in content.lines().enumerate() {
                        if pattern.is_match(line) {
                            matches.push((
                                path_sort_key(relative_path),
                                format!("{}:{}: {}", relative_path.display(), line_num + 1, line),
                            ));
                        }
                    }
                }

                Ok(matches)
            }),
        )
        .await
        .map_err(|_| OSAgentError::Timeout)?
        .map_err(|e| OSAgentError::ToolExecution(e.to_string()))??;

        let mut matches = result;

        if matches.is_empty() {
            Ok("No matches found".to_string())
        } else {
            matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            Ok(maybe_store_large_output(
                &self.workspace,
                self.writable,
                "grep",
                &matches
                    .into_iter()
                    .map(|(_, line)| line)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ))
        }
    }
}

pub struct GlobTool {
    workspace: PathBuf,
    writable: bool,
    timeout_seconds: u64,
}

impl GlobTool {
    pub fn new(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
        let timeout_seconds = config.tools.glob.timeout_seconds;

        if !workspace.exists() {
            let _ = fs::create_dir_all(&workspace);
        }

        let canonical_workspace = workspace.canonicalize().unwrap_or(workspace);

        Self {
            workspace: canonical_workspace,
            writable,
            timeout_seconds,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;

        if path.contains("..") {
            return Err(OSAgentError::ToolExecution(
                "Path cannot contain '..'".to_string(),
            ));
        }

        let full_path = if path.is_empty() || path == "." {
            self.workspace.clone()
        } else {
            self.workspace.join(path)
        };

        if full_path.starts_with(&self.workspace) {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for GlobTool {
    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by glob pattern using full relative-path matching, preferring source/config files over build artifacts"
    }

    fn when_to_use(&self) -> &str {
        "Use when you need file paths that match a naming or directory pattern; start with focused directories when possible"
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when searching inside file contents, and avoid broad generated/build trees unless they are specifically relevant"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Find all Rust files".to_string(),
                input: json!({
                    "pattern": "**/*.rs"
                }),
            },
            ToolExample {
                description: "Search under a subdirectory".to_string(),
                input: json!({
                    "pattern": "src/**/*.ts",
                    "path": "frontend"
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "pattern": {
                    "type": "string",
                    "description": "Glob pattern (for example '**/*.rs' or 'src/**/*.ts')"
                },
                "path": {
                    "type": "string",
                    "description": "Relative path to search in (default: workspace root)"
                }
            },
            "required": ["pattern"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let pattern = args["pattern"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'pattern' parameter".to_string())
        })?;
        let path = args["path"].as_str().unwrap_or(".");
        let search_path = self.validate_path(path)?;

        let pattern_owned = pattern.to_string();
        let workspace = self.workspace.clone();
        let timeout_seconds = self.timeout_seconds;

        let result = timeout(
            Duration::from_secs(timeout_seconds),
            tokio::task::spawn_blocking(move || {
                let matcher = match Glob::new(&pattern_owned) {
                    Ok(glob) => glob.compile_matcher(),
                    Err(e) => {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Invalid glob pattern '{}': {}",
                            pattern_owned, e
                        )))
                    }
                };

                let mut matches: Vec<((usize, usize, String), String)> = Vec::new();
                for entry in WalkDir::new(&search_path)
                    .into_iter()
                    .filter_map(|entry| entry.ok())
                {
                    if !entry.file_type().is_file() {
                        continue;
                    }

                    let relative_path = entry
                        .path()
                        .strip_prefix(&workspace)
                        .unwrap_or(entry.path());
                    if path_touches_backups(relative_path) {
                        continue;
                    }
                    if path_touches_tool_outputs(relative_path)
                        && search_path != workspace.join(".osa_tool_outputs")
                    {
                        continue;
                    }
                    if matcher.is_match(relative_path) {
                        matches.push((
                            path_sort_key(relative_path),
                            relative_path.display().to_string(),
                        ));
                    }
                }

                Ok(matches)
            }),
        )
        .await
        .map_err(|_| OSAgentError::Timeout)?
        .map_err(|e| OSAgentError::ToolExecution(e.to_string()))??;

        let mut matches = result;

        matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        if matches.is_empty() {
            Ok("No files found matching pattern".to_string())
        } else {
            Ok(maybe_store_large_output(
                &self.workspace,
                self.writable,
                "glob",
                &matches
                    .into_iter()
                    .map(|(_, path)| path)
                    .collect::<Vec<_>>()
                    .join("\n"),
            ))
        }
    }
}
