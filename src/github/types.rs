//! Serde shapes for the GitHub REST API. Only the fields OSA actually
//! consumes are modeled; everything else is ignored, so GitHub can add
//! response fields without breaking deserialization.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Deserialize)]
pub struct GhUser {
    pub login: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub avatar_url: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhOwner {
    pub login: String,
    #[serde(default)]
    pub avatar_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhRepoPermissions {
    #[serde(default)]
    pub admin: Option<bool>,
    #[serde(default)]
    pub push: Option<bool>,
    #[serde(default)]
    pub pull: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhRepo {
    pub full_name: String,
    #[serde(default)]
    pub private: bool,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub default_branch: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub pushed_at: Option<String>,
    #[serde(default)]
    pub open_issues_count: Option<u64>,
    #[serde(default)]
    pub permissions: Option<GhRepoPermissions>,
    #[serde(default)]
    pub owner: Option<GhOwner>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhLabel {
    pub name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhRef {
    pub sha: String,
    #[serde(rename = "ref")]
    pub ref_name: String,
    #[serde(default)]
    pub label: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhPullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub draft: Option<bool>,
    #[serde(default)]
    pub merged: Option<bool>,
    #[serde(default)]
    pub mergeable: Option<bool>,
    #[serde(default)]
    pub user: Option<GhOwner>,
    pub head: GhRef,
    pub base: GhRef,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub additions: Option<u64>,
    #[serde(default)]
    pub deletions: Option<u64>,
    #[serde(default)]
    pub changed_files: Option<u64>,
    #[serde(default)]
    pub commits: Option<u64>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<GhLabel>>,
}

impl GhPullRequest {
    pub fn author(&self) -> &str {
        self.user
            .as_ref()
            .map(|u| u.login.as_str())
            .unwrap_or("unknown")
    }

    pub fn is_draft(&self) -> bool {
        self.draft.unwrap_or(false)
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhPrFile {
    pub filename: String,
    pub status: String,
    #[serde(default)]
    pub additions: u64,
    #[serde(default)]
    pub deletions: u64,
    #[serde(default)]
    pub changes: u64,
    #[serde(default)]
    pub patch: Option<String>,
    #[serde(default)]
    pub previous_filename: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhIssue {
    pub number: u64,
    pub title: String,
    #[serde(default)]
    pub body: Option<String>,
    pub state: String,
    #[serde(default)]
    pub user: Option<GhOwner>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub labels: Option<Vec<GhLabel>>,
    #[serde(default)]
    pub comments: Option<u64>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub updated_at: Option<String>,
    /// Present when the entry is actually a pull request: the issues
    /// endpoint returns both, and the tool filters PRs out of issue lists.
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhComment {
    pub id: u64,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub user: Option<GhOwner>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub path: Option<String>,
    #[serde(default)]
    pub line: Option<u64>,
}

/// A conversation comment (issues/comments), used by the mention trigger.
#[derive(Debug, Clone, Deserialize)]
pub struct GhIssueComment {
    pub id: u64,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub user: Option<GhOwner>,
    #[serde(default)]
    pub created_at: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    /// "https://api.github.com/repos/{owner}/{repo}/issues/{number}"
    #[serde(default)]
    pub issue_url: Option<String>,
    /// Set instead of `issue_url` when the comment is an inline review
    /// comment: "https://api.github.com/repos/{owner}/{repo}/pulls/{number}"
    #[serde(default)]
    pub pull_request_url: Option<String>,
}

impl GhIssueComment {
    pub fn author(&self) -> &str {
        self.user
            .as_ref()
            .map(|u| u.login.as_str())
            .unwrap_or("unknown")
    }

    /// Inline review comments carry `pull_request_url`; conversation
    /// comments carry `issue_url`.
    pub fn is_review_comment(&self) -> bool {
        self.issue_url.is_none() && self.pull_request_url.is_some()
    }

    /// Issue/PR number parsed from `issue_url` (conversation comments) or
    /// `pull_request_url` (inline review comments).
    pub fn issue_number(&self) -> Option<u64> {
        let url = self
            .issue_url
            .as_deref()
            .or(self.pull_request_url.as_deref())?;
        url.rsplit('/').next()?.parse().ok()
    }

    pub fn created_at_utc(&self) -> Option<chrono::DateTime<chrono::Utc>> {
        self.created_at
            .as_deref()
            .and_then(|value| chrono::DateTime::parse_from_rfc3339(value).ok())
            .map(|dt| dt.with_timezone(&chrono::Utc))
    }
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhReview {
    pub id: u64,
    pub state: String,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub user: Option<GhOwner>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub submitted_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhBranchCommit {
    pub sha: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhBranch {
    pub name: String,
    pub commit: GhBranchCommit,
    #[serde(default)]
    pub protected: bool,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhCommitInner {
    pub message: String,
    #[serde(default)]
    pub author: Option<GhCommitAuthor>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhCommitAuthor {
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub date: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhCommitListItem {
    pub sha: String,
    pub commit: GhCommitInner,
    #[serde(default)]
    pub author: Option<GhOwner>,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhWorkflowRun {
    pub id: u64,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub display_title: Option<String>,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub head_branch: Option<String>,
    #[serde(default)]
    pub head_sha: Option<String>,
    #[serde(default)]
    pub event: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhWorkflowJobStep {
    pub name: String,
    #[serde(default)]
    pub status: Option<String>,
    #[serde(default)]
    pub conclusion: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhWorkflowJob {
    pub id: u64,
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub steps: Option<Vec<GhWorkflowJobStep>>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhRelease {
    pub tag_name: String,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub body: Option<String>,
    #[serde(default)]
    pub draft: bool,
    #[serde(default)]
    pub prerelease: bool,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub published_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhCheckRun {
    pub name: String,
    pub status: String,
    #[serde(default)]
    pub conclusion: Option<String>,
    #[serde(default)]
    pub html_url: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhCommitStatus {
    pub state: String,
    #[serde(default)]
    pub context: Option<String>,
    #[serde(default)]
    pub description: Option<String>,
    #[serde(default)]
    pub target_url: Option<String>,
    #[serde(default)]
    pub created_at: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhSearchCodeItem {
    pub name: String,
    pub path: String,
    #[serde(default)]
    pub html_url: Option<String>,
    pub repository: GhSearchRepo,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhSearchRepo {
    pub full_name: String,
}

#[derive(Debug, Clone, Deserialize)]
pub struct GhSearchIssueItem {
    pub number: u64,
    pub title: String,
    pub state: String,
    #[serde(default)]
    pub html_url: Option<String>,
    #[serde(default)]
    pub user: Option<GhOwner>,
    #[serde(default)]
    pub repository_url: Option<String>,
    #[serde(default)]
    pub pull_request: Option<serde_json::Value>,
}

/// One inline comment as sent to the review API.
#[derive(Debug, Clone, Serialize)]
pub struct GhReviewCommentInput {
    pub path: String,
    pub line: u64,
    pub side: String,
    pub body: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_line: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub start_side: Option<String>,
}

/// Device-flow session returned by POST /login/device/code.
#[derive(Debug, Clone, Deserialize)]
pub struct GhDeviceCodeResponse {
    pub device_code: String,
    pub user_code: String,
    pub verification_uri: String,
    /// Same page with the code pre-filled (`…?user_code=ABCD-EFGH`), so
    /// the user only has to click Authorize. GitHub returns it on the
    /// device-flow response; older/other servers may omit it.
    #[serde(default)]
    pub verification_uri_complete: Option<String>,
    #[serde(default)]
    pub expires_in: u64,
    #[serde(default)]
    pub interval: u64,
}

/// Poll response from POST /login/oauth/access_token during device flow.
#[derive(Debug, Clone, Deserialize)]
pub struct GhDeviceTokenResponse {
    #[serde(default)]
    pub access_token: Option<String>,
    #[serde(default)]
    pub token_type: Option<String>,
    #[serde(default)]
    pub scope: Option<String>,
    /// Present when the OAuth app expires user access tokens. Refreshable
    /// with the client id alone (no secret for device-flow tokens).
    #[serde(default)]
    pub refresh_token: Option<String>,
    /// Access-token lifetime in seconds, when expiry is enabled.
    #[serde(default)]
    pub expires_in: Option<u64>,
    #[serde(default)]
    pub error: Option<String>,
    #[serde(default)]
    pub error_description: Option<String>,
}

/// GitHub API error envelope.
#[derive(Debug, Clone, Deserialize)]
pub struct GhApiError {
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub documentation_url: Option<String>,
}
