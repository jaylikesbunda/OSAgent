mod channel;
mod checker;
mod installer;
mod version;

pub use channel::UpdateChannel;
pub use checker::{UpdateCheckResult, UpdateChecker};
pub use installer::{get_pending_update, PendingUpdate, UpdateInstaller, UpdateStatus};

pub fn get_current_version() -> String {
    env!("CARGO_PKG_VERSION").to_string()
}
