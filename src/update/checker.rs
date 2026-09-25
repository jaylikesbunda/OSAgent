use crate::update::channel::{resolve_manifest_channel, UpdateChannel};
use crate::update::installer::validate_update_tag;
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
    /// Exact release tag returned by latest.json. This remains available when
    /// that release belongs to a different channel than the one requested.
    pub latest_tag: Option<String>,
    /// Exact channel returned by latest.json.
    pub latest_channel: Option<crate::update::UpdateChannel>,
    pub update_available: bool,
    /// Channel requested by the caller.
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
    channel: Option<String>,
}

pub struct UpdateChecker {
    client: Client,
    current_version: String,
}

fn channel_matches(requested: UpdateChannel, manifest: UpdateChannel) -> bool {
    requested == manifest
}

fn empty_result(
    current_version: &str,
    channel: UpdateChannel,
    checked_at: chrono::DateTime<chrono::Utc>,
) -> UpdateCheckResult {
    UpdateCheckResult {
        current_version: current_version.to_string(),
        latest_version: None,
        latest_tag: None,
        latest_channel: None,
        update_available: false,
        channel,
        release_url: None,
        release_notes: None,
        checked_at,
        error: None,
    }
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

    /// Backwards-compatible check with no expected tag.
    pub async fn check(&self, channel: UpdateChannel) -> UpdateCheckResult {
        match self.check_exact(channel, None).await {
            Ok(result) => result,
            Err(error) => {
                let mut result = empty_result(&self.current_version, channel, chrono::Utc::now());
                result.error = Some(error);
                result
            }
        }
    }

    /// Check the current latest release, optionally requiring its exact tag.
    ///
    /// Network/manifest failures are represented in `error` for compatibility
    /// with the existing API. Invalid input and an expected-tag mismatch are
    /// returned as `Err`, so callers such as the HTTP API can reject them with
    /// a client error instead of presenting them as a successful check.
    pub async fn check_exact(
        &self,
        channel: UpdateChannel,
        expected_tag: Option<&str>,
    ) -> Result<UpdateCheckResult, String> {
        if let Some(tag) = expected_tag {
            validate_update_tag(tag)?;
        }

        let checked_at = chrono::Utc::now();
        let url = format!("{CDN_BASE_URL}/releases/latest.json");

        let response = match self.client.get(&url).send().await {
            Ok(response) => response,
            Err(error) => {
                let mut result = empty_result(&self.current_version, channel, checked_at);
                result.error = Some(format!("Failed to fetch manifest: {}", error));
                return Ok(result);
            }
        };

        if !response.status().is_success() {
            let mut result = empty_result(&self.current_version, channel, checked_at);
            result.error = Some(format!("Manifest returned HTTP {}", response.status()));
            return Ok(result);
        }

        let manifest = match response.json::<CdnManifest>().await {
            Ok(manifest) => manifest,
            Err(error) => {
                let mut result = empty_result(&self.current_version, channel, checked_at);
                result.error = Some(format!("Failed to parse manifest: {}", error));
                return Ok(result);
            }
        };

        let mut result = empty_result(&self.current_version, channel, checked_at);
        if let Err(error) = validate_update_tag(&manifest.tag) {
            result.error = Some(error);
            return Ok(result);
        }
        let manifest_channel =
            match resolve_manifest_channel(manifest.channel.as_deref(), &manifest.tag) {
                Ok(channel) => channel,
                Err(error) => {
                    result.error = Some(error);
                    return Ok(result);
                }
            };

        if let Some(expected) = expected_tag {
            if manifest.tag != expected {
                return Err(format!(
                    "Requested update tag '{}' does not match latest release '{}'",
                    expected, manifest.tag
                ));
            }
        }

        result.latest_tag = Some(manifest.tag.clone());
        result.latest_channel = Some(manifest_channel);

        if !channel_matches(channel, manifest_channel) {
            return Ok(result);
        }

        let latest_version = manifest.version.clone();
        let update_available = is_newer(&latest_version, &self.current_version)
            || (matches!(channel, UpdateChannel::Beta | UpdateChannel::Dev)
                && is_prerelease_of(&latest_version, &self.current_version));

        result.latest_version = Some(latest_version);
        result.update_available = update_available;
        result.release_url = Some(format!("{CDN_BASE_URL}/releases/{}/", manifest.tag));
        Ok(result)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::update::UpdateChannel;

    fn manifest(tag: &str, channel: Option<&str>) -> CdnManifest {
        CdnManifest {
            tag: tag.to_string(),
            version: tag.trim_start_matches('v').to_string(),
            channel: channel.map(str::to_string),
        }
    }

    #[test]
    fn channel_resolution_prefers_explicit_manifest_value() {
        assert_eq!(
            resolve_manifest_channel(Some("beta"), "v1.2.3").unwrap(),
            UpdateChannel::Beta
        );
        assert_eq!(
            resolve_manifest_channel(None, "v1.2.3-rc1").unwrap(),
            UpdateChannel::Beta
        );
        assert_eq!(
            resolve_manifest_channel(None, "v1.2.3-nightly").unwrap(),
            UpdateChannel::Dev
        );
    }

    #[test]
    fn channel_filtering_is_exact() {
        let stable = manifest("v1.2.3", Some("stable"));
        assert_eq!(
            resolve_manifest_channel(stable.channel.as_deref(), &stable.tag).unwrap(),
            UpdateChannel::Stable
        );
        assert!(channel_matches(
            UpdateChannel::Stable,
            resolve_manifest_channel(stable.channel.as_deref(), &stable.tag).unwrap()
        ));
        assert!(!channel_matches(UpdateChannel::Beta, UpdateChannel::Stable));
    }
}
