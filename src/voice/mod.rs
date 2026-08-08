pub mod piper;
pub mod whisper;
pub mod whisper_server;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use tokio::sync::broadcast;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceStatus {
    pub whisper_installed: bool,
    pub whisper_model: Option<String>,
    pub piper_installed: bool,
    pub piper_voice: Option<String>,
    pub models_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstallProgress {
    pub stage: String,
    pub progress: f32,
    pub message: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub model_type: String,
    pub name: String,
    pub size_mb: u64,
    pub lang: Option<String>,
    pub quality: Option<String>,
    pub url: String,
    pub installed: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledModel {
    pub id: String,
    pub model_type: String,
    pub name: String,
    pub path: String,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadProgress {
    pub model_id: String,
    pub model_type: String,
    pub stage: String,
    pub progress: f32,
    pub bytes_downloaded: u64,
    pub total_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadRequest {
    pub model_type: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VoiceModelsResponse {
    pub whisper: Vec<ModelInfo>,
    pub piper: Vec<ModelInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct InstalledModelsResponse {
    pub whisper: Vec<InstalledModel>,
    pub piper: Vec<InstalledModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeleteModelRequest {
    pub model_type: String,
    pub model_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct UploadResponse {
    pub success: bool,
    pub message: String,
    pub model: Option<InstalledModel>,
}

lazy_static::lazy_static! {
    static ref PROGRESS_TX: broadcast::Sender<DownloadProgress> = {
        // Headroom so a briefly slow SSE client drops frames rather than
        // falling far enough behind to matter.
        let (tx, _) = broadcast::channel(1024);
        tx
    };
}

pub fn get_progress_receiver() -> broadcast::Receiver<DownloadProgress> {
    PROGRESS_TX.subscribe()
}

pub fn broadcast_progress(progress: DownloadProgress) {
    let _ = PROGRESS_TX.send(progress);
}

/// Rate-limit progress frames to whole-percent changes. A multi-hundred-MB
/// download otherwise emits thousands of events, far faster than an SSE client
/// drains them. Always emits when the total size is unknown so such downloads
/// still show activity.
pub fn should_emit_progress(last_emitted_pct: &mut i64, downloaded: u64, total: u64) -> bool {
    if total == 0 {
        return true;
    }
    let pct = (downloaded.saturating_mul(100) / total) as i64;
    if pct != *last_emitted_pct {
        *last_emitted_pct = pct;
        return true;
    }
    false
}

/// Verify a finished download actually received everything the server promised.
/// A truncated archive is the most common cause of a "successful" install that
/// produced nothing, so this is checked before anything tries to extract it.
pub fn verify_download_complete(
    path: &std::path::Path,
    expected_bytes: u64,
    what: &str,
) -> Result<(), String> {
    let actual = std::fs::metadata(path)
        .map_err(|e| format!("Could not inspect downloaded {}: {}", what, e))?
        .len();

    if actual == 0 {
        return Err(format!("Downloaded {} is empty", what));
    }

    if expected_bytes > 0 && actual != expected_bytes {
        return Err(format!(
            "Downloaded {} is incomplete: got {} bytes, expected {}. Check your connection and retry.",
            what, actual, expected_bytes
        ));
    }

    Ok(())
}

/// Extract a zip in-process, reporting progress per entry.
///
/// This deliberately does not shell out to PowerShell's `Expand-Archive`: its
/// errors are non-terminating, so `powershell -Command` exits 0 even when
/// extraction fails outright, which silently produced empty installs.
pub fn extract_zip_with_progress(
    archive: &std::path::Path,
    dest: &std::path::Path,
    model_id: &str,
    model_type: &str,
) -> Result<(), String> {
    let file = std::fs::File::open(archive)
        .map_err(|e| format!("Could not open downloaded archive: {}", e))?;
    let mut zip = zip::ZipArchive::new(file).map_err(|e| {
        format!(
            "Downloaded archive is not a readable zip ({}). The download was likely incomplete or corrupted.",
            e
        )
    })?;

    std::fs::create_dir_all(dest)
        .map_err(|e| format!("Could not create extraction directory: {}", e))?;

    let total = zip.len();
    for index in 0..total {
        let mut entry = zip
            .by_index(index)
            .map_err(|e| format!("Could not read archive entry {}: {}", index, e))?;

        // enclosed_name() rejects absolute paths and `..` traversal.
        let Some(relative) = entry.enclosed_name() else {
            return Err(format!("Archive contains an unsafe path: {}", entry.name()));
        };
        let out_path = dest.join(relative);

        if entry.is_dir() {
            std::fs::create_dir_all(&out_path)
                .map_err(|e| format!("Could not create {}: {}", out_path.display(), e))?;
        } else {
            if let Some(parent) = out_path.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| format!("Could not create {}: {}", parent.display(), e))?;
            }
            let mut out_file = std::fs::File::create(&out_path)
                .map_err(|e| format!("Could not write {}: {}", out_path.display(), e))?;
            std::io::copy(&mut entry, &mut out_file)
                .map_err(|e| format!("Could not extract {}: {}", out_path.display(), e))?;
        }

        broadcast_progress(DownloadProgress {
            model_id: model_id.to_string(),
            model_type: model_type.to_string(),
            stage: format!("extracting ({}/{})", index + 1, total),
            progress: (index + 1) as f32 / total.max(1) as f32,
            bytes_downloaded: 0,
            total_bytes: 0,
        });
    }

    Ok(())
}

/// Locate a file anywhere in an extracted tree, matching case-insensitively.
pub fn find_file_recursive(dir: &std::path::Path, file_name: &str) -> Option<PathBuf> {
    for entry in std::fs::read_dir(dir).ok()?.flatten() {
        let path = entry.path();
        if path.is_dir() {
            if let Some(found) = find_file_recursive(&path, file_name) {
                return Some(found);
            }
        } else if entry
            .file_name()
            .to_string_lossy()
            .eq_ignore_ascii_case(file_name)
        {
            return Some(path);
        }
    }
    None
}

pub fn get_models_dir() -> PathBuf {
    let base = shellexpand::tilde("~/.osagent/voice");
    PathBuf::from(base.to_string())
}

pub fn ensure_models_dir() -> std::io::Result<PathBuf> {
    let dir = get_models_dir();
    std::fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn get_status() -> VoiceStatus {
    let whisper_status = whisper::get_status();
    let piper_status = piper::get_status();

    VoiceStatus {
        whisper_installed: whisper_status.binary_installed,
        whisper_model: whisper_status.model_name,
        piper_installed: piper_status.binary_installed,
        piper_voice: piper_status.voice_name,
        models_dir: get_models_dir().to_string_lossy().to_string(),
    }
}

pub fn get_available_models() -> VoiceModelsResponse {
    VoiceModelsResponse {
        whisper: whisper::get_available_models(),
        piper: piper::get_available_voices_all(),
    }
}

pub fn get_installed_models() -> InstalledModelsResponse {
    InstalledModelsResponse {
        whisper: whisper::find_installed_models(),
        piper: piper::find_installed_voices(),
    }
}

#[cfg(test)]
mod install_robustness_tests {
    use super::*;
    use std::io::Write;

    fn temp_dir(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("osa_voice_test_{}", name));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    fn make_zip(path: &std::path::Path) {
        let file = std::fs::File::create(path).unwrap();
        let mut zip = zip::ZipWriter::new(file);
        let opts: zip::write::FileOptions = Default::default();
        zip.add_directory("Release/", opts).unwrap();
        zip.start_file("Release/whisper-cli.exe", opts).unwrap();
        zip.write_all(b"binary").unwrap();
        zip.start_file("Release/whisper.dll", opts).unwrap();
        zip.write_all(b"dll").unwrap();
        zip.finish().unwrap();
    }

    #[test]
    fn extracts_nested_archive_and_finds_binary() {
        let dir = temp_dir("extract_ok");
        let archive = dir.join("a.zip");
        make_zip(&archive);
        let dest = dir.join("out");

        extract_zip_with_progress(&archive, &dest, "whisper-binary", "whisper").unwrap();

        assert!(dest.join("Release/whisper-cli.exe").exists());
        let found = find_file_recursive(&dest, "whisper-cli.exe");
        assert!(found.is_some(), "nested binary should be found");
    }

    #[test]
    fn corrupt_archive_reports_error_instead_of_succeeding() {
        let dir = temp_dir("extract_bad");
        let archive = dir.join("bad.zip");
        std::fs::File::create(&archive)
            .unwrap()
            .write_all(b"not a zip at all")
            .unwrap();

        let result =
            extract_zip_with_progress(&archive, &dir.join("out"), "whisper-binary", "whisper");
        assert!(result.is_err(), "corrupt archive must not report success");
    }

    #[test]
    fn truncated_download_is_rejected() {
        let dir = temp_dir("verify");
        let file = dir.join("part.bin");
        std::fs::File::create(&file)
            .unwrap()
            .write_all(b"12345")
            .unwrap();

        assert!(verify_download_complete(&file, 5, "test").is_ok());
        assert!(verify_download_complete(&file, 999, "test").is_err());

        let empty = dir.join("empty.bin");
        std::fs::File::create(&empty).unwrap();
        assert!(verify_download_complete(&empty, 0, "test").is_err());
    }
}
