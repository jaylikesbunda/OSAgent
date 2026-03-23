mod channel;
mod checker;
mod version;

pub use channel::{UpdateChannel, UpdateChannelSource};
pub use checker::{UpdateCheckResult, UpdateChecker};
pub use version::{compare_versions, parse_version};
