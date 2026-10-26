//! CodeRabbit-style review generation: prompt construction, strict
//! structured parsing, diff-position validation, and GitHub review
//! composition. Pure functions live here so they are unit-testable; the
//! orchestration (API calls, model calls, posting) lives in `engine.rs`.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

use super::diff::{render_numbered, FileDiff};
use super::types::{GhPullRequest, GhReviewCommentInput};

/// Character budget for a single model request (diffs + metadata).
pub const MAX_BATCH_CHARS: usize = 90_000;
/// Overall diff budget across all batches of one PR.
pub const MAX_TOTAL_PATCH_CHARS: usize = 480_000;
/// Hard cap on files pulled from the API per review.
pub const MAX_FILES: usize = 200;
/// Timeout for one model completion.
pub const MODEL_CALL_TIMEOUT_SECS: u64 = 300;

pub const REVIEW_SYSTEM_PROMPT: &str = r#"You are OSA's pull request reviewer: a precise, pragmatic senior engineer. You review code changes and report findings that affect correctness, security, performance, data integrity, or maintainability.

Rules:
- Report only issues you can justify from the diff. Never invent problems and never pad the list; an empty findings array is a valid and good review.
- Severity: "critical" breaks correctness, security, or data; "major" is a clear bug or risky behavior likely to cause problems; "minor" is worth fixing with low risk; "nitpick" is style/consistency only — use it sparingly.
- Do not comment on formatting, import order, or naming unless it is misleading enough to cause future bugs.
- Be concrete and actionable. If the exact replacement for the referenced line is obvious and localized, put the literal replacement code in "suggestion" (it will be applied as a GitHub suggestion to that single line).
- Reference lines exactly as shown in the numbered diff: added and context lines use their NEW number with side "RIGHT"; removed lines use their OLD number with side "LEFT". Never guess numbers.
- The summary is 2-6 sentences (or short bullets) describing what the change does and the review's headline conclusions.
- Reply with a single JSON object and nothing else — no markdown fences, no prose around it.

JSON shape:
{"summary": "markdown string", "findings": [{"path": "repo/relative/path", "line": 123, "side": "RIGHT", "severity": "critical|major|minor|nitpick", "title": "short title", "body": "markdown explanation", "suggestion": "optional exact replacement code for this one line"}]}"#;

const MAX_PR_BODY_CHARS: usize = 6_000;
const MAX_SUMMARY_CHARS: usize = 12_000;
const MAX_COMMENT_BODY_CHARS: usize = 6_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Severity {
    Critical,
    Major,
    Minor,
    Nitpick,
}

impl Severity {
    pub fn from_str_lossy(value: &str) -> Self {
        match value.trim().to_ascii_lowercase().as_str() {
            "critical" | "blocker" | "severe" | "high" => Self::Critical,
            "major" | "error" | "medium" | "warning" => Self::Major,
            "minor" | "low" | "info" => Self::Minor,
            _ => Self::Nitpick,
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Critical => "Critical",
            Self::Major => "Major",
            Self::Minor => "Minor",
            Self::Nitpick => "Nitpick",
        }
    }

    pub fn emoji(self) -> &'static str {
        match self {
            Self::Critical => "🚨",
            Self::Major => "⚠️",
            Self::Minor => "💡",
            Self::Nitpick => "🧹",
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ReviewFinding {
    pub path: String,
    pub line: u64,
    #[serde(default = "default_side")]
    pub side: String,
    #[serde(default)]
    pub severity: Option<Severity>,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub body: String,
    #[serde(default)]
    pub suggestion: Option<String>,
}

pub fn default_side() -> String {
    "RIGHT".to_string()
}

impl ReviewFinding {
    pub fn severity(&self) -> Severity {
        self.severity.unwrap_or(Severity::Minor)
    }

    pub fn normalized_side(&self) -> String {
        if self.side.eq_ignore_ascii_case("left") {
            "LEFT".to_string()
        } else {
            "RIGHT".to_string()
        }
    }

    /// Inline comment body posted to GitHub.
    pub fn render_comment_body(&self) -> String {
        let severity = self.severity();
        let title = if self.title.trim().is_empty() {
            severity.label().to_string()
        } else {
            self.title.trim().to_string()
        };
        let mut out = format!(
            "**{} {}: {}**\n\n{}",
            severity.emoji(),
            severity.label(),
            title,
            truncate_text(self.body.trim(), MAX_COMMENT_BODY_CHARS)
        );
        if let Some(suggestion) = self
            .suggestion
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("\n\n```suggestion\n{}\n```", suggestion));
        }
        out
    }
}

/// Task-level review output aggregated across batches.
#[derive(Debug, Clone, Default)]
pub struct ReviewDraft {
    pub summary_parts: Vec<String>,
    pub findings: Vec<ReviewFinding>,
    /// Non-fatal notes surfaced in the review body (truncated diffs,
    /// unparseable batch output, omitted findings).
    pub notes: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawReview {
    #[serde(default, alias = "overview", alias = "review_summary")]
    summary: Option<String>,
    #[serde(default, alias = "comments", alias = "issues")]
    findings: Option<Vec<RawFinding>>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct RawFinding {
    #[serde(default, alias = "file", alias = "filename", alias = "path")]
    path: Option<String>,
    #[serde(default, alias = "line_number", alias = "new_line", alias = "newLine")]
    line: Option<u64>,
    #[serde(default)]
    side: Option<String>,
    #[serde(default)]
    severity: Option<String>,
    #[serde(default, alias = "message", alias = "issue")]
    title: Option<String>,
    #[serde(default, alias = "description", alias = "details")]
    body: Option<String>,
    #[serde(default, alias = "fix", alias = "replacement", alias = "suggested_fix")]
    suggestion: Option<String>,
}

/// Parse a model completion into a structured review. Returns `Err` with a
/// repair hint when no JSON object can be recovered at all.
pub fn parse_review_response(text: &str) -> Result<RawParsed, String> {
    let json_text =
        extract_json_object(text).ok_or_else(|| "response contained no JSON object".to_string())?;
    let raw: RawReview = serde_json::from_str(json_text)
        .map_err(|e| format!("response was not the expected JSON shape: {}", e))?;

    let findings = raw
        .findings
        .unwrap_or_default()
        .into_iter()
        .filter_map(|raw| {
            let path = raw.path.unwrap_or_default().trim().to_string();
            let line = raw.line.unwrap_or(0);
            if path.is_empty() || line == 0 {
                return None;
            }
            let mut title = raw.title.unwrap_or_default().trim().to_string();
            let body = raw.body.unwrap_or_default().trim().to_string();
            if title.is_empty() {
                title = first_sentence(&body);
            }
            if title.is_empty() && body.is_empty() {
                return None;
            }
            Some(ReviewFinding {
                path,
                line,
                side: raw
                    .side
                    .unwrap_or_else(default_side)
                    .trim()
                    .to_ascii_uppercase(),
                severity: raw.severity.as_deref().map(Severity::from_str_lossy),
                title,
                body,
                suggestion: raw.suggestion.filter(|s| !s.trim().is_empty()),
            })
        })
        .collect::<Vec<_>>();

    Ok(RawParsed {
        summary: raw.summary.unwrap_or_default().trim().to_string(),
        findings,
    })
}

pub struct RawParsed {
    pub summary: String,
    pub findings: Vec<ReviewFinding>,
}

/// Locate the first complete top-level JSON object in `text`, tolerating
/// surrounding prose or markdown fences. String-aware so braces inside
/// strings do not terminate the scan.
pub fn extract_json_object(text: &str) -> Option<&str> {
    let bytes = text.as_bytes();
    let start = text.find('{')?;
    let mut depth = 0usize;
    let mut in_string = false;
    let mut escaped = false;
    for (offset, ch) in text[start..].char_indices() {
        let index = start + offset;
        if in_string {
            if escaped {
                escaped = false;
            } else if ch == '\\' {
                escaped = true;
            } else if ch == '"' {
                in_string = false;
            }
            continue;
        }
        match ch {
            '"' => in_string = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.saturating_sub(1);
                if depth == 0 {
                    return text.get(start..=index);
                }
            }
            _ => {}
        }
    }
    // Byte-index fallback for exotic unicode edge cases.
    let _ = bytes;
    None
}

/// Match a model-reported path to a PR file path. Handles `a/`/`b/`
/// prefixes and `./` noise; falls back to a unique basename match.
pub fn match_file_path<'a>(
    path: &str,
    files: &'a HashMap<String, FileDiff>,
) -> Option<&'a FileDiff> {
    let normalized = path.trim().trim_start_matches("./").trim_start_matches('/');
    let normalized = normalized
        .strip_prefix("a/")
        .or_else(|| normalized.strip_prefix("b/"))
        .unwrap_or(normalized);
    if let Some(exact) = files.get(normalized) {
        return Some(exact);
    }
    let lowered = normalized.to_ascii_lowercase();
    if let Some((_, file)) = files
        .iter()
        .find(|(key, _)| key.to_ascii_lowercase() == lowered)
    {
        return Some(file);
    }
    let basename = normalized.rsplit('/').next().unwrap_or(normalized);
    let mut matches = files
        .iter()
        .filter(|(key, _)| key.rsplit('/').next().unwrap_or(key) == basename);
    let first = matches.next();
    if matches.next().is_none() {
        first.map(|(_, file)| file)
    } else {
        None
    }
}

#[derive(Debug, Default)]
pub struct ValidationOutcome {
    /// Findings safe to post as inline comments, severity-ordered.
    pub accepted: Vec<ReviewFinding>,
    /// Findings that could not be placed inline (bad line, unknown file,
    /// over the inline cap). Summarized in the review body instead.
    pub dropped: Vec<ReviewFinding>,
}

/// Validate findings against the parsed diffs: path must exist and the
/// line must be inside a hunk on the given side. Caps inline comments at
/// `max_comments`, keeping the most severe.
pub fn validate_findings(
    findings: Vec<ReviewFinding>,
    files: &HashMap<String, FileDiff>,
    max_comments: usize,
) -> ValidationOutcome {
    let mut accepted = Vec::new();
    let mut dropped = Vec::new();
    let mut seen = std::collections::HashSet::new();

    for finding in findings {
        let dedupe_key = (
            finding.path.to_ascii_lowercase(),
            finding.line,
            finding.title.to_ascii_lowercase(),
        );
        if !seen.insert(dedupe_key) {
            continue;
        }
        let Some(file) = match_file_path(&finding.path, files) else {
            dropped.push(finding);
            continue;
        };
        if !file.has_patch || !file.is_valid_position(finding.line, &finding.normalized_side()) {
            dropped.push(finding);
            continue;
        }
        accepted.push(finding);
    }

    accepted.sort_by_key(|finding| finding.severity());
    if accepted.len() > max_comments {
        let overflow = accepted.split_off(max_comments);
        dropped.extend(overflow);
    }

    ValidationOutcome { accepted, dropped }
}

/// Convert accepted findings into GitHub review comment inputs.
pub fn to_review_comments(findings: &[ReviewFinding]) -> Vec<GhReviewCommentInput> {
    findings
        .iter()
        .map(|finding| GhReviewCommentInput {
            path: finding.path.trim().to_string(),
            line: finding.line,
            side: finding.normalized_side(),
            body: finding.render_comment_body(),
            start_line: None,
            start_side: None,
        })
        .collect()
}

pub fn severity_counts(findings: &[ReviewFinding]) -> HashMap<Severity, usize> {
    let mut counts = HashMap::new();
    for finding in findings {
        *counts.entry(finding.severity()).or_insert(0) += 1;
    }
    counts
}

/// Compose the review body: model summary, finding tallies, dropped
/// findings, and the dedupe marker. `review_marker` is the head SHA.
pub fn compose_review_body(
    summary: &str,
    accepted: &[ReviewFinding],
    dropped: &[ReviewFinding],
    notes: &[String],
    stats: &ReviewStats,
    review_marker: &str,
) -> String {
    let mut out = String::from("## OSA Review\n\n");
    let summary = summary.trim();
    if summary.is_empty() {
        out.push_str("_No summary produced._");
    } else {
        out.push_str(&truncate_text(summary, MAX_SUMMARY_CHARS));
    }

    let counts = severity_counts(accepted);
    out.push_str("\n\n");
    if accepted.is_empty() && dropped.is_empty() {
        out.push_str("No blocking issues found. ✅");
    } else {
        let mut tally = Vec::new();
        for severity in [
            Severity::Critical,
            Severity::Major,
            Severity::Minor,
            Severity::Nitpick,
        ] {
            if let Some(count) = counts.get(&severity) {
                tally.push(format!(
                    "{} {} {}",
                    severity.emoji(),
                    count,
                    severity.label().to_lowercase()
                ));
            }
        }
        if !tally.is_empty() {
            out.push_str(&format!("**Inline findings:** {}", tally.join(" · ")));
        }
    }

    if !dropped.is_empty() {
        out.push_str(
            "\n\n<details><summary>Additional findings (not pinned to a diff line)</summary>\n\n",
        );
        for finding in dropped.iter().take(30) {
            let severity = finding.severity();
            let title = if finding.title.trim().is_empty() {
                finding.body.lines().next().unwrap_or("finding").to_string()
            } else {
                finding.title.clone()
            };
            out.push_str(&format!(
                "- `{}:{}` {} {}\n",
                finding.path.trim(),
                finding.line,
                severity.emoji(),
                title.trim()
            ));
        }
        if dropped.len() > 30 {
            out.push_str(&format!("- …and {} more\n", dropped.len() - 30));
        }
        out.push_str("\n</details>");
    }

    if !notes.is_empty() {
        out.push_str("\n\n<details><summary>Review notes</summary>\n\n");
        for note in notes.iter().take(10) {
            out.push_str(&format!("- {}\n", note.trim()));
        }
        out.push_str("\n</details>");
    }

    out.push_str(&format!(
        "\n\n---\n<sub>Reviewed commit `{}` — {} file(s), +{}/-{}. Model: {}. OSA auto-review.</sub>\n<!-- osa-review:{} -->",
        short_sha(review_marker),
        stats.files_reviewed,
        stats.additions,
        stats.deletions,
        stats.model,
        review_marker
    ));
    truncate_text(&out, 60_000)
}

#[derive(Debug, Clone, Default)]
pub struct ReviewStats {
    pub files_reviewed: usize,
    pub additions: u64,
    pub deletions: u64,
    pub model: String,
}

/// Build the user message for one review batch.
pub fn build_review_user_prompt(
    repo: &str,
    pr: &GhPullRequest,
    commits: &[String],
    files: &[FileDiff],
    chunk_index: usize,
    chunk_count: usize,
    instructions: &str,
) -> String {
    let mut out = String::new();
    out.push_str(&format!("Repository: {}\n", repo));
    out.push_str(&format!(
        "Pull request #{}: {}\n",
        pr.number,
        pr.title.trim()
    ));
    out.push_str(&format!(
        "Author: {}\nBranches: {} <- {}\n",
        pr.author(),
        pr.base.ref_name,
        pr.head.ref_name
    ));
    out.push_str(&format!(
        "Stats: {} file(s) changed, +{}/-{}, {} commit(s)\n",
        pr.changed_files.unwrap_or(0),
        pr.additions.unwrap_or(0),
        pr.deletions.unwrap_or(0),
        pr.commits.unwrap_or(0)
    ));

    if let Some(body) = pr.body.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
        out.push_str("\nPR description:\n\"\"\"\n");
        out.push_str(&truncate_text(body, MAX_PR_BODY_CHARS));
        out.push_str("\n\"\"\"\n");
    }

    if !commits.is_empty() {
        out.push_str("\nCommit messages:\n");
        for commit in commits.iter().take(30) {
            out.push_str(&format!(
                "- {}\n",
                commit.lines().next().unwrap_or("").trim()
            ));
        }
    }

    if !instructions.trim().is_empty() {
        out.push_str("\nMaintainer review instructions (follow these closely):\n");
        out.push_str(instructions.trim());
        out.push('\n');
    }

    if chunk_count > 1 {
        out.push_str(&format!(
            "\nDiff files (part {} of {}):\n",
            chunk_index + 1,
            chunk_count
        ));
    } else {
        out.push_str("\nDiff files:\n");
    }

    for file in files {
        out.push_str(&format!(
            "\n### {} ({} lines in this hunk view)\n```diff\n{}```\n",
            file.filename,
            file.lines.len(),
            render_numbered(file)
        ));
    }

    out.push_str(
        "\nThe left gutter is the OLD file line number, the right gutter is the NEW file line number. \
         Review the changes above and reply with the JSON object only.\n",
    );
    out
}

/// Split files into batches bounded by `MAX_BATCH_CHARS`, preserving
/// order. Oversized single files are truncated with a note.
pub fn batch_files(files: Vec<FileDiff>, total_budget: usize) -> (Vec<Vec<FileDiff>>, Vec<String>) {
    let mut batches: Vec<Vec<FileDiff>> = Vec::new();
    let mut current: Vec<FileDiff> = Vec::new();
    let mut current_chars = 0usize;
    let mut consumed = 0usize;
    let mut notes = Vec::new();
    let mut skipped = 0usize;

    for mut file in files {
        let mut rendered_chars: usize = file.lines.iter().map(|l| l.content.len() + 12).sum();
        if consumed >= total_budget {
            skipped += 1;
            continue;
        }
        // Truncate a single file larger than a whole batch.
        if rendered_chars > MAX_BATCH_CHARS {
            let mut budget = MAX_BATCH_CHARS;
            file.lines.retain(|line| {
                let size = line.content.len() + 12;
                if budget >= size {
                    budget -= size;
                    true
                } else {
                    false
                }
            });
            notes.push(format!(
                "`{}` was truncated for review (large diff).",
                file.filename
            ));
            rendered_chars = MAX_BATCH_CHARS;
        }

        if current_chars + rendered_chars > MAX_BATCH_CHARS && !current.is_empty() {
            batches.push(std::mem::take(&mut current));
            current_chars = 0;
        }
        current_chars += rendered_chars;
        consumed += rendered_chars;
        current.push(file);
    }

    if !current.is_empty() {
        batches.push(current);
    }
    if skipped > 0 {
        notes.push(format!(
            "{} file(s) were skipped: diff exceeded the review budget.",
            skipped
        ));
    }
    (batches, notes)
}

/// Prompt used for the single repair retry after invalid JSON.
pub fn repair_prompt(previous: &str, error: &str) -> String {
    format!(
        "Your previous reply could not be parsed ({}). Reply with ONLY the corrected JSON object, no prose, no markdown fences.\n\nPrevious reply:\n{}",
        error,
        truncate_text(previous, 20_000)
    )
}

pub fn short_sha(sha: &str) -> String {
    sha.chars().take(8).collect()
}

fn first_sentence(text: &str) -> String {
    let candidate = text.lines().next().unwrap_or("").trim();
    let candidate = candidate
        .trim_start_matches('#')
        .trim_start_matches('*')
        .trim();
    truncate_text(candidate, 80)
}

fn truncate_text(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}…", head)
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::diff::parse_patch;

    fn sample_files() -> HashMap<String, FileDiff> {
        let mut files = HashMap::new();
        files.insert(
            "src/main.rs".to_string(),
            parse_patch(
                "src/main.rs",
                "@@ -10,4 +10,6 @@\n let a = 1;\n-let b = 2;\n+let b = 3;\n+let c = 4;\n let d = 5;\n",
            ),
        );
        files
    }

    #[test]
    fn extracts_json_from_fences_and_prose() {
        let text = "Here is my review:\n```json\n{\"summary\":\"ok\",\"findings\":[]}\n```\nDone.";
        let json = extract_json_object(text).expect("json found");
        assert!(json.starts_with("{\"summary\""));
        assert!(json.ends_with('}'));
    }

    #[test]
    fn extract_handles_braces_in_strings() {
        let text = "{\"summary\":\"uses {braces} inside\",\"findings\":[]}";
        let json = extract_json_object(text).expect("json found");
        assert_eq!(json, text);
    }

    #[test]
    fn parses_aliases_and_defaults() {
        let text = r#"{"overview":"looks fine","comments":[
            {"filename":"src/main.rs","new_line":11,"message":"Possible bug","description":"d","severity":"major"}
        ]}"#;
        let parsed = parse_review_response(text).expect("parses");
        assert_eq!(parsed.summary, "looks fine");
        assert_eq!(parsed.findings.len(), 1);
        assert_eq!(parsed.findings[0].line, 11);
        assert_eq!(parsed.findings[0].severity(), Severity::Major);
        assert_eq!(parsed.findings[0].side, "RIGHT");
    }

    #[test]
    fn invalid_json_returns_error_for_repair() {
        assert!(parse_review_response("Sorry, I cannot review this.").is_err());
    }

    #[test]
    fn validation_drops_unknown_paths_and_lines() {
        let files = sample_files();
        let findings = vec![
            ReviewFinding {
                path: "src/main.rs".into(),
                line: 11,
                side: "RIGHT".into(),
                severity: Some(Severity::Major),
                title: "ok".into(),
                body: "b".into(),
                suggestion: None,
            },
            ReviewFinding {
                path: "src/main.rs".into(),
                line: 9999,
                side: "RIGHT".into(),
                severity: Some(Severity::Minor),
                title: "bad line".into(),
                body: "b".into(),
                suggestion: None,
            },
            ReviewFinding {
                path: "nope.rs".into(),
                line: 1,
                side: "RIGHT".into(),
                severity: None,
                title: "bad path".into(),
                body: "b".into(),
                suggestion: None,
            },
        ];
        let outcome = validate_findings(findings, &files, 10);
        assert_eq!(outcome.accepted.len(), 1);
        assert_eq!(outcome.accepted[0].line, 11);
        assert_eq!(outcome.dropped.len(), 2);
    }

    #[test]
    fn validation_caps_and_keeps_most_severe() {
        let files = sample_files();
        let make = |line: u64, severity: Severity| ReviewFinding {
            path: "src/main.rs".into(),
            line,
            side: "RIGHT".into(),
            severity: Some(severity),
            title: format!("t{}", line),
            body: "b".into(),
            suggestion: None,
        };
        let findings = vec![
            make(10, Severity::Nitpick),
            make(11, Severity::Critical),
            make(12, Severity::Major),
        ];
        let outcome = validate_findings(findings, &files, 1);
        assert_eq!(outcome.accepted.len(), 1);
        assert_eq!(outcome.accepted[0].severity(), Severity::Critical);
        assert_eq!(outcome.dropped.len(), 2);
    }

    #[test]
    fn dedupe_same_title_line() {
        let files = sample_files();
        let finding = ReviewFinding {
            path: "src/main.rs".into(),
            line: 11,
            side: "RIGHT".into(),
            severity: Some(Severity::Major),
            title: "Same".into(),
            body: "b".into(),
            suggestion: None,
        };
        let outcome = validate_findings(vec![finding.clone(), finding], &files, 10);
        assert_eq!(outcome.accepted.len(), 1);
    }

    #[test]
    fn renders_suggestion_fences() {
        let finding = ReviewFinding {
            path: "a.rs".into(),
            line: 1,
            side: "RIGHT".into(),
            severity: Some(Severity::Critical),
            title: "Null deref".into(),
            body: "Guard it.".into(),
            suggestion: Some("let x = y?;".into()),
        };
        let rendered = finding.render_comment_body();
        assert!(rendered.contains("🚨 Critical: Null deref"));
        assert!(rendered.contains("```suggestion\nlet x = y?;\n```"));
    }

    #[test]
    fn compose_body_contains_marker_and_tally() {
        let finding = ReviewFinding {
            path: "a.rs".into(),
            line: 1,
            side: "RIGHT".into(),
            severity: Some(Severity::Major),
            title: "Bug".into(),
            body: "b".into(),
            suggestion: None,
        };
        let stats = ReviewStats {
            files_reviewed: 3,
            additions: 10,
            deletions: 2,
            model: "test-model".into(),
        };
        let body = compose_review_body(
            "Summary text",
            std::slice::from_ref(&finding),
            &[],
            &[],
            &stats,
            "abc12345deadbeef",
        );
        assert!(body.contains("## OSA Review"));
        assert!(body.contains("Summary text"));
        assert!(body.contains("⚠️ 1 major"));
        assert!(body.contains("<!-- osa-review:abc12345deadbeef -->"));
        assert!(body.contains("abc12345"));
    }

    #[test]
    fn batching_splits_large_sets() {
        let big_line = "x".repeat(1_000);
        let make_file = |name: &str, count: usize| {
            parse_patch(
                name,
                &format!(
                    "@@ -1,{0} +1,{0} @@\n{1}",
                    count,
                    (0..count)
                        .map(|i| format!("+{}{}\n", i, big_line))
                        .collect::<String>()
                ),
            )
        };
        let files = vec![
            make_file("a.rs", 50),
            make_file("b.rs", 50),
            make_file("c.rs", 50),
        ];
        let (batches, notes) = batch_files(files, MAX_TOTAL_PATCH_CHARS);
        assert!(batches.len() >= 2, "expected multiple batches");
        assert!(notes.is_empty());
        let total_files: usize = batches.iter().map(|b| b.len()).sum();
        assert_eq!(total_files, 3);
    }
}
