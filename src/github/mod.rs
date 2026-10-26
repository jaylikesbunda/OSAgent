//! GitHub integration: REST client, the `github` tool backend, and the
//! CodeRabbit-style pull request auto-reviewer.
//!
//! Layout:
//! - `types`   — GitHub API response shapes
//! - `client`  — HTTP client, auth resolution, error mapping
//! - `diff`    — unified-diff parsing for inline comment validation
//! - `review`  — review prompt, structured output parsing, composition
//! - `engine`  — orchestration (fetch → model → validate → post → state)
//! - `auto`    — background poller over the watched repositories

pub mod auto;
pub mod client;
pub mod diff;
pub mod engine;
pub mod review;
pub mod types;

pub use auto::spawn_auto_reviewer;
pub use client::{resolve_token, GithubClient, TokenSource};
pub use engine::{
    needs_review, normalize_repo, DeviceFlowPoll, EngineStatusSnapshot, GithubEngine, PollSummary,
    ReviewOptions, ReviewOutcome,
};

/// Shared OAuth app client id baked into official builds, the way the
/// `gh` CLI, VS Code, and GitHub's own MCP server ship theirs.
///
/// To enable zero-setup "Sign in with GitHub" for every OSA user:
/// 1. Register an OAuth app at https://github.com/settings/applications/new
///    (name: OSAgent; homepage/callback can be the project URL — the
///    device flow ignores the callback), then tick **Enable Device Flow**
///    in the app's settings.
/// 2. Paste the app's **Client ID** below and rebuild.
///
/// Client ids are public values — no client secret is involved in the
/// device flow. Users who prefer their own app can still paste a client
/// id in Settings > GitHub; it takes precedence over this default.
///
/// Release builders can also bake the id via the build environment
/// (`OSAGENT_GITHUB_OAUTH_CLIENT_ID=<id> cargo build`) without editing
/// this file; the env value wins over the constant.
pub const DEFAULT_GITHUB_OAUTH_CLIENT_ID: &str = "Ov23limJWSDpEeeQrZUo";

/// The shared client id for this build: compile-time env override first
/// (release CI), then the constant above.
fn shared_oauth_client_id() -> String {
    let from_env = option_env!("OSAGENT_GITHUB_OAUTH_CLIENT_ID")
        .map(str::trim)
        .filter(|id| !id.is_empty());
    match from_env {
        Some(id) => id.to_string(),
        None => DEFAULT_GITHUB_OAUTH_CLIENT_ID.trim().to_string(),
    }
}

/// Which OAuth app backs "Sign in with GitHub".
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthAppSource {
    /// The user configured their own client id.
    Custom,
    /// The client id baked into this build (zero-setup for users).
    Shared,
    /// Neither is set; sign-in requires an app registration first.
    None,
}

/// The client id to use for GitHub sign-in: user-configured first, then
/// the shared build default.
pub fn effective_oauth_client_id(configured: &str) -> String {
    match oauth_app_source(configured) {
        OAuthAppSource::Custom => configured.trim().to_string(),
        OAuthAppSource::Shared => shared_oauth_client_id(),
        OAuthAppSource::None => String::new(),
    }
}

/// Classify where the active sign-in client id comes from.
pub fn oauth_app_source(configured: &str) -> OAuthAppSource {
    classify_oauth_app(configured, &shared_oauth_client_id())
}

/// Pure precedence rule: an explicit client id always wins; otherwise the
/// shared build default applies when present.
fn classify_oauth_app(configured: &str, shared: &str) -> OAuthAppSource {
    if !configured.trim().is_empty() {
        OAuthAppSource::Custom
    } else if !shared.trim().is_empty() {
        OAuthAppSource::Shared
    } else {
        OAuthAppSource::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn oauth_app_precedence() {
        assert_eq!(classify_oauth_app("", ""), OAuthAppSource::None);
        assert_eq!(
            classify_oauth_app("", "Ov23liShared"),
            OAuthAppSource::Shared
        );
        assert_eq!(classify_oauth_app("Iv1Mine", ""), OAuthAppSource::Custom);
        assert_eq!(
            classify_oauth_app("Iv1Mine", "Ov23liShared"),
            OAuthAppSource::Custom
        );
        assert_eq!(classify_oauth_app("   ", "  "), OAuthAppSource::None);
    }

    #[test]
    fn effective_client_id_matches_source() {
        assert_eq!(effective_oauth_client_id("  Iv1Mine "), "Iv1Mine");
        let shared = shared_oauth_client_id();
        if shared.is_empty() {
            assert!(effective_oauth_client_id("").is_empty());
        } else {
            assert_eq!(effective_oauth_client_id(""), shared);
        }
    }
}
