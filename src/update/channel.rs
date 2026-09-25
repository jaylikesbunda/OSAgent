use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
    Dev,
}

impl std::fmt::Display for UpdateChannel {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Stable => write!(f, "stable"),
            Self::Beta => write!(f, "beta"),
            Self::Dev => write!(f, "dev"),
        }
    }
}

impl std::str::FromStr for UpdateChannel {
    type Err = String;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        match s.to_lowercase().as_str() {
            "stable" => Ok(Self::Stable),
            "beta" => Ok(Self::Beta),
            "dev" => Ok(Self::Dev),
            _ => Err(format!("Invalid update channel: {}", s)),
        }
    }
}

pub fn is_beta_tag(tag: &str) -> bool {
    let lower = tag.to_lowercase();
    lower.contains("beta") || lower.contains("rc") || lower.contains("alpha")
}

pub fn is_dev_tag(tag: &str) -> bool {
    let lower = tag.to_lowercase();
    lower.contains("dev") || lower.contains("nightly") || lower.contains("canary")
}

/// Resolve the channel advertised by a release manifest.
///
/// Newer manifests carry an explicit channel. Older manifests only carried a
/// tag, so retain a conservative tag-based fallback for those releases.
pub fn resolve_manifest_channel(
    advertised: Option<&str>,
    tag: &str,
) -> Result<UpdateChannel, String> {
    if let Some(channel) = advertised.map(str::trim).filter(|value| !value.is_empty()) {
        return channel.parse();
    }

    if is_dev_tag(tag) {
        Ok(UpdateChannel::Dev)
    } else if is_beta_tag(tag) {
        Ok(UpdateChannel::Beta)
    } else {
        Ok(UpdateChannel::Stable)
    }
}
