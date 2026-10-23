//! OpenTunnel bridge for remote access.
//!
//! OpenTunnel gives this machine a public `<id>.opentunnel.xyz` hostname
//! without an account: the first run creates the tunnel and generates the
//! key/token locally, and later runs reuse that identity. TLS terminates in
//! this process, so the relay only forwards encrypted bytes.

use crate::error::{OSAgentError, Result};
use std::path::PathBuf;

/// Where the stable identity lives. Keeping it under the OSA data directory
/// means the address survives restarts and updates.
fn storage_root() -> PathBuf {
    dirs_next::home_dir()
        .map(|home| home.join(".osagent").join("opentunnel"))
        .unwrap_or_else(|| std::env::temp_dir().join("osagent-opentunnel"))
}

pub fn client() -> opentunnel::Client {
    opentunnel::Client::new(opentunnel::ClientOptions {
        storage: Some(opentunnel::Storage::at(storage_root())),
        ..Default::default()
    })
}

/// Make sure a local identity exists, returning its hostname.
pub async fn ensure_identity(client: &opentunnel::Client, profile: &str) -> Result<String> {
    let identity = client
        .ensure(profile)
        .await
        .map_err(|error| OSAgentError::Unknown(format!("OpenTunnel: {error}")))?;
    Ok(identity.hostname)
}

/// Forward the tunnel hostname itself to the local web server.
pub fn connect(
    client: &opentunnel::Client,
    profile: &str,
    port: u16,
) -> Result<opentunnel::Tunnel> {
    let mut routes = opentunnel::Routes::new();
    routes.insert(
        "@".to_string(),
        opentunnel::Route::new(format!("127.0.0.1:{port}")),
    );
    client
        .connect(profile, routes)
        .map_err(|error| OSAgentError::Unknown(format!("OpenTunnel: {error}")))
}
