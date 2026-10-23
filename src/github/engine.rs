//! Review orchestration: resolves auth + config, fetches the pull request,
//! runs the model over batched diffs, validates findings, posts the review
//! to GitHub, and records state in SQLite. Shared by the `github` tool,
//! the web API, and the background auto-reviewer.

use std::collections::{HashMap, HashSet};
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use serde::Serialize;
use tokio::sync::Mutex;

use crate::agent::runtime::AgentRuntime;
use crate::config::{Config, GithubConfig};
use crate::error::{OSAgentError, Result};
use crate::storage::models::GithubReview;
use crate::storage::SqliteStorage;
use crate::storage::Message;

use super::client::{resolve_token, GithubClient, TokenSource};
use super::diff::parse_patch;
use super::review::{
    batch_files, build_review_user_prompt, compose_review_body, parse_review_response,
    repair_prompt, to_review_comments, validate_findings, ReviewDraft, ReviewFinding,
    ReviewStats, MAX_FILES, MAX_TOTAL_PATCH_CHARS, MODEL_CALL_TIMEOUT_SECS, REVIEW_SYSTEM_PROMPT,
};
use super::types::GhIssueComment;

const AUTO_RETRY_AFTER_MINUTES: i64 = 15;
const MAX_REVIEWS_PER_SWEEP: usize = 5;
/// First sight of a PR: only mentions from the last day may fire, so
/// enabling the feature does not review years of old chatter.
const MENTION_BACKLOG_HOURS: i64 = 24;

/// A mention comment accepted as a review request.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MentionTrigger {
    pub comment_id: u64,
    pub author: String,
    pub dry_run: bool,
    /// Review event override from the comment (`--approve` etc.).
    pub event: Option<String>,
    /// True for inline review-thread comments (different reaction/reply
    /// endpoints than conversation comments).
    pub is_review_comment: bool,
}

/// Does `body` contain `@handle` as a standalone mention? Case-insensitive,
/// with a trailing-boundary check so "@osagent" does not match "@osagents".
pub fn contains_mention(body: &str, handle: &str) -> bool {
    let handle = handle.trim().trim_start_matches('@').to_ascii_lowercase();
    if handle.is_empty() {
        return false;
    }
    let needle = format!("@{}", handle);
    let lower = body.to_ascii_lowercase();
    let mut search_from = 0usize;
    while let Some(index) = lower[search_from..].find(&needle) {
        let end = search_from + index + needle.len();
        let boundary = lower[end..]
            .chars()
            .next()
            .map(|c| !(c.is_ascii_alphanumeric() || c == '-' || c == '_'))
            .unwrap_or(true);
        if boundary {
            return true;
        }
        search_from = end;
    }
    false
}

/// Flags understood in a mention comment:
/// `--dry-run` (review locally, post nothing),
/// `--approve` / `--request-changes` (override the review event).
pub fn parse_mention_flags(body: &str) -> (bool, Option<String>) {
    let lower = body.to_ascii_lowercase();
    let dry_run = lower.contains("--dry-run") || lower.contains("--dry run");
    let event = if lower.contains("--request-changes")
        || lower.contains("--request_changes")
        || lower.contains("--requestchanges")
    {
        Some("REQUEST_CHANGES".to_string())
    } else if lower.contains("--approve") {
        Some("APPROVE".to_string())
    } else {
        None
    };
    (dry_run, event)
}

/// Highest conversation-comment id seen for one PR (baseline bookkeeping).
pub fn max_comment_id(comments: &[GhIssueComment], pr_number: u64) -> Option<u64> {
    comments
        .iter()
        .filter(|comment| comment.issue_number() == Some(pr_number))
        .map(|comment| comment.id)
        .max()
}

/// Pick the newest qualifying mention for `pr_number`.
///
/// `last_comment_id` is the highest comment id already consumed for this
/// PR (`None` = never seen). Comments at or below it are ignored, so each
/// mention fires exactly once. For a PR seen for the first time only
/// comments from the last `MENTION_BACKLOG_HOURS` may fire.
pub fn pick_mention_trigger(
    comments: &[GhIssueComment],
    pr_number: u64,
    handle: &str,
    last_comment_id: Option<u64>,
    now: chrono::DateTime<chrono::Utc>,
) -> Option<MentionTrigger> {
    let handle = handle.trim().trim_start_matches('@').to_ascii_lowercase();
    if handle.is_empty() {
        return None;
    }
    let mut best: Option<MentionTrigger> = None;
    for comment in comments {
        if comment.issue_number() != Some(pr_number) {
            continue;
        }
        match last_comment_id {
            Some(last) if comment.id <= last => continue,
            Some(_) => {}
            None => {
                let recent = comment
                    .created_at_utc()
                    .map(|created| {
                        now.signed_duration_since(created)
                            <= chrono::Duration::hours(MENTION_BACKLOG_HOURS)
                    })
                    .unwrap_or(false);
                if !recent {
                    continue;
                }
            }
        }
        let Some(body) = comment.body.as_deref() else {
            continue;
        };
        if !contains_mention(body, &handle) {
            continue;
        }
        let (dry_run, event) = parse_mention_flags(body);
        let candidate = MentionTrigger {
            comment_id: comment.id,
            author: comment.author().to_string(),
            dry_run,
            event,
            is_review_comment: comment.is_review_comment(),
        };
        if best
            .as_ref()
            .map(|current| candidate.comment_id > current.comment_id)
            .unwrap_or(true)
        {
            best = Some(candidate);
        }
    }
    best
}

#[derive(Debug, Clone)]
pub struct ReviewOptions {
    /// Post the review to GitHub. False runs the model for a dry-run result.
    pub post: bool,
    /// Re-review even when this head SHA was already reviewed.
    pub force: bool,
    /// Override the configured review event (COMMENT/REQUEST_CHANGES/APPROVE).
    pub event: Option<String>,
    /// Override the configured review model ("provider:model" or bare id).
    pub model: Option<String>,
    /// "manual" (tool/API) or "auto" (background poller).
    pub source: &'static str,
}

impl Default for ReviewOptions {
    fn default() -> Self {
        Self {
            post: true,
            force: true,
            event: None,
            model: None,
            source: "manual",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ReviewOutcome {
    pub repo: String,
    pub pr_number: u64,
    pub title: String,
    pub head_sha: String,
    pub posted: bool,
    pub review_url: Option<String>,
    pub summary: String,
    pub comments_posted: usize,
    pub comments_dropped: usize,
    pub skipped: Option<String>,
    pub model: String,
    pub duration_ms: u64,
}

impl ReviewOutcome {
    /// Model-facing rendering for the `github` tool.
    pub fn render(&self) -> String {
        let mut out = String::new();
        out.push_str(&format!(
            "{}#{} — {} (commit {})\n",
            self.repo,
            self.pr_number,
            self.title,
            super::review::short_sha(&self.head_sha)
        ));
        if let Some(skipped) = &self.skipped {
            out.push_str(&format!("Skipped: {}\n", skipped));
            return out;
        }
        if self.posted {
            out.push_str(&format!(
                "Review posted: {} inline comment(s){}.\n",
                self.comments_posted,
                if self.comments_dropped > 0 {
                    format!(", {} finding(s) summarized in the review body", self.comments_dropped)
                } else {
                    String::new()
                }
            ));
            if let Some(url) = &self.review_url {
                out.push_str(&format!("URL: {}\n", url));
            }
        } else {
            out.push_str(&format!(
                "Dry run (not posted): {} inline finding(s), {} dropped.\n",
                self.comments_posted, self.comments_dropped
            ));
        }
        out.push_str(&format!(
            "Model: {} · took {:.1}s\n\n",
            self.model,
            self.duration_ms as f64 / 1000.0
        ));
        out.push_str(&self.summary);
        out
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct PollSummary {
    pub swept: bool,
    pub repos: usize,
    pub reviewed: usize,
    pub skipped: usize,
    pub errors: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize)]
pub struct EngineStatusSnapshot {
    pub last_poll_at: Option<String>,
    pub last_poll_error: Option<String>,
    pub poll_running: bool,
    pub reviewing: Vec<String>,
}

#[derive(Debug, Clone, Default)]
struct EngineStatus {
    last_poll_at: Option<String>,
    last_poll_error: Option<String>,
    poll_running: bool,
    reviewing: Vec<String>,
}

#[derive(Debug, Clone)]
struct DeviceFlowSession {
    client_id: String,
    device_code: String,
    user_code: String,
    verification_uri: String,
    verification_uri_complete: Option<String>,
    expires_in: u64,
    interval: u64,
    started_at: Instant,
}

impl DeviceFlowSession {
    fn is_expired(&self) -> bool {
        self.started_at.elapsed() > Duration::from_secs(self.expires_in.max(60))
    }

    fn remaining_secs(&self) -> u64 {
        self.expires_in
            .saturating_sub(self.started_at.elapsed().as_secs())
    }

    fn snapshot(&self) -> DeviceFlowSnapshot {
        DeviceFlowSnapshot {
            user_code: self.user_code.clone(),
            verification_uri: self.verification_uri.clone(),
            verification_uri_complete: self.verification_uri_complete.clone(),
            expires_in: self.remaining_secs(),
            interval: self.interval,
        }
    }
}

/// Client-facing view of an in-flight sign-in (never the device code).
#[derive(Debug, Clone, Serialize)]
pub struct DeviceFlowSnapshot {
    pub user_code: String,
    pub verification_uri: String,
    /// URL with the code pre-filled. GitHub does not return one for every
    /// app, so OSA constructs it; the device page honors `?user_code=`.
    pub verification_uri_complete: Option<String>,
    /// Seconds until the code expires.
    pub expires_in: u64,
    pub interval: u64,
}

/// Result of starting (or resuming) a sign-in.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceFlowStart {
    /// True when an already-running sign-in was reused. Starting twice no
    /// longer strands the first code: the same one keeps being displayed
    /// and polled until it expires or completes.
    pub resumed: bool,
    #[serde(flatten)]
    pub snapshot: DeviceFlowSnapshot,
}

/// Append the user code so the device page opens pre-filled. GitHub does
/// not always return `verification_uri_complete`, but its login/device
/// page honors this query parameter (it survives the login redirect too).
pub fn build_verification_uri_complete(verification_uri: &str, user_code: &str) -> String {
    let separator = if verification_uri.contains('?') {
        '&'
    } else {
        '?'
    };
    format!(
        "{}{}user_code={}",
        verification_uri,
        separator,
        urlencoding::encode(user_code)
    )
}

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum DeviceFlowPoll {
    Pending,
    Complete {
        /// Never serialized: the caller persists it and the API only
        /// returns the verified identity.
        #[serde(skip)]
        token: String,
        #[serde(skip)]
        refresh_token: Option<String>,
        #[serde(skip)]
        expires_in: Option<u64>,
        #[serde(skip)]
        client_id: String,
        login: Option<String>,
        scopes: Vec<String>,
    },
    Expired,
}

/// Auth + config resolved for a GitHub call.
pub struct ResolvedGithub {
    pub client: GithubClient,
    pub token_source: TokenSource,
    pub config: GithubConfig,
}

/// Shared engine handle. Built once by the runtime and passed to the
/// `github` tool, the web routes, and the background poller. The runtime
/// back-reference is attached after `AgentRuntime` construction (the
/// engine only ever holds a `Weak`).
pub struct GithubEngine {
    storage: Arc<SqliteStorage>,
    runtime: OnceLock<Weak<AgentRuntime>>,
    review_lock: Mutex<()>,
    /// Serializes sign-in token refreshes so concurrent resolutions do not
    /// race the stored refresh token.
    refresh_lock: Mutex<()>,
    status: Mutex<EngineStatus>,
    device_flow: Mutex<Option<DeviceFlowSession>>,
}

impl GithubEngine {
    pub fn new(storage: Arc<SqliteStorage>) -> Self {
        Self {
            storage,
            runtime: OnceLock::new(),
            review_lock: Mutex::new(()),
            refresh_lock: Mutex::new(()),
            status: Mutex::new(EngineStatus::default()),
            device_flow: Mutex::new(None),
        }
    }

    pub fn attach_runtime(&self, runtime: &Arc<AgentRuntime>) {
        let _ = self.runtime.set(Arc::downgrade(runtime));
    }

    fn runtime(&self) -> Result<Arc<AgentRuntime>> {
        self.runtime
            .get()
            .and_then(Weak::upgrade)
            .ok_or_else(|| OSAgentError::Unknown("GitHub engine has no runtime handle".to_string()))
    }

    pub fn storage(&self) -> Arc<SqliteStorage> {
        self.storage.clone()
    }

    /// Read live config and resolve auth. Errors read like setup steps.
    /// Expiring sign-in tokens are refreshed just in time before use.
    pub async fn resolve(&self) -> Result<ResolvedGithub> {
        let runtime = self.runtime()?;
        let mut config = runtime.get_config().await;
        let (mut token, token_source) = resolve_token(&config).ok_or_else(|| {
            OSAgentError::ToolExecution(
                "GitHub is not connected. Add a personal access token in Settings > GitHub (or set GITHUB_TOKEN)."
                    .to_string(),
            )
        })?;

        // GitHub can issue sign-in tokens that expire (the "Expire user
        // access tokens" app setting). Refresh lazily, a few minutes ahead
        // of the deadline, so requests never race the expiry.
        if token_source == TokenSource::Config
            && config.github.token_needs_refresh(chrono::Utc::now().timestamp())
        {
            let _guard = self.refresh_lock.lock().await;
            // Re-read: another task may have refreshed while we waited.
            config = runtime.get_config().await;
            if config.github.token_needs_refresh(chrono::Utc::now().timestamp()) {
                match self.refresh_sign_in_token(&mut config).await {
                    Ok(()) => token = config.github.token.clone(),
                    Err(error) => {
                        tracing::warn!("GitHub token refresh failed: {}", error);
                    }
                }
            } else {
                token = config.github.token.clone();
            }
        }

        Ok(ResolvedGithub {
            client: GithubClient::new(token),
            token_source,
            config: config.github,
        })
    }

    /// Exchange the stored refresh token for a fresh access token and
    /// persist the result. Device-flow tokens refresh with the client id
    /// alone — no client secret.
    async fn refresh_sign_in_token(&self, config: &mut Config) -> Result<()> {
        let refresh_token = config.github.token_refresh.trim().to_string();
        if refresh_token.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "the stored sign-in token expired and no refresh token was saved; sign in with GitHub again in Settings > GitHub"
                    .to_string(),
            ));
        }
        let client_id = if config.github.token_client_id.trim().is_empty() {
            super::effective_oauth_client_id(&config.github.oauth_client_id)
        } else {
            config.github.token_client_id.trim().to_string()
        };
        if client_id.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "the stored sign-in token expired and no OAuth client id is available to refresh it; sign in with GitHub again in Settings > GitHub"
                    .to_string(),
            ));
        }

        let response = GithubClient::new("")
            .refresh_device_token(&client_id, &refresh_token)
            .await?;
        let Some(access_token) = response
            .access_token
            .filter(|value| !value.trim().is_empty())
        else {
            return Err(OSAgentError::ToolExecution(
                "GitHub token refresh returned no access token".to_string(),
            ));
        };

        config.github.token = access_token;
        if let Some(new_refresh) = response
            .refresh_token
            .filter(|value| !value.trim().is_empty())
        {
            config.github.token_refresh = new_refresh;
        }
        config.github.token_expires_at = response
            .expires_in
            .map(|secs| chrono::Utc::now().timestamp() + secs as i64)
            .unwrap_or(0);

        let runtime = self.runtime()?;
        runtime.replace_config(config.clone()).await;
        if let Err(error) = config.save_to_current_path() {
            tracing::warn!(
                "GitHub token refreshed but could not be persisted ({}); it will be re-refreshed next start",
                error
            );
        }
        tracing::info!("Refreshed the GitHub sign-in token");
        Ok(())
    }

    /// Live config without requiring a token (settings endpoints).
    pub async fn github_config(&self) -> Result<GithubConfig> {
        Ok(self.runtime()?.get_config().await.github)
    }

    pub async fn status_snapshot(&self) -> EngineStatusSnapshot {
        let status = self.status.lock().await;
        EngineStatusSnapshot {
            last_poll_at: status.last_poll_at.clone(),
            last_poll_error: status.last_poll_error.clone(),
            poll_running: status.poll_running,
            reviewing: status.reviewing.clone(),
        }
    }

    /// CodeRabbit-style review of one pull request. Serialized: one review
    /// runs at a time per process (model calls and posts are sequential).
    pub async fn review_pr(
        &self,
        repo: &str,
        number: u64,
        opts: ReviewOptions,
    ) -> Result<ReviewOutcome> {
        let repo = normalize_repo(repo)?;
        let review_key = format!("{}#{}", repo, number);
        {
            let mut status = self.status.lock().await;
            if !status.reviewing.contains(&review_key) {
                status.reviewing.push(review_key.clone());
            }
        }
        let _guard = self.review_lock.lock().await;
        let result = self.review_pr_inner(&repo, number, &opts).await;
        {
            let mut status = self.status.lock().await;
            status.reviewing.retain(|key| key != &review_key);
        }
        if let Err(ref error) = result {
            // Record hard failures against the head SHA when we can, so the
            // poller's retry window applies and the UI shows the error.
            if let Ok(resolved) = self.resolve().await {
                if let Ok(pr) = resolved.client.get_pull(&repo, number).await {
                    let _ = self.storage.upsert_github_review(&GithubReview::new(
                        repo.clone(),
                        number,
                        pr.head.sha.clone(),
                        "failed",
                        None,
                        0,
                        None,
                        Some(error.to_string()),
                    ));
                }
            }
        }
        result
    }

    async fn review_pr_inner(
        &self,
        repo: &str,
        number: u64,
        opts: &ReviewOptions,
    ) -> Result<ReviewOutcome> {
        let started = Instant::now();
        let ResolvedGithub {
            client, config, ..
        } = self.resolve().await?;

        let pr = client.get_pull(repo, number).await?;
        let head_sha = pr.head.sha.clone();

        // Skip work when this exact commit was already reviewed (auto mode).
        if !opts.force {
            if let Some(existing) = self.storage.github_review_for_sha(repo, number, &head_sha)? {
                if existing.status == "posted" {
                    return Ok(ReviewOutcome {
                        repo: repo.to_string(),
                        pr_number: number,
                        title: pr.title.clone(),
                        head_sha,
                        posted: false,
                        review_url: existing.review_url.clone(),
                        summary: String::new(),
                        comments_posted: 0,
                        comments_dropped: 0,
                        skipped: Some(format!(
                            "commit {} was already reviewed",
                            super::review::short_sha(&existing.head_sha)
                        )),
                        model: String::new(),
                        duration_ms: started.elapsed().as_millis() as u64,
                    });
                }
            }
        }

        let files = client
            .list_pull_files(repo, number, 2)
            .await?
            .into_iter()
            .take(MAX_FILES)
            .collect::<Vec<_>>();
        let commits = client
            .list_pull_commits(repo, number, 50)
            .await
            .unwrap_or_default()
            .into_iter()
            .map(|commit| commit.commit.message)
            .collect::<Vec<_>>();

        if files.is_empty() {
            return Err(OSAgentError::ToolExecution(format!(
                "{}#{} has no changed files to review.",
                repo, number
            )));
        }

        let mut diff_map: HashMap<String, super::diff::FileDiff> = HashMap::new();
        let diffs = files
            .iter()
            .map(|file| {
                let diff = parse_patch(
                    &file.filename,
                    file.patch.as_deref().unwrap_or_default(),
                );
                diff_map.insert(file.filename.clone(), diff.clone());
                diff
            })
            .collect::<Vec<_>>();

        let total_additions: u64 = files.iter().map(|f| f.additions).sum();
        let total_deletions: u64 = files.iter().map(|f| f.deletions).sum();
        let (batches, mut notes) = batch_files(diffs, MAX_TOTAL_PATCH_CHARS);
        if batches.len() > 1 {
            notes.push(format!(
                "The diff was large and was reviewed in {} passes.",
                batches.len()
            ));
        }

        let runtime = self.runtime()?;
        let model_ref = opts
            .model
            .clone()
            .unwrap_or_else(|| config.review_model.clone());
        let provider = runtime.provider_for_ref(&model_ref).await;
        let model_name = provider.current_model().await;

        let mut draft = ReviewDraft {
            notes,
            ..Default::default()
        };
        // opencode-go (and similar routers) reject a request that carries no
        // conversation id. A review has no chat session, so derive a stable
        // one from the pull request and the commit under review: every pass
        // and repair retry of the same review shares it.
        let review_session_id = build_review_session_id(repo, number, &head_sha);
        let mut successful_passes = 0usize;
        for (index, batch) in batches.iter().enumerate() {
            let user_prompt = build_review_user_prompt(
                repo,
                &pr,
                &commits,
                batch,
                index,
                batches.len(),
                &config.review_instructions,
            );
            let messages = vec![
                Message::system(REVIEW_SYSTEM_PROMPT.to_string()),
                Message::user(user_prompt),
            ];
            let completion = tokio::time::timeout(
                Duration::from_secs(MODEL_CALL_TIMEOUT_SECS),
                provider.complete(Some(&review_session_id), &messages, &[]),
            )
            .await;
            let raw = match completion {
                Ok(Ok(response)) => response.content.unwrap_or_default(),
                Ok(Err(error)) => {
                    draft.notes.push(format!(
                        "Review pass {}/{} could not run: {}",
                        index + 1,
                        batches.len(),
                        error
                    ));
                    continue;
                }
                Err(_) => {
                    draft.notes.push(format!(
                        "Review pass {}/{} timed out after {}s.",
                        index + 1,
                        batches.len(),
                        MODEL_CALL_TIMEOUT_SECS
                    ));
                    continue;
                }
            };

            let parsed = match parse_review_response(&raw) {
                Ok(parsed) => Ok(parsed),
                Err(parse_error) => {
                    // One repair retry: models occasionally wrap JSON in
                    // prose or fences even when told not to.
                    let repair_messages = vec![
                        Message::system(REVIEW_SYSTEM_PROMPT.to_string()),
                        Message::user(repair_prompt(&raw, &parse_error)),
                    ];
                    match tokio::time::timeout(
                        Duration::from_secs(MODEL_CALL_TIMEOUT_SECS),
                        provider.complete(Some(&review_session_id), &repair_messages, &[]),
                    )
                    .await
                    {
                        Ok(Ok(response)) => {
                            let text = response.content.unwrap_or_default();
                            parse_review_response(&text)
                        }
                        _ => Err(parse_error),
                    }
                }
            };

            match parsed {
                Ok(parsed) => {
                    successful_passes += 1;
                    if !parsed.summary.trim().is_empty() {
                        draft.summary_parts.push(parsed.summary);
                    }
                    draft.findings.extend(parsed.findings);
                }
                Err(error) => {
                    draft.notes.push(format!(
                        "Review pass {}/{} returned no usable JSON ({}).",
                        index + 1,
                        batches.len(),
                        error
                    ));
                }
            }
        }

        // A provider outage is not a clean review. Every pass failing means
        // there is nothing to say, so surface it as a failure (which the
        // poller retries) instead of posting a green review that only
        // contains error notes.
        if successful_passes == 0 {
            let reason = if draft.notes.is_empty() {
                "no review pass completed".to_string()
            } else {
                draft.notes.join(" ")
            };
            return Err(OSAgentError::Provider(format!(
                "GitHub review failed: {}",
                reason
            )));
        }

        let combined_summary = draft.summary_parts.join("\n\n");
        let outcome_validation = validate_findings(
            std::mem::take(&mut draft.findings),
            &diff_map,
            config.max_inline_comments.max(1),
        );
        let stats = ReviewStats {
            files_reviewed: files.len(),
            additions: total_additions,
            deletions: total_deletions,
            model: model_name.clone(),
        };
        let body = compose_review_body(
            &combined_summary,
            &outcome_validation.accepted,
            &outcome_validation.dropped,
            &draft.notes,
            &stats,
            &head_sha,
        );

        let event = opts
            .event
            .clone()
            .unwrap_or_else(|| config.normalized_review_event().to_string());
        let comments = to_review_comments(&outcome_validation.accepted);
        let comment_count = comments.len();
        let dropped_count = outcome_validation.dropped.len();

        let mut posted = false;
        let mut posted_inline = 0usize;
        let mut review_url = None;
        if opts.post {
            match client
                .create_review(repo, number, &head_sha, &body, &event, &comments)
                .await
            {
                Ok(review) => {
                    posted = true;
                    posted_inline = comment_count;
                    review_url = review.html_url;
                }
                Err(first_error) => {
                    // Comment placement can be rejected (stale diff, etc.).
                    // Degrade to a summary-only review, then to a plain
                    // issue comment, so the analysis is never lost.
                    let mut fallback_body = body.clone();
                    if comment_count > 0 {
                        fallback_body.push_str(&render_fallback_comments(&outcome_validation.accepted));
                    }
                    match client
                        .create_review(repo, number, &head_sha, &fallback_body, &event, &[])
                        .await
                    {
                        Ok(review) => {
                            posted = true;
                            posted_inline = 0;
                            review_url = review.html_url;
                        }
                        Err(_) => {
                            match client
                                .create_issue_comment(repo, number, &fallback_body)
                                .await
                            {
                                Ok(comment) => {
                                    posted = true;
                                    posted_inline = 0;
                                    review_url = comment.html_url;
                                }
                                Err(_) => {
                                    return Err(first_error);
                                }
                            }
                        }
                    }
                }
            }
        }

        let status = if posted {
            "posted"
        } else if opts.post {
            "failed"
        } else {
            "dry_run"
        };
        let summary_for_state = truncate_state(&combined_summary);
        let _ = self.storage.upsert_github_review(&GithubReview::new(
            repo.to_string(),
            number,
            head_sha.clone(),
            status,
            Some(summary_for_state),
            posted_inline as u64,
            review_url.clone(),
            None,
        ));

        Ok(ReviewOutcome {
            repo: repo.to_string(),
            pr_number: number,
            title: pr.title.clone(),
            head_sha,
            posted,
            review_url,
            summary: combined_summary,
            // Dry runs report the findings the review would post; real runs
            // report what actually landed inline.
            comments_posted: if opts.post { posted_inline } else { comment_count },
            comments_dropped: dropped_count,
            skipped: None,
            model: model_name,
            duration_ms: started.elapsed().as_millis() as u64,
        })
    }

    /// One auto-review sweep across the watched repositories.
    pub async fn poll_once(&self) -> Result<PollSummary> {
        {
            let mut status = self.status.lock().await;
            if status.poll_running {
                return Ok(PollSummary {
                    swept: false,
                    repos: 0,
                    reviewed: 0,
                    skipped: 0,
                    errors: Vec::new(),
                });
            }
            status.poll_running = true;
            status.last_poll_error = None;
        }

        let result = self.poll_once_inner().await;
        let mut status = self.status.lock().await;
        status.poll_running = false;
        status.last_poll_at = Some(chrono::Utc::now().to_rfc3339());
        if let Err(error) = &result {
            status.last_poll_error = Some(error.to_string());
        }
        result
    }

    async fn poll_once_inner(&self) -> Result<PollSummary> {
        let ResolvedGithub {
            client, config, ..
        } = self.resolve().await?;
        let mut summary = PollSummary {
            swept: true,
            repos: 0,
            reviewed: 0,
            skipped: 0,
            errors: Vec::new(),
        };
        let now = chrono::Utc::now();
        let mention_handle = config.normalized_mention_handle();
        let mentions_enabled = config.mentions_enabled();

        for repo in config.watch_repos.clone() {
            let repo = match normalize_repo(&repo) {
                Ok(repo) => repo,
                Err(error) => {
                    summary.errors.push(error.to_string());
                    continue;
                }
            };
            let pulls = match client.list_pulls(&repo, "open", 1).await {
                Ok(pulls) => pulls,
                Err(error) => {
                    summary.errors.push(format!("{}: {}", repo, error));
                    continue;
                }
            };
            summary.repos += 1;

            // Mention trigger: one repo-wide comment fetch covers every
            // open PR; each qualifying comment fires at most once.
            let mut mentions: HashMap<u64, MentionTrigger> = HashMap::new();
            let mut comment_baselines: HashMap<u64, u64> = HashMap::new();
            if mentions_enabled && !pulls.is_empty() {
                let open_numbers: HashSet<u64> =
                    pulls.iter().map(|pull| pull.number).collect();
                match client.list_repo_issue_comments(&repo, 2).await {
                    Ok(mut comments) => {
                        // Inline review threads count too: tagging inside a
                        // code comment should trigger like a conversation
                        // comment. A failure here degrades to conversation
                        // comments only rather than skipping the repo.
                        match client.list_repo_review_comments(&repo, 2).await {
                            Ok(mut review_comments) => comments.append(&mut review_comments),
                            Err(error) => summary.errors.push(format!(
                                "{}: review-comment scan failed: {}",
                                repo, error
                            )),
                        }
                        for number in &open_numbers {
                            if let Some(max_id) = max_comment_id(&comments, *number) {
                                comment_baselines.insert(*number, max_id);
                            }
                        }
                        for number in &open_numbers {
                            let last = self
                                .storage
                                .github_trigger_state(&repo, *number)?
                                .map(|value| value.max(0) as u64);
                            if let Some(trigger) = pick_mention_trigger(
                                &comments,
                                *number,
                                &mention_handle,
                                last,
                                now,
                            ) {
                                mentions.insert(*number, trigger);
                            }
                        }
                    }
                    Err(error) => summary
                        .errors
                        .push(format!("{}: mention scan failed: {}", repo, error)),
                }
            }

            for pr in pulls {
                if summary.reviewed >= MAX_REVIEWS_PER_SWEEP {
                    break;
                }
                let last = self
                    .storage
                    .latest_github_review(&repo, pr.number)
                    .ok()
                    .flatten();
                let auto_due = config.auto_review && needs_review(&config, &pr, last.as_ref(), now);
                let mention = mentions.get(&pr.number).cloned();

                if !auto_due && mention.is_none() {
                    // Nothing to do, but advance the comment baseline so
                    // consumed chatter is not re-scanned on every sweep.
                    self.advance_trigger_state(
                        &repo,
                        pr.number,
                        comment_baselines.get(&pr.number).copied(),
                    )?;
                    summary.skipped += 1;
                    continue;
                }

                // An explicit mention wins over commit-triggered review:
                // it re-reviews even the same commit and carries flags.
                let options = if let Some(ref trigger) = mention {
                    ReviewOptions {
                        post: !trigger.dry_run,
                        force: true,
                        event: trigger.event.clone(),
                        model: None,
                        source: "mention",
                    }
                } else {
                    ReviewOptions {
                        post: true,
                        force: false,
                        event: None,
                        model: None,
                        source: "auto",
                    }
                };

                // Acknowledge with 👀 and answer in the thread once done.
                if let Some(ref trigger) = mention {
                    self.acknowledge_mention(&client, &repo, trigger).await;
                }

                let outcome = self.review_pr(&repo, pr.number, options).await;
                // Consume the mention either way: a failure is recorded in
                // the review history, and re-requesting is one comment.
                self.advance_trigger_state(
                    &repo,
                    pr.number,
                    comment_baselines.get(&pr.number).copied(),
                )?;
                match outcome {
                    Ok(outcome) if outcome.skipped.is_none() => {
                        if let Some(ref trigger) = mention {
                            self.reply_to_mention(&client, &repo, pr.number, trigger, &outcome)
                                .await;
                        }
                        summary.reviewed += 1;
                    }
                    Ok(_) => summary.skipped += 1,
                    Err(error) => {
                        summary
                            .errors
                            .push(format!("{}#{}: {}", repo, pr.number, error));
                    }
                }
                // Be gentle on rate limits between reviews.
                tokio::time::sleep(Duration::from_secs(2)).await;
            }
        }
        Ok(summary)
    }

    /// Record the highest conversation-comment id consumed for a PR.
    fn advance_trigger_state(
        &self,
        repo: &str,
        pr_number: u64,
        max_id: Option<u64>,
    ) -> Result<()> {
        let Some(max_id) = max_id else {
            return Ok(());
        };
        let previous = self
            .storage
            .github_trigger_state(repo, pr_number)?
            .unwrap_or(0);
        if max_id as i64 > previous {
            self.storage
                .upsert_github_trigger_state(repo, pr_number, max_id as i64)?;
        }
        Ok(())
    }

    /// 👀 on the triggering comment so the requester can see OSA picked
    /// the mention up while the (slow) review runs.
    async fn acknowledge_mention(
        &self,
        poster: &GithubClient,
        repo: &str,
        trigger: &MentionTrigger,
    ) {
        let result = if trigger.is_review_comment {
            poster
                .add_review_comment_reaction(repo, trigger.comment_id, "eyes")
                .await
        } else {
            poster
                .add_issue_comment_reaction(repo, trigger.comment_id, "eyes")
                .await
        };
        if let Err(error) = result {
            tracing::warn!("GitHub mention reaction failed: {}", error);
        }
    }

    /// Answer the mention once the review has been handled: a link on
    /// success, a note that nothing was posted on dry runs.
    async fn reply_to_mention(
        &self,
        poster: &GithubClient,
        repo: &str,
        pr_number: u64,
        trigger: &MentionTrigger,
        outcome: &ReviewOutcome,
    ) {
        let body = if outcome.posted {
            match outcome.review_url.as_deref() {
                Some(url) => format!("✅ Done, review posted: {}", url),
                None => "✅ Done, review posted.".to_string(),
            }
        } else if trigger.dry_run {
            "Done, dry run only. Nothing was posted to GitHub; results are in OSA's review history."
                .to_string()
        } else {
            "Done, no review was posted. See OSA's review history for details.".to_string()
        };
        let result = if trigger.is_review_comment {
            poster
                .reply_to_review_comment(repo, pr_number, trigger.comment_id, &body)
                .await
        } else {
            // Conversation comments have no threading; a normal comment
            // continues the discussion.
            poster
                .create_issue_comment(repo, pr_number, &body)
                .await
                .map(|_| ())
        };
        if let Err(error) = result {
            tracing::warn!("GitHub mention reply failed: {}", error);
        }
    }

    // ── Device flow ─────────────────────────────────────────────────────

    /// Start a sign-in, or resume the in-flight one. Resuming keeps the
    /// displayed code and the polled device code in sync no matter how
    /// many times the pane asks (reloads, second tabs, double clicks).
    pub async fn start_device_flow(&self) -> Result<DeviceFlowStart> {
        {
            let session = self.device_flow.lock().await;
            if let Some(existing) = session.as_ref() {
                if !existing.is_expired() {
                    return Ok(DeviceFlowStart {
                        resumed: true,
                        snapshot: existing.snapshot(),
                    });
                }
            }
        }

        let config = self.github_config().await?;
        let client_id = super::effective_oauth_client_id(&config.oauth_client_id);
        if client_id.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "GitHub sign-in needs an OAuth app client id. Create one at github.com/settings/applications/new with 'Enable Device Flow' checked, then paste the client id in Settings > GitHub."
                    .to_string(),
            ));
        }
        let client = GithubClient::new("");
        let response = client
            .start_device_flow(&client_id, "repo read:user")
            .await?;

        // GitHub's TTL is 900s; clamp anything odd into a sane window.
        let expires_in = if response.expires_in == 0 {
            900
        } else {
            response.expires_in.clamp(120, 1800)
        };
        let verification_uri_complete = response
            .verification_uri_complete
            .clone()
            .filter(|uri| !uri.trim().is_empty())
            .or_else(|| {
                Some(build_verification_uri_complete(
                    &response.verification_uri,
                    &response.user_code,
                ))
            });

        let session = DeviceFlowSession {
            client_id,
            device_code: response.device_code,
            user_code: response.user_code,
            verification_uri: response.verification_uri,
            verification_uri_complete,
            expires_in,
            interval: response.interval.max(5),
            started_at: Instant::now(),
        };
        let snapshot = session.snapshot();
        tracing::info!(
            "GitHub sign-in started (code {}, expires in {}s)",
            snapshot.user_code,
            snapshot.expires_in
        );
        *self.device_flow.lock().await = Some(session);
        Ok(DeviceFlowStart {
            resumed: false,
            snapshot,
        })
    }

    /// In-flight sign-in for the settings pane, so a reload can resume it.
    pub async fn device_flow_snapshot(&self) -> Option<DeviceFlowSnapshot> {
        let session = self.device_flow.lock().await;
        session
            .as_ref()
            .filter(|session| !session.is_expired())
            .map(DeviceFlowSession::snapshot)
    }

    /// Drop the in-flight sign-in (user pressed Cancel).
    pub async fn cancel_device_flow(&self) {
        let mut session = self.device_flow.lock().await;
        if session.take().is_some() {
            tracing::info!("GitHub sign-in cancelled");
        }
    }

    pub async fn poll_device_flow(&self) -> Result<DeviceFlowPoll> {
        let session = {
            let session = self.device_flow.lock().await;
            session.clone()
        };
        let Some(session) = session else {
            return Err(OSAgentError::ToolExecution(
                "No GitHub sign-in in progress. Start it from Settings > GitHub.".to_string(),
            ));
        };
        if session.is_expired() {
            let mut guard = self.device_flow.lock().await;
            *guard = None;
            tracing::info!("GitHub sign-in code expired");
            return Ok(DeviceFlowPoll::Expired);
        }

        let client = GithubClient::new("");
        let polled = client
            .poll_device_flow(&session.client_id, &session.device_code)
            .await;
        let polled = match polled {
            Ok(polled) => polled,
            Err(error) => {
                tracing::warn!("GitHub device flow poll failed: {}", error);
                return Err(error);
            }
        };
        match polled {
            None => Ok(DeviceFlowPoll::Pending),
            Some(token_response) => {
                let Some(token) = token_response
                    .access_token
                    .clone()
                    .filter(|t| !t.trim().is_empty())
                else {
                    return Ok(DeviceFlowPoll::Pending);
                };
                {
                    let mut guard = self.device_flow.lock().await;
                    *guard = None;
                }
                // Verify + fetch the identity for the settings UI.
                let token_client = GithubClient::new(token.clone());
                let (user, scopes) = match token_client.whoami().await {
                    Ok(identity) => identity,
                    Err(error) => {
                        tracing::warn!(
                            "GitHub sign-in token was issued but identity check failed: {}",
                            error
                        );
                        return Err(error);
                    }
                };
                tracing::info!("GitHub sign-in completed for {}", user.login);
                Ok(DeviceFlowPoll::Complete {
                    token,
                    refresh_token: token_response
                        .refresh_token
                        .filter(|value| !value.trim().is_empty()),
                    expires_in: token_response.expires_in,
                    client_id: session.client_id.clone(),
                    login: Some(user.login),
                    scopes: if scopes.is_empty() {
                        token_response
                            .scope
                            .map(|s| s.split(',').map(|v| v.trim().to_string()).collect())
                            .unwrap_or_default()
                    } else {
                        scopes
                    },
                })
            }
        }
    }
}

/// Should the auto-reviewer pick up this pull request?
pub fn needs_review(
    config: &GithubConfig,
    pr: &super::types::GhPullRequest,
    last: Option<&GithubReview>,
    now: chrono::DateTime<chrono::Utc>,
) -> bool {
    if pr.is_draft() && !config.include_drafts {
        return false;
    }
    let author = pr.author().to_ascii_lowercase();
    if config
        .ignore_authors
        .iter()
        .any(|ignored| ignored.trim().to_ascii_lowercase() == author)
    {
        return false;
    }
    let head_sha = pr.head.sha.as_str();
    match last {
        None => true,
        Some(row) => {
            if row.head_sha != head_sha {
                return true;
            }
            if row.status == "posted" {
                return false;
            }
            // Failed or skipped at this SHA: retry after a cooldown so a
            // persistent failure does not burn the API every sweep.
            now.signed_duration_since(row.updated_at).num_minutes() >= AUTO_RETRY_AFTER_MINUTES
        }
    }
}

/// Accept "owner/repo", GitHub URLs, and "git@github.com:owner/repo.git".
pub fn normalize_repo(input: &str) -> Result<String> {
    let trimmed = input.trim().trim_end_matches(".git");
    if trimmed.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "Repository is required (owner/repo).".to_string(),
        ));
    }
    let candidate = if let Some(rest) = trimmed
        .strip_prefix("https://github.com/")
        .or_else(|| trimmed.strip_prefix("http://github.com/"))
        .or_else(|| trimmed.strip_prefix("github.com/"))
    {
        rest.trim_end_matches('/').to_string()
    } else if let Some(rest) = trimmed.strip_prefix("git@github.com:") {
        rest.trim_end_matches('/').to_string()
    } else {
        trimmed.to_string()
    };
    let parts: Vec<&str> = candidate.split('/').filter(|p| !p.is_empty()).collect();
    if parts.len() != 2 {
        return Err(OSAgentError::ToolExecution(format!(
            "Invalid repository '{}' — expected owner/repo.",
            input.trim()
        )));
    }
    Ok(format!("{}/{}", parts[0], parts[1]))
}

/// A stable conversation id for one review. A review runs outside any chat
/// session, and some providers (opencode-go) reject a request that carries no
/// session id, so the pull request number and the head commit are enough to
/// give every pass of the same review the same id.
fn build_review_session_id(repo: &str, number: u64, head_sha: &str) -> String {
    format!(
        "github-review-{}-{}-{}",
        repo.replace('/', "-"),
        number,
        head_sha.chars().take(12).collect::<String>()
    )
}

fn render_fallback_comments(findings: &[ReviewFinding]) -> String {
    let mut out = String::from(
        "\n\n### Inline comments (posted in the review body)\n",
    );
    for finding in findings.iter().take(30) {
        out.push_str(&format!(
            "\n**{} `{}:{}` — {}**\n\n{}\n",
            finding.severity().emoji(),
            finding.path.trim(),
            finding.line,
            finding.title.trim(),
            finding.body.trim()
        ));
        if let Some(suggestion) = finding
            .suggestion
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            out.push_str(&format!("\n```suggestion\n{}\n```\n", suggestion));
        }
    }
    out
}

fn truncate_state(summary: &str) -> String {
    summary.chars().take(4_000).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::github::types::{GhPullRequest, GhRef};

    fn pr(head_sha: &str, draft: bool, author: &str) -> GhPullRequest {
        GhPullRequest {
            number: 7,
            title: "t".to_string(),
            body: None,
            state: "open".to_string(),
            draft: Some(draft),
            merged: None,
            mergeable: None,
            user: Some(crate::github::types::GhOwner {
                login: author.to_string(),
                avatar_url: None,
            }),
            head: GhRef {
                sha: head_sha.to_string(),
                ref_name: "feature".to_string(),
                label: None,
            },
            base: GhRef {
                sha: "base".to_string(),
                ref_name: "main".to_string(),
                label: None,
            },
            html_url: None,
            additions: None,
            deletions: None,
            changed_files: None,
            commits: None,
            created_at: None,
            updated_at: None,
            labels: None,
        }
    }

    fn review_row(sha: &str, status: &str, updated: chrono::DateTime<chrono::Utc>) -> GithubReview {
        let mut row = GithubReview::new(
            "o/r".to_string(),
            7,
            sha.to_string(),
            status,
            None,
            0,
            None,
            None,
        );
        row.updated_at = updated;
        row
    }

    #[test]
    fn normalize_repo_accepts_urls_and_rejects_junk() {
        assert_eq!(normalize_repo("owner/repo").unwrap(), "owner/repo");
        assert_eq!(
            normalize_repo("https://github.com/owner/repo.git").unwrap(),
            "owner/repo"
        );
        assert_eq!(
            normalize_repo("git@github.com:owner/repo.git").unwrap(),
            "owner/repo"
        );
        assert!(normalize_repo("just-a-name").is_err());
        assert!(normalize_repo("").is_err());
    }

    #[test]
    fn a_review_gets_a_stable_non_empty_session_id() {
        // opencode-go rejects a review request without a session id, and every
        // pass of one review should share it.
        let id = build_review_session_id("owner/repo", 42, "0123456789abcdef0123456789abcdef");
        assert_eq!(id, "github-review-owner-repo-42-0123456789ab");
        assert_eq!(
            id,
            build_review_session_id("owner/repo", 42, "0123456789abcdef0123456789abcdef")
        );
        assert_ne!(
            id,
            build_review_session_id("owner/repo", 42, "fedcba9876543210fedcba9876543210")
        );
        assert!(!id.trim().is_empty());
    }

    #[test]
    fn auto_review_skips_drafts_and_ignored_authors() {
        let mut config = GithubConfig::default();
        let now = chrono::Utc::now();
        assert!(!needs_review(&config, &pr("a", true, "human"), None, now));
        config.include_drafts = true;
        assert!(needs_review(&config, &pr("a", true, "human"), None, now));
        assert!(!needs_review(
            &config,
            &pr("a", false, "dependabot[bot]"),
            None,
            now
        ));
    }

    #[test]
    fn auto_review_dedupes_by_sha_with_retry_window() {
        let config = GithubConfig::default();
        let now = chrono::Utc::now();
        // Already posted for this sha → skip.
        assert!(!needs_review(
            &config,
            &pr("abc", false, "human"),
            Some(&review_row("abc", "posted", now)),
            now
        ));
        // New sha → review.
        assert!(needs_review(
            &config,
            &pr("def", false, "human"),
            Some(&review_row("abc", "posted", now)),
            now
        ));
        // Failed recently → cooldown.
        assert!(!needs_review(
            &config,
            &pr("abc", false, "human"),
            Some(&review_row("abc", "failed", now)),
            now
        ));
        // Failed long ago → retry.
        let long_ago = now - chrono::Duration::minutes(30);
        assert!(needs_review(
            &config,
            &pr("abc", false, "human"),
            Some(&review_row("abc", "failed", long_ago)),
            now
        ));
    }

    fn comment(
        id: u64,
        pr_number: u64,
        body: &str,
        created: chrono::DateTime<chrono::Utc>,
    ) -> GhIssueComment {
        GhIssueComment {
            id,
            body: Some(body.to_string()),
            user: Some(crate::github::types::GhOwner {
                login: "reviewer".to_string(),
                avatar_url: None,
            }),
            created_at: Some(created.to_rfc3339()),
            html_url: None,
            issue_url: Some(format!(
                "https://api.github.com/repos/acme/widget/issues/{}",
                pr_number
            )),
            pull_request_url: None,
        }
    }

    #[test]
    fn contains_mention_requires_a_word_boundary() {
        assert!(contains_mention("@osagent review this", "osagent"));
        assert!(contains_mention("hey @OSAgent, take a look", "osagent"));
        assert!(contains_mention("@osagent, please", "osagent"));
        assert!(contains_mention("ping @osagent", "@osagent"));
        assert!(!contains_mention("@osagents are great", "osagent"));
        assert!(!contains_mention("@osa", "osagent"));
        assert!(!contains_mention("osagent without at", "osagent"));
        assert!(!contains_mention("@osagent", ""));
    }

    #[test]
    fn mention_flags_parse() {
        assert_eq!(parse_mention_flags("@osagent review"), (false, None));
        assert_eq!(
            parse_mention_flags("@osagent review --dry-run"),
            (true, None)
        );
        assert_eq!(
            parse_mention_flags("@osagent --approve"),
            (false, Some("APPROVE".to_string()))
        );
        assert_eq!(
            parse_mention_flags("@osagent --request-changes please"),
            (false, Some("REQUEST_CHANGES".to_string()))
        );
        // request-changes beats approve when both appear.
        assert_eq!(
            parse_mention_flags("@osagent --approve --request-changes"),
            (false, Some("REQUEST_CHANGES".to_string()))
        );
    }

    #[test]
    fn pick_mention_respects_baseline_and_recency() {
        let now = chrono::Utc::now();
        let comments = vec![
            comment(1, 7, "@osagent review", now - chrono::Duration::hours(2)),
            comment(2, 7, "@osagent again", now - chrono::Duration::hours(1)),
            comment(3, 8, "@osagent on another pr", now),
        ];
        // Baseline 1 → only comment 2 qualifies, and the newest wins.
        let picked = pick_mention_trigger(&comments, 7, "osagent", Some(1), now).expect("picked");
        assert_eq!(picked.comment_id, 2);
        assert!(!picked.is_review_comment);
        // Baseline covers everything → nothing.
        assert!(pick_mention_trigger(&comments, 7, "osagent", Some(3), now).is_none());
        // Other PRs are isolated.
        let other = pick_mention_trigger(&comments, 8, "osagent", None, now).expect("other");
        assert_eq!(other.comment_id, 3);
    }

    #[test]
    fn pick_mention_skips_old_history_on_first_sight() {
        let now = chrono::Utc::now();
        let comments = vec![
            comment(1, 7, "@osagent old", now - chrono::Duration::hours(48)),
            comment(2, 7, "@osagent fresh", now - chrono::Duration::minutes(5)),
        ];
        let picked = pick_mention_trigger(&comments, 7, "osagent", None, now).expect("picked");
        assert_eq!(picked.comment_id, 2, "only the recent mention fires");
        assert_eq!(max_comment_id(&comments, 7), Some(2));
    }

    #[test]
    fn pick_mention_ignores_non_matching_bodies() {
        let now = chrono::Utc::now();
        let comments = vec![
            comment(1, 7, "no mention here", now),
            comment(2, 7, "@osagents plural", now),
        ];
        assert!(pick_mention_trigger(&comments, 7, "osagent", None, now).is_none());
    }

    #[test]
    fn inline_review_comments_carry_their_pr_number() {
        let now = chrono::Utc::now();
        let mut inline = comment(5, 7, "@osagent-review take a look", now);
        inline.issue_url = None;
        inline.pull_request_url =
            Some("https://api.github.com/repos/acme/widget/pulls/7".to_string());
        assert_eq!(inline.issue_number(), Some(7));
        let picked =
            pick_mention_trigger(&[inline], 7, "osagent-review", None, now).expect("picked");
        assert_eq!(picked.comment_id, 5);
        assert!(picked.is_review_comment);
    }

    #[test]
    fn token_refresh_window() {
        let mut config = GithubConfig::default();
        // 0 = never expires: no refresh, whatever the clock says.
        assert!(!config.token_needs_refresh(1_000_000));
        // Expiring token: refresh inside the 5-minute margin only.
        config.token_expires_at = 1_000_000;
        assert!(!config.token_needs_refresh(900_000));
        assert!(config.token_needs_refresh(999_700));
        assert!(config.token_needs_refresh(1_000_001)); // already expired
    }

    #[test]
    fn builds_prefilled_verification_uri() {
        // GitHub does not return verification_uri_complete for every
        // app; OSA constructs it, and the device page honors it.
        assert_eq!(
            build_verification_uri_complete("https://github.com/login/device", "WDJB-MJHT"),
            "https://github.com/login/device?user_code=WDJB-MJHT"
        );
        // Existing query strings are respected.
        assert_eq!(
            build_verification_uri_complete("https://example.com/device?x=1", "AB-CD"),
            "https://example.com/device?x=1&user_code=AB-CD"
        );
    }
}
