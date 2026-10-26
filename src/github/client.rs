//! Thin GitHub REST client: bearer auth, JSON decoding, pagination, and
//! error mapping to OSAgentError with actionable messages (rate limits,
//! auth failures, missing scopes).

use crate::error::{OSAgentError, Result};
use reqwest::{header::HeaderMap, Client, Method, StatusCode};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::time::Duration;

use super::types::*;

pub const GITHUB_API_BASE: &str = "https://api.github.com";
/// Accept header for the raw unified diff of a pull request.
pub const DIFF_ACCEPT: &str = "application/vnd.github.v3.diff";
const USER_AGENT: &str = "OSAgent-GitHub";

/// Where the active token came from (surfaced in settings/status).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum TokenSource {
    Config,
    EnvGithubToken,
    EnvGhToken,
}

impl TokenSource {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Config => "config",
            Self::EnvGithubToken => "env:GITHUB_TOKEN",
            Self::EnvGhToken => "env:GH_TOKEN",
        }
    }
}

/// Resolve a token: explicit config first, then environment fallbacks.
pub fn resolve_token(config: &crate::config::Config) -> Option<(String, TokenSource)> {
    let from_config = config.github.token.trim();
    if !from_config.is_empty() {
        return Some((from_config.to_string(), TokenSource::Config));
    }
    if let Ok(token) = std::env::var("GITHUB_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some((token, TokenSource::EnvGithubToken));
        }
    }
    if let Ok(token) = std::env::var("GH_TOKEN") {
        let token = token.trim().to_string();
        if !token.is_empty() {
            return Some((token, TokenSource::EnvGhToken));
        }
    }
    None
}

pub struct GithubClient {
    http: Client,
    token: String,
    base: String,
}

impl GithubClient {
    pub fn new(token: impl Into<String>) -> Self {
        Self {
            http: Client::builder()
                .user_agent(USER_AGENT)
                .timeout(Duration::from_secs(120))
                .build()
                .unwrap_or_else(|_| Client::new()),
            token: token.into(),
            base: GITHUB_API_BASE.to_string(),
        }
    }

    #[cfg(test)]
    pub fn with_base(token: impl Into<String>, base: impl Into<String>) -> Self {
        let mut client = Self::new(token);
        client.base = base.into();
        client
    }

    fn url(&self, path: &str) -> String {
        format!("{}{}", self.base.trim_end_matches('/'), path)
    }

    /// One JSON request. `body` is serialized as JSON when present.
    async fn request_json(
        &self,
        method: Method,
        path: &str,
        body: Option<Value>,
        accept: Option<&str>,
    ) -> Result<(StatusCode, HeaderMap, Value)> {
        let mut req = self
            .http
            .request(method, self.url(path))
            .bearer_auth(&self.token)
            .header("Accept", accept.unwrap_or("application/vnd.github+json"))
            .header("X-GitHub-Api-Version", "2022-11-28");
        if let Some(body) = body {
            req = req.json(&body);
        }
        let response = req
            .send()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("GitHub request failed: {}", e)))?;
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await.map_err(|e| {
            OSAgentError::ToolExecution(format!("GitHub response could not be read: {}", e))
        })?;

        if !status.is_success() {
            return Err(map_error_response(status, &headers, &text, path));
        }

        let value = if text.trim().is_empty() {
            Value::Null
        } else {
            serde_json::from_str(&text).map_err(|e| {
                OSAgentError::ToolExecution(format!(
                    "GitHub returned unexpected JSON for {}: {}",
                    path, e
                ))
            })?
        };
        Ok((status, headers, value))
    }

    async fn get_json<T: DeserializeOwned>(&self, path: &str) -> Result<T> {
        let (_, _, value) = self.request_json(Method::GET, path, None, None).await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub payload for {}: {}", path, e))
        })
    }

    /// Fetch pages of a JSON array endpoint. Returns at most `max_pages`
    /// pages of `per_page` items.
    async fn get_paged<T: DeserializeOwned>(
        &self,
        path: &str,
        query: &str,
        per_page: u32,
        max_pages: u32,
    ) -> Result<Vec<T>> {
        let mut all = Vec::new();
        for page in 1..=max_pages.max(1) {
            let separator = if path.contains('?') { '&' } else { '?' };
            let full = format!(
                "{}{}per_page={}&page={}{}",
                path,
                separator,
                per_page.clamp(1, 100),
                page,
                if query.is_empty() {
                    String::new()
                } else {
                    format!("&{}", query)
                }
            );
            let (_, _, value) = self.request_json(Method::GET, &full, None, None).await?;
            let page_items: Vec<T> = serde_json::from_value(value).map_err(|e| {
                OSAgentError::ToolExecution(format!(
                    "Unexpected GitHub payload for {}: {}",
                    full, e
                ))
            })?;
            let count = page_items.len();
            all.extend(page_items);
            if count < per_page as usize {
                break;
            }
        }
        Ok(all)
    }

    /// Raw text fetch (pull request diffs).
    pub async fn get_text(&self, path: &str, accept: &str) -> Result<String> {
        let mut req = self
            .http
            .get(self.url(path))
            .bearer_auth(&self.token)
            .header("Accept", accept)
            .header("X-GitHub-Api-Version", "2022-11-28");
        if accept == DIFF_ACCEPT {
            req = req.header("User-Agent", USER_AGENT);
        }
        let response = req
            .send()
            .await
            .map_err(|e| OSAgentError::ToolExecution(format!("GitHub request failed: {}", e)))?;
        let status = response.status();
        let headers = response.headers().clone();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            return Err(map_error_response(status, &headers, &text, path));
        }
        Ok(text)
    }

    // ── Identity / repos ────────────────────────────────────────────────

    /// Authenticated user plus the token's OAuth scopes (if reported).
    pub async fn whoami(&self) -> Result<(GhUser, Vec<String>)> {
        let (_, headers, value) = self.request_json(Method::GET, "/user", None, None).await?;
        let user: GhUser = serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub user payload: {}", e))
        })?;
        let scopes = headers
            .get("x-oauth-scopes")
            .and_then(|v| v.to_str().ok())
            .map(|v| {
                v.split(',')
                    .map(|s| s.trim().to_string())
                    .filter(|s| !s.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        Ok((user, scopes))
    }

    pub async fn get_repo(&self, repo: &str) -> Result<GhRepo> {
        self.get_json(&format!("/repos/{}", repo)).await
    }

    pub async fn list_user_repos(&self, max_pages: u32) -> Result<Vec<GhRepo>> {
        self.get_paged(
            "/user/repos",
            "affiliation=owner,collaborator&sort=pushed",
            100,
            max_pages,
        )
        .await
    }

    // ── Pull requests ───────────────────────────────────────────────────

    pub async fn get_pull(&self, repo: &str, number: u64) -> Result<GhPullRequest> {
        self.get_json(&format!("/repos/{}/pulls/{}", repo, number))
            .await
    }

    pub async fn list_pulls(
        &self,
        repo: &str,
        state: &str,
        max_pages: u32,
    ) -> Result<Vec<GhPullRequest>> {
        self.get_paged(
            &format!("/repos/{}/pulls", repo),
            &format!("state={}&sort=updated&direction=desc", state),
            100,
            max_pages,
        )
        .await
    }

    pub async fn list_pull_files(
        &self,
        repo: &str,
        number: u64,
        max_pages: u32,
    ) -> Result<Vec<GhPrFile>> {
        self.get_paged(
            &format!("/repos/{}/pulls/{}/files", repo, number),
            "",
            100,
            max_pages,
        )
        .await
    }

    pub async fn list_pull_commits(
        &self,
        repo: &str,
        number: u64,
        max: usize,
    ) -> Result<Vec<GhCommitListItem>> {
        let mut commits = self
            .get_paged(
                &format!("/repos/{}/pulls/{}/commits", repo, number),
                "",
                100,
                1,
            )
            .await?;
        commits.truncate(max);
        Ok(commits)
    }

    /// Raw unified diff for a pull request (may be large; caller caps it).
    pub async fn pull_diff(&self, repo: &str, number: u64) -> Result<String> {
        self.get_text(&format!("/repos/{}/pulls/{}", repo, number), DIFF_ACCEPT)
            .await
    }

    pub async fn create_pull(
        &self,
        repo: &str,
        title: &str,
        head: &str,
        base: &str,
        body: Option<&str>,
        draft: bool,
    ) -> Result<GhPullRequest> {
        let mut payload = json!({
            "title": title,
            "head": head,
            "base": base,
        });
        if let Some(body) = body {
            payload["body"] = json!(body);
        }
        if draft {
            payload["draft"] = json!(true);
        }
        let (_, _, value) = self
            .request_json(
                Method::POST,
                &format!("/repos/{}/pulls", repo),
                Some(payload),
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub pull payload: {}", e))
        })
    }

    /// Merge a pull request. `method` is merge/squash/rebase.
    pub async fn merge_pull(&self, repo: &str, number: u64, method: &str) -> Result<Value> {
        let payload = json!({ "merge_method": method });
        let (_, _, value) = self
            .request_json(
                Method::PUT,
                &format!("/repos/{}/pulls/{}/merge", repo, number),
                Some(payload),
                None,
            )
            .await?;
        Ok(value)
    }

    /// Close a pull request (the API models closing as a state update).
    pub async fn close_pull(&self, repo: &str, number: u64) -> Result<GhPullRequest> {
        let (_, _, value) = self
            .request_json(
                Method::PATCH,
                &format!("/repos/{}/pulls/{}", repo, number),
                Some(json!({ "state": "closed" })),
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub pull payload: {}", e))
        })
    }

    // ── Reviews / comments ──────────────────────────────────────────────

    /// Post a review with optional inline comments.
    pub async fn create_review(
        &self,
        repo: &str,
        number: u64,
        commit_id: &str,
        body: &str,
        event: &str,
        comments: &[GhReviewCommentInput],
    ) -> Result<GhReview> {
        let mut payload = json!({
            "commit_id": commit_id,
            "body": body,
            "event": event,
        });
        if !comments.is_empty() {
            payload["comments"] = serde_json::to_value(comments).unwrap_or_default();
        }
        let (_, _, value) = self
            .request_json(
                Method::POST,
                &format!("/repos/{}/pulls/{}/reviews", repo, number),
                Some(payload),
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub review payload: {}", e))
        })
    }

    pub async fn create_issue_comment(
        &self,
        repo: &str,
        number: u64,
        body: &str,
    ) -> Result<GhComment> {
        let (_, _, value) = self
            .request_json(
                Method::POST,
                &format!("/repos/{}/issues/{}/comments", repo, number),
                Some(json!({ "body": body })),
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub comment payload: {}", e))
        })
    }

    /// Add a reaction to a conversation comment (👀 while a mention runs).
    pub async fn add_issue_comment_reaction(
        &self,
        repo: &str,
        comment_id: u64,
        content: &str,
    ) -> Result<()> {
        self.request_json(
            Method::POST,
            &format!("/repos/{}/issues/comments/{}/reactions", repo, comment_id),
            Some(json!({ "content": content })),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Add a reaction to an inline review comment.
    pub async fn add_review_comment_reaction(
        &self,
        repo: &str,
        comment_id: u64,
        content: &str,
    ) -> Result<()> {
        self.request_json(
            Method::POST,
            &format!("/repos/{}/pulls/comments/{}/reactions", repo, comment_id),
            Some(json!({ "content": content })),
            None,
        )
        .await
        .map(|_| ())
    }

    /// Reply inside an inline review-comment thread.
    pub async fn reply_to_review_comment(
        &self,
        repo: &str,
        pr_number: u64,
        comment_id: u64,
        body: &str,
    ) -> Result<()> {
        self.request_json(
            Method::POST,
            &format!(
                "/repos/{}/pulls/{}/comments/{}/replies",
                repo, pr_number, comment_id
            ),
            Some(json!({ "body": body })),
            None,
        )
        .await
        .map(|_| ())
    }

    pub async fn list_reviews(&self, repo: &str, number: u64) -> Result<Vec<GhReview>> {
        self.get_json(&format!(
            "/repos/{}/pulls/{}/reviews?per_page=50",
            repo, number
        ))
        .await
    }

    /// Repository-wide conversation comments, newest first. One request
    /// covers every open PR, which is what the mention trigger needs.
    pub async fn list_repo_issue_comments(
        &self,
        repo: &str,
        max_pages: u32,
    ) -> Result<Vec<GhIssueComment>> {
        self.get_paged(
            &format!("/repos/{}/issues/comments", repo),
            "sort=created&direction=desc",
            100,
            max_pages,
        )
        .await
    }

    /// Repository-wide inline review comments, newest first. The mention
    /// trigger also honors tags written inside code review threads.
    pub async fn list_repo_review_comments(
        &self,
        repo: &str,
        max_pages: u32,
    ) -> Result<Vec<GhIssueComment>> {
        self.get_paged(
            &format!("/repos/{}/pulls/comments", repo),
            "sort=created&direction=desc",
            100,
            max_pages,
        )
        .await
    }

    // ── Issues ──────────────────────────────────────────────────────────

    pub async fn list_issues(
        &self,
        repo: &str,
        state: &str,
        max_pages: u32,
    ) -> Result<Vec<GhIssue>> {
        self.get_paged(
            &format!("/repos/{}/issues", repo),
            &format!("state={}&sort=updated&direction=desc", state),
            50,
            max_pages,
        )
        .await
    }

    pub async fn get_issue(&self, repo: &str, number: u64) -> Result<GhIssue> {
        self.get_json(&format!("/repos/{}/issues/{}", repo, number))
            .await
    }

    pub async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: Option<&str>,
        labels: &[String],
    ) -> Result<GhIssue> {
        let mut payload = json!({ "title": title });
        if let Some(body) = body {
            payload["body"] = json!(body);
        }
        if !labels.is_empty() {
            payload["labels"] = serde_json::to_value(labels).unwrap_or_default();
        }
        let (_, _, value) = self
            .request_json(
                Method::POST,
                &format!("/repos/{}/issues", repo),
                Some(payload),
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub issue payload: {}", e))
        })
    }

    /// Close an issue (the API models closing as a state update).
    pub async fn close_issue(&self, repo: &str, number: u64) -> Result<GhIssue> {
        let (_, _, value) = self
            .request_json(
                Method::PATCH,
                &format!("/repos/{}/issues/{}", repo, number),
                Some(json!({ "state": "closed" })),
                None,
            )
            .await?;
        serde_json::from_value(value).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub issue payload: {}", e))
        })
    }

    // ── Branches / commits / checks ─────────────────────────────────────

    pub async fn list_branches(&self, repo: &str, max_pages: u32) -> Result<Vec<GhBranch>> {
        self.get_paged(&format!("/repos/{}/branches", repo), "", 100, max_pages)
            .await
    }

    pub async fn list_commits(
        &self,
        repo: &str,
        branch: Option<&str>,
        max_pages: u32,
    ) -> Result<Vec<GhCommitListItem>> {
        let query = branch
            .map(|b| format!("sha={}", urlencoding::encode(b)))
            .unwrap_or_default();
        self.get_paged(&format!("/repos/{}/commits", repo), &query, 50, max_pages)
            .await
    }

    pub async fn list_check_runs(&self, repo: &str, git_ref: &str) -> Result<Vec<GhCheckRun>> {
        #[derive(serde::Deserialize)]
        struct CheckRunsEnvelope {
            check_runs: Vec<GhCheckRun>,
        }
        let envelope: CheckRunsEnvelope = self
            .get_json(&format!(
                "/repos/{}/commits/{}/check-runs?per_page=50",
                repo,
                urlencoding::encode(git_ref)
            ))
            .await?;
        Ok(envelope.check_runs)
    }

    pub async fn list_commit_statuses(&self, repo: &str, sha: &str) -> Result<Vec<GhCommitStatus>> {
        self.get_json(&format!(
            "/repos/{}/commits/{}/statuses?per_page=50",
            repo, sha
        ))
        .await
    }

    /// Post a commit status (requires push access).
    pub async fn create_commit_status(
        &self,
        repo: &str,
        sha: &str,
        state: &str,
        description: Option<&str>,
        context: Option<&str>,
    ) -> Result<Value> {
        let mut payload = json!({ "state": state });
        if let Some(description) = description {
            payload["description"] = json!(description);
        }
        if let Some(context) = context {
            payload["context"] = json!(context);
        }
        let (_, _, value) = self
            .request_json(
                Method::POST,
                &format!("/repos/{}/statuses/{}", repo, sha),
                Some(payload),
                None,
            )
            .await?;
        Ok(value)
    }

    // ── Actions / releases / search ─────────────────────────────────────

    pub async fn list_workflow_runs(
        &self,
        repo: &str,
        branch: Option<&str>,
        max_pages: u32,
    ) -> Result<Vec<GhWorkflowRun>> {
        #[derive(serde::Deserialize)]
        struct RunsEnvelope {
            workflow_runs: Vec<GhWorkflowRun>,
        }
        let mut query = String::new();
        if let Some(branch) = branch {
            query = format!("branch={}", urlencoding::encode(branch));
        }
        let runs: Vec<RunsEnvelope> = self
            .get_paged(
                &format!("/repos/{}/actions/runs", repo),
                &query,
                30,
                max_pages,
            )
            .await?;
        Ok(runs.into_iter().flat_map(|r| r.workflow_runs).collect())
    }

    pub async fn list_run_jobs(&self, repo: &str, run_id: u64) -> Result<Vec<GhWorkflowJob>> {
        #[derive(serde::Deserialize)]
        struct JobsEnvelope {
            jobs: Vec<GhWorkflowJob>,
        }
        let envelope: JobsEnvelope = self
            .get_json(&format!(
                "/repos/{}/actions/runs/{}/jobs?per_page=50",
                repo, run_id
            ))
            .await?;
        Ok(envelope.jobs)
    }

    pub async fn list_releases(&self, repo: &str, max_pages: u32) -> Result<Vec<GhRelease>> {
        self.get_paged(&format!("/repos/{}/releases", repo), "", 30, max_pages)
            .await
    }

    pub async fn search_code(&self, query: &str, limit: u32) -> Result<Vec<GhSearchCodeItem>> {
        #[derive(serde::Deserialize)]
        struct SearchEnvelope<T> {
            items: Vec<T>,
        }
        let envelope: SearchEnvelope<GhSearchCodeItem> = self
            .get_json(&format!(
                "/search/code?q={}&per_page={}",
                urlencoding::encode(query),
                limit.clamp(1, 50)
            ))
            .await?;
        Ok(envelope.items)
    }

    pub async fn search_issues(&self, query: &str, limit: u32) -> Result<Vec<GhSearchIssueItem>> {
        #[derive(serde::Deserialize)]
        struct SearchEnvelope<T> {
            items: Vec<T>,
        }
        let envelope: SearchEnvelope<GhSearchIssueItem> = self
            .get_json(&format!(
                "/search/issues?q={}&per_page={}",
                urlencoding::encode(query),
                limit.clamp(1, 50)
            ))
            .await?;
        Ok(envelope.items)
    }

    // ── Device flow (OAuth app sign-in) ─────────────────────────────────

    /// Start an OAuth device flow. Uses GitHub's public device endpoints so
    /// this works without an API-version header.
    pub async fn start_device_flow(
        &self,
        client_id: &str,
        scope: &str,
    ) -> Result<GhDeviceCodeResponse> {
        let request = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        let response = request
            .post("https://github.com/login/device/code")
            .header("Accept", "application/json")
            .form(&[("client_id", client_id), ("scope", scope)])
            .send()
            .await
            .map_err(|e| {
                OSAgentError::ToolExecution(format!("GitHub device flow start failed: {}", e))
            })?;
        let status = response.status();
        let text = response.text().await.unwrap_or_default();
        if !status.is_success() {
            let hint = if matches!(
                status,
                StatusCode::NOT_FOUND | StatusCode::UNAUTHORIZED | StatusCode::UNPROCESSABLE_ENTITY
            ) {
                " This usually means the OAuth app client id is wrong, or the app has not had Device Flow enabled. Check the client id in Settings > GitHub and the app's settings on github.com."
            } else {
                ""
            };
            return Err(OSAgentError::ToolExecution(format!(
                "GitHub device flow start failed with HTTP {}: {}{}",
                status,
                truncate(&text, 300),
                hint
            )));
        }
        serde_json::from_str(&text).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub device flow payload: {}", e))
        })
    }

    /// Poll once for a device-flow token. `Ok(None)` means authorization is
    /// still pending.
    pub async fn poll_device_flow(
        &self,
        client_id: &str,
        device_code: &str,
    ) -> Result<Option<GhDeviceTokenResponse>> {
        let request = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        let response = request
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client_id),
                ("device_code", device_code),
                ("grant_type", "urn:ietf:params:oauth:grant-type:device_code"),
            ])
            .send()
            .await
            .map_err(|e| {
                OSAgentError::ToolExecution(format!("GitHub device flow poll failed: {}", e))
            })?;
        let text = response.text().await.unwrap_or_default();
        let parsed: GhDeviceTokenResponse = serde_json::from_str(&text).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub device flow payload: {}", e))
        })?;
        match parsed.error.as_deref() {
            None | Some("") => Ok(Some(parsed)),
            Some("authorization_pending") => Ok(None),
            Some("slow_down") => Ok(None),
            Some(other) => Err(OSAgentError::ToolExecution(format!(
                "GitHub device flow failed: {}",
                parsed.error_description.as_deref().unwrap_or(other)
            ))),
        }
    }

    /// Refresh an expiring sign-in token. Device-flow tokens refresh with
    /// the client id alone — no client secret.
    pub async fn refresh_device_token(
        &self,
        client_id: &str,
        refresh_token: &str,
    ) -> Result<GhDeviceTokenResponse> {
        let request = Client::builder()
            .user_agent(USER_AGENT)
            .timeout(Duration::from_secs(30))
            .build()
            .unwrap_or_else(|_| Client::new());
        let response = request
            .post("https://github.com/login/oauth/access_token")
            .header("Accept", "application/json")
            .form(&[
                ("client_id", client_id),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ])
            .send()
            .await
            .map_err(|e| {
                OSAgentError::ToolExecution(format!("GitHub token refresh failed: {}", e))
            })?;
        let text = response.text().await.unwrap_or_default();
        let parsed: GhDeviceTokenResponse = serde_json::from_str(&text).map_err(|e| {
            OSAgentError::ToolExecution(format!("Unexpected GitHub token refresh payload: {}", e))
        })?;
        match parsed.error.as_deref() {
            None | Some("") => Ok(parsed),
            Some(other) => Err(OSAgentError::ToolExecution(format!(
                "GitHub token refresh failed ({}): {}. Sign in with GitHub again in Settings > GitHub.",
                other,
                parsed.error_description.as_deref().unwrap_or("")
            ))),
        }
    }
}

/// Map a non-success GitHub response to an actionable error.
fn map_error_response(
    status: StatusCode,
    headers: &HeaderMap,
    body: &str,
    path: &str,
) -> OSAgentError {
    let message = serde_json::from_str::<GhApiError>(body)
        .ok()
        .and_then(|e| e.message)
        .unwrap_or_else(|| truncate(body.trim(), 300));

    if status == StatusCode::UNAUTHORIZED {
        return OSAgentError::ToolExecution(format!(
            "GitHub authentication failed (401). The token is missing or invalid — update it in Settings > GitHub. [{}]",
            path
        ));
    }
    if status == StatusCode::FORBIDDEN {
        let remaining = headers
            .get("x-ratelimit-remaining")
            .and_then(|v| v.to_str().ok())
            .unwrap_or("");
        if remaining == "0" {
            let reset = headers
                .get("x-ratelimit-reset")
                .and_then(|v| v.to_str().ok())
                .and_then(|v| v.parse::<i64>().ok())
                .map(|ts| {
                    chrono::DateTime::from_timestamp(ts, 0)
                        .map(|dt| dt.format("%H:%M UTC").to_string())
                        .unwrap_or_else(|| "soon".to_string())
                })
                .unwrap_or_else(|| "soon".to_string());
            return OSAgentError::ToolExecution(format!(
                "GitHub API rate limit reached (resets at {}). [{}]",
                reset, path
            ));
        }
        return OSAgentError::ToolExecution(format!(
            "GitHub refused the request (403, likely missing token scope): {} [{}]",
            message, path
        ));
    }
    if status == StatusCode::NOT_FOUND {
        return OSAgentError::ToolExecution(format!(
            "GitHub returned 404 for {} — the repository, pull request, or resource was not found (or the token cannot see it).",
            path
        ));
    }
    if status == StatusCode::UNPROCESSABLE_ENTITY {
        return OSAgentError::ToolExecution(format!(
            "GitHub rejected the request (422): {} [{}]",
            message, path
        ));
    }
    OSAgentError::ToolExecution(format!(
        "GitHub error (HTTP {}) for {}: {}",
        status, path, message
    ))
}

fn truncate(text: &str, max: usize) -> String {
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

    #[test]
    fn token_resolution_prefers_config() {
        let mut config = crate::config::Config::default_config();
        assert!(resolve_token(&config).is_none() || std::env::var("GITHUB_TOKEN").is_ok());
        config.github.token = "  cfg-token  ".to_string();
        let (token, source) = resolve_token(&config).expect("config token resolves");
        assert_eq!(token, "cfg-token");
        assert_eq!(source, TokenSource::Config);
    }

    #[test]
    fn error_mapping_mentions_auth() {
        let err = map_error_response(
            StatusCode::UNAUTHORIZED,
            &HeaderMap::new(),
            "{\"message\":\"Bad credentials\"}",
            "/user",
        );
        let text = err.to_string();
        assert!(text.contains("401"));
        assert!(text.contains("Settings > GitHub"));
    }

    #[test]
    fn error_mapping_detects_rate_limit() {
        let mut headers = HeaderMap::new();
        headers.insert("x-ratelimit-remaining", "0".parse().expect("header"));
        let err = map_error_response(
            StatusCode::FORBIDDEN,
            &headers,
            "{\"message\":\"API rate limit exceeded\"}",
            "/repos/a/b/pulls",
        );
        assert!(err.to_string().contains("rate limit"));
    }

    #[test]
    fn parses_device_code_response_with_prefilled_uri() {
        // Shape documented by GitHub, including the complete URI that
        // pre-fills the code so the user only clicks Authorize.
        let raw = r#"{
            "device_code": "3584d835",
            "user_code": "WDJB-MJHT",
            "verification_uri": "https://github.com/login/device",
            "verification_uri_complete": "https://github.com/login/device?user_code=WDJB-MJHT",
            "expires_in": 900,
            "interval": 5
        }"#;
        let parsed: GhDeviceCodeResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(parsed.user_code, "WDJB-MJHT");
        assert_eq!(
            parsed.verification_uri_complete.as_deref(),
            Some("https://github.com/login/device?user_code=WDJB-MJHT")
        );
        // Older responses without the complete URI still parse.
        let bare = r#"{"device_code":"d","user_code":"AB-CD","verification_uri":"https://github.com/login/device"}"#;
        let parsed: GhDeviceCodeResponse = serde_json::from_str(bare).expect("parses bare");
        assert!(parsed.verification_uri_complete.is_none());
    }

    /// Reactions and review-comment replies hit the right endpoints with
    /// the right bodies.
    #[tokio::test]
    async fn posts_reactions_and_review_replies() {
        use axum::extract::Json as AxumJson;
        use axum::routing::post;
        use axum::{Json, Router};
        use serde_json::Value;
        use std::sync::{Arc, Mutex};

        let captured: Arc<Mutex<Vec<(String, Value)>>> = Arc::new(Mutex::new(Vec::new()));
        let record = |path: &'static str, captured: Arc<Mutex<Vec<(String, Value)>>>| {
            move |AxumJson(body): AxumJson<Value>| {
                let captured = captured.clone();
                async move {
                    captured.lock().unwrap().push((path.to_string(), body));
                    Json(serde_json::json!({"id": 1}))
                }
            }
        };

        let app = Router::new()
            .route(
                "/repos/o/r/issues/comments/11/reactions",
                post(record("/issues/comments/11/reactions", captured.clone())),
            )
            .route(
                "/repos/o/r/pulls/comments/12/reactions",
                post(record("/pulls/comments/12/reactions", captured.clone())),
            )
            .route(
                "/repos/o/r/pulls/7/comments/12/replies",
                post(record("/pulls/7/comments/12/replies", captured.clone())),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = GithubClient::with_base("tok", format!("http://{}", addr));
        client
            .add_issue_comment_reaction("o/r", 11, "eyes")
            .await
            .expect("issue reaction");
        client
            .add_review_comment_reaction("o/r", 12, "eyes")
            .await
            .expect("review reaction");
        client
            .reply_to_review_comment("o/r", 7, 12, "Done, review posted.")
            .await
            .expect("review reply");

        let calls = captured.lock().unwrap().clone();
        assert_eq!(calls.len(), 3);
        assert_eq!(calls[0].0, "/issues/comments/11/reactions");
        assert_eq!(calls[0].1["content"], "eyes");
        assert_eq!(calls[1].0, "/pulls/comments/12/reactions");
        assert_eq!(calls[1].1["content"], "eyes");
        assert_eq!(calls[2].0, "/pulls/7/comments/12/replies");
        assert_eq!(calls[2].1["body"], "Done, review posted.");
    }

    #[test]
    fn parses_expiring_token_and_refresh_response() {
        // App with "Expire user access tokens" enabled.
        let raw = r#"{
            "access_token": "gho_at",
            "token_type": "bearer",
            "scope": "repo,read:user",
            "refresh_token": "ghr_rt",
            "expires_in": 28800
        }"#;
        let parsed: GhDeviceTokenResponse = serde_json::from_str(raw).expect("parses");
        assert_eq!(parsed.access_token.as_deref(), Some("gho_at"));
        assert_eq!(parsed.refresh_token.as_deref(), Some("ghr_rt"));
        assert_eq!(parsed.expires_in, Some(28800));

        // Refresh response rotates both tokens.
        let refreshed =
            r#"{"access_token":"gho_at2","refresh_token":"ghr_rt2","expires_in":28800}"#;
        let parsed: GhDeviceTokenResponse = serde_json::from_str(refreshed).expect("parses");
        assert_eq!(parsed.access_token.as_deref(), Some("gho_at2"));
        assert_eq!(parsed.refresh_token.as_deref(), Some("ghr_rt2"));

        // Non-expiring app: no refresh fields.
        let plain = r#"{"access_token":"gho_plain","token_type":"bearer","scope":"repo"}"#;
        let parsed: GhDeviceTokenResponse = serde_json::from_str(plain).expect("parses");
        assert!(parsed.refresh_token.is_none());
        assert!(parsed.expires_in.is_none());
    }

    /// End-to-end client check against a local mock of the GitHub API:
    /// identity, PR fetch, file diff parsing, and review posting with an
    /// inline comment. Uses `with_base` so no real network is involved.
    #[tokio::test]
    async fn talks_to_a_mock_github_server() {
        use axum::extract::Json as AxumJson;
        use axum::routing::{get, post};
        use axum::{Json, Router};
        use serde_json::Value;
        use std::sync::{Arc, Mutex};

        let captured_review: Arc<Mutex<Option<Value>>> = Arc::new(Mutex::new(None));
        let captured = captured_review.clone();

        let app = Router::new()
            .route(
                "/user",
                get(|| async {
                    Json(json!({
                        "login": "octo",
                        "name": "Octo Cat",
                        "html_url": "https://github.com/octo"
                    }))
                }),
            )
            .route(
                "/repos/o/r/pulls/1",
                get(|| async {
                    Json(json!({
                        "number": 1,
                        "title": "Add widget",
                        "body": "Adds a widget.",
                        "state": "open",
                        "user": { "login": "octo" },
                        "head": { "sha": "headsha", "ref": "feature" },
                        "base": { "sha": "basesha", "ref": "main" },
                        "additions": 2,
                        "deletions": 1,
                        "changed_files": 1,
                        "commits": 1,
                        "html_url": "https://github.com/o/r/pull/1"
                    }))
                }),
            )
            .route(
                "/repos/o/r/pulls/1/files",
                get(|| async {
                    Json(json!([{
                        "filename": "src/widget.rs",
                        "status": "modified",
                        "additions": 2,
                        "deletions": 1,
                        "changes": 3,
                        "patch": "@@ -1,2 +1,3 @@\n fn x() {}\n-old\n+new\n+extra\n"
                    }]))
                }),
            )
            .route(
                "/repos/o/r/pulls/1/reviews",
                post(move |AxumJson(body): AxumJson<Value>| {
                    let captured = captured.clone();
                    async move {
                        *captured.lock().unwrap() = Some(body);
                        Json(json!({
                            "id": 9,
                            "state": "COMMENTED",
                            "html_url": "https://github.com/o/r/pull/1#pullrequestreview-9"
                        }))
                    }
                }),
            );

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move {
            let _ = axum::serve(listener, app).await;
        });

        let client = GithubClient::with_base("tok", format!("http://{}", addr));

        let (user, _) = client.whoami().await.expect("whoami");
        assert_eq!(user.login, "octo");

        let pr = client.get_pull("o/r", 1).await.expect("pull");
        assert_eq!(pr.number, 1);
        assert_eq!(pr.author(), "octo");
        assert_eq!(pr.head.sha, "headsha");

        let files = client.list_pull_files("o/r", 1, 1).await.expect("files");
        assert_eq!(files.len(), 1);
        let diff = crate::github::diff::parse_patch(
            &files[0].filename,
            files[0].patch.as_deref().unwrap(),
        );
        assert!(diff.is_valid_position(2, "RIGHT"));

        let review = client
            .create_review(
                "o/r",
                1,
                "headsha",
                "body",
                "COMMENT",
                &[GhReviewCommentInput {
                    path: "src/widget.rs".to_string(),
                    line: 2,
                    side: "RIGHT".to_string(),
                    body: "nit".to_string(),
                    start_line: None,
                    start_side: None,
                }],
            )
            .await
            .expect("review");
        assert_eq!(review.id, 9);
        let body = captured_review.lock().unwrap().clone().expect("captured");
        assert_eq!(body["commit_id"], "headsha");
        assert_eq!(body["event"], "COMMENT");
        assert_eq!(body["comments"][0]["path"], "src/widget.rs");
        assert_eq!(body["comments"][0]["line"], 2);
    }
}
