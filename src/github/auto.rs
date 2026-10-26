//! Background auto-reviewer. Polls the watched repositories for open
//! pull requests with new commits and runs the shared review engine on
//! them — the CodeRabbit-style workflow, without needing a public
//! webhook endpoint. Config is re-read every cycle, so settings changes
//! apply without a restart.

use std::sync::Arc;
use std::time::Duration;

use crate::agent::runtime::AgentRuntime;
use tracing::{info, warn};

/// Startup grace period before the first sweep.
const INITIAL_DELAY_SECS: u64 = 15;

/// Spawn the poller. The task exits when the runtime signals shutdown
/// (restart or process exit), so it is safe to call once per server run.
pub fn spawn_auto_reviewer(agent: Arc<AgentRuntime>) {
    tokio::spawn(async move {
        let engine = agent.github();
        let mut shutdown = agent.subscribe_shutdown();

        if sleep_or_shutdown(&mut shutdown, INITIAL_DELAY_SECS).await {
            return;
        }

        loop {
            let config = agent.get_config().await.github;
            if config.any_trigger_enabled() && !config.watch_repos.is_empty() {
                // Skip silently when GitHub is not connected: the settings
                // pane shows the setup hint, and a warning every sweep
                // would just be noise.
                let connected =
                    crate::github::client::resolve_token(&agent.get_config().await).is_some();
                if connected {
                    match engine.poll_once().await {
                        Ok(summary) if summary.swept => {
                            if summary.reviewed > 0 || !summary.errors.is_empty() {
                                info!(
                                    "GitHub auto-review sweep: {} repo(s), {} reviewed, {} skipped, {} error(s)",
                                    summary.repos,
                                    summary.reviewed,
                                    summary.skipped,
                                    summary.errors.len()
                                );
                            }
                            for error in &summary.errors {
                                warn!("GitHub auto-review: {}", error);
                            }
                        }
                        Ok(_) => {}
                        Err(error) => warn!("GitHub auto-review sweep failed: {}", error),
                    }
                }
            }

            let interval = config.poll_interval_secs();
            if sleep_or_shutdown(&mut shutdown, interval).await {
                return;
            }
        }
    });
}

/// Sleep `seconds`, waking early when shutdown is signaled. Returns true
/// when the caller should stop.
async fn sleep_or_shutdown(
    shutdown: &mut tokio::sync::watch::Receiver<bool>,
    seconds: u64,
) -> bool {
    tokio::select! {
        _ = tokio::time::sleep(Duration::from_secs(seconds)) => false,
        changed = shutdown.changed() => {
            changed.is_err() || *shutdown.borrow()
        }
    }
}
