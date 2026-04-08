use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::guard::{ensure_relative_path_not_backups, path_touches_backups};
use crate::tools::output::{
    maybe_store_large_output_result, path_touches_tool_outputs, LargeOutputResult,
};
use crate::tools::registry::{Tool, ToolExample, ToolResult};
use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;
use tokio::time::timeout;
use tracing::debug;
use walkdir::WalkDir;

static RG_AVAILABLE: AtomicBool = AtomicBool::new(true);

fn rg_binary_name() -> &'static str {
    if cfg!(windows) {
        "rg.exe"
    } else {
        "rg"
    }
}

fn check_rg_available() -> bool {
    let binary = rg_binary_name();
    Command::new(binary)
        .arg("--version")
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn ensure_rg_checked() -> bool {
    if RG_AVAILABLE.load(Ordering::Relaxed) {
        if check_rg_available() {
            return true;
        }
        RG_AVAILABLE.store(false, Ordering::Relaxed);
        debug!("ripgrep not found, falling back to walkdir");
    }
    false
}

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
    workspaces: Vec<PathBuf>,
    writable: bool,
    timeout_seconds: u64,
}

impl GrepTool {
    fn default_workspace(&self) -> Result<PathBuf> {
        self.workspaces.first().cloned().ok_or_else(|| {
            OSAgentError::ToolExecution(
                "No workspace configured. Set a workspace path in settings.".to_string(),
            )
        })
    }

    pub fn new(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspaces: Vec<PathBuf> = config
            .get_active_workspace()
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
        let timeout_seconds = config.tools.grep.timeout_seconds;

        Self {
            workspaces,
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

        let default_ws = self.default_workspace()?;
        let full_path = if path.is_empty() || path == "." {
            default_ws.clone()
        } else {
            default_ws.join(path)
        };

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws)) {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    async fn execute_rg_grep(
        &self,
        pattern: &str,
        search_path: &Path,
        file_pattern: Option<&str>,
        case_sensitive: bool,
        timeout_secs: u64,
    ) -> Result<(LargeOutputResult, usize)> {
        let mut cmd = Command::new(rg_binary_name());
        cmd.args([
            "--no-heading",
            "--with-filename",
            "--line-number",
            "--color=never",
            "--no-messages",
            "--hidden",
        ]);

        if !case_sensitive {
            cmd.arg("-i");
        }

        if let Some(fp) = file_pattern {
            cmd.args(["--glob", fp]);
        }

        cmd.args([
            "--glob",
            "!.osagent_backups",
            "--glob",
            "!.osa_tool_outputs",
            "--field-match-separator=:",
            "--max-count=500",
        ]);

        cmd.arg("--").arg(pattern).arg(search_path);

        let output = timeout(
            Duration::from_secs(timeout_secs),
            tokio::task::spawn_blocking(move || cmd.output()),
        )
        .await
        .map_err(|_| OSAgentError::Timeout)?
        .map_err(|e| OSAgentError::ToolExecution(e.to_string()))??;

        if !output.status.success() && !output.stderr.is_empty() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            if stderr.contains("no matches") || stderr.contains("No files") {
                return Ok((
                    LargeOutputResult {
                        display_output: "No matches found".to_string(),
                        truncated: false,
                        original_chars: 0,
                        original_lines: 0,
                        output_path: None,
                    },
                    0,
                ));
            }
        }

        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        if stdout.is_empty() {
            return Ok((
                LargeOutputResult {
                    display_output: "No matches found".to_string(),
                    truncated: false,
                    original_chars: 0,
                    original_lines: 0,
                    output_path: None,
                },
                0,
            ));
        }

        let matches = stdout.lines().count();
        Ok((
            maybe_store_large_output_result(
                &self.default_workspace()?,
                self.writable,
                "grep",
                &stdout,
            ),
            matches,
        ))
    }

    async fn execute_walkdir_grep(
        &self,
        pattern_str: &str,
        search_path: &Path,
        file_pattern: Option<&str>,
        case_sensitive: bool,
        timeout_secs: u64,
    ) -> Result<(LargeOutputResult, usize)> {
        let matcher = compile_file_matcher(file_pattern)?;
        let workspace = self.default_workspace()?;
        let pattern_str_owned = pattern_str.to_string();
        let writable = self.writable;
        let search_path = search_path.to_path_buf();

        let result = timeout(
            Duration::from_secs(timeout_secs),
            tokio::task::spawn_blocking(move || {
                let pattern = match if case_sensitive {
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
        .map_err(|_| OSAgentError::Timeout)?;

        let mut matches = result.map_err(|e| OSAgentError::ToolExecution(e.to_string()))??;

        if matches.is_empty() {
            Ok((
                LargeOutputResult {
                    display_output: "No matches found".to_string(),
                    truncated: false,
                    original_chars: 0,
                    original_lines: 0,
                    output_path: None,
                },
                0,
            ))
        } else {
            let match_count = matches.len();
            matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
            let output = matches
                .into_iter()
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
            Ok((
                maybe_store_large_output_result(
                    &self.default_workspace()?,
                    writable,
                    "grep",
                    &output,
                ),
                match_count,
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
        "Search file contents with regular expressions and optional glob filtering, preferring source/config files over build artifacts. Uses ripgrep when available for significantly faster searches."
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
        let result = self.execute_result(args).await?;
        Ok(result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let pattern_str = args["pattern"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'pattern' parameter".to_string())
        })?;

        let path = args["path"].as_str().unwrap_or(".");
        let file_pattern = args["file_pattern"].as_str();
        let case_sensitive = args["case_sensitive"].as_bool().unwrap_or(true);

        let search_path = self.validate_path(path)?;

        if ensure_rg_checked() {
            match self
                .execute_rg_grep(
                    pattern_str,
                    &search_path,
                    file_pattern,
                    case_sensitive,
                    self.timeout_seconds,
                )
                .await
            {
                Ok((result, matches)) => {
                    return Ok(ToolResult {
                        output: result.display_output,
                        title: Some(path.to_string()),
                        metadata: json!({
                            "matches": matches,
                            "truncated": result.truncated,
                            "output_path": result.output_path,
                            "original_chars": result.original_chars,
                            "original_lines": result.original_lines,
                            "path": path,
                            "file_pattern": file_pattern,
                            "case_sensitive": case_sensitive
                        }),
                    })
                }
                Err(e) => {
                    debug!("ripgrep grep failed ({}), falling back to walkdir", e);
                }
            }
        }

        let (result, matches) = self
            .execute_walkdir_grep(
                pattern_str,
                &search_path,
                file_pattern,
                case_sensitive,
                self.timeout_seconds,
            )
            .await?;

        Ok(ToolResult {
            output: result.display_output,
            title: Some(path.to_string()),
            metadata: json!({
                "matches": matches,
                "truncated": result.truncated,
                "output_path": result.output_path,
                "original_chars": result.original_chars,
                "original_lines": result.original_lines,
                "path": path,
                "file_pattern": file_pattern,
                "case_sensitive": case_sensitive
            }),
        })
    }
}

pub struct GlobTool {
    workspaces: Vec<PathBuf>,
    writable: bool,
    timeout_seconds: u64,
}

impl GlobTool {
    fn default_workspace(&self) -> Result<PathBuf> {
        self.workspaces.first().cloned().ok_or_else(|| {
            OSAgentError::ToolExecution(
                "No workspace configured. Set a workspace path in settings.".to_string(),
            )
        })
    }

    pub fn new(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspaces: Vec<PathBuf> = config
            .get_active_workspace()
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
        let timeout_seconds = config.tools.glob.timeout_seconds;

        Self {
            workspaces,
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

        let default_ws = self.default_workspace()?;
        let full_path = if path.is_empty() || path == "." {
            default_ws.clone()
        } else {
            default_ws.join(path)
        };

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws)) {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    async fn execute_rg_glob(
        &self,
        pattern: &str,
        search_path: &Path,
        timeout_secs: u64,
    ) -> Result<(LargeOutputResult, usize)> {
        let mut cmd = Command::new(rg_binary_name());
        cmd.args([
            "--files",
            "--hidden",
            "--no-messages",
            "--glob",
            pattern,
            "--glob",
            "!.osagent_backups",
            "--glob",
            "!.osa_tool_outputs",
        ]);

        cmd.arg(search_path);

        let output = match timeout(
            Duration::from_secs(timeout_secs),
            tokio::task::spawn_blocking(move || cmd.output()),
        )
        .await
        {
            Ok(inner_result) => match inner_result {
                Ok(output) => output,
                Err(e) => return Err(OSAgentError::ToolExecution(e.to_string())),
            },
            Err(_) => return Err(OSAgentError::Timeout),
        };

        let output = output.map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;
        let stdout = String::from_utf8_lossy(&output.stdout).to_string();

        if stdout.is_empty() {
            return Ok((
                LargeOutputResult {
                    display_output: "No files found matching pattern".to_string(),
                    truncated: false,
                    original_chars: 0,
                    original_lines: 0,
                    output_path: None,
                },
                0,
            ));
        }

        let workspace_str = self.default_workspace()?.to_string_lossy().to_string();
        let relative_lines: Vec<String> = stdout
            .lines()
            .filter_map(|line| {
                if line.starts_with(&workspace_str) {
                    Some(line[workspace_str.len()..].trim_start_matches(std::path::MAIN_SEPARATOR))
                        .map(|s| s.to_string())
                } else {
                    Some(line.to_string())
                }
            })
            .collect();

        if relative_lines.is_empty() {
            return Ok((
                LargeOutputResult {
                    display_output: "No files found matching pattern".to_string(),
                    truncated: false,
                    original_chars: 0,
                    original_lines: 0,
                    output_path: None,
                },
                0,
            ));
        }

        let count = relative_lines.len();
        Ok((
            maybe_store_large_output_result(
                &self.default_workspace()?,
                self.writable,
                "glob",
                &relative_lines.join("\n"),
            ),
            count,
        ))
    }

    async fn execute_walkdir_glob(
        &self,
        pattern: &str,
        search_path: &Path,
        timeout_secs: u64,
    ) -> Result<(LargeOutputResult, usize)> {
        let pattern_owned = pattern.to_string();
        let workspace = self.default_workspace()?;
        let writable = self.writable;
        let search_path = search_path.to_path_buf();

        let result = timeout(
            Duration::from_secs(timeout_secs),
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
        .map_err(|_| OSAgentError::Timeout)?;

        let mut matches = result.map_err(|e| OSAgentError::ToolExecution(e.to_string()))??;

        matches.sort_by(|a, b| a.0.cmp(&b.0).then_with(|| a.1.cmp(&b.1)));

        if matches.is_empty() {
            Ok((
                LargeOutputResult {
                    display_output: "No files found matching pattern".to_string(),
                    truncated: false,
                    original_chars: 0,
                    original_lines: 0,
                    output_path: None,
                },
                0,
            ))
        } else {
            let count = matches.len();
            let output = matches
                .into_iter()
                .map(|(_, path)| path)
                .collect::<Vec<_>>()
                .join("\n");
            Ok((
                maybe_store_large_output_result(
                    &self.default_workspace()?,
                    writable,
                    "glob",
                    &output,
                ),
                count,
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
        "Find files by glob pattern using full relative-path matching. Uses ripgrep when available for significantly faster searches."
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
        let result = self.execute_result(args).await?;
        Ok(result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let pattern = args["pattern"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'pattern' parameter".to_string())
        })?;
        let path = args["path"].as_str().unwrap_or(".");
        let search_path = self.validate_path(path)?;

        if ensure_rg_checked() {
            match self
                .execute_rg_glob(pattern, &search_path, self.timeout_seconds)
                .await
            {
                Ok((result, count)) => {
                    return Ok(ToolResult {
                        output: result.display_output,
                        title: Some(path.to_string()),
                        metadata: json!({
                            "count": count,
                            "truncated": result.truncated,
                            "output_path": result.output_path,
                            "original_chars": result.original_chars,
                            "original_lines": result.original_lines,
                            "path": path,
                            "pattern": pattern
                        }),
                    })
                }
                Err(e) => {
                    debug!("ripgrep glob failed ({}), falling back to walkdir", e);
                }
            }
        }

        let (result, count) = self
            .execute_walkdir_glob(pattern, &search_path, self.timeout_seconds)
            .await?;
        Ok(ToolResult {
            output: result.display_output,
            title: Some(path.to_string()),
            metadata: json!({
                "count": count,
                "truncated": result.truncated,
                "output_path": result.output_path,
                "original_chars": result.original_chars,
                "original_lines": result.original_lines,
                "path": path,
                "pattern": pattern
            }),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::tempdir;

    fn config_for_workspace(path: &str) -> Config {
        let mut config = Config::default();
        config.agent.workspace = path.to_string();
        config.agent.workspaces.clear();
        config.agent.active_workspace = None;
        config.ensure_workspace_defaults();
        config
    }

    #[tokio::test]
    async fn grep_returns_zero_matches_metadata() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.txt"), "hello\nworld\n").expect("write file");

        let tool = GrepTool::new(config_for_workspace(&dir.path().to_string_lossy()));
        let result = Tool::execute_result(
            &tool,
            json!({
                "pattern": "not-present",
                "path": "."
            }),
        )
        .await
        .expect("grep result");

        assert_eq!(result.metadata["matches"], 0);
        assert!(result.output.contains("No matches found"));
    }

    #[tokio::test]
    async fn glob_returns_count_metadata() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("one.rs"), "fn a() {}\n").expect("write one");
        std::fs::write(dir.path().join("two.txt"), "hello\n").expect("write two");

        let tool = GlobTool::new(config_for_workspace(&dir.path().to_string_lossy()));
        let result = Tool::execute_result(
            &tool,
            json!({
                "pattern": "**/*.rs",
                "path": "."
            }),
        )
        .await
        .expect("glob result");

        assert_eq!(result.metadata["count"], 1);
        assert!(result.output.contains("one.rs"));
    }
}
