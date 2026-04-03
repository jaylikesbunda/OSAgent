use crate::update::channel::UpdateChannel;
use chrono::{DateTime, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use tokio::fs::{self, File};
use tokio::io::AsyncWriteExt;
use tokio::time::Duration;

const CDN_BASE_URL: &str = "https://2c8b11c572ea0e7bbc6ac6f5a87d81c8.r2.cloudflarestorage.com/osagent-releases";

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
pub struct PendingUpdate {
    pub tag: String,
    pub launcher_path: PathBuf,
    pub created_at: DateTime<Utc>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdnManifest {
    tag: String,
    version: String,
    #[serde(default)]
    assets: std::collections::HashMap<String, CdnAssetEntry>,
    #[serde(default)]
    sha256: std::collections::HashMap<String, String>,
}

#[derive(Debug, Clone, Deserialize)]
struct CdnAssetEntry {
    archive: String,
    url: String,
}

pub struct UpdateInstaller {
    client: Client,
}

impl UpdateInstaller {
    pub fn new() -> Self {
        let client = Client::builder()
            .timeout(Duration::from_secs(300))
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
        _channel: UpdateChannel,
    ) -> Result<Option<(String, String, String)>, String> {
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

        let platform_key = self.detect_platform();
        let archive_name = self.platform_archive_name();

        let download_url = if let Some(asset) = manifest.assets.get(platform_key) {
            if !asset.url.is_empty() {
                asset.url.clone()
            } else {
                format!("{CDN_BASE_URL}/releases/{}/{}", manifest.tag, archive_name)
            }
        } else {
            format!("{CDN_BASE_URL}/releases/{}/{}", manifest.tag, archive_name)
        };

        Ok(Some((manifest.tag.clone(), archive_name, download_url)))
    }

    pub fn update_dir(&self) -> Result<PathBuf, String> {
        let base = dirs_next::home_dir().ok_or("Could not find home directory")?;
        Ok(base.join(".osagent").join("updates"))
    }

    pub fn pending_update_file(&self) -> Result<PathBuf, String> {
        let base = dirs_next::home_dir().ok_or("Could not find home directory")?;
        Ok(base.join(".osagent").join("pending_update.json"))
    }

    pub async fn download_release<F>(
        &self,
        download_url: &str,
        tag: &str,
        archive_name: &str,
        progress_callback: F,
    ) -> Result<PathBuf, String>
    where
        F: Fn(u64, u64) + Send + 'static,
    {
        let update_dir = self.update_dir()?;
        fs::create_dir_all(&update_dir)
            .await
            .map_err(|e| format!("Failed to create update directory: {}", e))?;

        let dest_path = update_dir.join(tag).join(archive_name);

        if let Some(parent) = dest_path.parent() {
            fs::create_dir_all(parent)
                .await
                .map_err(|e| format!("Failed to create update subdirectory: {}", e))?;
        }

        let response = self
            .client
            .get(download_url)
            .send()
            .await
            .map_err(|e| format!("Download request failed: {}", e))?;

        if !response.status().is_success() {
            return Err(format!("Download failed with HTTP {}", response.status()));
        }

        let total_bytes = response.content_length().unwrap_or(0);
        let bytes_clone = Arc::new(AtomicU64::new(0));
        let total = total_bytes;

        let mut file = File::create(&dest_path)
            .await
            .map_err(|e| format!("Failed to create file: {}", e))?;

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
                    let current = bytes_clone.fetch_add(data.len() as u64, Ordering::Relaxed)
                        + data.len() as u64;
                    if total > 0 {
                        progress_callback(current, total);
                    }
                }
                None => break,
            }
        }

        file.flush()
            .await
            .map_err(|e| format!("Flush error: {}", e))?;

        Ok(dest_path)
    }

    pub async fn extract_update(&self, archive_path: &Path, tag: &str) -> Result<PathBuf, String> {
        let update_dir = self.update_dir()?;
        let extract_dir = update_dir.join(tag);

        fs::create_dir_all(&extract_dir)
            .await
            .map_err(|e| format!("Failed to create extraction directory: {}", e))?;

        let is_zip = archive_path
            .extension()
            .map(|e| e == "zip")
            .unwrap_or(false);

        if is_zip {
            self.extract_zip(archive_path, &extract_dir).await?;
        } else {
            self.extract_tar_gz(archive_path, &extract_dir).await?;
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
                let outpath = dest.join(file.mangled_name());

                if file.name().ends_with('/') {
                    std::fs::create_dir_all(&outpath)
                        .map_err(|e| format!("Failed to create directory: {}", e))?;
                } else {
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

            archive
                .unpack(&dest)
                .map_err(|e| format!("Failed to extract tar.gz: {}", e))?;
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

    pub async fn prepare_update(&self, archive_path: &Path, tag: &str) -> Result<PathBuf, String> {
        let extract_dir = self.extract_update(archive_path, tag).await?;

        let launcher_path = self.find_launcher_in_dir(&extract_dir)?;

        let update_dir = self.update_dir()?;
        let staged_dir = update_dir.join(tag);
        let staged_launcher = staged_dir.join(self.launcher_binary_name());

        if launcher_path != staged_launcher {
            if staged_launcher.exists() {
                std::fs::remove_file(&staged_launcher)
                    .map_err(|e| format!("Failed to remove existing staged launcher: {}", e))?;
            }
            std::fs::copy(&launcher_path, &staged_launcher)
                .map_err(|e| format!("Failed to copy launcher to staging: {}", e))?;
        }

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

    pub fn mark_update_pending(&self, tag: &str, launcher_path: &Path) -> Result<(), String> {
        let pending_file = self.pending_update_file()?;

        if let Some(parent) = pending_file.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Failed to create pending update directory: {}", e))?;
        }

        let pending = PendingUpdate {
            tag: tag.to_string(),
            launcher_path: launcher_path.to_path_buf(),
            created_at: Utc::now(),
        };

        let json = serde_json::to_string_pretty(&pending)
            .map_err(|e| format!("Failed to serialize pending update: {}", e))?;

        std::fs::write(&pending_file, json)
            .map_err(|e| format!("Failed to write pending update file: {}", e))?;

        Ok(())
    }

    pub fn clear_pending_update(&self) -> Result<(), String> {
        let pending_file = self.pending_update_file()?;
        if pending_file.exists() {
            std::fs::remove_file(&pending_file)
                .map_err(|e| format!("Failed to remove pending update file: {}", e))?;
        }
        Ok(())
    }

    pub fn cleanup_update_files(&self, tag: &str) -> Result<(), String> {
        let update_dir = self.update_dir()?;
        let tag_dir = update_dir.join(tag);
        if tag_dir.exists() {
            std::fs::remove_dir_all(&tag_dir)
                .map_err(|e| format!("Failed to remove update directory: {}", e))?;
        }
        Ok(())
    }
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
