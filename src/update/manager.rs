use crate::update::checker::{UpdateCheckResult, UpdateChecker};
use crate::update::installer::{validate_update_tag, write_atomic, PendingUpdate, UpdateInstaller};
use crate::update::{build_version, UpdateChannel};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::AbortHandle;
use tracing::warn;
use uuid::Uuid;

const STATE_FILE_NAME: &str = "update_state.json";
const PENDING_FILE_NAME: &str = "pending_update.json";
const PREPARED_FILE_NAME: &str = "prepared_update.json";
const PROGRESS_PERSIST_INTERVAL_BYTES: u64 = 512 * 1024;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdatePhase {
    Idle,
    Checking,
    Available,
    Downloading,
    Preparing,
    Ready,
    Installing,
    Cancelled,
    Error,
}

impl std::fmt::Display for UpdatePhase {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let value = match self {
            Self::Idle => "idle",
            Self::Checking => "checking",
            Self::Available => "available",
            Self::Downloading => "downloading",
            Self::Preparing => "preparing",
            Self::Ready => "ready",
            Self::Installing => "installing",
            Self::Cancelled => "cancelled",
            Self::Error => "error",
        };
        formatter.write_str(value)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateManagerState {
    pub phase: UpdatePhase,
    pub channel: Option<UpdateChannel>,
    pub tag: Option<String>,
    pub version: Option<String>,
    pub progress: Option<f32>,
    pub bytes_downloaded: Option<u64>,
    pub total_bytes: Option<u64>,
    pub message: Option<String>,
    pub error: Option<String>,
    pub transaction_id: Option<String>,
    pub checked_at: Option<DateTime<Utc>>,
    pub updated_at: DateTime<Utc>,
}

impl Default for UpdateManagerState {
    fn default() -> Self {
        Self {
            phase: UpdatePhase::Idle,
            channel: None,
            tag: None,
            version: None,
            progress: None,
            bytes_downloaded: None,
            total_bytes: None,
            message: None,
            error: None,
            transaction_id: None,
            checked_at: None,
            updated_at: Utc::now(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InstallMode {
    Standalone,
    LauncherManaged,
}

impl InstallMode {
    pub fn from_environment() -> Self {
        if std::env::var("OSAGENT_LAUNCHER_MANAGED")
            .ok()
            .as_deref()
            .map(str::trim)
            == Some("1")
        {
            Self::LauncherManaged
        } else {
            Self::Standalone
        }
    }
}

#[derive(Debug, Error, Clone)]
pub enum UpdateManagerError {
    #[error("Another update operation is already in progress ({0})")]
    Busy(String),
    #[error("{0}")]
    InvalidRequest(String),
    #[error("No matching update release is available: {0}")]
    NoRelease(String),
    #[error("{0}")]
    Unsupported(String),
    #[error("{0}")]
    Internal(String),
}

impl From<String> for UpdateManagerError {
    fn from(value: String) -> Self {
        Self::Internal(value)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OperationKind {
    Check,
    Download,
    Install,
}

impl OperationKind {
    fn as_str(self) -> &'static str {
        match self {
            Self::Check => "check",
            Self::Download => "download",
            Self::Install => "install",
        }
    }
}

#[derive(Debug, Clone)]
struct ActiveOperation {
    token: String,
    kind: OperationKind,
}

struct ManagerInner {
    installer: UpdateInstaller,
    install_mode: InstallMode,
    state_path: PathBuf,
    pending_path: PathBuf,
    prepared_path: PathBuf,
    updates_dir: PathBuf,
    state: Mutex<UpdateManagerState>,
    operation: AsyncMutex<Option<ActiveOperation>>,
    active_task: Mutex<Option<(String, AbortHandle, Arc<tokio::sync::Notify>)>>,
}

struct TaskCompletion(Arc<tokio::sync::Notify>);

impl Drop for TaskCompletion {
    fn drop(&mut self) {
        self.0.notify_one();
    }
}

#[derive(Clone)]
pub struct UpdateManager {
    inner: Arc<ManagerInner>,
}

impl UpdateManager {
    pub fn new(installer: UpdateInstaller, install_mode: InstallMode) -> Arc<Self> {
        let base = dirs_next::home_dir()
            .or_else(|| std::env::current_dir().ok())
            .unwrap_or_else(|| PathBuf::from("."))
            .join(".osagent");
        Self::with_paths(
            installer,
            install_mode,
            base.join(STATE_FILE_NAME),
            base.join(PENDING_FILE_NAME),
            base.join(PREPARED_FILE_NAME),
        )
    }

    fn with_paths(
        installer: UpdateInstaller,
        install_mode: InstallMode,
        state_path: PathBuf,
        pending_path: PathBuf,
        prepared_path: PathBuf,
    ) -> Arc<Self> {
        let persisted = read_state(&state_path);
        let updates_dir = pending_path
            .parent()
            .unwrap_or_else(|| Path::new("."))
            .join("updates");
        let inner = Arc::new(ManagerInner {
            installer,
            install_mode,
            state_path,
            pending_path,
            prepared_path,
            updates_dir,
            state: Mutex::new(persisted),
            operation: AsyncMutex::new(None),
            active_task: Mutex::new(None),
        });
        let manager = Self { inner };
        manager.reconcile_markers();
        Arc::new(manager)
    }

    pub fn install_mode(&self) -> InstallMode {
        self.inner.install_mode
    }

    pub fn status(&self) -> UpdateManagerState {
        self.reconcile_helper_completion();
        self.raw_status()
    }

    fn raw_status(&self) -> UpdateManagerState {
        self.inner
            .state
            .lock()
            .expect("update state lock poisoned")
            .clone()
    }

    fn reconcile_helper_completion(&self) {
        let state = self.raw_status();
        if state.phase != UpdatePhase::Installing {
            return;
        }

        if let Ok(Some(pending)) = read_marker(&self.inner.pending_path) {
            if pending.phase.as_deref() == Some("failed") {
                let _ = self.update_state(|current| {
                    current.phase = UpdatePhase::Error;
                    current.message = Some("The launcher could not install the update".to_string());
                    current.error = pending.error.clone();
                });
            } else if pending.phase.as_deref() == Some("committed") {
                let _ = self.update_state(|current| {
                    current.phase = UpdatePhase::Idle;
                    current.progress = Some(100.0);
                    current.message = Some("Update installed successfully".to_string());
                    current.error = None;
                    current.transaction_id = None;
                });
            }
            return;
        }

        if !self.inner.pending_path.exists() {
            let applied = state
                .version
                .as_deref()
                .map(|version| version.trim_start_matches('v') == build_version())
                .unwrap_or(false);
            let _ = self.update_state(|current| {
                current.phase = if applied {
                    UpdatePhase::Idle
                } else {
                    UpdatePhase::Error
                };
                current.progress = if applied { Some(100.0) } else { None };
                current.message = Some(if applied {
                    "Update installed successfully".to_string()
                } else {
                    "The update handoff ended before installation completed".to_string()
                });
                current.error = if applied {
                    None
                } else {
                    Some("The launcher did not confirm the prepared update".to_string())
                };
                current.transaction_id = None;
            });
        }
    }

    /// Run a foreground check. The operation gate is shared with background
    /// downloads and install arming, so only one coordinator operation exists.
    pub async fn check(
        &self,
        channel: UpdateChannel,
        expected_tag: Option<&str>,
    ) -> Result<UpdateCheckResult, UpdateManagerError> {
        if let Some(tag) = expected_tag {
            validate_update_tag(tag).map_err(UpdateManagerError::InvalidRequest)?;
        }

        let operation = self.begin_operation(OperationKind::Check).await?;
        if let Err(error) = self.update_state(|state| {
            state.phase = UpdatePhase::Checking;
            state.channel = Some(channel);
            state.error = None;
            state.message = Some(format!("Checking the {} update channel...", channel));
        }) {
            self.release_operation(&operation.token).await;
            return Err(error.into());
        }

        let manager = self.clone();
        let token = operation.token.clone();
        let expected_tag = expected_tag.map(str::to_string);
        let completion = Arc::new(tokio::sync::Notify::new());
        let task_completion = TaskCompletion(completion.clone());
        let task = tokio::spawn(async move {
            let _task_completion = task_completion;
            manager.run_check(token, channel, expected_tag).await
        });
        self.register_task(&operation.token, task.abort_handle(), completion);

        match task.await {
            Ok(result) => result,
            Err(error) if error.is_cancelled() => Err(UpdateManagerError::InvalidRequest(
                "Update check was cancelled".to_string(),
            )),
            Err(error) => {
                self.fail_interrupted_operation(&operation.token, "check", &error.to_string())
                    .await;
                Err(UpdateManagerError::Internal(format!(
                    "Update check task failed: {}",
                    error
                )))
            }
        }
    }

    /// Start a background download and return immediately. Network resolution,
    /// byte progress, preparation, and marker creation all happen in the
    /// spawned task; callers poll `status()` for progress.
    pub async fn start_download(
        &self,
        channel: UpdateChannel,
        expected_tag: Option<&str>,
    ) -> Result<UpdateManagerState, UpdateManagerError> {
        if let Some(tag) = expected_tag {
            validate_update_tag(tag).map_err(UpdateManagerError::InvalidRequest)?;
        }

        let operation = self.begin_operation(OperationKind::Download).await?;
        if let Err(error) = self.update_state(|state| {
            state.phase = UpdatePhase::Downloading;
            state.channel = Some(channel);
            state.progress = Some(0.0);
            state.bytes_downloaded = Some(0);
            state.total_bytes = None;
            state.message = Some("Starting update download...".to_string());
            state.error = None;
        }) {
            self.release_operation(&operation.token).await;
            return Err(error.into());
        }

        let manager = self.clone();
        let token = operation.token.clone();
        let expected_tag = expected_tag.map(str::to_string);
        let completion = Arc::new(tokio::sync::Notify::new());
        let task_completion = TaskCompletion(completion.clone());
        let task = tokio::spawn(async move {
            let _task_completion = task_completion;
            manager.run_download(token, channel, expected_tag).await;
        });
        self.register_task(&operation.token, task.abort_handle(), completion);
        Ok(self.status())
    }

    /// Arm the launcher marker for a prepared update.
    ///
    /// Standalone mode is rejected before reading or writing any launcher
    /// marker. On launcher-managed mode the returned success means the pending
    /// marker was durably written; only then may the caller signal shutdown.
    pub async fn install(
        &self,
        expected_tag: Option<&str>,
    ) -> Result<UpdateManagerState, UpdateManagerError> {
        if self.inner.install_mode == InstallMode::Standalone {
            return Err(UpdateManagerError::Unsupported(
                "Automatic installation is only available when OSAgent is managed by the launcher (OSAGENT_LAUNCHER_MANAGED=1); no pending marker was written"
                    .to_string(),
            ));
        }
        if let Some(tag) = expected_tag {
            validate_update_tag(tag).map_err(UpdateManagerError::InvalidRequest)?;
        }

        let operation = self.begin_operation(OperationKind::Install).await?;
        let result = self.install_prepared(expected_tag).await;
        self.release_operation(&operation.token).await;
        result
    }

    pub async fn cancel(&self) -> Result<UpdateManagerState, UpdateManagerError> {
        let mut operation = self.inner.operation.lock().await;
        let Some(active) = operation.clone() else {
            return Err(UpdateManagerError::Busy(
                "There is no cancellable update operation in progress".to_string(),
            ));
        };

        let phase = self.status().phase;
        if active.kind == OperationKind::Install || phase == UpdatePhase::Preparing {
            return Err(UpdateManagerError::Busy(
                "The update cannot be cancelled while it is being installed or prepared"
                    .to_string(),
            ));
        }
        if !matches!(phase, UpdatePhase::Checking | UpdatePhase::Downloading) {
            return Err(UpdateManagerError::Busy(
                "There is no cancellable update operation in progress".to_string(),
            ));
        }

        let (abort, completion) = {
            let mut active_task = self
                .inner
                .active_task
                .lock()
                .expect("active update task lock poisoned");
            match active_task.as_ref() {
                Some((token, abort, completion)) if token == &active.token => {
                    let abort = abort.clone();
                    let completion = completion.clone();
                    active_task.take();
                    (Some(abort), Some(completion))
                }
                _ => (None, None),
            }
        };

        // Keep the async operation gate locked until the aborted task has
        // actually stopped. This prevents a new operation from overlapping a
        // download that has not unwound yet.
        if let Some(abort) = abort {
            abort.abort();
        }
        if let Some(completion) = completion {
            completion.notified().await;
        }

        let state = self.update_state(|state| {
            state.phase = UpdatePhase::Cancelled;
            state.progress = None;
            state.message = Some(if active.kind == OperationKind::Check {
                "Update check cancelled".to_string()
            } else {
                "Update download cancelled".to_string()
            });
            state.error = None;
        });
        *operation = None;
        drop(operation);
        Ok(state?)
    }

    async fn begin_operation(
        &self,
        kind: OperationKind,
    ) -> Result<ActiveOperation, UpdateManagerError> {
        let mut operation = self.inner.operation.lock().await;
        if let Some(active) = operation.as_ref() {
            return Err(UpdateManagerError::Busy(format!(
                "A {} operation is already in progress",
                active.kind.as_str()
            )));
        }
        let active = ActiveOperation {
            token: Uuid::new_v4().to_string(),
            kind,
        };
        *operation = Some(active.clone());
        Ok(active)
    }

    fn register_task(&self, token: &str, abort: AbortHandle, completion: Arc<tokio::sync::Notify>) {
        let mut active_task = self
            .inner
            .active_task
            .lock()
            .expect("active update task lock poisoned");
        if let Some((_, old_abort, _)) = active_task.replace((token.to_string(), abort, completion))
        {
            old_abort.abort();
        }
    }

    async fn release_operation(&self, token: &str) {
        let mut operation = self.inner.operation.lock().await;
        if operation.as_ref().map(|active| active.token.as_str()) == Some(token) {
            *operation = None;
        }
    }

    async fn fail_interrupted_operation(&self, token: &str, operation: &str, error: &str) {
        let _ = self.update_state(|state| {
            state.phase = UpdatePhase::Error;
            state.message = Some(format!("Update {} task failed", operation));
            state.error = Some(error.to_string());
        });
        self.release_operation(token).await;
    }

    async fn run_check(
        &self,
        token: String,
        channel: UpdateChannel,
        expected_tag: Option<String>,
    ) -> Result<UpdateCheckResult, UpdateManagerError> {
        let checker = UpdateChecker::new(build_version());
        let result = checker.check_exact(channel, expected_tag.as_deref()).await;
        let has_prepared = self.has_valid_prepared_marker();
        let state_result = match &result {
            Ok(result) => self.update_state(|state| {
                state.phase = if has_prepared {
                    UpdatePhase::Ready
                } else if result.error.is_some() {
                    UpdatePhase::Error
                } else if result.update_available {
                    UpdatePhase::Available
                } else {
                    UpdatePhase::Idle
                };
                state.channel = Some(channel);
                if !has_prepared {
                    state.tag = result.latest_tag.clone();
                    state.version = result.latest_version.clone();
                }
                state.checked_at = Some(result.checked_at);
                state.message = if has_prepared {
                    Some("Prepared update remains ready to install".to_string())
                } else if result.update_available {
                    Some(format!(
                        "Update {} is available",
                        result
                            .latest_tag
                            .as_deref()
                            .unwrap_or(result.latest_version.as_deref().unwrap_or(""))
                    ))
                } else if result.error.is_some() {
                    result.error.clone()
                } else {
                    Some("OSAgent is up to date".to_string())
                };
                state.error = result.error.clone();
            }),
            Err(error) => self.update_state(|state| {
                state.phase = UpdatePhase::Error;
                state.message = Some(error.clone());
                state.error = Some(error.clone());
            }),
        };
        self.release_operation(&token).await;

        match (result, state_result) {
            (Ok(result), Ok(_)) => Ok(result),
            (Ok(_), Err(error)) => Err(error.into()),
            (Err(error), _) => Err(UpdateManagerError::InvalidRequest(error)),
        }
    }

    async fn run_download(
        &self,
        token: String,
        channel: UpdateChannel,
        expected_tag: Option<String>,
    ) {
        let asset = match self
            .inner
            .installer
            .find_release_for_platform(channel, expected_tag.as_deref())
            .await
        {
            Ok(Some(asset)) => asset,
            Ok(None) => {
                let _ = self.update_state(|state| {
                    state.phase = UpdatePhase::Idle;
                    state.progress = None;
                    state.message = Some(format!(
                        "No release is available on the {} channel",
                        channel
                    ));
                    state.error = None;
                });
                self.release_operation(&token).await;
                return;
            }
            Err(error) => {
                let message = if error.contains("does not match latest release") {
                    "The selected release changed; check for updates again"
                } else {
                    "Download failed"
                };
                self.record_operation_error(message, &error);
                self.release_operation(&token).await;
                return;
            }
        };

        if validate_update_tag(&asset.tag).is_err() {
            self.record_operation_error(
                "Download failed",
                "Release manifest returned an unsafe tag",
            );
            self.release_operation(&token).await;
            return;
        }
        if let Err(error) = self.inner.installer.cleanup_stale_updates(Some(&asset.tag)) {
            warn!("Failed to clean stale update payloads: {}", error);
        }

        if let Err(error) = self.update_state(|state| {
            state.tag = Some(asset.tag.clone());
            state.version = Some(asset.version.clone());
            state.message = Some(format!("Downloading {}...", asset.tag));
        }) {
            self.record_operation_error("Download failed", &error);
            self.release_operation(&token).await;
            return;
        }

        let progress_manager = self.clone();
        let last_persisted = Arc::new(AtomicU64::new(0));
        let callback_last_persisted = last_persisted.clone();
        let archive_path = self
            .inner
            .installer
            .download_release(&asset, move |downloaded, total| {
                let next_persist_at = callback_last_persisted.load(Ordering::Relaxed);
                let should_persist = downloaded >= next_persist_at;
                if should_persist {
                    callback_last_persisted.store(
                        downloaded.saturating_add(PROGRESS_PERSIST_INTERVAL_BYTES),
                        Ordering::Relaxed,
                    );
                }
                // Status reads see every received chunk. Persist a checkpoint
                // at a lower frequency so large downloads do not turn into a
                // disk sync for every network packet.
                let snapshot = progress_manager.mutate_state(|state| {
                    state.bytes_downloaded = Some(downloaded);
                    state.total_bytes = if total > 0 { Some(total) } else { None };
                    state.progress = if total > 0 {
                        Some((downloaded as f64 / total as f64 * 100.0) as f32)
                    } else {
                        None
                    };
                });
                if should_persist {
                    if let Err(error) = progress_manager.persist_state(&snapshot) {
                        warn!("Failed to persist update progress: {}", error);
                    } else {
                        tracing::debug!("Update progress: {}/{} bytes", downloaded, total);
                    }
                }
            })
            .await;

        let archive_path = match archive_path {
            Ok(path) => path,
            Err(error) => {
                self.record_operation_error("Download failed", &error);
                self.release_operation(&token).await;
                return;
            }
        };

        let downloaded_bytes = std::fs::metadata(&archive_path)
            .map(|metadata| metadata.len())
            .unwrap_or_default();
        if let Err(error) = self.update_state(|state| {
            state.phase = UpdatePhase::Preparing;
            state.progress = Some(100.0);
            state.bytes_downloaded = Some(downloaded_bytes);
            state.message = Some("Preparing verified update...".to_string());
        }) {
            self.record_operation_error("Update preparation failed", &error);
            self.release_operation(&token).await;
            return;
        }

        let staged_path = match self
            .inner
            .installer
            .prepare_update(&archive_path, &asset.tag)
            .await
        {
            Ok(path) => path,
            Err(error) => {
                self.record_operation_error("Update preparation failed", &error);
                self.release_operation(&token).await;
                return;
            }
        };

        if let Err(error) = self
            .inner
            .installer
            .mark_prepared_update(&asset.tag, &staged_path)
        {
            self.record_operation_error("Failed to mark update as prepared", &error);
            self.release_operation(&token).await;
            return;
        }
        if archive_path != staged_path {
            let _ = std::fs::remove_file(&archive_path);
        }
        let _ = self.inner.installer.cleanup_stale_updates(Some(&asset.tag));

        if let Err(error) = self.update_state(|state| {
            state.phase = UpdatePhase::Ready;
            state.progress = Some(100.0);
            state.message = Some("Update ready to install".to_string());
            state.error = None;
        }) {
            warn!(
                "Prepared update marker exists, but state persistence failed: {}",
                error
            );
        }
        self.release_operation(&token).await;
    }

    async fn install_prepared(
        &self,
        expected_tag: Option<&str>,
    ) -> Result<UpdateManagerState, UpdateManagerError> {
        let prepared = match read_marker(&self.inner.prepared_path)
            .map_err(UpdateManagerError::Internal)?
        {
            Some(prepared) => prepared,
            None => {
                let pending = read_marker(&self.inner.pending_path)
                    .map_err(UpdateManagerError::Internal)?
                    .filter(|pending| pending.armed && pending.phase.as_deref() == Some("failed"))
                    .ok_or_else(|| {
                        UpdateManagerError::InvalidRequest(
                            "No prepared update found. Download an update first.".to_string(),
                        )
                    })?;
                pending
            }
        };

        if let Some(expected) = expected_tag {
            if prepared.tag != expected {
                return Err(UpdateManagerError::InvalidRequest(format!(
                    "Requested update tag '{}' does not match prepared update '{}'",
                    expected, prepared.tag
                )));
            }
        }
        self.inner
            .installer
            .validate_staged_path_under(
                &self.inner.updates_dir,
                &prepared.tag,
                &prepared.staged_path,
            )
            .map_err(UpdateManagerError::InvalidRequest)?;

        if let Some(pending) =
            read_marker(&self.inner.pending_path).map_err(UpdateManagerError::Internal)?
        {
            if pending.armed && pending.phase.as_deref() != Some("failed") {
                return Err(UpdateManagerError::Busy(format!(
                    "Update {} is already armed for installation",
                    pending.tag
                )));
            }
        }

        self.update_state(|state| {
            state.phase = UpdatePhase::Installing;
            state.channel = state.channel.or(Some(UpdateChannel::Stable));
            state.tag = Some(prepared.tag.clone());
            state.version = Some(prepared.tag.trim_start_matches('v').to_string());
            state.message = Some("Arming update for the launcher...".to_string());
            state.error = None;
        })?;

        let pending = match self.inner.installer.mark_update_pending(
            &prepared.tag,
            &prepared.staged_path,
            true,
            Some("installing"),
        ) {
            Ok(pending) => pending,
            Err(error) => {
                let _ = self.update_state(|state| {
                    state.phase = UpdatePhase::Error;
                    state.message = Some("Failed to arm update for installation".to_string());
                    state.error = Some(error.clone());
                });
                return Err(UpdateManagerError::Internal(error));
            }
        };

        // From this point onward the launcher contract is armed. Status
        // persistence is best-effort so a state-file error cannot strand the
        // process while leaving a valid pending marker behind.
        if let Err(error) = self.update_state(|state| {
            state.phase = UpdatePhase::Installing;
            state.transaction_id = pending.transaction_id.clone();
            state.message = Some("Update armed; shutting down for installation".to_string());
        }) {
            warn!(
                "Update marker armed, but state persistence failed: {}",
                error
            );
        }
        if let Err(error) = self.inner.installer.clear_prepared_update() {
            warn!(
                "Update armed, but prepared marker cleanup failed: {}",
                error
            );
        }
        Ok(self.status())
    }

    fn has_valid_prepared_marker(&self) -> bool {
        read_marker(&self.inner.prepared_path)
            .ok()
            .flatten()
            .map(|prepared| {
                self.inner
                    .installer
                    .validate_staged_path_under(
                        &self.inner.updates_dir,
                        &prepared.tag,
                        &prepared.staged_path,
                    )
                    .is_ok()
            })
            .unwrap_or(false)
    }

    fn record_operation_error(&self, message: &str, error: &str) {
        if let Err(persist_error) = self.update_state(|state| {
            state.phase = UpdatePhase::Error;
            state.message = Some(message.to_string());
            state.error = Some(error.to_string());
        }) {
            warn!(
                "{}: {} (state persistence also failed: {})",
                message, error, persist_error
            );
        }
    }

    fn mutate_state<F>(&self, update: F) -> UpdateManagerState
    where
        F: FnOnce(&mut UpdateManagerState),
    {
        let mut state = self.inner.state.lock().expect("update state lock poisoned");
        update(&mut state);
        state.updated_at = Utc::now();
        state.clone()
    }

    fn persist_state(&self, snapshot: &UpdateManagerState) -> Result<(), String> {
        let json = serde_json::to_string_pretty(snapshot)
            .map_err(|error| format!("Failed to serialize update state: {}", error))?;
        write_atomic(&self.inner.state_path, &json)
    }

    fn update_state<F>(&self, update: F) -> Result<UpdateManagerState, String>
    where
        F: FnOnce(&mut UpdateManagerState),
    {
        let snapshot = self.mutate_state(update);
        self.persist_state(&snapshot)?;
        Ok(snapshot)
    }

    fn reconcile_markers(&self) {
        let persisted_phase = self.raw_status().phase;
        let mut reconciliation_error = None;

        let pending = match read_marker(&self.inner.pending_path) {
            Ok(pending) => pending,
            Err(error) => {
                reconciliation_error = Some(error);
                None
            }
        };

        if let Some(pending) = pending {
            if pending.armed
                && self
                    .inner
                    .installer
                    .validate_staged_path_under(
                        &self.inner.updates_dir,
                        &pending.tag,
                        &pending.staged_path,
                    )
                    .is_ok()
            {
                let phase = pending.phase.as_deref().unwrap_or("installing");
                let _ = self.update_state(|state| {
                    state.phase = if phase == "failed" {
                        UpdatePhase::Error
                    } else if phase == "committed" {
                        UpdatePhase::Idle
                    } else {
                        UpdatePhase::Installing
                    };
                    state.tag = Some(pending.tag.clone());
                    state.version = Some(pending.tag.trim_start_matches('v').to_string());
                    state.transaction_id = pending.transaction_id.clone();
                    state.message = Some(if phase == "failed" {
                        "The launcher could not install the update".to_string()
                    } else if phase == "committed" {
                        "Update installed successfully".to_string()
                    } else {
                        "Update is armed for the launcher".to_string()
                    });
                    state.error = if phase == "failed" {
                        pending.error.clone()
                    } else {
                        None
                    };
                });
                let _ = std::fs::remove_file(&self.inner.prepared_path);
                return;
            }
            reconciliation_error = Some(format!(
                "Pending update {} is invalid and was preserved for recovery",
                pending.tag
            ));
        }

        let prepared = match read_marker(&self.inner.prepared_path) {
            Ok(prepared) => prepared,
            Err(error) => {
                let _ = std::fs::remove_file(&self.inner.prepared_path);
                reconciliation_error = Some(error);
                None
            }
        };

        if let Some(prepared) = prepared {
            if self
                .inner
                .installer
                .validate_staged_path_under(
                    &self.inner.updates_dir,
                    &prepared.tag,
                    &prepared.staged_path,
                )
                .is_ok()
            {
                let _ = self.update_state(|state| {
                    state.phase = UpdatePhase::Ready;
                    state.tag = Some(prepared.tag.clone());
                    state.version = Some(prepared.tag.trim_start_matches('v').to_string());
                    state.progress = Some(100.0);
                    state.message = Some("Update ready to install".to_string());
                    state.error = None;
                });
                return;
            }
            let _ = std::fs::remove_file(&self.inner.prepared_path);
            reconciliation_error = Some(format!(
                "Discarded prepared update {} because its staged payload is missing or unsafe",
                prepared.tag
            ));
        }

        if let Some(error) = reconciliation_error {
            let _ = self.update_state(|state| {
                state.phase = UpdatePhase::Error;
                state.message = Some("Update marker reconciliation failed".to_string());
                state.error = Some(error);
            });
            return;
        }

        if matches!(
            persisted_phase,
            UpdatePhase::Checking
                | UpdatePhase::Downloading
                | UpdatePhase::Preparing
                | UpdatePhase::Installing
        ) {
            let installed_version = self
                .raw_status()
                .version
                .map(|version| version.trim_start_matches('v') == build_version())
                .unwrap_or(false);
            let _ = self.update_state(|state| {
                state.phase = if persisted_phase == UpdatePhase::Installing && installed_version {
                    UpdatePhase::Idle
                } else {
                    UpdatePhase::Cancelled
                };
                state.progress = if persisted_phase == UpdatePhase::Installing && installed_version
                {
                    Some(100.0)
                } else {
                    None
                };
                state.message = if persisted_phase == UpdatePhase::Installing && installed_version {
                    Some("Update installed successfully".to_string())
                } else {
                    Some("Previous update operation was interrupted by restart".to_string())
                };
                state.error = None;
                state.transaction_id = None;
            });
        } else if persisted_phase == UpdatePhase::Ready {
            // A ready state without a marker is stale and must not be
            // installable through the API.
            let _ = self.update_state(|state| {
                state.phase = UpdatePhase::Idle;
                state.progress = None;
                state.message = None;
            });
        }
    }
}

fn read_state(path: &Path) -> UpdateManagerState {
    let Ok(contents) = std::fs::read_to_string(path) else {
        return UpdateManagerState::default();
    };
    match serde_json::from_str(&contents) {
        Ok(state) => state,
        Err(error) => {
            warn!(
                "Ignoring unreadable update state {}: {}",
                path.display(),
                error
            );
            UpdateManagerState::default()
        }
    }
}

fn read_marker(path: &Path) -> Result<Option<PendingUpdate>, String> {
    if !path.exists() {
        return Ok(None);
    }
    let contents = std::fs::read_to_string(path)
        .map_err(|error| format!("Failed to read {}: {}", path.display(), error))?;
    let marker = serde_json::from_str(&contents)
        .map_err(|error| format!("Failed to parse {}: {}", path.display(), error))?;
    Ok(Some(marker))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::tempdir;

    fn paths(root: &Path) -> (PathBuf, PathBuf, PathBuf) {
        (
            root.join(STATE_FILE_NAME),
            root.join(PENDING_FILE_NAME),
            root.join(PREPARED_FILE_NAME),
        )
    }

    #[tokio::test]
    async fn interrupted_download_recovers_as_cancelled() {
        let root = tempdir().unwrap();
        let (state_path, pending_path, prepared_path) = paths(root.path());
        let state = UpdateManagerState {
            phase: UpdatePhase::Downloading,
            bytes_downloaded: Some(1234),
            ..UpdateManagerState::default()
        };
        std::fs::write(&state_path, serde_json::to_vec_pretty(&state).unwrap()).unwrap();

        let manager = UpdateManager::with_paths(
            UpdateInstaller::new(),
            InstallMode::LauncherManaged,
            state_path,
            pending_path,
            prepared_path,
        );
        assert_eq!(manager.status().phase, UpdatePhase::Cancelled);
        assert_eq!(manager.status().bytes_downloaded, Some(1234));
    }

    #[tokio::test]
    async fn operation_gate_rejects_overlapping_work() {
        let root = tempdir().unwrap();
        let (state_path, pending_path, prepared_path) = paths(root.path());
        let manager = UpdateManager::with_paths(
            UpdateInstaller::new(),
            InstallMode::Standalone,
            state_path,
            pending_path,
            prepared_path,
        );
        let active = manager.begin_operation(OperationKind::Check).await.unwrap();

        let error = manager
            .start_download(UpdateChannel::Stable, None)
            .await
            .unwrap_err();
        assert!(matches!(error, UpdateManagerError::Busy(_)));
        manager.release_operation(&active.token).await;
    }

    #[tokio::test]
    async fn prepared_marker_reconciles_to_ready_on_construction() {
        let root = tempdir().unwrap();
        let (state_path, pending_path, prepared_path) = paths(root.path());
        let release_dir = root.path().join("updates").join("v1.2.3");
        std::fs::create_dir_all(&release_dir).unwrap();
        let staged_path = release_dir.join("osagent-launcher");
        std::fs::write(&staged_path, b"launcher").unwrap();
        let marker = PendingUpdate {
            tag: "v1.2.3".to_string(),
            staged_path,
            kind: crate::update::PendingUpdateKind::BinarySwap,
            armed: false,
            created_at: Utc::now(),
            transaction_id: None,
            phase: Some("ready".to_string()),
            error: None,
        };
        std::fs::write(&prepared_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();

        let manager = UpdateManager::with_paths(
            UpdateInstaller::new(),
            InstallMode::LauncherManaged,
            state_path,
            pending_path,
            prepared_path,
        );
        assert_eq!(manager.status().phase, UpdatePhase::Ready);
        assert_eq!(manager.status().tag.as_deref(), Some("v1.2.3"));
        assert_eq!(manager.status().version.as_deref(), Some("1.2.3"));
    }

    #[tokio::test]
    async fn armed_pending_marker_reconciles_to_installing() {
        let root = tempdir().unwrap();
        let (state_path, pending_path, prepared_path) = paths(root.path());
        let release_dir = root.path().join("updates").join("v2.0.0");
        std::fs::create_dir_all(&release_dir).unwrap();
        let staged_path = release_dir.join("osagent-launcher");
        std::fs::write(&staged_path, b"launcher").unwrap();
        let marker = PendingUpdate {
            tag: "v2.0.0".to_string(),
            staged_path,
            kind: crate::update::PendingUpdateKind::BinarySwap,
            armed: true,
            created_at: Utc::now(),
            transaction_id: Some("transaction-123".to_string()),
            phase: Some("installing".to_string()),
            error: None,
        };
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();

        let manager = UpdateManager::with_paths(
            UpdateInstaller::new(),
            InstallMode::LauncherManaged,
            state_path,
            pending_path,
            prepared_path,
        );
        assert_eq!(manager.status().phase, UpdatePhase::Installing);
        assert_eq!(
            manager.status().transaction_id.as_deref(),
            Some("transaction-123")
        );
    }

    #[tokio::test]
    async fn failed_pending_marker_reconciles_to_error_and_is_preserved() {
        let root = tempdir().unwrap();
        let (state_path, pending_path, prepared_path) = paths(root.path());
        let release_dir = root.path().join("updates").join("v2.0.1");
        std::fs::create_dir_all(&release_dir).unwrap();
        let staged_path = release_dir.join("osagent-launcher");
        std::fs::write(&staged_path, b"launcher").unwrap();
        let marker = PendingUpdate {
            tag: "v2.0.1".to_string(),
            staged_path,
            kind: crate::update::PendingUpdateKind::BinarySwap,
            armed: true,
            created_at: Utc::now(),
            transaction_id: Some("transaction-failed".to_string()),
            phase: Some("failed".to_string()),
            error: Some("health check timed out".to_string()),
        };
        std::fs::write(&pending_path, serde_json::to_vec_pretty(&marker).unwrap()).unwrap();

        let manager = UpdateManager::with_paths(
            UpdateInstaller::new(),
            InstallMode::LauncherManaged,
            state_path,
            pending_path.clone(),
            prepared_path,
        );
        assert_eq!(manager.status().phase, UpdatePhase::Error);
        assert_eq!(
            manager.status().error.as_deref(),
            Some("health check timed out")
        );
        assert!(pending_path.exists());
    }

    #[tokio::test]
    async fn standalone_install_fails_without_pending_marker() {
        let root = tempdir().unwrap();
        let (state_path, pending_path, prepared_path) = paths(root.path());
        std::fs::write(&prepared_path, "{}\n").unwrap();
        let manager = UpdateManager::with_paths(
            UpdateInstaller::new(),
            InstallMode::Standalone,
            state_path,
            pending_path.clone(),
            prepared_path,
        );

        let error = manager.install(None).await.unwrap_err();
        assert!(matches!(error, UpdateManagerError::Unsupported(_)));
        assert!(!pending_path.exists());
        assert!(!manager.status().transaction_id.is_some());
    }
}
