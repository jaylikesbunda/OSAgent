use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::lsp::client::LspClient;
use crate::tools::file_cache::FileReadCache;
use crate::tools::guard::ensure_relative_path_not_backups;
use crate::tools::registry::{Tool, ToolExample};
use async_trait::async_trait;
use chrono::Utc;
use serde_json::{json, Value};
use std::cmp::max;
use std::collections::HashMap;
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
    lsp: LspClient,
}

impl ApplyPatchTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspace = PathBuf::from(shellexpand::tilde(&config.agent.workspace).to_string());
            return Self {
                workspace,
                backup_dir: PathBuf::new(),
                cache,
                lsp: LspClient::new(HashMap::new()),
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
            lsp: LspClient::new(HashMap::new()),
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;

        if path.trim().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Path cannot be empty".to_string(),
            ));
        }

        let full_path = self.workspace.join(path);
        let full_path = full_path.canonicalize().unwrap_or(full_path);
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

        // 1. Exact match
        for start in search_start..=max_start {
            if original_lines[start..start + expected.len()] == *expected {
                return Some(start);
            }
        }

        // 2. Line-trimmed match (whitespace around lines may differ)
        for start in search_start..=max_start {
            let window = &original_lines[start..start + expected.len()];
            if window
                .iter()
                .zip(expected.iter())
                .all(|(a, b)| a.trim() == b.trim())
            {
                return Some(start);
            }
        }

        // 3. Similarity-based fuzzy match
        let best = (search_start..=max_start)
            .map(|start| {
                let window = &original_lines[start..start + expected.len()];
                let sim = line_block_similarity(window, expected);
                (start, sim)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal));

        if let Some((start, similarity)) = best {
            if similarity >= 0.8 {
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
                        let Some(actual) = original_lines.get(source_idx) else {
                            return Err(OSAgentError::ToolExecution(
                                "Patch context extended past end of file".to_string(),
                            ));
                        };
                        if actual != value && actual.trim() != value.trim() {
                            return Err(OSAgentError::ToolExecution(format!(
                                "Patch context did not match file contents (line {}: expected '{}', found '{}')",
                                source_idx + 1,
                                value,
                                actual
                            )));
                        }
                        result.push(actual.clone());
                        source_idx += 1;
                    }
                    PatchLine::Remove(value) => {
                        let Some(actual) = original_lines.get(source_idx) else {
                            return Err(OSAgentError::ToolExecution(
                                "Patch removal extended past end of file".to_string(),
                            ));
                        };
                        if actual != value && actual.trim() != value.trim() {
                            return Err(OSAgentError::ToolExecution(format!(
                                "Patch removal did not match file contents (line {}: expected '{}', found '{}')",
                                source_idx + 1,
                                value,
                                actual
                            )));
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

    async fn check_lsp_diagnostics(&self, file_path: &PathBuf, display_path: &str) -> String {
        let path_str = file_path.to_string_lossy().to_string();
        let workspace = self.workspace.clone();
        let result = tokio::time::timeout(
            std::time::Duration::from_secs(25),
            self.lsp.diagnostics(&path_str, &workspace),
        )
        .await;

        let Ok(diagnostics) = result else {
            return String::new();
        };

        let interesting: Vec<_> = diagnostics
            .iter()
            .filter(|d| d.severity <= 2 && !d.message.is_empty())
            .take(15)
            .collect();
        if interesting.is_empty() {
            return String::new();
        }

        let mut out = format!("LSP diagnostics for {}:\n", display_path);
        for d in interesting {
            let level = if d.severity == 1 { "error" } else { "warning" };
            let code = d
                .code
                .as_deref()
                .map(|c| format!(" [{}]", c))
                .unwrap_or_default();
            let source = d
                .source
                .as_deref()
                .map(|s| format!(" ({})", s))
                .unwrap_or_default();
            out.push_str(&format!(
                "- line {}:{}: {}{}: {}{}\n",
                d.line + 1,
                d.character + 1,
                level,
                code,
                d.message,
                source
            ));
        }
        out
    }
}

#[async_trait]
impl Tool for ApplyPatchTool {
    fn name(&self) -> &str {
        "apply_patch"
    }

    fn description(&self) -> &str {
        "Apply structured multi-file patches for atomic changes across one or more files.\n\nUsage:\n- Use for precise multi-hunk edits where edit_file would require multiple calls.\n- Use for coordinated changes across multiple files in a single operation.\n- The patch format uses *** Begin Patch / *** End Patch envelopes with *** Add File / *** Update File / *** Delete File headers.\n- Update hunks use @@ markers with - (remove) and + (add) lines, like unified diff.\n- Hunk matching falls back to whitespace-insensitive and similarity-based matching when the context is not byte-exact.\n- Read the target files first before constructing a patch.\n- After applying, LSP diagnostics (when an LSP server is available) may be reported so you can fix errors.\n- Creates automatic backups in .osagent_backups before modifying."
    }

    fn when_to_use(&self) -> &str {
        "Use when you need to change multiple locations in a file or across files atomically. Preferred over edit_file for complex changes spanning several sections."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for simple single replacements that edit_file can handle."
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
                    let diagnostics = self.check_lsp_diagnostics(&file_path, &path).await;
                    if !diagnostics.is_empty() {
                        results.push(diagnostics);
                    }
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
                        let diagnostics = self.check_lsp_diagnostics(&target_path, &path).await;
                        if !diagnostics.is_empty() {
                            results.push(diagnostics);
                        }
                    } else {
                        fs::write(&source_path, updated).map_err(|e| {
                            OSAgentError::ToolExecution(format!("Failed to write file: {}", e))
                        })?;
                        if let Ok(canonical) = source_path.canonicalize() {
                            self.cache.invalidate(&canonical);
                        }
                        results.push(format!("Updated {}", path));
                        let diagnostics = self.check_lsp_diagnostics(&target_path, &path).await;
                        if !diagnostics.is_empty() {
                            results.push(diagnostics);
                        }
                    }
                }
            }
        }

        Ok(results.join("\n"))
    }
}

fn line_block_similarity(actual: &[String], expected: &[String]) -> f64 {
    if actual.len() != expected.len() || expected.is_empty() {
        return 0.0;
    }

    let matching = actual
        .iter()
        .zip(expected.iter())
        .filter(|(a, b)| a.trim() == b.trim())
        .count();
    let line_sim = matching as f64 / expected.len() as f64;

    let actual_joined: String = actual.iter().map(|l| l.trim()).collect::<Vec<_>>().join("");
    let expected_joined: String = expected
        .iter()
        .map(|l| l.trim())
        .collect::<Vec<_>>()
        .join("");
    let char_sim = lcs_ratio(&actual_joined, &expected_joined);

    0.5 * line_sim + 0.5 * char_sim
}

fn lcs_ratio(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let mut prev = vec![0usize; b_chars.len() + 1];
    let mut curr = vec![0usize; b_chars.len() + 1];

    for i in 1..=a_chars.len() {
        for j in 1..=b_chars.len() {
            curr[j] = if a_chars[i - 1] == b_chars[j - 1] {
                prev[j - 1] + 1
            } else {
                prev[j].max(curr[j - 1])
            };
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    let lcs = prev[b_chars.len()] as f64;
    let max_len = max(a_chars.len(), b_chars.len()).max(1);
    lcs / max_len as f64
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lines(content: &str) -> Vec<String> {
        content.lines().map(|l| l.to_string()).collect()
    }

    #[test]
    fn exact_hunk_located() {
        let original = lines("aaa\nbbb\nfoo\nbar\nbaz\n");
        let expected = lines("foo\nbar\nbaz");
        assert_eq!(
            ApplyPatchTool::find_hunk_start(&original, 0, &expected),
            Some(2)
        );
    }

    #[test]
    fn hunk_search_resumes_after_cursor() {
        let original = lines("foo\nbar\nbaz\nfoo\nbar\nbaz\n");
        let expected = lines("foo\nbar\nbaz");
        assert_eq!(
            ApplyPatchTool::find_hunk_start(&original, 3, &expected),
            Some(3)
        );
    }

    #[test]
    fn trimmed_hunk_located() {
        let original = lines("fn foo() {\n  let x = 1;\n}\nfn bar() {\n}");
        let expected = lines("fn foo() {\n    let x = 1;\n}");
        assert_eq!(
            ApplyPatchTool::find_hunk_start(&original, 0, &expected),
            Some(0)
        );
    }

    #[test]
    fn fuzzy_hunk_located_by_similarity() {
        let original = lines("fn foo() {\n    let x = 1;\n    let y = 2;\n}");
        let expected = lines("fn foo() {\n    let x = 1;\n    let y = 3;\n}");
        assert_eq!(
            ApplyPatchTool::find_hunk_start(&original, 0, &expected),
            Some(0)
        );
    }

    #[test]
    fn similarity_rejects_unrelated_block() {
        let original = lines("fn foo() {\n    let x = 1;\n    let y = 2;\n}");
        let expected = lines("fn bar() {\n    let z = 9;\n}");
        let start = ApplyPatchTool::find_hunk_start(&original, 0, &expected);
        assert!(start.is_none() || start.unwrap() > original.len() - expected.len());
    }

    #[test]
    fn line_block_similarity_scores() {
        assert_eq!(
            line_block_similarity(&lines("a\nb\nc"), &lines("a\nb\nc")),
            1.0
        );
        assert_eq!(
            line_block_similarity(&lines("a\nb"), &lines("a\nb\nc")),
            0.0
        );
        let sim = line_block_similarity(
            &lines("fn foo() {\n    let x = 1;\n}"),
            &lines("fn foo() {\n    let x = 2;\n}"),
        );
        assert!(sim >= 0.8, "expected similarity >= 0.8, got {}", sim);
    }
}
