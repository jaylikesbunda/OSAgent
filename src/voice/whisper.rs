use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tracing::info;

use super::{broadcast_progress, get_models_dir, InstalledModel, ModelInfo};

const WHISPER_CPP_VERSION: &str = "1.8.3";

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperStatus {
    pub binary_installed: bool,
    pub model_name: Option<String>,
    pub model_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum WhisperModel {
    Tiny,
    Base,
    Small,
    Medium,
}

impl WhisperModel {
    pub fn id(&self) -> &'static str {
        match self {
            WhisperModel::Tiny => "tiny",
            WhisperModel::Base => "base",
            WhisperModel::Small => "small",
            WhisperModel::Medium => "medium",
        }
    }

    pub fn size_mb(&self) -> u64 {
        match self {
            WhisperModel::Tiny => 75,
            WhisperModel::Base => 142,
            WhisperModel::Small => 466,
            WhisperModel::Medium => 1500,
        }
    }

    pub fn from_str(s: &str) -> Option<Self> {
        match s.to_lowercase().as_str() {
            "tiny" => Some(WhisperModel::Tiny),
            "base" => Some(WhisperModel::Base),
            "small" => Some(WhisperModel::Small),
            "medium" => Some(WhisperModel::Medium),
            _ => None,
        }
    }
}

fn get_binary_path() -> PathBuf {
    let dir = get_models_dir();
    #[cfg(target_os = "windows")]
    {
        dir.join("whisper.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        dir.join("whisper")
    }
}

fn get_model_path(model: &WhisperModel) -> PathBuf {
    get_models_dir().join(format!("ggml-{}.bin", model.id()))
}

fn get_custom_model_path(model_id: &str) -> PathBuf {
    get_models_dir().join(format!("ggml-{}.bin", model_id))
}

pub fn get_status() -> WhisperStatus {
    let binary_path = get_binary_path();
    let binary_installed = binary_path.exists();

    let model_path = find_downloaded_model();

    WhisperStatus {
        binary_installed,
        model_name: model_path.as_ref().map(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        model_path: model_path.map(|p| p.to_string_lossy().to_string()),
    }
}

fn find_downloaded_model() -> Option<PathBuf> {
    let dir = get_models_dir();
    if !dir.exists() {
        return None;
    }

    for model in ["base", "small", "medium", "tiny"] {
        let path = dir.join(format!("ggml-{}.bin", model));
        if path.exists() {
            return Some(path);
        }
    }
    None
}

fn find_model_by_id(model_id: &str) -> Option<PathBuf> {
    let dir = get_models_dir();
    if !dir.exists() {
        return None;
    }

    let standard_path = dir.join(format!("ggml-{}.bin", model_id));
    if standard_path.exists() {
        return Some(standard_path);
    }

    let custom_path = dir.join(format!("{}.bin", model_id));
    if custom_path.exists() {
        return Some(custom_path);
    }

    None
}

pub fn get_available_models() -> Vec<ModelInfo> {
    let installed = find_installed_models();
    let installed_ids: std::collections::HashSet<String> =
        installed.iter().map(|m| m.id.clone()).collect();

    vec![
        ModelInfo {
            id: "tiny".to_string(),
            model_type: "whisper".to_string(),
            name: "Whisper Tiny".to_string(),
            size_mb: 75,
            lang: None,
            quality: None,
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-tiny.bin"
                .to_string(),
            installed: installed_ids.contains("tiny"),
        },
        ModelInfo {
            id: "base".to_string(),
            model_type: "whisper".to_string(),
            name: "Whisper Base".to_string(),
            size_mb: 142,
            lang: None,
            quality: None,
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-base.bin"
                .to_string(),
            installed: installed_ids.contains("base"),
        },
        ModelInfo {
            id: "small".to_string(),
            model_type: "whisper".to_string(),
            name: "Whisper Small".to_string(),
            size_mb: 466,
            lang: None,
            quality: None,
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-small.bin"
                .to_string(),
            installed: installed_ids.contains("small"),
        },
        ModelInfo {
            id: "medium".to_string(),
            model_type: "whisper".to_string(),
            name: "Whisper Medium".to_string(),
            size_mb: 1500,
            lang: None,
            quality: None,
            url: "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-medium.bin"
                .to_string(),
            installed: installed_ids.contains("medium"),
        },
    ]
}

pub fn find_installed_models() -> Vec<InstalledModel> {
    let dir = get_models_dir();
    if !dir.exists() {
        return Vec::new();
    }

    let mut models = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                if name.starts_with("ggml-") && name.ends_with(".bin") {
                    let id = name
                        .strip_prefix("ggml-")
                        .and_then(|s| s.strip_suffix(".bin"))
                        .unwrap_or("")
                        .to_string();

                    let size_bytes = path.metadata().map(|m| m.len()).unwrap_or(0);

                    models.push(InstalledModel {
                        id,
                        model_type: "whisper".to_string(),
                        name: format!(
                            "Whisper {}",
                            name.strip_prefix("ggml-")
                                .and_then(|s| s.strip_suffix(".bin"))
                                .unwrap_or("")
                                .chars()
                                .next()
                                .map(|c| c.to_uppercase())
                                .map(|s| s.to_string())
                                .unwrap_or_default()
                        ),
                        path: path.to_string_lossy().to_string(),
                        size_bytes,
                    });
                }
            }
        }
    }
    models
}

pub fn delete_model(model_id: &str) -> Result<(), String> {
    let path = get_custom_model_path(model_id);
    if !path.exists() {
        return Err(format!("Model '{}' not found", model_id));
    }
    std::fs::remove_file(&path).map_err(|e| format!("Failed to delete model: {}", e))?;
    info!("Deleted Whisper model: {}", model_id);
    Ok(())
}

pub async fn install_binary() -> Result<(), String> {
    info!("Installing Whisper.cpp binary...");

    let dir = super::ensure_models_dir()
        .map_err(|e| format!("Failed to create models directory: {}", e))?;

    let binary_path = get_binary_path();
    if binary_path.exists() {
        info!("Whisper binary already installed");
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let url = format!(
            "https://github.com/ggml-org/whisper.cpp/releases/download/v{}/whisper-bin-x64.zip",
            WHISPER_CPP_VERSION
        );
        download_and_extract_binary(&url, &dir, "whisper.exe").await?;
    }

    #[cfg(target_os = "macos")]
    {
        return Err("Automatic Whisper installation not available for macOS. Please install whisper.cpp manually from https://github.com/ggml-org/whisper.cpp".to_string());
    }

    #[cfg(target_os = "linux")]
    {
        return Err("Automatic Whisper installation not available for Linux. Please install whisper.cpp manually from https://github.com/ggml-org/whisper.cpp".to_string());
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err("Unsupported platform for automatic Whisper installation. Please install whisper.cpp manually.".to_string());
    }

    info!("Whisper binary installed successfully");
    Ok(())
}

async fn download_and_extract_binary(
    url: &str,
    dir: &PathBuf,
    binary_name: &str,
) -> Result<(), String> {
    info!("Downloading from: {}", url);

    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Failed to download: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_bytes = response.content_length().unwrap_or(0);
    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read response: {}", e))?;

    let archive_path = dir.join("whisper_archive");
    std::fs::write(&archive_path, &bytes).map_err(|e| format!("Failed to write archive: {}", e))?;

    broadcast_progress(super::DownloadProgress {
        model_id: "whisper-binary".to_string(),
        model_type: "whisper".to_string(),
        stage: "extracting".to_string(),
        progress: 1.0,
        bytes_downloaded: total_bytes,
        total_bytes,
    });

    info!("Extracting binary...");

    #[cfg(target_os = "windows")]
    {
        let output = Command::new("powershell")
            .args([
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                    archive_path.display(),
                    dir.display()
                ),
            ])
            .output()
            .map_err(|e| format!("Failed to extract archive: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Extraction failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let extracted_binary = dir.join("Release").join("whisper-cli.exe");
        let final_binary = dir.join(binary_name);
        if extracted_binary.exists() {
            std::fs::copy(&extracted_binary, &final_binary)
                .map_err(|e| format!("Failed to copy binary: {}", e))?;

            let dlls = ["ggml.dll", "ggml-cpu.dll", "ggml-base.dll", "SDL2.dll"];
            for dll in dlls {
                let src = dir.join("Release").join(dll);
                let dst = dir.join(dll);
                if src.exists() {
                    let _ = std::fs::copy(&src, &dst);
                }
            }

            let _ = std::fs::remove_dir_all(dir.join("Release"));
        }
    }

    #[cfg(not(target_os = "windows"))]
    {
        let output = Command::new("tar")
            .args([
                "-xzf",
                &archive_path.to_string_lossy(),
                "-C",
                &dir.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("Failed to extract archive: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Extraction failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let extracted_binary = dir.join("main");
        let final_binary = dir.join(binary_name);
        if extracted_binary.exists() {
            std::fs::rename(extracted_binary, &final_binary)
                .map_err(|e| format!("Failed to rename binary: {}", e))?;

            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&final_binary, std::fs::Permissions::from_mode(0o755))
                    .map_err(|e| format!("Failed to set permissions: {}", e))?;
            }
        }
    }

    let _ = std::fs::remove_file(&archive_path);
    Ok(())
}

pub async fn download_model(model_id: &str) -> Result<PathBuf, String> {
    let model = WhisperModel::from_str(model_id)
        .ok_or_else(|| format!("Unknown Whisper model: {}", model_id))?;

    info!(
        "Downloading Whisper {} model ({}MB)...",
        model.id(),
        model.size_mb()
    );

    let _ = super::ensure_models_dir();
    let model_path = get_model_path(&model);

    if model_path.exists() {
        info!("Model already downloaded");
        return Ok(model_path);
    }

    let url = format!(
        "https://huggingface.co/ggerganov/whisper.cpp/resolve/main/ggml-{}.bin",
        model.id()
    );

    info!("Downloading from: {}", url);

    let response = reqwest::get(&url)
        .await
        .map_err(|e| format!("Failed to download model: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_bytes = response
        .content_length()
        .unwrap_or(model.size_mb() * 1024 * 1024);

    broadcast_progress(super::DownloadProgress {
        model_id: model_id.to_string(),
        model_type: "whisper".to_string(),
        stage: "downloading".to_string(),
        progress: 0.0,
        bytes_downloaded: 0,
        total_bytes,
    });

    let bytes = response
        .bytes()
        .await
        .map_err(|e| format!("Failed to read model: {}", e))?;

    let downloaded = bytes.len() as u64;
    broadcast_progress(super::DownloadProgress {
        model_id: model_id.to_string(),
        model_type: "whisper".to_string(),
        stage: "downloading".to_string(),
        progress: 1.0,
        bytes_downloaded: downloaded,
        total_bytes,
    });

    std::fs::write(&model_path, &bytes).map_err(|e| format!("Failed to write model: {}", e))?;

    broadcast_progress(super::DownloadProgress {
        model_id: model_id.to_string(),
        model_type: "whisper".to_string(),
        stage: "complete".to_string(),
        progress: 1.0,
        bytes_downloaded: downloaded,
        total_bytes: downloaded,
    });

    info!("Model downloaded successfully");
    Ok(model_path)
}

pub async fn transcribe(
    audio_path: &std::path::Path,
    language: Option<&str>,
    model_id: Option<&str>,
) -> Result<String, String> {
    let binary_path = get_binary_path();
    if !binary_path.exists() {
        return Err("Whisper binary not installed. Run voice installation first.".to_string());
    }

    let model_path = model_id
        .and_then(find_model_by_id)
        .or_else(find_downloaded_model)
        .ok_or_else(|| "No Whisper model installed. Download a model first.".to_string())?;

    let mut args = vec![
        "-f".to_string(),
        audio_path.to_string_lossy().to_string(),
        "-m".to_string(),
        model_path.to_string_lossy().to_string(),
        "-nt".to_string(),
        "--output-txt".to_string(),
    ];

    if let Some(lang) = language {
        args.push("-l".to_string());
        args.push(lang.to_string());
    }

    let output = Command::new(&binary_path)
        .args(&args)
        .output()
        .map_err(|e| format!("Failed to run Whisper: {}", e))?;

    if !output.status.success() {
        return Err(format!(
            "Whisper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);

    for line in stdout.lines() {
        if !line.starts_with('[') && !line.is_empty() {
            if let Some(after_bracket) = line.split(']').nth(1) {
                return Ok(after_bracket.trim().to_string());
            }
        }
    }

    Ok(stdout.lines().last().unwrap_or("").to_string())
}

pub async fn install_all(model: WhisperModel) -> Result<(), String> {
    install_binary().await?;
    download_model(model.id()).await?;
    Ok(())
}
