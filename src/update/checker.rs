use crate::update::channel::{is_beta_tag, UpdateChannel};
use crate::update::version::is_newer;
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const GITHUB_API_BASE: &str = "https://api.github.com";
const DEFAULT_REPO: &str = "owner/osagent";
const USER_AGENT: &str = "osagent-update-checker/0.1.0";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UpdateCheckResult {
    pub current_version: String,
    pub latest_version: Option<String>,
    pub update_available: bool,
    pub channel: UpdateChannel,
    pub release_url: Option<String>,
    pub release_notes: Option<String>,
    pub checked_at: chrono::DateTime<chrono::Utc>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Deserialize)]
struct GitHubRelease {
    tag_name: String,
    name: String,
    html_url: String,
    body: Option<String>,
    prerelease: bool,
    draft: bool,
}

pub struct UpdateChecker {
    client: Client,
    repo: String,
    current_version: String,
}

impl UpdateChecker {
    pub fn new(current_version: &str) -> Self {
        Self::with_repo(current_version, DEFAULT_REPO)
    }

    pub fn with_repo(current_version: &str, repo: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            repo: repo.to_string(),
            current_version: current_version.to_string(),
        }
    }

    pub async fn check(&self, channel: UpdateChannel) -> UpdateCheckResult {
        let checked_at = chrono::Utc::now();

        match self.fetch_releases().await {
            Ok(releases) => {
                let filtered = self.filter_by_channel(releases, channel);

                if let Some(latest) = filtered.first() {
                    let latest_version = latest.tag_name.trim_start_matches('v').to_string();
                    let update_available = is_newer(&latest_version, &self.current_version);

                    UpdateCheckResult {
                        current_version: self.current_version.clone(),
                        latest_version: Some(latest_version),
                        update_available,
                        channel,
                        release_url: Some(latest.html_url.clone()),
                        release_notes: latest.body.clone(),
                        checked_at,
                        error: None,
                    }
                } else {
                    UpdateCheckResult {
                        current_version: self.current_version.clone(),
                        latest_version: None,
                        update_available: false,
                        channel,
                        release_url: None,
                        release_notes: None,
                        checked_at,
                        error: Some("No releases found for channel".to_string()),
                    }
                }
            }
            Err(e) => UpdateCheckResult {
                current_version: self.current_version.clone(),
                latest_version: None,
                update_available: false,
                channel,
                release_url: None,
                release_notes: None,
                checked_at,
                error: Some(format!("Failed to fetch releases: {}", e)),
            },
        }
    }

    async fn fetch_releases(&self) -> Result<Vec<GitHubRelease>, reqwest::Error> {
        let url = format!("{}/repos/{}/releases", GITHUB_API_BASE, self.repo);
        let response = self.client.get(&url).send().await?;
        let releases: Vec<GitHubRelease> = response.json().await?;
        Ok(releases.into_iter().filter(|r| !r.draft).collect())
    }

    fn filter_by_channel(
        &self,
        releases: Vec<GitHubRelease>,
        channel: UpdateChannel,
    ) -> Vec<GitHubRelease> {
        let mut filtered: Vec<GitHubRelease> = match channel {
            UpdateChannel::Stable => releases
                .into_iter()
                .filter(|r| !r.prerelease && !is_beta_tag(&r.tag_name))
                .collect(),
            UpdateChannel::Beta => releases
                .into_iter()
                .filter(|r| r.prerelease || is_beta_tag(&r.tag_name))
                .collect(),
            UpdateChannel::Dev => releases,
        };

        filtered.sort_by(|a, b| {
            let v_a = crate::update::version::parse_version(&a.tag_name);
            let v_b = crate::update::version::parse_version(&b.tag_name);
            match (v_a, v_b) {
                (Some(va), Some(vb)) => vb.cmp(&va),
                _ => std::cmp::Ordering::Equal,
            }
        });

        filtered
    }
}

pub fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
