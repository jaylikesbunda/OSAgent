use crate::update::channel::{is_beta_tag, UpdateChannel};
use crate::update::version::{is_newer, is_prerelease_of};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::time::Duration;

const CDN_BASE_URL: &str = "https://osa.fuckyourcdn.com";
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
struct CdnManifest {
    tag: String,
    version: String,
    released_at: Option<String>,
    channel: Option<String>,
    #[serde(default)]
    assets: std::collections::HashMap<String, CdnAssetEntry>,
    #[serde(default)]
    sha256: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdnAssetEntry {
    archive: String,
    url: String,
}

pub struct UpdateChecker {
    client: Client,
    current_version: String,
}

impl UpdateChecker {
    pub fn new(current_version: &str) -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(10))
            .user_agent(USER_AGENT)
            .build()
            .unwrap_or_else(|_| Client::new());

        Self {
            client,
            current_version: current_version.to_string(),
        }
    }

    pub async fn check(&self, channel: UpdateChannel) -> UpdateCheckResult {
        let checked_at = chrono::Utc::now();
        let url = format!("{CDN_BASE_URL}/releases/latest.json");

        match self.client.get(&url).send().await {
            Ok(response) if response.status().is_success() => {
                match response.json::<CdnManifest>().await {
                    Ok(manifest) => {
                        let manifest_channel = manifest.channel.as_deref().unwrap_or("stable");
                        if channel == UpdateChannel::Stable && is_beta_tag(&manifest.tag) {
                            return UpdateCheckResult {
                                current_version: self.current_version.clone(),
                                latest_version: None,
                                update_available: false,
                                channel,
                                release_url: None,
                                release_notes: None,
                                checked_at,
                                error: None,
                            };
                        }
                        if channel == UpdateChannel::Beta
                            && !is_beta_tag(&manifest.tag)
                            && manifest_channel != "beta"
                        {
                            return UpdateCheckResult {
                                current_version: self.current_version.clone(),
                                latest_version: None,
                                update_available: false,
                                channel,
                                release_url: None,
                                release_notes: None,
                                checked_at,
                                error: None,
                            };
                        }

                        let latest_version = manifest.version.clone();
                        let update_available = is_newer(&latest_version, &self.current_version)
                            || (channel == UpdateChannel::Beta
                                && is_prerelease_of(&latest_version, &self.current_version));

                        UpdateCheckResult {
                            current_version: self.current_version.clone(),
                            latest_version: Some(latest_version),
                            update_available,
                            channel,
                            release_url: Some(format!("{CDN_BASE_URL}/releases/{}/", manifest.tag)),
                            release_notes: None,
                            checked_at,
                            error: None,
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
                        error: Some(format!("Failed to parse manifest: {}", e)),
                    },
                }
            }
            Ok(response) => UpdateCheckResult {
                current_version: self.current_version.clone(),
                latest_version: None,
                update_available: false,
                channel,
                release_url: None,
                release_notes: None,
                checked_at,
                error: Some(format!("Manifest returned HTTP {}", response.status())),
            },
            Err(e) => UpdateCheckResult {
                current_version: self.current_version.clone(),
                latest_version: None,
                update_available: false,
                channel,
                release_url: None,
                release_notes: None,
                checked_at,
                error: Some(format!("Failed to fetch manifest: {}", e)),
            },
        }
    }
}

pub fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
