//! The `github` tool: authenticated GitHub operations plus the
//! CodeRabbit-style `review_pr` action. Registered as a deferred
//! built-in (activated via `tool_search`) because the schema is large.
//!
//! All operations run against the shared `GithubEngine`, which resolves
//! the token from live config and keeps secrets away from the model.

use async_trait::async_trait;
use serde_json::{json, Value};
use std::sync::Arc;

use crate::error::{OSAgentError, Result};
use crate::github::diff::parse_patch;
use crate::github::engine::{normalize_repo, GithubEngine, ReviewOptions};
use crate::github::review::{match_file_path, short_sha};
use crate::github::types::GhReviewCommentInput;
use crate::tools::registry::{Tool, ToolExample};

const DEFAULT_LIMIT: usize = 10;
const MAX_LIMIT: usize = 50;
const MAX_DIFF_CHARS: usize = 80_000;
/// Review generation includes model calls; give it room beyond the
/// default tool budget.
const TOOL_TIMEOUT_MS: u64 = 20 * 60 * 1000;

pub struct GithubTool {
    engine: Arc<GithubEngine>,
}

impl GithubTool {
    pub fn new(engine: Arc<GithubEngine>) -> Self {
        Self { engine }
    }

    fn require_str<'a>(args: &'a Value, key: &str) -> Result<&'a str> {
        args[key]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                OSAgentError::ToolExecution(format!("'{}' is required for this action", key))
            })
    }

    fn require_u64(args: &Value, key: &str) -> Result<u64> {
        args[key].as_u64().ok_or_else(|| {
            OSAgentError::ToolExecution(format!("'{}' is required for this action", key))
        })
    }

    fn limit(args: &Value) -> usize {
        args["limit"]
            .as_u64()
            .map(|l| (l as usize).clamp(1, MAX_LIMIT))
            .unwrap_or(DEFAULT_LIMIT)
    }

    /// `repo` argument, falling back to a single watched repo.
    async fn resolve_repo(&self, args: &Value) -> Result<String> {
        if let Some(repo) = args["repo"]
            .as_str()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            return normalize_repo(repo);
        }
        let config = self.engine.github_config().await?;
        let watched: Vec<String> = config
            .watch_repos
            .iter()
            .filter_map(|repo| normalize_repo(repo).ok())
            .collect();
        match watched.as_slice() {
            [single] => Ok(single.clone()),
            [] => Err(OSAgentError::ToolExecution(
                "'repo' is required (owner/repo). Add watched repositories in Settings > GitHub to make it optional."
                    .to_string(),
            )),
            _ => Err(OSAgentError::ToolExecution(
                "'repo' is required (owner/repo) — several repositories are watched, so there is no default."
                    .to_string(),
            )),
        }
    }

    async fn run(&self, action: &str, args: &Value) -> Result<String> {
        let resolved = self.engine.resolve().await?;
        let client = &resolved.client;

        match action {
            "user" => {
                let (user, scopes) = client.whoami().await?;
                Ok(format!(
                    "Authenticated as {} ({}){}{}",
                    user.login,
                    user.name.as_deref().unwrap_or("no display name"),
                    user.html_url
                        .map(|url| format!(" · {}", url))
                        .unwrap_or_default(),
                    if scopes.is_empty() {
                        " · token scopes: (fine-grained or unknown)".to_string()
                    } else {
                        format!(" · token scopes: {}", scopes.join(", "))
                    }
                ))
            }

            "pr_list" => {
                let repo = self.resolve_repo(args).await?;
                let state = args["state"].as_str().unwrap_or("open");
                let pulls = client.list_pulls(&repo, state, 1).await?;
                if pulls.is_empty() {
                    return Ok(format!("No {} pull requests in {}.", state, repo));
                }
                let mut out = format!("Pull requests in {} ({}):\n", repo, state);
                for pr in pulls.iter().take(Self::limit(args)) {
                    out.push_str(&format!(
                        "- #{} {} [{}] by {}{}{}\n  {}\n",
                        pr.number,
                        pr.title.trim(),
                        if pr.is_draft() {
                            "draft"
                        } else {
                            pr.state.as_str()
                        },
                        pr.author(),
                        pr.changed_files
                            .map(|f| format!(" · {} files", f))
                            .unwrap_or_default(),
                        pr.head
                            .label
                            .as_deref()
                            .map(|l| format!(" · {}", l))
                            .unwrap_or_default(),
                        pr.html_url.clone().unwrap_or_default()
                    ));
                }
                Ok(out)
            }

            "pr_view" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let pr = client.get_pull(&repo, number).await?;
                let mut out = format!(
                    "# {}#{} — {}\nState: {}{}\nAuthor: {}\nBranches: {} <- {}\nCommits: {} · Files: {} · +{}/-{}\n",
                    repo,
                    pr.number,
                    pr.title.trim(),
                    pr.state,
                    if pr.merged == Some(true) { " (merged)" } else { "" },
                    pr.author(),
                    pr.base.ref_name,
                    pr.head.ref_name,
                    pr.commits.unwrap_or(0),
                    pr.changed_files.unwrap_or(0),
                    pr.additions.unwrap_or(0),
                    pr.deletions.unwrap_or(0),
                );
                if let Some(labels) = &pr.labels {
                    if !labels.is_empty() {
                        out.push_str(&format!(
                            "Labels: {}\n",
                            labels
                                .iter()
                                .map(|l| l.name.as_str())
                                .collect::<Vec<_>>()
                                .join(", ")
                        ));
                    }
                }
                if let Some(url) = &pr.html_url {
                    out.push_str(&format!("URL: {}\n", url));
                }
                if let Some(body) = pr.body.as_deref().map(str::trim).filter(|b| !b.is_empty()) {
                    out.push_str(&format!("\n{}", truncate(body, 4_000)));
                }
                Ok(out)
            }

            "pr_diff" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let diff = client.pull_diff(&repo, number).await?;
                if diff.trim().is_empty() {
                    return Ok(format!("{}#{} has no diff.", repo, number));
                }
                Ok(truncate(&diff, MAX_DIFF_CHARS))
            }

            "pr_files" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let files = client.list_pull_files(&repo, number, 2).await?;
                let mut out = format!("Files in {}#{} ({}):\n", repo, number, files.len());
                for file in files.iter().take(Self::limit(args) * 5) {
                    out.push_str(&format!(
                        "- {} [{}] +{}/-{}",
                        file.filename, file.status, file.additions, file.deletions
                    ));
                    if file.patch.is_none() {
                        out.push_str(" (no patch — binary or too large)");
                    }
                    out.push('\n');
                }
                Ok(out)
            }

            "review_pr" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let post = args["post"].as_bool().unwrap_or(true);
                let force = args["force"].as_bool().unwrap_or(true);
                let outcome = self
                    .engine
                    .review_pr(
                        &repo,
                        number,
                        ReviewOptions {
                            post,
                            force,
                            event: args["event"].as_str().map(|s| s.to_string()),
                            model: args["model"].as_str().map(|s| s.to_string()),
                            source: "manual",
                        },
                    )
                    .await?;
                Ok(outcome.render())
            }

            "pr_review" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let mut body = args["body"].as_str().unwrap_or_default().trim().to_string();
                let event = args["event"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .unwrap_or("COMMENT")
                    .to_ascii_uppercase();
                let pr = client.get_pull(&repo, number).await?;

                let comments = self
                    .validated_comments(client, &repo, number, args, &mut body)
                    .await?;
                if body.is_empty() && comments.is_empty() {
                    return Err(OSAgentError::ToolExecution(
                        "Provide a review body, comments, or both.".to_string(),
                    ));
                }
                let review = client
                    .create_review(&repo, number, &pr.head.sha, &body, &event, &comments)
                    .await?;
                Ok(format!(
                    "Posted {} review on {}#{}: {} inline comment(s).{}",
                    event,
                    repo,
                    number,
                    comments.len(),
                    review
                        .html_url
                        .map(|url| format!(" {}", url))
                        .unwrap_or_default()
                ))
            }

            "pr_comment" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let body = Self::require_str(args, "body")?;
                let comment = client.create_issue_comment(&repo, number, body).await?;
                Ok(format!(
                    "Commented on {}#{}.{}",
                    repo,
                    number,
                    comment
                        .html_url
                        .map(|url| format!(" {}", url))
                        .unwrap_or_default()
                ))
            }

            "pr_create" => {
                let repo = self.resolve_repo(args).await?;
                let title = Self::require_str(args, "title")?;
                let head = Self::require_str(args, "head")?;
                let base = Self::require_str(args, "base")?;
                let pr = client
                    .create_pull(
                        &repo,
                        title,
                        head,
                        base,
                        args["body"].as_str(),
                        args["draft"].as_bool().unwrap_or(false),
                    )
                    .await?;
                Ok(format!(
                    "Created {}#{}: {}{}",
                    repo,
                    pr.number,
                    pr.title.trim(),
                    pr.html_url
                        .map(|url| format!(" {}", url))
                        .unwrap_or_default()
                ))
            }

            "pr_merge" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let method = args["method"]
                    .as_str()
                    .map(str::trim)
                    .filter(|m| ["merge", "squash", "rebase"].contains(m))
                    .unwrap_or("merge");
                let result = client.merge_pull(&repo, number, method).await?;
                Ok(format!(
                    "Merge request for {}#{} ({}): {}{}",
                    repo,
                    number,
                    method,
                    result["message"].as_str().unwrap_or("done"),
                    result["sha"]
                        .as_str()
                        .map(|sha| format!(" → {}", short_sha(sha)))
                        .unwrap_or_default()
                ))
            }

            "pr_close" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let pr = client.close_pull(&repo, number).await?;
                Ok(format!("{}#{} is now {}.", repo, number, pr.state))
            }

            "issue_list" => {
                let repo = self.resolve_repo(args).await?;
                let state = args["state"].as_str().unwrap_or("open");
                let issues = client.list_issues(&repo, state, 1).await?;
                let issues: Vec<_> = issues
                    .into_iter()
                    .filter(|issue| issue.pull_request.is_none())
                    .collect();
                if issues.is_empty() {
                    return Ok(format!("No {} issues in {}.", state, repo));
                }
                let mut out = format!("Issues in {} ({}):\n", repo, state);
                for issue in issues.iter().take(Self::limit(args)) {
                    out.push_str(&format!(
                        "- #{} {} [{}] by {} · {} comment(s)",
                        issue.number,
                        issue.title.trim(),
                        issue.state,
                        issue
                            .user
                            .as_ref()
                            .map(|u| u.login.as_str())
                            .unwrap_or("unknown"),
                        issue.comments.unwrap_or(0)
                    ));
                    if let Some(labels) = &issue.labels {
                        if !labels.is_empty() {
                            out.push_str(&format!(
                                " [{}]",
                                labels
                                    .iter()
                                    .map(|l| l.name.as_str())
                                    .collect::<Vec<_>>()
                                    .join(", ")
                            ));
                        }
                    }
                    if let Some(url) = issue.html_url.as_ref() {
                        out.push_str(&format!("\n  {}", url));
                    }
                    out.push('\n');
                }
                Ok(out)
            }

            "issue_view" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let issue = client.get_issue(&repo, number).await?;
                let mut out = format!(
                    "# {}#{} — {}\nState: {}\nAuthor: {}\n",
                    repo,
                    issue.number,
                    issue.title.trim(),
                    issue.state,
                    issue
                        .user
                        .as_ref()
                        .map(|u| u.login.as_str())
                        .unwrap_or("unknown"),
                );
                if let Some(url) = &issue.html_url {
                    out.push_str(&format!("URL: {}\n", url));
                }
                if let Some(body) = issue
                    .body
                    .as_deref()
                    .map(str::trim)
                    .filter(|b| !b.is_empty())
                {
                    out.push_str(&format!("\n{}", truncate(body, 4_000)));
                }
                Ok(out)
            }

            "issue_create" => {
                let repo = self.resolve_repo(args).await?;
                let title = Self::require_str(args, "title")?;
                let labels: Vec<String> = args["labels"]
                    .as_array()
                    .map(|items| {
                        items
                            .iter()
                            .filter_map(|v| v.as_str().map(|s| s.to_string()))
                            .collect()
                    })
                    .unwrap_or_default();
                let issue = client
                    .create_issue(&repo, title, args["body"].as_str(), &labels)
                    .await?;
                Ok(format!(
                    "Created {}#{}: {}{}",
                    repo,
                    issue.number,
                    issue.title.trim(),
                    issue
                        .html_url
                        .map(|url| format!(" {}", url))
                        .unwrap_or_default()
                ))
            }

            "issue_comment" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let body = Self::require_str(args, "body")?;
                let comment = client.create_issue_comment(&repo, number, body).await?;
                Ok(format!(
                    "Commented on {}#{}.{}",
                    repo,
                    number,
                    comment
                        .html_url
                        .map(|url| format!(" {}", url))
                        .unwrap_or_default()
                ))
            }

            "issue_close" => {
                let repo = self.resolve_repo(args).await?;
                let number = Self::require_u64(args, "number")?;
                let issue = client.close_issue(&repo, number).await?;
                Ok(format!("{}#{} is now {}.", repo, number, issue.state))
            }

            "repo_view" => {
                let repo = self.resolve_repo(args).await?;
                let repo_info = client.get_repo(&repo).await?;
                let mut out = format!("{}\n", repo_info.full_name);
                if let Some(description) = &repo_info.description {
                    out.push_str(&format!("Description: {}\n", description));
                }
                out.push_str(&format!(
                    "Default branch: {}\nVisibility: {}\nOpen issues: {}\n",
                    repo_info.default_branch.as_deref().unwrap_or("unknown"),
                    if repo_info.private {
                        "private"
                    } else {
                        "public"
                    },
                    repo_info.open_issues_count.unwrap_or(0),
                ));
                if let Some(url) = &repo_info.html_url {
                    out.push_str(&format!("URL: {}\n", url));
                }
                Ok(out)
            }

            "branch_list" => {
                let repo = self.resolve_repo(args).await?;
                let branches = client.list_branches(&repo, 1).await?;
                let mut out = format!("Branches in {} ({}):\n", repo, branches.len());
                for branch in branches.iter().take(Self::limit(args) * 5) {
                    out.push_str(&format!(
                        "- {} {}{}\n",
                        branch.name,
                        short_sha(&branch.commit.sha),
                        if branch.protected { " [protected]" } else { "" }
                    ));
                }
                Ok(out)
            }

            "commit_list" => {
                let repo = self.resolve_repo(args).await?;
                let branch = args["branch"].as_str().filter(|s| !s.trim().is_empty());
                let commits = client.list_commits(&repo, branch, 1).await?;
                let mut out = format!("Recent commits in {}:\n", repo);
                for commit in commits.iter().take(Self::limit(args) * 2) {
                    let author = commit
                        .author
                        .as_ref()
                        .map(|a| a.login.as_str())
                        .or_else(|| {
                            commit
                                .commit
                                .author
                                .as_ref()
                                .and_then(|a| a.name.as_deref())
                        })
                        .unwrap_or("unknown");
                    out.push_str(&format!(
                        "- {} {} ({}{})\n",
                        short_sha(&commit.sha),
                        commit.commit.message.lines().next().unwrap_or("").trim(),
                        author,
                        commit
                            .commit
                            .author
                            .as_ref()
                            .and_then(|a| a.date.as_deref())
                            .map(|d| format!(", {}", d.get(..10).unwrap_or(d)))
                            .unwrap_or_default()
                    ));
                }
                Ok(out)
            }

            "checks" => {
                let repo = self.resolve_repo(args).await?;
                let git_ref = args["sha"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .ok_or_else(|| {
                        OSAgentError::ToolExecution(
                            "'sha' (branch, tag, or commit) is required for checks".to_string(),
                        )
                    })?;
                let checks = client.list_check_runs(&repo, git_ref).await?;
                let statuses = client
                    .list_commit_statuses(&repo, git_ref)
                    .await
                    .unwrap_or_default();
                let mut out = format!("Checks for {}@{}:\n", repo, git_ref);
                for check in &checks {
                    out.push_str(&format!(
                        "- {} [{} {}]\n",
                        check.name,
                        check.status,
                        check.conclusion.as_deref().unwrap_or("pending")
                    ));
                }
                for status in &statuses {
                    out.push_str(&format!(
                        "- {} [{}]{}\n",
                        status.context.as_deref().unwrap_or("status"),
                        status.state,
                        status
                            .description
                            .as_deref()
                            .map(|d| format!(" — {}", d))
                            .unwrap_or_default()
                    ));
                }
                if checks.is_empty() && statuses.is_empty() {
                    out.push_str("(no checks reported)\n");
                }
                Ok(out)
            }

            "commit_status_post" => {
                let repo = self.resolve_repo(args).await?;
                let sha = Self::require_str(args, "sha")?;
                let state = Self::require_str(args, "status")?.to_ascii_lowercase();
                if !["error", "failure", "pending", "success"].contains(&state.as_str()) {
                    return Err(OSAgentError::ToolExecution(
                        "status must be one of: error, failure, pending, success".to_string(),
                    ));
                }
                client
                    .create_commit_status(
                        &repo,
                        sha,
                        &state,
                        args["description"].as_str(),
                        args["context"].as_str(),
                    )
                    .await?;
                Ok(format!(
                    "Set commit status '{}' on {}@{}.",
                    state, repo, sha
                ))
            }

            "actions_runs" => {
                let repo = self.resolve_repo(args).await?;
                let branch = args["branch"].as_str().filter(|s| !s.trim().is_empty());
                let runs = client.list_workflow_runs(&repo, branch, 1).await?;
                if runs.is_empty() {
                    return Ok(format!("No workflow runs in {}.", repo));
                }
                let mut out = format!("Workflow runs in {}:\n", repo);
                for run in runs.iter().take(Self::limit(args)) {
                    out.push_str(&format!(
                        "- #{} {} [{} {}] on {} ({}){}\n",
                        run.id,
                        run.display_title
                            .as_deref()
                            .or(run.name.as_deref())
                            .unwrap_or("workflow"),
                        run.status,
                        run.conclusion.as_deref().unwrap_or("running"),
                        run.head_branch.as_deref().unwrap_or("?"),
                        run.event.as_deref().unwrap_or("push"),
                        run.html_url
                            .as_ref()
                            .map(|url| format!("\n  {}", url))
                            .unwrap_or_default()
                    ));
                }
                Ok(out)
            }

            "actions_log" => {
                let repo = self.resolve_repo(args).await?;
                let run_id = Self::require_u64(args, "run_id")?;
                let jobs = client.list_run_jobs(&repo, run_id).await?;
                if jobs.is_empty() {
                    return Ok(format!("Run {} in {} has no jobs yet.", run_id, repo));
                }
                let mut out = format!("Jobs for run #{} in {}:\n", run_id, repo);
                for job in jobs.iter().take(20) {
                    out.push_str(&format!(
                        "## {} [{} {}]{}\n",
                        job.name,
                        job.status,
                        job.conclusion.as_deref().unwrap_or("running"),
                        job.html_url
                            .as_ref()
                            .map(|url| format!(" {}", url))
                            .unwrap_or_default()
                    ));
                    if let Some(steps) = &job.steps {
                        for step in steps {
                            let failed = step
                                .conclusion
                                .as_deref()
                                .map(|c| c != "success" && c != "skipped")
                                .unwrap_or(false);
                            if failed {
                                out.push_str(&format!(
                                    "- ⚠️ {} [{}]\n",
                                    step.name,
                                    step.conclusion.as_deref().unwrap_or("")
                                ));
                            }
                        }
                    }
                }
                out.push_str(
                    "\nNote: raw logs are large; use the GitHub UI link above for full output.",
                );
                Ok(out)
            }

            "release_list" => {
                let repo = self.resolve_repo(args).await?;
                let releases = client.list_releases(&repo, 1).await?;
                if releases.is_empty() {
                    return Ok(format!("No releases in {}.", repo));
                }
                let mut out = format!("Releases in {}:\n", repo);
                for release in releases.iter().take(Self::limit(args)) {
                    out.push_str(&format!(
                        "- {} {} ({}){}\n",
                        release.tag_name,
                        release.name.as_deref().unwrap_or(""),
                        release.published_at.as_deref().unwrap_or("unpublished"),
                        if release.prerelease {
                            " [prerelease]"
                        } else {
                            ""
                        }
                    ));
                }
                Ok(out)
            }

            "search_code" => {
                let query = Self::require_str(args, "query")?;
                let results = client.search_code(query, Self::limit(args) as u32).await?;
                if results.is_empty() {
                    return Ok("No code results.".to_string());
                }
                let mut out = format!("Code results for '{}':\n", query);
                for item in results {
                    out.push_str(&format!(
                        "- {} :: {}\n",
                        item.repository.full_name, item.path
                    ));
                }
                Ok(out)
            }

            "search_issues" => {
                let query = Self::require_str(args, "query")?;
                let results = client
                    .search_issues(query, Self::limit(args) as u32)
                    .await?;
                if results.is_empty() {
                    return Ok("No issue or PR results.".to_string());
                }
                let mut out = format!("Issue/PR results for '{}':\n", query);
                for item in results {
                    let repo_name = item
                        .repository_url
                        .as_deref()
                        .and_then(|url| {
                            let mut parts = url.rsplit('/');
                            let name = parts.next()?;
                            let owner = parts.next()?;
                            Some(format!("{}/{}", owner, name))
                        })
                        .unwrap_or_default();
                    out.push_str(&format!(
                        "- {}{}{} [{}] {}\n",
                        repo_name,
                        if repo_name.is_empty() { "" } else { "#" },
                        item.number,
                        item.state,
                        item.title.trim()
                    ));
                }
                Ok(out)
            }

            "review_history" => {
                let repo = args["repo"]
                    .as_str()
                    .map(str::trim)
                    .filter(|s| !s.is_empty())
                    .map(normalize_repo)
                    .transpose()?;
                let reviews = self
                    .engine
                    .storage()
                    .list_github_reviews(repo.as_deref(), Self::limit(args))?;
                if reviews.is_empty() {
                    return Ok("No reviews recorded yet.".to_string());
                }
                let mut out = "OSA review history:\n".to_string();
                for review in reviews {
                    out.push_str(&format!(
                        "- {}#{} @{} [{}] {} comment(s){} {} {}\n",
                        review.repo,
                        review.pr_number,
                        short_sha(&review.head_sha),
                        review.status,
                        review.comments_count,
                        review
                            .review_url
                            .as_ref()
                            .map(|url| format!(" {}", url))
                            .unwrap_or_default(),
                        review.updated_at.format("%Y-%m-%d %H:%M"),
                        review
                            .error
                            .as_ref()
                            .map(|e| format!("error: {}", truncate(e, 200)))
                            .unwrap_or_default()
                    ));
                }
                Ok(out)
            }

            other => Err(OSAgentError::ToolExecution(format!(
                "Unknown github action: {}",
                other
            ))),
        }
    }

    /// Parse + diff-validate user-provided inline comments for `pr_review`.
    /// Comments that cannot be pinned to a diff line are appended to the
    /// review body instead of failing the call.
    async fn validated_comments(
        &self,
        client: &crate::github::client::GithubClient,
        repo: &str,
        number: u64,
        args: &Value,
        body: &mut String,
    ) -> Result<Vec<GhReviewCommentInput>> {
        let Some(raw) = args["comments"].as_array().filter(|c| !c.is_empty()) else {
            return Ok(Vec::new());
        };
        let files = client.list_pull_files(repo, number, 2).await?;
        let diffs: std::collections::HashMap<String, _> = files
            .iter()
            .map(|file| {
                (
                    file.filename.clone(),
                    parse_patch(&file.filename, file.patch.as_deref().unwrap_or_default()),
                )
            })
            .collect();

        let mut comments = Vec::new();
        for (index, raw) in raw.iter().enumerate() {
            let path = raw["path"].as_str().unwrap_or_default().trim();
            let line = raw["line"].as_u64().unwrap_or(0);
            let text = raw["body"].as_str().unwrap_or_default().trim();
            if path.is_empty() || line == 0 || text.is_empty() {
                return Err(OSAgentError::ToolExecution(format!(
                    "comments[{}] needs path, line, and body",
                    index
                )));
            }
            let side = raw["side"]
                .as_str()
                .map(str::trim)
                .filter(|s| s.eq_ignore_ascii_case("left") || s.eq_ignore_ascii_case("right"))
                .map(|s| s.to_ascii_uppercase())
                .unwrap_or_else(|| "RIGHT".to_string());
            let valid = match_file_path(path, &diffs)
                .map(|file| file.has_patch && file.is_valid_position(line, &side))
                .unwrap_or(false);
            if valid {
                comments.push(GhReviewCommentInput {
                    path: path.to_string(),
                    line,
                    side,
                    body: text.to_string(),
                    start_line: None,
                    start_side: None,
                });
            } else {
                body.push_str(&format!("\n\n**`{}:{}`** {}\n", path, line, text));
            }
        }
        Ok(comments)
    }
}

#[async_trait]
impl Tool for GithubTool {
    fn name(&self) -> &str {
        "github"
    }

    fn description(&self) -> &str {
        "GitHub operations: pull requests (list, view, diff, files, create, merge, close), CodeRabbit-style PR review, reviews and comments, issues, branches, commits, checks, Actions runs and logs, releases, and code search. `action` picks the operation; `repo` is `owner/repo` and is optional when one repository is watched."
    }

    fn when_to_use(&self) -> &str {
        "anything on GitHub - PR review, CI failures, issues, releases"
    }

    fn when_not_to_use(&self) -> &str {
        "generic web pages on github.com (`web_fetch`) or local git (`bash`)"
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(TOOL_TIMEOUT_MS)
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Review a pull request like CodeRabbit".to_string(),
                input: json!({
                    "action": "review_pr",
                    "repo": "owner/repo",
                    "number": 123
                }),
            },
            ToolExample {
                description: "List open pull requests".to_string(),
                input: json!({
                    "action": "pr_list",
                    "repo": "owner/repo"
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": [
                        "user", "pr_list", "pr_view", "pr_diff", "pr_files", "review_pr",
                        "pr_review", "pr_comment", "pr_create", "pr_merge", "pr_close",
                        "issue_list", "issue_view", "issue_create", "issue_comment", "issue_close",
                        "repo_view", "branch_list", "commit_list", "checks", "commit_status_post",
                        "actions_runs", "actions_log", "release_list", "search_code",
                        "search_issues", "review_history"
                    ],
                    "description": "GitHub operation to perform"
                },
                "repo": {
                    "type": "string",
                    "description": "Repository as owner/repo (GitHub URLs also accepted). Optional when exactly one repository is watched in Settings > GitHub."
                },
                "number": {
                    "type": "integer",
                    "description": "Pull request or issue number"
                },
                "title": { "type": "string", "description": "Title for pr_create / issue_create" },
                "body": { "type": "string", "description": "Markdown body for comments, reviews, PRs, and issues" },
                "head": { "type": "string", "description": "Head branch for pr_create" },
                "base": { "type": "string", "description": "Base branch for pr_create (e.g. main)" },
                "branch": { "type": "string", "description": "Branch filter for commit_list / actions_runs" },
                "draft": { "type": "boolean", "description": "Create as draft (pr_create)" },
                "state": { "type": "string", "enum": ["open", "closed", "all"], "description": "State filter for pr_list / issue_list" },
                "status": { "type": "string", "enum": ["error", "failure", "pending", "success"], "description": "Commit status state for commit_status_post" },
                "limit": { "type": "integer", "minimum": 1, "maximum": 50, "description": "Maximum results (default 10)" },
                "sha": { "type": "string", "description": "Commit SHA / branch / tag for checks and commit_status_post" },
                "query": { "type": "string", "description": "Search query for search_code / search_issues" },
                "event": {
                    "type": "string",
                    "enum": ["COMMENT", "REQUEST_CHANGES", "APPROVE"],
                    "description": "Review event type for review_pr / pr_review (default COMMENT)"
                },
                "post": { "type": "boolean", "description": "For review_pr: set false for a dry run that is not posted to GitHub (default true)" },
                "force": { "type": "boolean", "description": "For review_pr: re-review even if this commit was already reviewed (default true for manual calls)" },
                "model": { "type": "string", "description": "For review_pr: model override as provider:model (default uses the configured review model)" },
                "method": { "type": "string", "enum": ["merge", "squash", "rebase"], "description": "Merge method for pr_merge" },
                "run_id": { "type": "integer", "description": "Workflow run id for actions_log" },
                "description": { "type": "string", "description": "Commit status description (commit_status_post)" },
                "context": { "type": "string", "description": "Commit status context label (commit_status_post)" },
                "labels": {
                    "type": "array",
                    "items": { "type": "string" },
                    "description": "Labels for issue_create"
                },
                "comments": {
                    "type": "array",
                    "description": "Inline comments for pr_review: [{path, line, side (RIGHT|LEFT), body}]",
                    "items": {
                        "type": "object",
                        "properties": {
                            "path": { "type": "string" },
                            "line": { "type": "integer" },
                            "side": { "type": "string", "enum": ["RIGHT", "LEFT"] },
                            "body": { "type": "string" }
                        },
                        "required": ["path", "line", "body"]
                    }
                }
            },
            "required": ["action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("'action' is required".to_string()))?;
        // `commit_status_post` and `checks` both use `sha`; `pr_review` and
        // `review_pr` both review. Keep the dispatch flat and explicit.
        self.run(action, &args).await
    }
}

fn truncate(text: &str, max: usize) -> String {
    let mut chars = text.chars();
    let head: String = chars.by_ref().take(max).collect();
    if chars.next().is_some() {
        format!("{}\n…[truncated]", head)
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_declares_review_action() {
        let engine = Arc::new(GithubEngine::new(Arc::new(
            crate::storage::SqliteStorage::new_in_memory().expect("in-memory storage"),
        )));
        let tool = GithubTool::new(engine);
        let params = tool.parameters();
        let actions = params["properties"]["action"]["enum"]
            .as_array()
            .expect("action enum");
        assert!(actions.iter().any(|a| a == "review_pr"));
        assert!(actions.iter().any(|a| a == "pr_review"));
        assert_eq!(params["required"][0], "action");
    }
}
