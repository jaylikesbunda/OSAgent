//! Quick-context code search: replaces the old MeiliSearch-backed indexer
//! with a batch of derived greps run in a single call. The query is split
//! into distinctive terms, expanded into several patterns (plain, camelCase/
//! snake_case phrase joins, definition anchors), executed against the live
//! workspace in one ripgrep invocation (walkdir fallback), then merged and
//! ranked per file.

use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::codesearch_tokenizer::{camel_case_join, extract_query_terms, snake_case_join};
use crate::tools::guard::path_touches_backups;
use crate::tools::output::path_touches_tool_outputs;
use crate::tools::registry::{Tool, ToolExample, ToolOutcome, ToolResult};
use crate::tools::search::{discouraged_path_penalty, ensure_rg_checked, rg_binary_name};
use async_trait::async_trait;
use rayon::prelude::*;
use regex::Regex;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::fs;
use std::io::{BufRead, Read, Seek};
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::time::timeout;
use tracing::debug;

const MAX_PATTERNS: usize = 14;
const MAX_TERMS: usize = 6;
const MAX_HITS_PER_FILE_SNIPPETS: usize = 4;
const MAX_FILES_SHOWN: usize = 8;
const MAX_COLLECTED_HITS: usize = 8_000;
/// Files larger than this are data dumps, not code context; skip them.
const MAX_SCAN_FILE_BYTES: u64 = 1_000_000;
/// Upper bound on candidate files scanned per call.
const MAX_SCANNED_FILES: usize = 20_000;
/// Per-file hit cap so one noisy file can't flood the result set.
const MAX_HITS_PER_FILE: usize = 400;

/// Scan one already-validated file for pattern hits.
///
/// Returns an empty Vec for unreadable/binary files. The binary probe reads
/// a small header and then rewinds, so short files are still searched.
fn scan_file_for_patterns(
    abs_path: &Path,
    rel_path: &Path,
    compiled: &[(usize, Regex)],
) -> Vec<Hit> {
    let Ok(file) = fs::File::open(abs_path) else {
        return Vec::new();
    };
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);

    let mut header = [0u8; 4096];
    let header_len = match reader.read(&mut header) {
        Ok(n) => n,
        Err(_) => return Vec::new(),
    };
    if header[..header_len].contains(&0) {
        return Vec::new();
    }
    // Rewind: short files were fully consumed by the probe.
    if reader.seek(std::io::SeekFrom::Start(0)).is_err() {
        return Vec::new();
    }

    let mut hits: Vec<Hit> = Vec::new();
    let mut line = String::new();
    let mut line_no = 0usize;
    loop {
        line.clear();
        match reader.read_line(&mut line) {
            Ok(0) => break,
            Ok(_) => {
                line_no += 1;
                let text = line.trim_end_matches(['\n', '\r']);
                if let Some(&(idx, _)) = compiled.iter().find(|(_, re)| re.is_match(text)) {
                    hits.push(Hit {
                        rel_path: rel_path.to_path_buf(),
                        line_no,
                        text: text.to_string(),
                        pattern_idx: idx,
                    });
                    if hits.len() >= MAX_HITS_PER_FILE {
                        break;
                    }
                }
            }
            Err(_) => break,
        }
    }
    hits
}

/// One derived search pattern and how strongly a match counts.
struct SearchPattern {
    source: String,
    /// Score weight when this pattern hits.
    weight: i64,
    /// Index of the query term this pattern derives from.
    term_idx: usize,
}

struct Hit {
    rel_path: PathBuf,
    line_no: usize,
    text: String,
    pattern_idx: usize,
}

struct ScoredFile {
    rel_path: PathBuf,
    score: i64,
    hits: Vec<Hit>,
}

/// Map a language name (as accepted by the old indexer) to file globs.
fn language_globs(language: &str) -> Option<Vec<&'static str>> {
    let globs: &[(&str, &[&str])] = &[
        ("rust", &["*.rs"]),
        ("python", &["*.py"]),
        ("javascript", &["*.js", "*.jsx"]),
        ("typescript", &["*.ts", "*.tsx"]),
        ("go", &["*.go"]),
        ("java", &["*.java"]),
        ("kotlin", &["*.kt"]),
        ("swift", &["*.swift"]),
        ("c", &["*.c", "*.h"]),
        ("cpp", &["*.cpp", "*.hpp", "*.cc", "*.hh"]),
        ("csharp", &["*.cs"]),
        ("ruby", &["*.rb"]),
        ("php", &["*.php"]),
        ("scala", &["*.scala"]),
        ("bash", &["*.sh", "*.bash", "*.zsh"]),
        ("shell", &["*.sh", "*.bash", "*.zsh"]),
        ("zsh", &["*.zsh"]),
        ("html", &["*.html", "*.htm"]),
        ("css", &["*.css", "*.scss", "*.sass", "*.less"]),
        ("json", &["*.json"]),
        ("yaml", &["*.yaml", "*.yml"]),
        ("toml", &["*.toml"]),
        ("markdown", &["*.md", "*.rst"]),
        ("sql", &["*.sql"]),
        ("lua", &["*.lua"]),
        ("vim", &["*.vim"]),
        ("powershell", &["*.ps1"]),
        ("vue", &["*.vue"]),
        ("svelte", &["*.svelte"]),
    ];
    let lower = language.to_ascii_lowercase();
    globs
        .iter()
        .find(|(name, _)| *name == lower)
        .map(|(_, patterns)| patterns.to_vec())
}

/// Build the derived pattern set from extracted query terms.
fn build_patterns(query: &str) -> Vec<SearchPattern> {
    let mut patterns = Vec::new();

    let terms = extract_query_terms(query);
    if terms.is_empty() {
        // Nothing distinctive survived tokenization (e.g. a pasted error
        // fragment full of stopwords): fall back to the raw query so exact
        // phrases still match.
        let raw = query.trim();
        if !raw.is_empty() {
            patterns.push(SearchPattern {
                source: regex::escape(raw),
                weight: 10,
                term_idx: 0,
            });
        }
        return patterns;
    }

    let selected = &terms[..terms.len().min(MAX_TERMS)];

    // Plain term matches: the workhorse.
    for (idx, term) in selected.iter().enumerate() {
        if patterns.len() >= MAX_PATTERNS {
            break;
        }
        patterns.push(SearchPattern {
            source: regex::escape(term),
            weight: 10,
            term_idx: idx,
        });
    }

    // Multi-term phrase joins: strong signals the model meant one identifier.
    if let Some(camel) = camel_case_join(selected) {
        patterns.push(SearchPattern {
            source: regex::escape(&camel),
            weight: 30,
            term_idx: 0,
        });
    }
    if let Some(snake) = snake_case_join(selected) {
        patterns.push(SearchPattern {
            source: regex::escape(&snake),
            weight: 30,
            term_idx: 0,
        });
    }

    // Definition anchors: declarations naming the top few terms.
    for (idx, term) in selected.iter().take(4).enumerate() {
        if patterns.len() >= MAX_PATTERNS {
            break;
        }
        patterns.push(SearchPattern {
            source: format!(
                r"\b(?:fn|func|function|def|class|struct|enum|trait|interface|impl)\s+\w*{}\w*\b",
                regex::escape(term)
            ),
            weight: 25,
            term_idx: idx,
        });
    }

    patterns
}

/// Merge raw hits into per-file scores.
///
/// Scoring favors breadth (distinct patterns/terms hitting in one file) over
/// raw density, adds bonuses for definition anchors and near-line term
/// co-occurrence, and subtracts a penalty for build/vendored paths.
fn score_hits(patterns: &[SearchPattern], hits: Vec<Hit>) -> Vec<ScoredFile> {
    let mut by_file: HashMap<PathBuf, Vec<Hit>> = HashMap::new();
    for hit in hits {
        by_file.entry(hit.rel_path.clone()).or_default().push(hit);
    }

    let mut scored: Vec<ScoredFile> = by_file
        .into_iter()
        .map(|(rel_path, mut hits)| {
            hits.sort_by_key(|h| h.line_no);

            let mut score: i64 = 0;
            let mut patterns_hit: HashSet<usize> = HashSet::new();
            let mut has_definition = false;

            for hit in &hits {
                patterns_hit.insert(hit.pattern_idx);
                let weight = patterns[hit.pattern_idx].weight;
                score += weight.min(20); // density contributes, capped per line
                if weight >= 25 {
                    has_definition = true;
                }
            }

            score += (patterns_hit.len() as i64) * 40;

            // Terms appearing together within a small window means this file
            // ties the concepts together, not just mentions them.
            let mut co_occur = false;
            'outer: for (i, a) in hits.iter().enumerate() {
                for b in hits.iter().skip(i + 1) {
                    if b.line_no > a.line_no + 5 {
                        break;
                    }
                    if patterns[a.pattern_idx].term_idx != patterns[b.pattern_idx].term_idx {
                        co_occur = true;
                        break 'outer;
                    }
                }
            }
            if co_occur {
                score += 20;
            }
            if has_definition {
                score += 15;
            }

            score -= discouraged_path_penalty(&rel_path) as i64;

            ScoredFile {
                rel_path,
                score,
                hits,
            }
        })
        .collect();

    scored.sort_by(|a, b| {
        b.score
            .cmp(&a.score)
            .then_with(|| a.rel_path.cmp(&b.rel_path))
    });
    scored
}

fn render_results(query: &str, scored: &[ScoredFile], limit: usize) -> String {
    let mut output = String::new();
    output.push_str(&format!(
        "Found {} file(s) relevant to \"{}\" (top {} shown):\n\n",
        scored.len(),
        query.trim(),
        scored.len().min(limit)
    ));

    for file in scored.iter().take(limit) {
        output.push_str(&format!(
            "**{}** (score {})\n",
            file.rel_path.display(),
            file.score
        ));
        let mut last_line_shown: Option<usize> = None;
        let mut shown = 0;
        for hit in &file.hits {
            if shown >= MAX_HITS_PER_FILE_SNIPPETS {
                break;
            }
            // Skip hits that crowd the previous snippet; show spread-out ones.
            if let Some(prev) = last_line_shown {
                if hit.line_no <= prev + 2 && shown > 0 {
                    continue;
                }
            }
            let text: String = hit.text.trim().chars().take(160).collect();
            output.push_str(&format!("- {}: {}\n", hit.line_no, text));
            last_line_shown = Some(hit.line_no);
            shown += 1;
        }
        output.push('\n');
    }

    output.push_str("Use read_file on promising paths for full context.\n");
    output
}

pub struct CodeSearchTool {
    workspaces: Vec<PathBuf>,
    timeout_seconds: u64,
}

impl CodeSearchTool {
    fn default_workspace(&self) -> Result<PathBuf> {
        self.workspaces.first().cloned().ok_or_else(|| {
            OSAgentError::ToolExecution(
                "No workspace configured. Set a workspace path in settings.".to_string(),
            )
        })
    }

    pub fn new(config: Config) -> Self {
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
            timeout_seconds,
        }
    }

    async fn execute_rg_batch(
        &self,
        patterns: &[SearchPattern],
        language_globs: Option<&[&str]>,
        search_path: &Path,
    ) -> Result<Vec<Hit>> {
        let workspace = self.default_workspace()?;
        let mut cmd = tokio::process::Command::new(rg_binary_name());
        cmd.args([
            "--no-heading",
            "--with-filename",
            "--line-number",
            "--color=never",
            "--no-messages",
            "--hidden",
            "-i",
            "--max-count=250",
        ])
        .kill_on_drop(true)
        .stdin(std::process::Stdio::null());

        if let Some(globs) = language_globs {
            for glob in globs {
                cmd.args(["--glob", glob]);
            }
        }

        cmd.args([
            "--glob",
            "!.osagent_backups",
            "--glob",
            "!.osa_tool_outputs",
        ]);

        for pattern in patterns {
            cmd.args(["-e", &pattern.source]);
        }

        cmd.arg("--").arg(search_path);

        let output = timeout(Duration::from_secs(self.timeout_seconds), cmd.output())
            .await
            .map_err(|_| OSAgentError::Timeout)?
            .map_err(|e| OSAgentError::ToolExecution(e.to_string()))?;

        let stdout = String::from_utf8_lossy(&output.stdout);
        let prefix = search_path.to_string_lossy().to_string();
        // Compile once up front; classifying thousands of lines must not
        // re-parse pattern sources.
        let compiled: Vec<Regex> = patterns
            .iter()
            .map(|p| {
                Regex::new(&format!(r"(?i){}", p.source))
                    .map_err(|e| OSAgentError::ToolExecution(format!("Invalid pattern: {}", e)))
            })
            .collect::<crate::error::Result<Vec<_>>>()?;
        let mut hits = Vec::new();

        for line in stdout.lines() {
            if hits.len() >= MAX_COLLECTED_HITS {
                break;
            }
            // Lines look like `<abs path>:<line>:<content>`; strip the known
            // search-path prefix so the drive-letter colon can't confuse the
            // split, then take relpath : lineno : text.
            let Some(rest) = line.strip_prefix(&prefix) else {
                continue;
            };
            let rest = rest.trim_start_matches(['/', '\\']);
            let mut parts = rest.splitn(3, ':');
            let (Some(rel), Some(line_no), Some(text)) = (parts.next(), parts.next(), parts.next())
            else {
                continue;
            };
            let Ok(line_no) = line_no.parse::<usize>() else {
                continue;
            };
            let rel_path = PathBuf::from(rel);
            if path_touches_backups(&rel_path) || path_touches_tool_outputs(&rel_path) {
                continue;
            }
            // Which pattern matched this line: retest with precompiled regexes.
            let pattern_idx = compiled
                .iter()
                .position(|re| re.is_match(text))
                .unwrap_or(0);
            hits.push(Hit {
                rel_path,
                line_no,
                text: text.to_string(),
                pattern_idx,
            });
        }

        Ok(hits)
    }

    async fn execute_walkdir_batch(
        &self,
        patterns: &[SearchPattern],
        allowed_extensions: Option<&[String]>,
        search_path: &Path,
    ) -> Result<Vec<Hit>> {
        let compiled: Result<Vec<(usize, Regex)>> = patterns
            .iter()
            .enumerate()
            .map(|(idx, p)| {
                Regex::new(&format!(r"(?i){}", p.source))
                    .map(|re| (idx, re))
                    .map_err(|e| OSAgentError::ToolExecution(format!("Invalid pattern: {}", e)))
            })
            .collect();
        let compiled = compiled?;

        let workspace = self.default_workspace()?;
        let allowed_extensions: Option<Vec<String>> = allowed_extensions.map(<[String]>::to_vec);

        // Phase 1 (cheap, sequential): traverse and filter down to candidate
        // files via the shared FastWalk path — ignore-crate traversal with
        // builtin + configured excludes, no process spawn. No content
        // reading here.
        let walker = crate::tools::search::FastWalk::new(
            workspace,
            search_path.to_path_buf(),
            &[],
            None,
            MAX_SCANNED_FILES,
        )?;
        let mut candidates: Vec<(PathBuf, PathBuf)> = Vec::new();
        for (abs, rel) in walker.collect_files() {
            let entry_path = abs;
            let relative_path = PathBuf::from(rel);
            if let Some(exts) = allowed_extensions.as_deref() {
                let matches_ext = entry_path
                    .extension()
                    .and_then(|e| e.to_str())
                    .map(|e| {
                        exts.iter()
                            .any(|allowed| allowed == &e.to_ascii_lowercase())
                    })
                    .unwrap_or(false);
                if !matches_ext {
                    continue;
                }
            }

            // Giant data dumps (model catalogs, result archives) are noise
            // for quick context; skip them rather than burn seconds reading.
            let oversize = std::fs::metadata(&entry_path)
                .map(|m| m.len() > MAX_SCAN_FILE_BYTES)
                .unwrap_or(true);
            if oversize {
                continue;
            }

            candidates.push((entry_path, relative_path));
            if candidates.len() >= MAX_SCANNED_FILES {
                break;
            }
        }

        if candidates.is_empty() {
            return Ok(Vec::new());
        }

        // Phase 2: scan candidates across the rayon pool from a blocking
        // thread, keeping the async-side timeout and cancellation contract.
        let cancelled = Arc::new(AtomicBool::new(false));
        let cancel = cancelled.clone();
        let timeout_secs = self.timeout_seconds;

        let scan = move || -> Vec<Hit> {
            candidates
                .par_iter()
                .flat_map_iter(|(abs, rel)| {
                    if cancel.load(Ordering::Relaxed) {
                        return Vec::new();
                    }
                    scan_file_for_patterns(abs, rel, &compiled)
                })
                .collect::<Vec<_>>()
        };

        let joined = timeout(
            Duration::from_secs(timeout_secs),
            tokio::task::spawn_blocking(scan),
        )
        .await;

        match joined {
            Ok(Ok(hits)) => Ok(hits),
            Ok(Err(e)) => Err(OSAgentError::ToolExecution(e.to_string())),
            Err(_) => {
                cancelled.store(true, Ordering::Relaxed);
                Err(OSAgentError::Timeout)
            }
        }
    }
}

#[async_trait]
impl Tool for CodeSearchTool {
    fn timeout_ms(&self) -> Option<u64> {
        Some(self.timeout_seconds.saturating_mul(1_000).max(1_000))
    }

    fn name(&self) -> &str {
        "codesearch"
    }

    fn description(&self) -> &str {
        "Quick-context code search: fans one natural-language query out into a batch of derived greps (keyword terms, camelCase/snake_case identifier variants, definition anchors) and returns the most relevant files with matching snippets ranked by hit quality. Reads live files, no index, always current.\n\nUsage:\n- Best when you're unsure what to look for or where: give 2-6 distinctive concept terms rather than full sentences.\n- Results rank files that tie multiple query terms together higher than files mentioning just one.\n- Follow up with read_file on promising paths."
    }

    fn when_to_use(&self) -> &str {
        "Use FIRST for vague 'where/how does X work' questions before grepping; returns ranked files with snippets."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for exact literal matches or regex (use grep), file names (use glob), or when you already know the file (use read_file)."
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Find authentication-related code".to_string(),
                input: json!({
                    "query": "authentication login user session",
                    "limit": 10
                }),
            },
            ToolExample {
                description: "Search Python files only".to_string(),
                input: json!({
                    "query": "process_data transform",
                    "language": "python",
                    "limit": 5
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "query": {
                    "type": "string",
                    "description": "Concept terms describing what you're looking for. Distinctive words beat sentences; identifiers in any casing work."
                },
                "language": {
                    "type": "string",
                    "description": "Filter by programming language (e.g. 'rust', 'python', 'javascript', 'typescript')"
                },
                "limit": {
                    "type": "integer",
                    "description": "Maximum number of files to return (default: 8, max: 50)",
                    "default": 8
                }
            },
            "required": ["query"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        Ok(self.execute_result(args).await?.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let query = args["query"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'query' parameter".to_string()))?;
        let limit = args["limit"]
            .as_u64()
            .unwrap_or(MAX_FILES_SHOWN as u64)
            .min(50) as usize;
        let language = args["language"].as_str();

        let search_path = self.default_workspace()?;
        let patterns = build_patterns(query);
        if patterns.is_empty() {
            return Ok(ToolResult {
                output: "No usable search terms in query. Try distinctive keywords.".to_string(),
                outcome: ToolOutcome::Success,
                title: Some(query.chars().take(40).collect()),
                metadata: json!({ "files": 0, "query": query }),
                attachments: Vec::new(),
            });
        }

        let lang_globs = language.map(language_globs).unwrap_or(None);
        let lang_extensions: Option<Vec<String>> = lang_globs.as_ref().map(|globs| {
            globs
                .iter()
                .map(|g| g.trim_start_matches("*.").to_string())
                .collect()
        });

        let hits = if ensure_rg_checked() {
            match self
                .execute_rg_batch(&patterns, lang_globs.as_deref(), &search_path)
                .await
            {
                Ok(hits) => hits,
                Err(e) => {
                    debug!("codesearch ripgrep failed ({}), falling back to walkdir", e);
                    self.execute_walkdir_batch(&patterns, lang_extensions.as_deref(), &search_path)
                        .await?
                }
            }
        } else {
            self.execute_walkdir_batch(&patterns, lang_extensions.as_deref(), &search_path)
                .await?
        };

        let scored = score_hits(&patterns, hits);
        let total_files = scored.len();

        let output = if scored.is_empty() {
            format!(
                "No matches found for \"{}\". Try different or shorter keywords, or drop the language filter.",
                query.trim()
            )
        } else {
            render_results(query, &scored, limit)
        };

        Ok(ToolResult {
            output,
            outcome: ToolOutcome::Success,
            title: Some(query.chars().take(40).collect()),
            metadata: json!({
                "files": total_files,
                "shown": total_files.min(limit),
                "patterns": patterns.len(),
                "query": query,
                "language": language,
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

    #[test]
    fn patterns_include_variants_and_anchors() {
        let patterns = build_patterns("process data");
        assert!(patterns.iter().any(|p| p.source == "process"));
        assert!(patterns.iter().any(|p| p.source == "data"));
        assert!(patterns.iter().any(|p| p.source == "processData"));
        assert!(patterns.iter().any(|p| p.source == "process_data"));
        assert!(patterns
            .iter()
            .any(|p| p.source.contains(r"\b(?:fn|func") && p.source.contains("process")));
    }

    #[test]
    fn raw_query_fallback_when_all_stopwords() {
        let patterns = build_patterns("the and of");
        assert_eq!(patterns.len(), 1);
        assert!(patterns[0].source.contains("the and of"));
    }

    #[test]
    fn multi_term_file_ranks_first() {
        let patterns = vec![
            SearchPattern {
                source: "retry".into(),
                weight: 10,
                term_idx: 0,
            },
            SearchPattern {
                source: "backoff".into(),
                weight: 10,
                term_idx: 1,
            },
            SearchPattern {
                source: r"\b(?:fn|def)\s+\w*retry\w*\b".into(),
                weight: 25,
                term_idx: 0,
            },
        ];
        let hits = vec![
            Hit {
                rel_path: PathBuf::from("good.rs"),
                line_no: 10,
                text: "fn retry_with_backoff() {".into(),
                pattern_idx: 2,
            },
            Hit {
                rel_path: PathBuf::from("good.rs"),
                line_no: 12,
                text: "    backoff = Duration::from_secs(2);".into(),
                pattern_idx: 1,
            },
            Hit {
                rel_path: PathBuf::from("meh.rs"),
                line_no: 99,
                text: "// TODO retry later".into(),
                pattern_idx: 0,
            },
        ];

        let scored = score_hits(&patterns, hits);
        assert_eq!(scored.len(), 2);
        assert_eq!(scored[0].rel_path, PathBuf::from("good.rs"));
        assert!(scored[0].score > scored[1].score);
    }

    #[tokio::test]
    async fn end_to_end_finds_and_ranks_files() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(
            dir.path().join("core.py"),
            "import time\n\ndef retry_request(attempt):\n    delay = backoff_for(attempt)\n    time.sleep(delay)\n",
        )
        .expect("write core");
        std::fs::write(dir.path().join("util.py"), "# retry helper\n").expect("write util");

        let tool = CodeSearchTool::new(config_for_workspace(&dir.path().to_string_lossy()));
        let result = Tool::execute_result(
            &tool,
            json!({ "query": "retry backoff request", "path": "." }),
        )
        .await
        .expect("codesearch result");

        assert_eq!(result.metadata["files"], 2);
        assert!(result.output.contains("core.py"));
        assert!(result.output.contains("util.py"));
        // The file tying multiple terms + a definition together leads.
        let core_pos = result.output.find("core.py").expect("core present");
        let util_pos = result.output.find("util.py").expect("util present");
        assert!(core_pos < util_pos);
    }

    #[tokio::test]
    async fn no_matches_is_friendly_not_failure() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("a.rs"), "fn main() {}\n").expect("write");

        let tool = CodeSearchTool::new(config_for_workspace(&dir.path().to_string_lossy()));
        let result = Tool::execute_result(&tool, json!({ "query": "zzz_unfindable_term" }))
            .await
            .expect("result");

        assert_eq!(result.outcome, ToolOutcome::Success);
        assert!(result.metadata["files"] == 0);
        assert!(result.output.contains("No matches found"));
    }

    #[tokio::test]
    async fn language_filter_restricts_results() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("x.rs"), "let payload_limit = 10;\n").expect("rs");
        std::fs::write(dir.path().join("y.py"), "payload_limit = 10\n").expect("py");

        let tool = CodeSearchTool::new(config_for_workspace(&dir.path().to_string_lossy()));
        let result = Tool::execute_result(
            &tool,
            json!({ "query": "payload limit", "language": "python" }),
        )
        .await
        .expect("result");

        assert!(result.output.contains("y.py"));
        assert!(!result.output.contains("x.rs"));
    }
}
