mod channel;
mod checker;
mod installer;
mod version;

pub use channel::UpdateChannel;
pub use checker::{UpdateCheckResult, UpdateChecker};
pub use installer::{get_pending_update, PendingUpdate, UpdateInstaller, UpdateStatus};

pub fn build_version() -> &'static str {
    option_env!("OSAGENT_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub fn get_current_version() -> String {
    build_version().to_string()
}
