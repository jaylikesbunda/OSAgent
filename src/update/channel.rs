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

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateChannelSource {
    Config,
    GitTag,
    GitBranch,
    Default,
}

pub fn is_beta_tag(tag: &str) -> bool {
    let lower = tag.to_lowercase();
    lower.contains("beta") || lower.contains("rc") || lower.contains("alpha")
}

pub fn resolve_effective_channel(
    config_channel: Option<UpdateChannel>,
    install_kind: InstallKind,
    git_tag: Option<&str>,
    git_branch: Option<&str>,
) -> (UpdateChannel, UpdateChannelSource) {
    if let Some(channel) = config_channel {
        return (channel, UpdateChannelSource::Config);
    }

    match install_kind {
        InstallKind::Git => {
            if let Some(tag) = git_tag {
                let channel = if is_beta_tag(tag) {
                    UpdateChannel::Beta
                } else {
                    UpdateChannel::Stable
                };
                return (channel, UpdateChannelSource::GitTag);
            }
            if let Some(branch) = git_branch {
                if branch != "HEAD" {
                    return (UpdateChannel::Dev, UpdateChannelSource::GitBranch);
                }
            }
            (UpdateChannel::Dev, UpdateChannelSource::Default)
        }
        InstallKind::Package => (UpdateChannel::Stable, UpdateChannelSource::Default),
        InstallKind::Unknown => (UpdateChannel::Stable, UpdateChannelSource::Default),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallKind {
    Git,
    Package,
    Unknown,
}
