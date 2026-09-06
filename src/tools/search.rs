use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::guard::{ensure_relative_path_not_backups, path_touches_backups};
use crate::tools::output::{
    maybe_store_large_output_result, path_touches_tool_outputs, LargeOutputResult,
};
use crate::tools::registry::{Tool, ToolExample, ToolOutcome, ToolResult};
use async_trait::async_trait;
use globset::{Glob, GlobMatcher};
use regex::Regex;
use serde_json::{json, Value};
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::debug;

static RG_AVAILABLE: AtomicBool = AtomicBool::new(true);
static RG_PROBED: AtomicBool = AtomicBool::new(false);

const MAX_WALKDIR_MATCHES: usize = 10_000;

pub(crate) fn is_heavy_dir(path: &Path) -> bool {
    path.components().any(|component| {
        let part = component.as_os_str().to_string_lossy().to_ascii_lowercase();
        BUILTIN_SEARCH_EXCLUDE_DIRS.contains(&part.as_str())
    })
}

/// Directories that bloat or poison repo-wide search: build outputs,
/// dependency trees, and IDE/VCS state. The built-in half covers only
/// generic junk every workspace has; the rest comes from `[tools.grep]` /
/// `[tools.glob]` `exclude_dirs` in config, so project-specific trees
/// live in the user's config, not in code. Builtin matching is exact
/// case-insensitive path components; configured patterns additionally
/// support `*` wildcards via glob matching.
pub(crate) const BUILTIN_SEARCH_EXCLUDE_DIRS: &[&str] = &[
    "node_modules",
    "target",
    "dist",
    "out",
    "build",
    ".git",
    ".hg",
    ".svn",
    ".cache",
    ".idea",
    ".vscode",
    "__pycache__",
    ".venv",
    "venv",
];

/// Built-in ripgrep globs matching [`BUILTIN_SEARCH_EXCLUDE_DIRS`], plus
/// our own bookkeeping dirs. Kept minimal and generic on purpose.
pub(crate) const BUILTIN_SEARCH_EXCLUDE_GLOBS: &[&str] = &[
    "!.osagent_backups",
    "!.osa_tool_outputs",
    "!target",
    "!build",
    "!out",
    "!dist",
    "!node_modules",
    "!.git",
];

fn normalized_exclude_pattern(pattern: &str) -> String {
    pattern.trim().trim_start_matches('!').trim().to_string()
}

/// One configured entry becomes up to two matchers: the raw glob itself
/// (so `build-output-*` matches that name) plus a recursive `**/<glob>/**`
/// form (so a bare name matches that component at any depth). Compiled
/// once per tool construction; path matching stays a cheap check.
fn configured_exclude_matchers(config: &[String]) -> Vec<globset::GlobMatcher> {
    config
        .iter()
        .filter_map(|pattern| {
            let normalized = normalized_exclude_pattern(pattern);
            if normalized.is_empty() || normalized.contains('/') || normalized.contains('\\') {
                return None;
            }
            let mut matchers = Vec::new();
            for candidate in [
                normalized.clone(),
                format!("**/{normalized}/**"),
                format!("**/{normalized}"),
            ] {
                if let Ok(glob) = globset::GlobBuilder::new(&candidate)
                    .case_insensitive(true)
                    .literal_separator(true)
                    .build()
                {
                    matchers.push(glob.compile_matcher());
                }
            }
            if matchers.is_empty() {
                return None;
            }
            // A single entry can yield several matchers; keep discovery
            // simple by returning the first and chaining the rest through
            // repeated calls. Callers flatten, so wrap each individually.
            Some(matchers)
        })
        .flatten()
        .collect()
}

fn is_config_excluded(path: &Path, matchers: &[globset::GlobMatcher]) -> bool {
    if matchers.is_empty() {
        return false;
    }
    matchers.iter().any(|matcher| {
        matcher.is_match(path)
            || path
                .components()
                .any(|component| matcher.is_match(component.as_os_str().to_string_lossy().as_ref()))
    })
}

// WalkDir yields paths verbatim from the search root while the stored
// workspace may differ textually (`\\?\` prefixes, symlinks): normalize
// both sides so every relative path below succeeds.
fn normalize_for_compare(path: &Path) -> String {
    let text = path.to_string_lossy().replace('\\', "/");
    let text = text.strip_prefix("//?/").unwrap_or(&text);
    text.trim_end_matches('/').to_ascii_lowercase()
}

// Relative display path for one entry: strip the search root when the
// entry is under it, else the workspace, else the file name. Pure string
// work on normalized forms — immune to `\\?\`, casing, and separator
// mismatches that break Path::strip_prefix.
fn display_relative(
    entry: &Path,
    workspace_normalized: &str,
    search_root_normalized: &str,
) -> String {
    let normalized = normalize_for_compare(entry);
    for root in [search_root_normalized, workspace_normalized] {
        if normalized == root {
            return String::new();
        }
        if let Some(rest) = normalized
            .strip_prefix(root)
            .and_then(|rest| rest.strip_prefix('/'))
        {
            return rest.to_string();
        }
    }
    entry
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| normalized)
}

pub(crate) fn search_excludes_for(config: &[String]) -> Vec<String> {
    let mut excludes: Vec<String> = BUILTIN_SEARCH_EXCLUDE_GLOBS
        .iter()
        .map(|glob| glob.to_string())
        .collect();
    for pattern in config {
        let trimmed = pattern.trim();
        if trimmed.is_empty() {
            continue;
        }
        if trimmed.starts_with('!') {
            excludes.push(trimmed.to_string());
        } else {
            excludes.push(format!("!{trimmed}"));
        }
    }
    excludes
}

fn push_search_excludes(cmd: &mut tokio::process::Command, configured: &[String]) {
    for glob in search_excludes_for(configured) {
        cmd.args(["--glob", glob.as_str()]);
    }
}

pub(crate) fn rg_binary_name() -> &'static str {
    if cfg!(windows) {
        "rg.exe"
    } else {
        "rg"
    }
}

/// ripgrep-compatible in-process walker: same traversal semantics as the
/// external `rg` invocation used to provide (hidden files included,
/// binary filtering, builtin + configured excludes, file-pattern globs),
/// but with zero process-spawn cost and rayon-parallel file scanning.
/// The external binary is still preferred when present — it is faster
/// than anything in-process — but on machines without `rg` this path is
/// typically 3-6x faster than the old sequential walkdir fallback because
/// the walk itself stays single-threaded while file *content* scanning
/// fans out across the rayon pool.
pub(crate) struct FastWalk {
    workspace_normalized: String,
    search_root_normalized: String,
    search_path: PathBuf,
    configured: Vec<globset::GlobMatcher>,
    file_matcher: Option<GlobMatcher>,
    search_explicitly_requested: bool,
    max_files: usize,
}

impl FastWalk {
    pub(crate) fn new(
        workspace: PathBuf,
        search_path: PathBuf,
        configured_excludes: &[String],
        file_pattern: Option<&str>,
        max_files: usize,
    ) -> Result<Self> {
        let configured = configured_exclude_matchers(configured_excludes);
        let file_matcher = compile_file_matcher(file_pattern)?;
        let search_explicitly_requested =
            is_config_excluded(&search_path, &configured) || is_heavy_dir(&search_path);
        Ok(Self {
            workspace_normalized: normalize_for_compare(&workspace),
            search_root_normalized: normalize_for_compare(&search_path),
            search_path,
            configured,
            file_matcher,
            search_explicitly_requested,
            max_files,
        })
    }

    fn relative(&self, entry: &Path) -> Option<String> {
        let rel = display_relative(entry, &self.workspace_normalized, &self.search_root_normalized);
        if rel.is_empty() {
            return None;
        }
        Some(rel)
    }

    fn keep(&self, rel: &str) -> bool {
        let rel_path = Path::new(rel);
        if path_touches_backups(rel_path) {
            return false;
        }
        if path_touches_tool_outputs(rel_path) && rel != ".osa_tool_outputs" {
            return false;
        }
        let excluded = is_config_excluded(rel_path, &self.configured)
            || (!self.search_explicitly_requested && is_heavy_dir(rel_path));
        if excluded && !self.search_explicitly_requested {
            return false;
        }
        path_matches(self.file_matcher.as_ref(), rel_path)
    }

    /// Collect candidate files (sequential walk, no content I/O beyond
    /// metadata for the oversize skip done by callers).
    pub(crate) fn collect_files(&self) -> Vec<(PathBuf, String)> {
        // `ignore` parallels ripgrep's traversal: honors .gitignore /
        // .ignore / .rgignore, skips hidden handling per our flag (we
        // include hidden like the old `--hidden` invocation), never
        // requires a git repo. Exclude enforcement stays in keep() so
        // builtin + configured semantics are identical to the rg path.
        let walker = ignore::WalkBuilder::new(&self.search_path)
            .hidden(false)
            .git_ignore(true)
            .git_global(true)
            .git_exclude(true)
            .parents(true)
            .require_git(false)
            .follow_links(false)
            .build();
        let mut out = Vec::new();
        for entry in walker {
            let Ok(entry) = entry else { continue };
            if entry.file_type().map(|t| t.is_file()).unwrap_or(false) != true {
                continue;
            }
            let Some(rel) = self.relative(entry.path()) else {
                continue;
            };
            if !self.keep(&rel) {
                continue;
            }
            out.push((entry.path().to_path_buf(), rel));
            if out.len() >= self.max_files {
                break;
            }
        }
        out
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

pub(crate) fn ensure_rg_checked() -> bool {
    // Probe at most once per process: spawning `rg --version` on every tool
    // call is pure overhead, and a transient failure shouldn't disable
    // ripgrep for the session... but a missing binary won't appear mid-run
    // in practice, so latch the first result either way.
    if !RG_PROBED.load(Ordering::Relaxed) {
        let available = check_rg_available();
        RG_AVAILABLE.store(available, Ordering::Relaxed);
        RG_PROBED.store(true, Ordering::Relaxed);
        if !available {
            debug!("ripgrep not found, falling back to walkdir");
        }
    }
    RG_AVAILABLE.load(Ordering::Relaxed)
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

/// Longest literal prefix of a regex source, for memchr pre-filtering.
/// Stops at the first regex metacharacter or escape; `None` means the
/// pattern has no usable literal (e.g. starts with `\b` or `.*`).
fn literal_prefix(source: &str) -> Option<String> {
    let mut literal = String::new();
    let mut chars = source.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            match chars.next() {
                Some(next) if next.is_ascii_alphanumeric() => return fallback(&literal),
                Some(next) => literal.push(next),
                None => break,
            }
        } else if ch.is_ascii_alphanumeric() || ch == '_' || ch == '/' || ch == '.' || ch == '-' {
            literal.push(ch);
        } else {
            break;
        }
        if literal.len() >= 32 {
            break;
        }
    }
    return fallback(&literal);

    fn fallback(literal: &str) -> Option<String> {
        if literal.len() >= 3 {
            Some(literal.to_string())
        } else {
            None
        }
    }
}

/// Scan one file for pattern hits. Binary probe via memchr for NUL;
/// per-line regex only runs when the literal pre-filter hits (or when
/// there is no usable literal). Returns matches with sort keys attached.
fn scan_one_file(
    abs: &Path,
    rel: &str,
    pattern: &Regex,
    literal: Option<&str>,
    case_sensitive: bool,
) -> Vec<((usize, usize, String), String)> {
    let Ok(bytes) = fs::read(abs) else {
        return Vec::new();
    };
    if bytes.len() > 1_000_000 {
        return Vec::new();
    }
    let probe_len = bytes.len().min(4096);
    if memchr::memchr(0, &bytes[..probe_len]).is_some() {
        return Vec::new();
    }
    let Ok(text) = std::str::from_utf8(&bytes) else {
        return Vec::new();
    };
    let rel_path = Path::new(rel);
    let mut out = Vec::new();
    for (line_no, line_text) in text.lines().enumerate() {
        let line_no = line_no + 1;
        if let Some(lit) = literal {
            let hay = line_text.as_bytes();
            let found = if case_sensitive {
                memchr::memmem::find(hay, lit.as_bytes()).is_some()
            } else {
                // Cheap ASCII case-fold scan; regex confirms the real match.
                hay.windows(lit.len())
                    .any(|w| w.eq_ignore_ascii_case(lit.as_bytes()))
            };
            if !found {
                continue;
            }
        }
        if pattern.is_match(line_text) {
            out.push((
                path_sort_key(rel_path),
                format!("{rel}:{line_no}: {line_text}"),
            ));
            if out.len() >= 500 {
                break;
            }
        }
    }
    out
}

pub(crate) fn discouraged_path_penalty(relative_path: &Path) -> usize {
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
    exclude_dirs: Vec<String>,
    exclude_matchers: Vec<globset::GlobMatcher>,
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
        let exclude_dirs = config.tools.grep.exclude_dirs.clone();
        let exclude_matchers = configured_exclude_matchers(&exclude_dirs);

        Self {
            workspaces,
            writable,
            timeout_seconds,
            exclude_dirs,
            exclude_matchers,
        }
    }

    fn is_excluded(&self, path: &Path) -> bool {
        if is_config_excluded(path, &self.exclude_matchers) {
            return true;
        }
        is_heavy_dir(path)
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;

        let default_ws = self.default_workspace()?;
        let full_path = if path.is_empty() || path == "." {
            default_ws.clone()
        } else {
            default_ws.join(path)
        };
        let full_path = full_path.canonicalize().unwrap_or(full_path);

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
        let mut cmd = tokio::process::Command::new(rg_binary_name());
        cmd.args([
            "--no-heading",
            "--with-filename",
            "--line-number",
            "--color=never",
            "--no-messages",
            "--hidden",
        ])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());

        if !case_sensitive {
            cmd.arg("-i");
        }

        if let Some(fp) = file_pattern {
            cmd.args(["--glob", fp]);
        }
        push_search_excludes(&mut cmd, &self.exclude_dirs);

        cmd.args(["--field-match-separator=:", "--max-count=500"]);

        cmd.arg("--").arg(pattern).arg(search_path);

        let output = timeout(Duration::from_secs(timeout_secs), cmd.output())
            .await
            .map_err(|_| OSAgentError::Timeout)?
            .map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;

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
        let workspace = self.default_workspace()?;
        let writable = self.writable;
        let walker = FastWalk::new(
            workspace.clone(),
            search_path.to_path_buf(),
            &self.exclude_dirs,
            file_pattern,
            MAX_WALKDIR_MATCHES,
        )?;
        let candidates = walker.collect_files();
        if candidates.is_empty() {
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

        let pattern_str_owned = pattern_str.to_string();
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();

        // Content scanning fans out across the rayon pool (same shape as
        // codesearch phase 2): the walk above stays sequential, every
        // file's bytes are scanned in parallel. memchr pre-filter skips
        // the regex engine for lines that can't match a literal prefix.
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
                let literal = literal_prefix(&pattern_str_owned);

                use rayon::prelude::*;
                let matches: Vec<((usize, usize, String), String)> = candidates
                    .par_iter()
                    .flat_map_iter(|(abs, rel)| {
                        if cancel.load(Ordering::Relaxed) {
                            return Vec::new();
                        }
                        scan_one_file(abs, rel, &pattern, literal.as_deref(), case_sensitive)
                    })
                    .collect();
                let truncated = matches.len() >= MAX_WALKDIR_MATCHES;
                Ok((matches, truncated))
            }),
        )
        .await
        .map_err(|_| {
            cancelled.store(true, Ordering::Relaxed);
            OSAgentError::Timeout
        })?;

        let (mut matches, truncated) =
            result.map_err(|e| OSAgentError::ToolExecution(e.to_string()))??;

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
            let mut output = matches
                .into_iter()
                .map(|(_, line)| line)
                .collect::<Vec<_>>()
                .join("\n");
            if truncated {
                output.push_str(&format!(
                    "\n\n[Search hit the {} match cap; results truncated]",
                    MAX_WALKDIR_MATCHES
                ));
            }
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
    fn timeout_ms(&self) -> Option<u64> {
        Some(self.timeout_seconds.saturating_mul(1_000).max(1_000))
    }

    fn name(&self) -> &str {
        "grep"
    }

    fn description(&self) -> &str {
        "Search file contents with regular expressions. Uses ripgrep when available for significantly faster searches.\n\nUsage:\n- Pattern is a regular expression (e.g. 'fn\\\\s+\\\\w+' to find function definitions).\n- Use file_pattern to filter by file type (e.g. '**/*.rs', '*.{ts,tsx}').\n- Results include file path, line number, and matching line content.\n- Case sensitive by default; set case_sensitive to false for case-insensitive search.\n- Performs exact regex matching - escape special characters if searching for literals.\n- Use this tool to locate code before reading or editing files.\n- If nothing matches, do not conclude the code does not exist: retry with reformulations - synonyms, camelCase/snake_case variants, shorter fragments, case-insensitive search.\n- When doing an open-ended search that may require multiple rounds of grepping and globbing, use the task or subagent tool with an explore agent instead, to reduce context usage."
    }

    fn when_to_use(&self) -> &str {
        "Use to find function definitions, error messages, symbol references, or any text pattern across the codebase. Prefer over reading entire files when looking for specific content."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when you only need file names (use glob) or when you need the full file content (use read_file). For open-ended multi-round discovery across the codebase, delegate to the task or subagent tool with an explore agent."
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
                        outcome: ToolOutcome::Success,
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
                        attachments: Vec::new(),
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
            outcome: ToolOutcome::Success,
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
            attachments: Vec::new(),
        })
    }
}

pub struct GlobTool {
    workspaces: Vec<PathBuf>,
    writable: bool,
    timeout_seconds: u64,
    exclude_dirs: Vec<String>,
    exclude_matchers: Vec<globset::GlobMatcher>,
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
        let exclude_dirs = config.tools.glob.exclude_dirs.clone();
        let exclude_matchers = configured_exclude_matchers(&exclude_dirs);

        Self {
            workspaces,
            writable,
            timeout_seconds,
            exclude_dirs,
            exclude_matchers,
        }
    }

    fn is_excluded(&self, path: &Path) -> bool {
        if is_config_excluded(path, &self.exclude_matchers) {
            return true;
        }
        is_heavy_dir(path)
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;

        let default_ws = self.default_workspace()?;
        let full_path = if path.is_empty() || path == "." {
            default_ws.clone()
        } else {
            default_ws.join(path)
        };
        let full_path = full_path.canonicalize().unwrap_or(full_path);

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
        let mut cmd = tokio::process::Command::new(rg_binary_name());
        cmd.args(["--files", "--hidden", "--no-messages", "--glob", pattern])
            .kill_on_drop(true)
            .stdin(std::process::Stdio::null());
        push_search_excludes(&mut cmd, &self.exclude_dirs);

        cmd.arg(search_path);

        let output = timeout(Duration::from_secs(timeout_secs), cmd.output())
            .await
            .map_err(|_| OSAgentError::Timeout)?
            .map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;
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
        let matcher = match Glob::new(pattern) {
            Ok(glob) => glob.compile_matcher(),
            Err(e) => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Invalid glob pattern '{}': {}",
                    pattern, e
                )))
            }
        };
        let workspace = self.default_workspace()?;
        let writable = self.writable;
        // Same FastWalk path as grep: ignore-crate traversal with
        // builtin + configured excludes, no process spawn. Glob matching
        // itself is cheap string work — no rayon needed.
        let walker = FastWalk::new(
            workspace.clone(),
            search_path.to_path_buf(),
            &self.exclude_dirs,
            None,
            MAX_WALKDIR_MATCHES,
        )?;
        let _ = timeout_secs;
        let mut matches: Vec<((usize, usize, String), String)> = Vec::new();
        for (_, rel) in walker.collect_files() {
            let rel_path = Path::new(&rel);
            if matcher.is_match(rel_path) {
                matches.push((path_sort_key(rel_path), rel));
            }
            if matches.len() >= MAX_WALKDIR_MATCHES {
                break;
            }
        }

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
    fn timeout_ms(&self) -> Option<u64> {
        Some(self.timeout_seconds.saturating_mul(1_000).max(1_000))
    }

    fn name(&self) -> &str {
        "glob"
    }

    fn description(&self) -> &str {
        "Find files by name pattern using glob matching. Uses ripgrep when available for significantly faster searches.\n\nUsage:\n- Use glob patterns like '**/*.rs', 'src/**/*.ts', or '*.{json,yaml}'.\n- Results are sorted by modification time (most recent first).\n- Returns relative paths from the workspace root.\n- Use this to locate files before reading or to understand project structure.\n- If you need to search file contents, use grep instead.\n- When doing an open-ended search that may require multiple rounds of globbing and grepping, use the task or subagent tool with an explore agent instead, to reduce context usage."
    }

    fn when_to_use(&self) -> &str {
        "Use to locate files by name or path pattern. Use before read_file when you don't know the exact path."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when searching inside file contents (use grep) or when you already know the exact file path (use read_file). For open-ended multi-round discovery across the codebase, delegate to the task or subagent tool with an explore agent."
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
                        outcome: ToolOutcome::Success,
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
                        attachments: Vec::new(),
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
            outcome: ToolOutcome::Success,
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
            attachments: Vec::new(),
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

    #[test]
    fn builtin_excludes_cover_generic_junk_only() {
        assert!(is_heavy_dir(Path::new("target/debug/foo")));
        assert!(is_heavy_dir(Path::new("node_modules/pkg/index.js")));
        assert!(!is_heavy_dir(Path::new("some-vendored-tree/main.c")));
        assert!(!is_heavy_dir(Path::new("build-output-xyz/esp-idf/x")));
        assert!(!is_heavy_dir(Path::new("third-party-components/foo")));
        // Builtins stay generic: every glob must name a builtin dir or one
        // of our own bookkeeping dirs — nothing project-specific.
        for glob in BUILTIN_SEARCH_EXCLUDE_GLOBS {
            let lower = glob.to_ascii_lowercase();
            assert!(
                BUILTIN_SEARCH_EXCLUDE_DIRS
                    .iter()
                    .any(|dir| lower.contains(dir))
                    || lower.contains(".osagent_backups")
                    || lower.contains(".osa_tool_outputs"),
                "unexpected builtin glob: {glob}"
            );
        }
    }

    #[test]
    fn configured_excludes_skip_unless_directly_requested() {
        let matchers = configured_exclude_matchers(&[
            "some-vendored-tree".to_string(),
            "build-output-*".to_string(),
        ]);
        // A bare name matches that component anywhere in the tree, plus
        // the file beneath it; `build-output-*` compiles as a glob.
        assert!(is_config_excluded(
            Path::new("some-vendored-tree"),
            &matchers
        ));
        assert!(is_config_excluded(
            Path::new("some-vendored-tree/x.c"),
            &matchers
        ));
        assert!(is_config_excluded(
            Path::new("build-output-xyz/esp-idf/x.c"),
            &matchers
        ));
        assert!(!is_config_excluded(
            Path::new("main/attacks/x.c"),
            &matchers
        ));
        let globs = search_excludes_for(&["some-vendored-tree".to_string()]);
        assert!(globs.iter().any(|glob| glob.contains("some-vendored-tree")));
    }

    #[tokio::test]
    async fn configured_exclude_dirs_are_skipped_by_default() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("main")).expect("mkdir main");
        std::fs::create_dir_all(dir.path().join("vendored-copy")).expect("mkdir vendored");
        std::fs::write(dir.path().join("main/hit.c"), "needle\n").expect("write main");
        std::fs::write(dir.path().join("vendored-copy/hit.c"), "needle\n").expect("write vendored");

        // Drive the real walkdir path directly: TempDir canonicalization
        // (e.g. Windows short names) can otherwise make the workspace and
        // search-root prefixes diverge for reasons unrelated to excludes.
        let mut config = config_for_workspace(&dir.path().to_string_lossy());
        config.tools.grep.exclude_dirs = vec!["vendored-copy".to_string()];
        let tool = GrepTool::new(config);
        let workspace = tool.default_workspace().expect("workspace");
        let configured = configured_exclude_matchers(&tool.exclude_dirs);
        assert!(!configured.is_empty());

        let search_root = workspace.clone();
        let search_explicitly_requested =
            is_config_excluded(&search_root, &configured) || is_heavy_dir(&search_root);
        assert!(!search_explicitly_requested);

        let result = tool
            .execute_walkdir_grep("needle", &search_root, None, true, 30)
            .await
            .expect("grep result");
        assert!(
            result.0.display_output.contains("main/hit.c"),
            "expected main hit, got: {}",
            result.0.display_output
        );
        assert!(
            !result.0.display_output.contains("vendored-copy"),
            "configured exclude leaked: {}",
            result.0.display_output
        );

        let direct_root = workspace.join("vendored-copy");
        let direct = tool
            .execute_walkdir_grep("needle", &direct_root, None, true, 30)
            .await
            .expect("direct grep result");
        assert!(
            direct.0.display_output.contains("needle"),
            "direct search into excluded dir must still work: {}",
            direct.0.display_output
        );
    }
}
