mod channel;
mod checker;
mod installer;
mod manager;
mod version;

pub use channel::UpdateChannel;
pub use checker::{UpdateCheckResult, UpdateChecker};
pub use installer::{
    get_pending_update, get_prepared_update, sniff_payload_format, PayloadFormat, PendingUpdate,
    PendingUpdateKind, ReleaseAsset, UpdateInstaller, UpdateStatus,
};
pub use manager::{
    InstallMode, UpdateManager, UpdateManagerError, UpdateManagerState, UpdatePhase,
};

pub fn build_version() -> &'static str {
    option_env!("OSAGENT_APP_VERSION").unwrap_or(env!("CARGO_PKG_VERSION"))
}

pub fn get_current_version() -> String {
    build_version().to_string()
}
