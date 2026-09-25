use crate::update::channel::{resolve_manifest_channel, UpdateChannel};
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::{self, File, OpenOptions};
use tokio::io::AsyncWriteExt;
use tokio::time::Duration;

const CDN_BASE_URL: &str = "https://osa.fuckyourcdn.com";

/// How many times a single download is attempted before giving up. Attempts
/// after the first resume from the bytes already on disk when the CDN honours
/// range requests.
const MAX_DOWNLOAD_ATTEMPTS: usize = 4;

/// Anything smaller than this is an error page or a truncated body, never a
/// real release payload.
const MIN_PAYLOAD_BYTES: u64 = 4096;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum UpdateStatus {
    #[default]
    Idle,
    Downloading {
        progress: f32,
        bytes_downloaded: u64,
        total_bytes: u64,
    },
    Ready {
        tag: String,
        version: String,
    },
    Installing,
    Error {
        message: String,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PendingUpdateKind {
    BinarySwap,
    Installer,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PendingUpdate {
    pub tag: String,
    #[serde(alias = "launcher_path")]
    pub staged_path: PathBuf,
    #[serde(default = "default_pending_update_kind")]
    pub kind: PendingUpdateKind,
    #[serde(default = "default_pending_update_armed")]
    pub armed: bool,
    pub created_at: DateTime<Utc>,
    /// Optional coordinator fields. Older launchers ignore them, preserving
    /// the original marker contract.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub transaction_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

fn default_pending_update_kind() -> PendingUpdateKind {
    PendingUpdateKind::BinarySwap
}

fn default_pending_update_armed() -> bool {
    true
}

/// The release payload selected for this platform, resolved entirely from the
/// manifest rather than from guessed filenames.
#[derive(Debug, Clone)]
pub struct ReleaseAsset {
    pub tag: String,
    pub version: String,
    /// Filename the payload is stored under on disk. Derived from the manifest
    /// (or the download URL), never from a hardcoded per-platform guess — the
    /// two disagree whenever a platform ships a package instead of an archive.
    pub file_name: String,
    pub url: String,
    pub sha256: Option<String>,
    /// True when the payload is a platform installer/package rather than an
    /// archive we can unpack and binary-swap.
    pub is_installer: bool,
}

/// What a downloaded payload actually is, determined by inspecting its bytes.
/// The file extension is a hint; this is the ground truth.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PayloadFormat {
    Zip,
    TarGz,
    WindowsExecutable,
    DebPackage,
    DiskImage,
    Elf,
    /// An HTML/JSON/text body — i.e. a CDN error page served with a 200.
    TextBody,
    Unknown,
}

impl PayloadFormat {
    fn describe(&self) -> &'static str {
        match self {
            Self::Zip => "zip archive",
            Self::TarGz => "gzip-compressed tar archive",
            Self::WindowsExecutable => "Windows executable",
            Self::DebPackage => "Debian package",
            Self::DiskImage => "macOS disk image",
            Self::Elf => "ELF binary",
            Self::TextBody => "text/HTML document (not a release payload)",
            Self::Unknown => "unrecognized data",
        }
    }

    fn is_package(&self) -> bool {
        matches!(
            self,
            Self::WindowsExecutable | Self::DebPackage | Self::DiskImage
        )
    }
}

#[derive(Debug, Clone, Deserialize)]
struct CdnManifest {
    tag: String,
    version: String,
    #[serde(default)]
    channel: Option<String>,
    #[serde(default)]
    assets: std::collections::HashMap<String, CdnAssetEntry>,
    #[serde(default)]
    sha256: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdnAssetEntry {
    #[serde(default)]
    archive: String,
    #[serde(default)]
    url: String,
    #[serde(default)]
    installer: Option<String>,
}

pub struct UpdateInstaller {
    client: Client,
}

#[cfg(windows)]
fn copy_windows_runtime_files(src_launcher: &Path, staged_dir: &Path) -> Result<(), String> {
    let Some(src_dir) = src_launcher.parent() else {
        return Ok(());
    };

    for entry in std::fs::read_dir(src_dir)
        .map_err(|e| format!("Failed to read launcher directory: {}", e))?
    {
        let entry = entry.map_err(|e| format!("Failed to read launcher directory entry: {}", e))?;
        let path = entry.path();
        if !path.is_file() {
            continue;
        }

        let is_runtime_dll = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("dll"))
            .unwrap_or(false);
        if !is_runtime_dll {
            continue;
        }

        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = staged_dir.join(name);
        if path == dest {
            continue;
        }
        std::fs::copy(&path, &dest)
            .map_err(|e| format!("Failed to copy runtime file to staging: {}", e))?;
    }

    Ok(())
}

/// Pull the filename out of a URL, ignoring any query string or fragment.
fn file_name_from_url(url: &str) -> Option<String> {
    let without_fragment = url.split('#').next().unwrap_or(url);
    let without_query = without_fragment
        .split('?')
        .next()
        .unwrap_or(without_fragment);
    let candidate = without_query.rsplit('/').next()?.trim();
    if candidate.is_empty() || !candidate.contains('.') {
        return None;
    }
    Some(candidate.to_string())
}

/// Reject anything that could escape the destination directory when joined.
fn is_safe_relative_path(path: &Path) -> bool {
    path.components().all(|component| match component {
        Component::Normal(part) => !part.to_string_lossy().contains(':'),
        Component::CurDir => true,
        _ => false,
    })
}

/// Validate a release tag before it is used as a directory component or placed
/// in a launcher marker.
pub(crate) fn validate_update_tag(tag: &str) -> Result<(), String> {
    if tag.is_empty() || tag == "." || tag == ".." {
        return Err("Update tag cannot be empty or a relative path".to_string());
    }
    if tag.len() > 128 {
        return Err("Update tag is too long (maximum 128 characters)".to_string());
    }
    if !tag.chars().all(|character| {
        character.is_ascii_alphanumeric() || matches!(character, '.' | '-' | '_' | '+')
    }) {
        return Err(format!(
            "Update tag '{}' contains unsafe path characters; use only letters, numbers, '.', '-', '_' and '+'",
            tag
        ));
    }
    if tag.ends_with('.') {
        return Err("Update tag cannot end with '.'".to_string());
    }

    // Windows treats these names (with any extension) as device paths rather
    // than ordinary directories.
    let device_stem = tag
        .split('.')
        .next()
        .unwrap_or_default()
        .to_ascii_uppercase();
    let is_reserved_device = matches!(device_stem.as_str(), "CON" | "PRN" | "AUX" | "NUL")
        || device_stem
            .strip_prefix("COM")
            .or_else(|| device_stem.strip_prefix("LPT"))
            .is_some_and(|suffix| suffix.len() == 1 && matches!(suffix.as_bytes()[0], b'1'..=b'9'));
    if is_reserved_device {
        return Err(format!("Update tag '{}' is a reserved device name", tag));
    }
    Ok(())
}

/// Identify a payload by its magic bytes. Extensions lie — a `.deb` served
/// under a `.tar.gz` filename is exactly the failure this guards against.
pub fn sniff_payload_format(path: &Path) -> Result<PayloadFormat, String> {
    use std::io::{Read, Seek, SeekFrom};

    let mut file =
        std::fs::File::open(path).map_err(|e| format!("Failed to open payload: {}", e))?;

    let mut head = [0u8; 512];
    let read = file
        .read(&mut head)
        .map_err(|e| format!("Failed to read payload header: {}", e))?;
    let head = &head[..read];

    if head.starts_with(b"PK\x03\x04") || head.starts_with(b"PK\x05\x06") {
        return Ok(PayloadFormat::Zip);
    }
    if head.starts_with(&[0x1f, 0x8b]) {
        return Ok(PayloadFormat::TarGz);
    }
    if head.starts_with(b"MZ") {
        return Ok(PayloadFormat::WindowsExecutable);
    }
    if head.starts_with(b"!<arch>\n") {
        return Ok(PayloadFormat::DebPackage);
    }
    if head.starts_with(b"\x7fELF") {
        return Ok(PayloadFormat::Elf);
    }
    // UDIF disk images carry a `koly` trailer in the last 512 bytes.
    if let Ok(len) = file.seek(SeekFrom::End(0)) {
        if len >= 512 {
            let mut trailer = [0u8; 512];
            if file.seek(SeekFrom::End(-512)).is_ok()
                && file.read_exact(&mut trailer).is_ok()
                && trailer.starts_with(b"koly")
            {
                return Ok(PayloadFormat::DiskImage);
            }
        }
    }

    let leading: Vec<u8> = head
        .iter()
        .copied()
        .skip_while(|b| b.is_ascii_whitespace())
        .take(16)
        .collect();
    if leading.starts_with(b"<") || leading.starts_with(b"{") || leading.starts_with(b"[") {
        return Ok(PayloadFormat::TextBody);
    }

    Ok(PayloadFormat::Unknown)
}

/// Pick the payload to download for a platform.
///
/// `assets.<platform>.url` is the OTA archive and `.installer` is the manual
/// package; `sha256.<platform>` covers the archive and
/// `sha256.<platform>-installer` covers the package.
fn select_asset(
    manifest: &CdnManifest,
    platform_key: &str,
    prefer_installer: bool,
    fallback_archive_name: &str,
) -> ReleaseAsset {
    let entry = manifest.assets.get(platform_key);
    let installer_url = entry
        .and_then(|e| e.installer.as_ref())
        .filter(|url| !url.is_empty());
    let archive_url = entry.map(|e| e.url.as_str()).filter(|url| !url.is_empty());

    let (download_url, is_installer) = match (prefer_installer, installer_url, archive_url) {
        // Windows: the installer is the supported path.
        (true, Some(installer), _) => (installer.clone(), true),
        // Otherwise take the archive, which is the only thing we can unpack.
        (_, _, Some(archive)) => (archive.to_string(), false),
        // No archive published: an installer is better than nothing, and
        // `prepare_update` will say so clearly if it cannot be applied.
        (_, Some(installer), None) => (installer.clone(), true),
        // Nothing usable in the manifest; fall back to the conventional path.
        _ => (
            format!(
                "{CDN_BASE_URL}/releases/{}/{}",
                manifest.tag, fallback_archive_name
            ),
            false,
        ),
    };

    // Name the local file after what is actually being downloaded. Naming it
    // after a guessed platform archive is what made a `.deb` land on disk as
    // `osagent-linux-x86_64.tar.gz` and then fail inside the gzip decoder.
    let file_name = file_name_from_url(&download_url)
        .or_else(|| {
            entry
                .map(|e| e.archive.clone())
                .filter(|name| !name.is_empty())
        })
        .unwrap_or_else(|| fallback_archive_name.to_string());

    let sha256 = if is_installer {
        manifest.sha256.get(&format!("{platform_key}-installer"))
    } else {
        manifest.sha256.get(platform_key)
    };

    ReleaseAsset {
        tag: manifest.tag.clone(),
        version: manifest.version.clone(),
        file_name,
        url: download_url,
        sha256: sha256.cloned(),
        is_installer,
    }
}

fn hash_file(path: &Path) -> Result<String, String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)
        .map_err(|e| format!("Failed to open payload for hashing: {}", e))?;
    let mut hasher = Sha256::new();
    let mut buffer = vec![0u8; 64 * 1024];

    loop {
        let read = file
            .read(&mut buffer)
            .map_err(|e| format!("Failed to read payload while hashing: {}", e))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }

    Ok(hasher
        .finalize()
        .iter()
        .map(|byte| format!("{:02x}", byte))
        .collect())
}

impl UpdateInstaller {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(1800))
            .connect_timeout(Duration::from_secs(30))
            .user_agent("osagent-update-installer/0.1.0")
            .build()
            .unwrap_or_else(|_| Client::new());

        Self { client }
    }

    fn detect_platform(&self) -> &'static str {
        #[cfg(all(target_os = "windows", target_arch = "x86_64"))]
        return "windows-x86_64";
        #[cfg(all(target_os = "windows", target_arch = "aarch64"))]
        return "windows-arm64";
        #[cfg(all(target_os = "linux", target_arch = "x86_64"))]
        return "linux-x86_64";
        #[cfg(all(target_os = "linux", target_arch = "aarch64"))]
        return "linux-arm64";
        #[cfg(all(target_os = "macos", target_arch = "x86_64"))]
        return "macos-x86_64";
        #[cfg(all(target_os = "macos", target_arch = "aarch64"))]
        return "macos-arm64";
        #[cfg(not(any(
            all(
                target_os = "windows",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ),
            all(
                target_os = "linux",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ),
            all(
                target_os = "macos",
                any(target_arch = "x86_64", target_arch = "aarch64")
            ),
        )))]
        return "unknown";
    }

    /// Fallback filename, used only when the manifest gives us neither an
    /// `archive` name nor a usable URL.
    fn platform_archive_name(&self) -> String {
        match self.detect_platform() {
            "windows-x86_64" | "windows-arm64" => "osagent-windows-x86_64.zip".to_string(),
            "linux-x86_64" => "osagent-linux-x86_64.tar.gz".to_string(),
            "linux-arm64" => "osagent-linux-arm64.tar.gz".to_string(),
            "macos-arm64" => "osagent-macos-arm64.tar.gz".to_string(),
            "macos-x86_64" => "osagent-macos-x86_64.tar.gz".to_string(),
            _ => "osagent-unknown.tar.gz".to_string(),
        }
    }

    fn launcher_binary_name(&self) -> &'static str {
        #[cfg(target_os = "windows")]
        return "osagent-launcher.exe";
        #[cfg(not(target_os = "windows"))]
        return "osagent-launcher";
    }

    pub async fn find_release_for_platform(
        &self,
        channel: UpdateChannel,
        expected_tag: Option<&str>,
    ) -> Result<Option<ReleaseAsset>, String> {
        if let Some(tag) = expected_tag {
            validate_update_tag(tag)?;
        }

        let url = format!("{CDN_BASE_URL}/releases/latest.json");
        let response = self
            .client
            .get(&url)
            .send()
            .await
            .map_err(|e| format!("Failed to fetch manifest: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Manifest returned HTTP {}", response.status()));
        }

        let manifest: CdnManifest = response
            .json()
            .await
            .map_err(|e| format!("Failed to parse manifest: {}", e))?;
        validate_update_tag(&manifest.tag)?;

        if let Some(expected) = expected_tag {
            if expected != manifest.tag {
                return Err(format!(
                    "Requested update tag '{}' does not match latest release '{}'",
                    expected, manifest.tag
                ));
            }
        }

        let manifest_channel =
            resolve_manifest_channel(manifest.channel.as_deref(), &manifest.tag)?;
        if manifest_channel != channel {
            return Ok(None);
        }

        Ok(Some(select_asset(
            &manifest,
            self.detect_platform(),
            // Only Windows can execute an installer unattended; everywhere else
            // the archive is the only payload the updater can apply.
            cfg!(target_os = "windows"),
            &self.platform_archive_name(),
        )))
    }

    /// Validate a staged path against the release directory. Besides catching
    /// stale markers, this prevents a malformed marker from making the
    /// launcher execute a file outside the update staging area.
    pub fn validate_staged_path(&self, tag: &str, staged_path: &Path) -> Result<(), String> {
        let update_dir = self.update_dir()?;
        self.validate_staged_path_under(&update_dir, tag, staged_path)
    }

    pub fn validate_staged_path_under(
        &self,
        update_dir: &Path,
        tag: &str,
        staged_path: &Path,
    ) -> Result<(), String> {
        validate_update_tag(tag)?;
        if !staged_path.is_file() {
            return Err(format!(
                "Staged update is missing at {}; re-download the update",
                staged_path.display()
            ));
        }

        let release_dir = update_dir.join(tag);
        let canonical_release_dir = std::fs::canonicalize(&release_dir).map_err(|error| {
            format!(
                "Staged update directory is unavailable at {}: {}",
                release_dir.display(),
                error
            )
        })?;
        let canonical_staged_path = std::fs::canonicalize(staged_path).map_err(|error| {
            format!(
                "Staged update is unreadable at {}: {}",
                staged_path.display(),
                error
            )
        })?;

        if !canonical_staged_path.starts_with(&canonical_release_dir) {
            return Err(format!(
                "Staged update path {} is outside {}",
                staged_path.display(),
                release_dir.display()
            ));
        }
        Ok(())
    }

    fn pending_update_kind_for_path(&self, path: &Path) -> PendingUpdateKind {
        let is_package = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| {
                ["exe", "msi", "deb", "dmg", "pkg", "rpm", "appimage"]
                    .iter()
                    .any(|known| ext.eq_ignore_ascii_case(known))
            })
            .unwrap_or(false);

        if is_package {
            PendingUpdateKind::Installer
        } else {
            PendingUpdateKind::BinarySwap
        }
    }

    pub fn update_dir(&self) -> Result<PathBuf, String> {
        let base = dirs_next::home_dir().ok_or("Could not find home directory")?;
        Ok(base.join(".osagent").join("updates"))
    }

    pub fn pending_update_file(&self) -> Result<PathBuf, String> {
        let base = dirs_next::home_dir().ok_or("Could not find home directory")?;
        Ok(base.join(".osagent").join("pending_update.json"))
    }

    pub fn prepared_update_file(&self) -> Result<PathBuf, String> {
        let base = dirs_next::home_dir().ok_or("Could not find home directory")?;
        Ok(base.join(".osagent").join("prepared_update.json"))
    }

    /// Download a release payload with retries, resume, size verification and
    /// checksum verification. The payload only appears at its final path once
    /// every check has passed, so a half-written or wrong-content file can
    /// never be handed to the extractor.
    pub async fn download_release<F>(
        &self,
        asset: &ReleaseAsset,
        progress_callback: F,
    ) -> Result<PathBuf, String>
    where
        F: Fn(u64, u64) + Send + 'static,
    {
        validate_update_tag(&asset.tag)?;
        let update_dir = self.update_dir()?;
        let dest_dir = update_dir.join(&asset.tag);
        fs::create_dir_all(&dest_dir)
            .await
            .map_err(|e| format!("Failed to create update directory: {}", e))?;

        let dest_path = dest_dir.join(&asset.file_name);
        let part_path = dest_dir.join(format!("{}.part", asset.file_name));

        // A completed payload from an earlier run is reusable only if it still
        // matches the manifest checksum.
        if let Some(expected) = asset.sha256.as_deref() {
            if dest_path.exists() {
                let candidate = dest_path.clone();
                let expected = expected.to_string();
                let matches = tokio::task::spawn_blocking(move || {
                    hash_file(&candidate)
                        .map(|actual| actual.eq_ignore_ascii_case(&expected))
                        .unwrap_or(false)
                })
                .await
                .unwrap_or(false);

                if matches {
                    tracing::info!(
                        "Reusing already-downloaded update payload {}",
                        dest_path.display()
                    );
                    return Ok(dest_path);
                }
                let _ = fs::remove_file(&dest_path).await;
            }
        } else if dest_path.exists() {
            let _ = fs::remove_file(&dest_path).await;
        }

        let _ = fs::remove_file(&part_path).await;

        let progress_callback = Arc::new(progress_callback);
        let mut last_error = String::new();

        for attempt in 1..=MAX_DOWNLOAD_ATTEMPTS {
            match self
                .download_attempt(asset, &part_path, progress_callback.clone())
                .await
            {
                Ok(()) => {
                    last_error.clear();
                    break;
                }
                Err(err) => {
                    last_error = err;
                    tracing::warn!(
                        "Update download attempt {}/{} failed: {}",
                        attempt,
                        MAX_DOWNLOAD_ATTEMPTS,
                        last_error
                    );
                    if attempt == MAX_DOWNLOAD_ATTEMPTS {
                        break;
                    }
                    // Back off, then resume from whatever landed on disk.
                    tokio::time::sleep(Duration::from_millis(500 * (1 << (attempt - 1)))).await;
                }
            }
        }

        if !last_error.is_empty() {
            let _ = fs::remove_file(&part_path).await;
            return Err(format!(
                "Download failed after {} attempts: {}",
                MAX_DOWNLOAD_ATTEMPTS, last_error
            ));
        }

        let downloaded_bytes = fs::metadata(&part_path)
            .await
            .map(|meta| meta.len())
            .map_err(|e| format!("Downloaded payload is unreadable: {}", e))?;

        if downloaded_bytes < MIN_PAYLOAD_BYTES {
            let _ = fs::remove_file(&part_path).await;
            return Err(format!(
                "Downloaded payload is only {} bytes, which is too small to be a release. \
                 The CDN likely returned an error page for {}",
                downloaded_bytes, asset.url
            ));
        }

        if let Some(expected) = asset.sha256.as_deref() {
            let candidate = part_path.clone();
            let actual = tokio::task::spawn_blocking(move || hash_file(&candidate))
                .await
                .map_err(|e| format!("Checksum task failed: {}", e))??;

            if !actual.eq_ignore_ascii_case(expected) {
                let _ = fs::remove_file(&part_path).await;
                return Err(format!(
                    "Checksum mismatch for {}: manifest expects {}, downloaded data hashes to {}. \
                     The download was corrupted or tampered with; nothing was installed.",
                    asset.file_name, expected, actual
                ));
            }
            tracing::info!("Verified SHA-256 for {}", asset.file_name);
        } else {
            tracing::warn!(
                "Manifest has no SHA-256 for {}; installing unverified payload",
                asset.file_name
            );
        }

        fs::rename(&part_path, &dest_path)
            .await
            .map_err(|e| format!("Failed to finalize downloaded payload: {}", e))?;

        Ok(dest_path)
    }

    /// One download pass. Resumes from an existing `.part` file when the server
    /// honours the range request, and restarts cleanly when it does not.
    async fn download_attempt<F>(
        &self,
        asset: &ReleaseAsset,
        part_path: &Path,
        progress_callback: Arc<F>,
    ) -> Result<(), String>
    where
        F: Fn(u64, u64) + Send + 'static,
    {
        let resume_from = match fs::metadata(part_path).await {
            Ok(meta) => meta.len(),
            Err(_) => 0,
        };

        let mut request = self.client.get(&asset.url);
        if resume_from > 0 {
            request = request.header(reqwest::header::RANGE, format!("bytes={}-", resume_from));
        }

        let response = request
            .send()
            .await
            .map_err(|e| format!("Download request failed: {}", e))?;

        let status = response.status();
        if !status.is_success() {
            return Err(format!("Download failed with HTTP {}", status));
        }

        // 206 means our range was honoured; anything else restarts the file.
        let resuming = resume_from > 0 && status == reqwest::StatusCode::PARTIAL_CONTENT;
        let already_on_disk = if resuming { resume_from } else { 0 };

        let expected_total = response
            .content_length()
            .map(|len| len + already_on_disk)
            .unwrap_or(0);

        let mut file = if resuming {
            OpenOptions::new()
                .append(true)
                .open(part_path)
                .await
                .map_err(|e| format!("Failed to reopen partial download: {}", e))?
        } else {
            File::create(part_path)
                .await
                .map_err(|e| format!("Failed to create file: {}", e))?
        };

        let written = Arc::new(AtomicU64::new(already_on_disk));
        let mut response = response;

        loop {
            let chunk = response
                .chunk()
                .await
                .map_err(|e| format!("Read error: {}", e))?;
            match chunk {
                Some(data) => {
                    file.write_all(&data)
                        .await
                        .map_err(|e| format!("Write error: {}", e))?;
                    let current =
                        written.fetch_add(data.len() as u64, Ordering::Relaxed) + data.len() as u64;
                    // Report every received byte. A missing Content-Length is
                    // represented as total=0, but the downloaded count must
                    // still advance in real time.
                    progress_callback(current, expected_total);
                }
                None => break,
            }
        }

        file.flush()
            .await
            .map_err(|e| format!("Flush error: {}", e))?;
        // Make sure the bytes are actually on disk before we hash them.
        file.sync_all()
            .await
            .map_err(|e| format!("Sync error: {}", e))?;
        drop(file);

        // A silently truncated body is the classic cause of a corrupt archive.
        // Catch it here, while retrying is still cheap.
        let final_len = fs::metadata(part_path)
            .await
            .map(|meta| meta.len())
            .unwrap_or(0);
        if expected_total > 0 && final_len != expected_total {
            return Err(format!(
                "Incomplete download: got {} of {} bytes",
                final_len, expected_total
            ));
        }

        Ok(())
    }

    pub async fn extract_update(&self, archive_path: &Path, tag: &str) -> Result<PathBuf, String> {
        validate_update_tag(tag)?;
        let update_dir = self.update_dir()?;
        // Extract into a dedicated subdirectory so leftovers from a previous
        // attempt can be wiped without touching the downloaded payload.
        let extract_dir = update_dir.join(tag).join("extracted");

        if extract_dir.exists() {
            fs::remove_dir_all(&extract_dir)
                .await
                .map_err(|e| format!("Failed to clear previous extraction directory: {}", e))?;
        }
        fs::create_dir_all(&extract_dir)
            .await
            .map_err(|e| format!("Failed to create extraction directory: {}", e))?;

        let format = sniff_payload_format(archive_path)?;
        match format {
            PayloadFormat::Zip => self.extract_zip(archive_path, &extract_dir).await?,
            PayloadFormat::TarGz => self.extract_tar_gz(archive_path, &extract_dir).await?,
            other => {
                return Err(format!(
                    "Cannot extract {}: the downloaded file is a {}, not an archive",
                    archive_path.display(),
                    other.describe()
                ));
            }
        }

        Ok(extract_dir)
    }

    async fn extract_zip(&self, archive: &Path, dest: &Path) -> Result<(), String> {
        let archive = archive.to_path_buf();
        let dest = dest.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&archive)
                .map_err(|e| format!("Failed to open archive: {}", e))?;
            let mut archive =
                zip::ZipArchive::new(file).map_err(|e| format!("Failed to read zip: {}", e))?;

            for i in 0..archive.len() {
                let mut file = archive
                    .by_index(i)
                    .map_err(|e| format!("Failed to read zip entry: {}", e))?;

                // `enclosed_name` rejects absolute paths and `..` traversal.
                let Some(relative) = file.enclosed_name() else {
                    return Err(format!(
                        "Refusing to extract zip entry with unsafe path: {}",
                        file.name()
                    ));
                };
                if !is_safe_relative_path(relative) {
                    return Err(format!(
                        "Refusing to extract zip entry with unsafe path: {}",
                        file.name()
                    ));
                }
                let outpath = dest.join(relative);

                if file.name().ends_with('/') {
                    std::fs::create_dir_all(&outpath)
                        .map_err(|e| format!("Failed to create directory: {}", e))?;
                    continue;
                }

                if let Some(p) = outpath.parent() {
                    if !p.exists() {
                        std::fs::create_dir_all(p)
                            .map_err(|e| format!("Failed to create directory: {}", e))?;
                    }
                }
                let mut outfile = std::fs::File::create(&outpath)
                    .map_err(|e| format!("Failed to create file: {}", e))?;
                std::io::copy(&mut file, &mut outfile)
                    .map_err(|e| format!("Failed to write file: {}", e))?;

                // Zip carries the executable bit; without it the extracted
                // launcher cannot be started on Unix.
                #[cfg(unix)]
                {
                    use std::os::unix::fs::PermissionsExt;
                    if let Some(mode) = file.unix_mode() {
                        let _ = std::fs::set_permissions(
                            &outpath,
                            std::fs::Permissions::from_mode(mode),
                        );
                    }
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("Zip extraction task failed: {}", e))?
    }

    async fn extract_tar_gz(&self, archive: &Path, dest: &Path) -> Result<(), String> {
        let archive = archive.to_path_buf();
        let dest = dest.to_path_buf();

        tokio::task::spawn_blocking(move || {
            let file = std::fs::File::open(&archive)
                .map_err(|e| format!("Failed to open archive: {}", e))?;
            let decoder = flate2::read::GzDecoder::new(file);
            let mut archive = tar::Archive::new(decoder);

            let entries = archive
                .entries()
                .map_err(|e| format!("Failed to read tar entries: {}", e))?;

            for entry in entries {
                let mut entry = entry.map_err(|e| format!("Failed to read tar entry: {}", e))?;
                let path = entry
                    .path()
                    .map_err(|e| format!("Failed to read tar entry path: {}", e))?
                    .into_owned();

                // `unpack_in` refuses to write outside `dest` and returns false
                // rather than erroring when an entry tries to escape.
                let unpacked = entry
                    .unpack_in(&dest)
                    .map_err(|e| format!("Failed to extract {}: {}", path.display(), e))?;
                if !unpacked {
                    return Err(format!(
                        "Refusing to extract tar entry with unsafe path: {}",
                        path.display()
                    ));
                }
            }
            Ok(())
        })
        .await
        .map_err(|e| format!("Tar extraction task failed: {}", e))?
    }

    pub fn find_launcher_in_dir(&self, dir: &Path) -> Result<PathBuf, String> {
        let launcher_name = self.launcher_binary_name();
        let entries =
            std::fs::read_dir(dir).map_err(|e| format!("Failed to read directory: {}", e))?;

        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                if let Ok(found) = self.find_launcher_in_dir(&path) {
                    return Ok(found);
                }
            } else {
                let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
                if name == launcher_name {
                    return Ok(path);
                }
            }
        }

        Err(format!(
            "Launcher binary '{}' not found in archive",
            launcher_name
        ))
    }

    /// Turn a verified payload into something the launcher can apply on the
    /// next start: either a staged launcher binary or a staged installer.
    pub async fn prepare_update(&self, archive_path: &Path, tag: &str) -> Result<PathBuf, String> {
        validate_update_tag(tag)?;
        let format = sniff_payload_format(archive_path)?;

        if format == PayloadFormat::TextBody || format == PayloadFormat::Unknown {
            return Err(format!(
                "Downloaded update is not a valid release payload (detected {}). \
                 The release may be broken or the CDN returned an error page.",
                format.describe()
            ));
        }

        if format.is_package() {
            return self.stage_package(archive_path, tag, format);
        }

        let extract_dir = self.extract_update(archive_path, tag).await?;
        let launcher_path = self.find_launcher_in_dir(&extract_dir)?;

        let update_dir = self.update_dir()?;
        let staged_dir = update_dir.join(tag);
        std::fs::create_dir_all(&staged_dir)
            .map_err(|e| format!("Failed to create staging directory: {}", e))?;
        let staged_launcher = staged_dir.join(self.launcher_binary_name());

        if launcher_path != staged_launcher {
            if staged_launcher.exists() {
                std::fs::remove_file(&staged_launcher)
                    .map_err(|e| format!("Failed to remove existing staged launcher: {}", e))?;
            }
            std::fs::copy(&launcher_path, &staged_launcher)
                .map_err(|e| format!("Failed to copy launcher to staging: {}", e))?;
        }

        #[cfg(windows)]
        copy_windows_runtime_files(&launcher_path, &staged_dir)?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            if let Ok(mut perms) = std::fs::metadata(&staged_launcher).map(|m| m.permissions()) {
                let mut mode = perms.mode();
                mode |= 0o111;
                perms.set_mode(mode);
                std::fs::set_permissions(&staged_launcher, perms)
                    .map_err(|e| format!("Failed to set executable permissions: {}", e))?;
            }
        }

        Ok(staged_launcher)
    }

    /// Stage an installer/package payload.
    ///
    /// Only the Windows launcher can run one of these unattended, so this is
    /// defined per platform rather than as one function with `cfg` blocks —
    /// a `cfg`'d block that ends the function body reads as a needless `return`
    /// on the platforms where the rest is compiled out.
    #[cfg(not(target_os = "windows"))]
    fn stage_package(
        &self,
        payload: &Path,
        tag: &str,
        format: PayloadFormat,
    ) -> Result<PathBuf, String> {
        Err(format!(
            "Release {} ships a {} ({}) for your platform, which cannot be applied \
             automatically. Install it manually: {}",
            tag,
            format.describe(),
            payload
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            payload.display()
        ))
    }

    #[cfg(target_os = "windows")]
    fn stage_package(
        &self,
        payload: &Path,
        tag: &str,
        format: PayloadFormat,
    ) -> Result<PathBuf, String> {
        if format != PayloadFormat::WindowsExecutable {
            return Err(format!(
                "Downloaded update is a {}, which cannot be installed on Windows",
                format.describe()
            ));
        }

        let update_dir = self.update_dir()?;
        let staged_dir = update_dir.join(tag);
        std::fs::create_dir_all(&staged_dir)
            .map_err(|e| format!("Failed to create installer staging directory: {}", e))?;
        let staged_installer =
            staged_dir.join(payload.file_name().ok_or("Installer filename missing")?);
        if payload != staged_installer {
            std::fs::copy(payload, &staged_installer)
                .map_err(|e| format!("Failed to stage installer: {}", e))?;
        }
        Ok(staged_installer)
    }

    /// Arm a launcher update and return the exact marker that was persisted.
    /// A transaction ID is generated at the arming boundary so every attempt
    /// is distinguishable even when two downloads stage the same release tag.
    pub fn mark_update_pending(
        &self,
        tag: &str,
        staged_path: &Path,
        armed: bool,
        phase: Option<&str>,
    ) -> Result<PendingUpdate, String> {
        validate_update_tag(tag)?;
        // Never arm an update whose staged payload has gone missing or escaped
        // its release directory — the launcher must execute only coordinator
        // staged files.
        self.validate_staged_path(tag, staged_path)?;

        let pending_file = self.pending_update_file()?;
        let pending = PendingUpdate {
            tag: tag.to_string(),
            staged_path: staged_path.to_path_buf(),
            kind: self.pending_update_kind_for_path(staged_path),
            armed,
            created_at: Utc::now(),
            transaction_id: armed.then(|| uuid::Uuid::new_v4().to_string()),
            phase: phase.map(str::to_string),
            error: None,
        };

        let json = serde_json::to_string_pretty(&pending)
            .map_err(|e| format!("Failed to serialize pending update: {}", e))?;
        write_atomic(&pending_file, &json)?;
        Ok(pending)
    }

    pub fn mark_prepared_update(&self, tag: &str, staged_path: &Path) -> Result<(), String> {
        validate_update_tag(tag)?;
        self.validate_staged_path(tag, staged_path)?;
        let prepared_file = self.prepared_update_file()?;

        let prepared = PendingUpdate {
            tag: tag.to_string(),
            staged_path: staged_path.to_path_buf(),
            kind: self.pending_update_kind_for_path(staged_path),
            armed: false,
            created_at: Utc::now(),
            transaction_id: None,
            phase: Some("ready".to_string()),
            error: None,
        };

        let json = serde_json::to_string_pretty(&prepared)
            .map_err(|e| format!("Failed to serialize prepared update: {}", e))?;

        write_atomic(&prepared_file, &json)
    }

    pub fn clear_pending_update(&self) -> Result<(), String> {
        let pending_file = self.pending_update_file()?;
        if pending_file.exists() {
            std::fs::remove_file(&pending_file)
                .map_err(|e| format!("Failed to remove pending update file: {}", e))?;
        }
        Ok(())
    }

    pub fn clear_prepared_update(&self) -> Result<(), String> {
        let prepared_file = self.prepared_update_file()?;
        if prepared_file.exists() {
            std::fs::remove_file(&prepared_file)
                .map_err(|e| format!("Failed to remove prepared update file: {}", e))?;
        }
        Ok(())
    }

    pub fn cleanup_update_files(&self, tag: &str) -> Result<(), String> {
        validate_update_tag(tag)?;
        let update_dir = self.update_dir()?;
        let tag_dir = update_dir.join(tag);
        if tag_dir.exists() {
            std::fs::remove_dir_all(&tag_dir)
                .map_err(|e| format!("Failed to remove update directory: {}", e))?;
        }
        Ok(())
    }

    /// Drop staged files for every tag except the one still in use, so a failed
    /// update does not leave the disk filling up with dead payloads.
    pub fn cleanup_stale_updates(&self, keep_tag: Option<&str>) -> Result<(), String> {
        if let Some(tag) = keep_tag {
            validate_update_tag(tag)?;
        }
        let update_dir = self.update_dir()?;
        if !update_dir.exists() {
            return Ok(());
        }

        for entry in std::fs::read_dir(&update_dir)
            .map_err(|e| format!("Failed to read update directory: {}", e))?
            .flatten()
        {
            let path = entry.path();
            if !path.is_dir() {
                continue;
            }
            let name = entry.file_name().to_string_lossy().to_string();
            if Some(name.as_str()) == keep_tag {
                continue;
            }
            let _ = std::fs::remove_dir_all(&path);
        }

        Ok(())
    }
}

#[cfg(windows)]
fn replace_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    use std::os::windows::ffi::OsStrExt;

    const MOVEFILE_REPLACE_EXISTING: u32 = 0x00000001;
    const MOVEFILE_WRITE_THROUGH: u32 = 0x00000008;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    let source = wide(source);
    let destination = wide(destination);
    let result = unsafe {
        extern "system" {
            fn MoveFileExW(
                existing_file_name: *const u16,
                new_file_name: *const u16,
                flags: u32,
            ) -> i32;
        }
        MoveFileExW(
            source.as_ptr(),
            destination.as_ptr(),
            MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
        )
    };
    if result == 0 {
        return Err(std::io::Error::last_os_error().to_string());
    }
    Ok(())
}

#[cfg(not(windows))]
fn replace_file_atomic(source: &Path, destination: &Path) -> Result<(), String> {
    std::fs::rename(source, destination).map_err(|error| error.to_string())
}

/// Write via a unique, same-directory temp file and flush it before rename so
/// concurrent writers cannot corrupt each other and a crash cannot leave the
/// launcher parsing a truncated marker/state file.
pub(crate) fn write_atomic(path: &Path, contents: &str) -> Result<(), String> {
    use std::io::Write;
    #[cfg(unix)]
    use std::os::unix::fs::OpenOptionsExt;

    let parent = path
        .parent()
        .ok_or_else(|| format!("Atomic write path has no parent: {}", path.display()))?;
    std::fs::create_dir_all(parent)
        .map_err(|error| format!("Failed to create {}: {}", parent.display(), error))?;

    let file_name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("update");
    let temp_path = parent.join(format!(".{}.{}.tmp", file_name, uuid::Uuid::new_v4()));

    let write_result = (|| -> Result<(), String> {
        let mut options = std::fs::OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options
            .open(&temp_path)
            .map_err(|error| format!("Failed to create {}: {}", temp_path.display(), error))?;
        file.write_all(contents.as_bytes())
            .map_err(|error| format!("Failed to write {}: {}", temp_path.display(), error))?;
        file.sync_all()
            .map_err(|error| format!("Failed to sync {}: {}", temp_path.display(), error))?;
        drop(file);

        replace_file_atomic(&temp_path, path).map_err(|error| {
            format!(
                "Failed to finalize {} from {}: {}",
                path.display(),
                temp_path.display(),
                error
            )
        })?;

        // Persist the directory entry as well on Unix. Opening directories is
        // not portable to Windows, where rename already provides the durable
        // replacement guarantee we need here.
        #[cfg(unix)]
        {
            if let Ok(directory) = std::fs::File::open(parent) {
                let _ = directory.sync_all();
            }
        }
        Ok(())
    })();

    if write_result.is_err() {
        let _ = std::fs::remove_file(&temp_path);
    }
    write_result
}

impl Default for UpdateInstaller {
    fn default() -> Self {
        Self::new()
    }
}

pub fn get_pending_update() -> Option<PendingUpdate> {
    let base = dirs_next::home_dir()?;
    let pending_file = base.join(".osagent").join("pending_update.json");
    if !pending_file.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&pending_file).ok()?;
    serde_json::from_str(&json).ok()
}

pub fn get_prepared_update() -> Option<PendingUpdate> {
    let base = dirs_next::home_dir()?;
    let prepared_file = base.join(".osagent").join("prepared_update.json");
    if !prepared_file.exists() {
        return None;
    }
    let json = std::fs::read_to_string(&prepared_file).ok()?;
    serde_json::from_str(&json).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    fn temp_file(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("osagent-update-test-{}", name));
        let mut file = std::fs::File::create(&path).unwrap();
        file.write_all(bytes).unwrap();
        path
    }

    #[test]
    fn atomic_write_replaces_existing_state() {
        let root = std::env::temp_dir().join(format!("osagent-atomic-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&root).unwrap();
        let path = root.join("state.json");
        write_atomic(&path, "{\"value\":1}").unwrap();
        write_atomic(&path, "{\"value\":2}").unwrap();
        let value = std::fs::read_to_string(&path).unwrap();
        assert!(value.contains("\"value\": 2") || value.contains("\"value\":2"));
        let _ = std::fs::remove_dir_all(root);
    }

    #[test]
    fn sniffs_gzip() {
        let path = temp_file("gzip", &[0x1f, 0x8b, 0x08, 0x00, 0x00]);
        assert_eq!(sniff_payload_format(&path).unwrap(), PayloadFormat::TarGz);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sniffs_deb_named_as_tar_gz() {
        // The exact failure mode this module regressed on: a Debian package
        // downloaded under a `.tar.gz` filename.
        let path = temp_file("deb.tar.gz", b"!<arch>\ndebian-binary   ");
        assert_eq!(
            sniff_payload_format(&path).unwrap(),
            PayloadFormat::DebPackage
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sniffs_error_page() {
        let path = temp_file("html", b"<!DOCTYPE html><html><body>404</body></html>");
        assert_eq!(
            sniff_payload_format(&path).unwrap(),
            PayloadFormat::TextBody
        );
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn sniffs_zip_and_exe() {
        let zip = temp_file("zip", b"PK\x03\x04rest");
        assert_eq!(sniff_payload_format(&zip).unwrap(), PayloadFormat::Zip);
        let _ = std::fs::remove_file(&zip);

        let exe = temp_file("exe", b"MZ\x90\x00");
        assert_eq!(
            sniff_payload_format(&exe).unwrap(),
            PayloadFormat::WindowsExecutable
        );
        let _ = std::fs::remove_file(&exe);
    }

    #[test]
    fn derives_file_name_from_url() {
        assert_eq!(
            file_name_from_url("https://cdn.example/releases/v1/osagent-linux-x86_64.deb").unwrap(),
            "osagent-linux-x86_64.deb"
        );
        assert_eq!(
            file_name_from_url("https://cdn.example/a/b/app.tar.gz?token=1#f").unwrap(),
            "app.tar.gz"
        );
        assert!(file_name_from_url("https://cdn.example/releases/").is_none());
    }

    #[test]
    fn rejects_unsafe_archive_paths() {
        assert!(is_safe_relative_path(Path::new("bin/osagent-launcher")));
        assert!(!is_safe_relative_path(Path::new("../escape")));
        assert!(!is_safe_relative_path(Path::new("/etc/passwd")));
    }

    #[test]
    fn legacy_marker_json_remains_compatible() {
        let marker: PendingUpdate = serde_json::from_str(
            r#"{
                "tag":"v1.2.3",
                "staged_path":"C:\\updates\\v1.2.3\\osagent-launcher.exe",
                "kind":"binary_swap",
                "armed":false,
                "created_at":"2026-01-01T00:00:00Z"
            }"#,
        )
        .unwrap();
        assert_eq!(marker.tag, "v1.2.3");
        assert!(!marker.armed);
        assert!(marker.transaction_id.is_none());
        assert!(marker.phase.is_none());
    }

    #[test]
    fn rejects_unsafe_release_tags() {
        assert!(validate_update_tag("v1.2.3-beta.1+build.7").is_ok());
        assert!(validate_update_tag("../escape").is_err());
        assert!(validate_update_tag("v1/beta").is_err());
        assert!(validate_update_tag("v1\\beta").is_err());
        assert!(validate_update_tag("v1:beta").is_err());
        assert!(validate_update_tag("v1.").is_err());
        assert!(validate_update_tag("CON").is_err());
        assert!(validate_update_tag("").is_err());
    }

    #[test]
    fn hashes_known_content() {
        let path = temp_file("hash", b"abc");
        assert_eq!(
            hash_file(&path).unwrap(),
            "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
        );
        let _ = std::fs::remove_file(&path);
    }

    /// The manifest shape produced by upload-to-r2.sh.
    const NEW_MANIFEST: &str = r#"{
        "tag": "v0.4.0",
        "version": "0.4.0",
        "assets": {
            "linux-x86_64": {
                "archive": "osagent-linux-x86_64.tar.gz",
                "url": "https://cdn/releases/v0.4.0/osagent-linux-x86_64.tar.gz",
                "installer": "https://cdn/releases/v0.4.0/osagent-linux-x86_64.deb"
            },
            "macos-arm64": {
                "archive": "osagent-macos-arm64.tar.gz",
                "url": "https://cdn/releases/v0.4.0/osagent-macos-arm64.tar.gz",
                "installer": "https://cdn/releases/v0.4.0/osagent-macos-arm64.dmg"
            },
            "windows-x86_64": {
                "installer": "https://cdn/releases/v0.4.0/osagent-windows-x86_64-setup.exe"
            }
        },
        "sha256": {
            "linux-x86_64": "aaa",
            "linux-x86_64-installer": "bbb",
            "macos-arm64": "ccc",
            "macos-arm64-installer": "ddd",
            "windows-x86_64-installer": "eee"
        }
    }"#;

    /// The v0.3.0 manifest, where `url` pointed at a package.
    const OLD_MANIFEST: &str = r#"{
        "tag": "v0.3.0",
        "version": "0.3.0",
        "assets": {
            "linux-x86_64": {
                "archive": "osagent-linux-x86_64.deb",
                "url": "https://cdn/releases/v0.3.0/osagent-linux-x86_64.deb"
            }
        },
        "sha256": { "linux-x86_64": "aaa" }
    }"#;

    fn parse(json: &str) -> CdnManifest {
        serde_json::from_str(json).unwrap()
    }

    #[test]
    fn selects_archive_on_unix_platforms() {
        let manifest = parse(NEW_MANIFEST);

        let linux = select_asset(&manifest, "linux-x86_64", false, "fallback.tar.gz");
        assert_eq!(linux.file_name, "osagent-linux-x86_64.tar.gz");
        assert!(!linux.is_installer);
        assert_eq!(linux.sha256.as_deref(), Some("aaa"));

        let macos = select_asset(&manifest, "macos-arm64", false, "fallback.tar.gz");
        assert_eq!(macos.file_name, "osagent-macos-arm64.tar.gz");
        assert!(!macos.is_installer);
        assert_eq!(macos.sha256.as_deref(), Some("ccc"));
    }

    #[test]
    fn selects_installer_on_windows() {
        let manifest = parse(NEW_MANIFEST);
        let windows = select_asset(&manifest, "windows-x86_64", true, "fallback.zip");
        assert_eq!(windows.file_name, "osagent-windows-x86_64-setup.exe");
        assert!(windows.is_installer);
        assert_eq!(windows.sha256.as_deref(), Some("eee"));
    }

    #[test]
    fn file_name_tracks_the_payload_not_the_platform_guess() {
        // Against the old manifest the client must still name the file after
        // the .deb it is really fetching, so the format check reports the real
        // problem instead of a bogus gzip error.
        let manifest = parse(OLD_MANIFEST);
        let linux = select_asset(
            &manifest,
            "linux-x86_64",
            false,
            "osagent-linux-x86_64.tar.gz",
        );
        assert_eq!(linux.file_name, "osagent-linux-x86_64.deb");
    }

    #[test]
    fn falls_back_when_platform_is_absent() {
        let manifest = parse(NEW_MANIFEST);
        let unknown = select_asset(&manifest, "freebsd-x86_64", false, "osagent-unknown.tar.gz");
        assert_eq!(unknown.file_name, "osagent-unknown.tar.gz");
        assert!(unknown
            .url
            .ends_with("/releases/v0.4.0/osagent-unknown.tar.gz"));
        assert!(unknown.sha256.is_none());
    }

    #[test]
    fn uses_installer_when_no_archive_is_published() {
        let manifest = parse(NEW_MANIFEST);
        // Not preferring an installer, but Windows publishes only one.
        let windows = select_asset(&manifest, "windows-x86_64", false, "fallback.zip");
        assert!(windows.is_installer);
        assert_eq!(windows.file_name, "osagent-windows-x86_64-setup.exe");
    }

    #[test]
    fn package_extensions_map_to_installer_kind() {
        let installer = UpdateInstaller::new();
        assert!(matches!(
            installer.pending_update_kind_for_path(Path::new("setup.exe")),
            PendingUpdateKind::Installer
        ));
        assert!(matches!(
            installer.pending_update_kind_for_path(Path::new("osagent.deb")),
            PendingUpdateKind::Installer
        ));
        assert!(matches!(
            installer.pending_update_kind_for_path(Path::new("osagent-launcher")),
            PendingUpdateKind::BinarySwap
        ));
    }
}
