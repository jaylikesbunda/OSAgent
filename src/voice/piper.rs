use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tokio::io::AsyncWriteExt;
use tracing::info;

use super::{broadcast_progress, get_models_dir, InstalledModel, ModelInfo};

const PIPER_VERSION: &str = "2023.11.14-2";
const PROGRESS_WRITE_CHUNK_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiperStatus {
    pub binary_installed: bool,
    pub voice_name: Option<String>,
    pub voice_path: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PiperVoice {
    pub name: String,
    pub lang: String,
    pub quality: String,
    pub url: String,
}

pub fn get_available_voices(lang: &str) -> Vec<PiperVoice> {
    match lang {
        "en" => vec![
            PiperVoice {
                name: "en_US-libritts-high".to_string(),
                lang: "en".to_string(),
                quality: "high".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/libritts/high/en_US-libritts-high.onnx".to_string(),
            },
            PiperVoice {
                name: "en_GB-semaine-medium".to_string(),
                lang: "en".to_string(),
                quality: "medium".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_GB/semaine/medium/en_GB-semaine-medium.onnx".to_string(),
            },
            PiperVoice {
                name: "en_US-lessac-medium".to_string(),
                lang: "en".to_string(),
                quality: "medium".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/lessac/medium/en_US-lessac-medium.onnx".to_string(),
            },
            PiperVoice {
                name: "en_US-amy-medium".to_string(),
                lang: "en".to_string(),
                quality: "medium".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/amy/medium/en_US-amy-medium.onnx".to_string(),
            },
        ],
        "de" => vec![
            PiperVoice {
                name: "de_DE-thorsten-medium".to_string(),
                lang: "de".to_string(),
                quality: "medium".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/de/de_DE/thorsten/medium/de_DE-thorsten-medium.onnx".to_string(),
            },
        ],
        "fr" => vec![
            PiperVoice {
                name: "fr_FR-siwis-medium".to_string(),
                lang: "fr".to_string(),
                quality: "medium".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/fr/fr_FR/siwis/medium/fr_FR-siwis-medium.onnx".to_string(),
            },
        ],
        "es" => vec![
            PiperVoice {
                name: "es_ES-sharvard-medium".to_string(),
                lang: "es".to_string(),
                quality: "medium".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/es/es_ES/sharvard/medium/es_ES-sharvard-medium.onnx".to_string(),
            },
        ],
        _ => vec![
            PiperVoice {
                name: "en_US-libritts-high".to_string(),
                lang: "en".to_string(),
                quality: "high".to_string(),
                url: "https://huggingface.co/rhasspy/piper-voices/resolve/main/en/en_US/libritts/high/en_US-libritts-high.onnx".to_string(),
            },
        ],
    }
}

pub fn get_available_voices_all() -> Vec<ModelInfo> {
    let installed = find_installed_voices();
    let installed_names: std::collections::HashSet<String> =
        installed.iter().map(|m| m.name.clone()).collect();

    let mut result = Vec::new();
    for lang in &["en", "de", "fr", "es"] {
        for voice in get_available_voices(lang) {
            result.push(ModelInfo {
                id: voice.name.clone(),
                model_type: "piper".to_string(),
                name: voice.name.clone(),
                size_mb: 0,
                lang: Some(voice.lang.clone()),
                quality: Some(voice.quality.clone()),
                url: voice.url,
                installed: installed_names.contains(&voice.name),
            });
        }
    }

    for voice in installed {
        if result.iter().any(|model| model.id == voice.id) {
            continue;
        }

        result.push(ModelInfo {
            id: voice.id.clone(),
            model_type: "piper".to_string(),
            name: voice.name.clone(),
            size_mb: ((voice.size_bytes as f64) / (1024.0 * 1024.0)).ceil() as u64,
            lang: Some(detect_voice_lang(&voice.id)),
            quality: Some("custom".to_string()),
            url: String::new(),
            installed: true,
        });
    }

    result
}

fn get_binary_path() -> PathBuf {
    let dir = get_models_dir();
    #[cfg(target_os = "windows")]
    {
        dir.join("piper.exe")
    }
    #[cfg(not(target_os = "windows"))]
    {
        dir.join("piper")
    }
}

fn get_voice_path(voice_name: &str) -> PathBuf {
    get_models_dir().join(format!("{}.onnx", voice_name))
}

fn get_voice_json_path(voice_name: &str) -> PathBuf {
    get_models_dir().join(format!("{}.onnx.json", voice_name))
}

pub fn get_status() -> PiperStatus {
    let binary_path = get_binary_path();
    let binary_installed = binary_path.exists();

    let voice_path = find_downloaded_voice();

    PiperStatus {
        binary_installed,
        voice_name: voice_path.as_ref().map(|p| {
            p.file_stem()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        voice_path: voice_path.map(|p| p.to_string_lossy().to_string()),
    }
}

fn find_downloaded_voice() -> Option<PathBuf> {
    let dir = get_models_dir();
    if !dir.exists() {
        return None;
    }

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "onnx").unwrap_or(false) {
                return Some(path);
            }
        }
    }
    None
}

pub fn find_installed_voices() -> Vec<InstalledModel> {
    let dir = get_models_dir();
    if !dir.exists() {
        return Vec::new();
    }

    let mut voices = Vec::new();
    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|e| e == "onnx").unwrap_or(false) {
                if let Some(name) = path.file_stem().and_then(|n| n.to_str()) {
                    let size_bytes = path.metadata().map(|m| m.len()).unwrap_or(0);
                    let _lang = detect_voice_lang(name);

                    voices.push(InstalledModel {
                        id: name.to_string(),
                        model_type: "piper".to_string(),
                        name: name.to_string(),
                        path: path.to_string_lossy().to_string(),
                        size_bytes,
                    });
                }
            }
        }
    }
    voices
}

fn detect_voice_lang(name: &str) -> String {
    if name.starts_with("en_") {
        "en".to_string()
    } else if name.starts_with("de_") {
        "de".to_string()
    } else if name.starts_with("fr_") {
        "fr".to_string()
    } else if name.starts_with("es_") {
        "es".to_string()
    } else {
        "en".to_string()
    }
}

pub fn delete_voice(voice_id: &str) -> Result<(), String> {
    let voice_path = get_voice_path(voice_id);
    let json_path = get_voice_json_path(voice_id);

    if !voice_path.exists() {
        return Err(format!("Voice '{}' not found", voice_id));
    }

    std::fs::remove_file(&voice_path).map_err(|e| format!("Failed to delete voice: {}", e))?;
    if json_path.exists() {
        let _ = std::fs::remove_file(&json_path);
    }
    info!("Deleted Piper voice: {}", voice_id);
    Ok(())
}

pub async fn install_binary() -> Result<(), String> {
    info!("Installing Piper TTS binary...");

    let dir = super::ensure_models_dir()
        .map_err(|e| format!("Failed to create models directory: {}", e))?;

    let binary_path = get_binary_path();
    if binary_path.exists() {
        info!("Piper binary already installed");
        return Ok(());
    }

    #[cfg(target_os = "windows")]
    {
        let url = format!(
            "https://github.com/rhasspy/piper/releases/download/{}/piper_windows_amd64.zip",
            PIPER_VERSION
        );
        download_and_extract_binary(&url, &dir, "piper.exe").await?;
    }

    #[cfg(target_os = "macos")]
    {
        let url = format!(
            "https://github.com/rhasspy/piper/releases/download/{}/piper_macos_x64.tar.gz",
            PIPER_VERSION
        );
        download_and_extract_binary(&url, &dir, "piper").await?;
    }

    #[cfg(target_os = "linux")]
    {
        let url = format!(
            "https://github.com/rhasspy/piper/releases/download/{}/piper_linux_x64.tar.gz",
            PIPER_VERSION
        );
        download_and_extract_binary(&url, &dir, "piper").await?;
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        return Err("Unsupported platform for automatic Piper installation. Please install piper manually from https://github.com/rhasspy/piper".to_string());
    }

    info!("Piper binary installed successfully");
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

    #[cfg(target_os = "windows")]
    let archive_path = dir.join("piper_archive.zip");
    #[cfg(not(target_os = "windows"))]
    let archive_path = dir.join("piper_archive.tar.gz");
    let total_bytes = response.content_length().unwrap_or(0);
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&archive_path)
        .await
        .map_err(|e| format!("Failed to create archive: {}", e))?;

    broadcast_progress(super::DownloadProgress {
        model_id: "piper-binary".to_string(),
        model_type: "piper".to_string(),
        stage: "downloading runtime".to_string(),
        progress: 0.0,
        bytes_downloaded: 0,
        total_bytes,
    });

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed to read response: {}", e))?;
        for part in chunk.chunks(PROGRESS_WRITE_CHUNK_SIZE) {
            file.write_all(part)
                .await
                .map_err(|e| format!("Failed to write archive: {}", e))?;
            downloaded += part.len() as u64;

            broadcast_progress(super::DownloadProgress {
                model_id: "piper-binary".to_string(),
                model_type: "piper".to_string(),
                stage: "downloading runtime".to_string(),
                progress: if total_bytes > 0 {
                    downloaded as f32 / total_bytes as f32
                } else {
                    0.0
                },
                bytes_downloaded: downloaded,
                total_bytes,
            });
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush archive: {}", e))?;

    broadcast_progress(super::DownloadProgress {
        model_id: "piper-binary".to_string(),
        model_type: "piper".to_string(),
        stage: "extracting".to_string(),
        progress: 1.0,
        bytes_downloaded: downloaded,
        total_bytes,
    });

    info!("Extracting binary...");

    #[cfg(target_os = "windows")]
    {
        let extract_dir = dir.join("piper_extract");
        let _ = std::fs::create_dir_all(&extract_dir);

        let output = Command::new("powershell")
            .args([
                "-Command",
                &format!(
                    "Expand-Archive -Path '{}' -DestinationPath '{}' -Force",
                    archive_path.display(),
                    extract_dir.display()
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

        let piper_dir = extract_dir.join("piper");
        let final_binary = dir.join(binary_name);
        let extracted_binary = piper_dir.join(binary_name);

        if extracted_binary.exists() {
            std::fs::copy(&extracted_binary, &final_binary)
                .map_err(|e| format!("Failed to copy binary: {}", e))?;

            if let Ok(entries) = std::fs::read_dir(&piper_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.extension().map(|e| e == "dll").unwrap_or(false) {
                        let dest = dir.join(entry.file_name());
                        let _ = std::fs::copy(&path, &dest);
                    }
                    if entry.file_name() == "libtashkeel_model.ort" {
                        let dest = dir.join("libtashkeel_model.ort");
                        let _ = std::fs::copy(&path, &dest);
                    }
                }
            }

            let espeak_src = piper_dir.join("espeak-ng-data");
            let espeak_dst = dir.join("espeak-ng-data");
            if espeak_src.exists() {
                let _ = std::fs::create_dir_all(&espeak_dst);
                fn copy_dir_all(
                    src: &std::path::Path,
                    dst: &std::path::Path,
                ) -> std::io::Result<()> {
                    std::fs::create_dir_all(dst)?;
                    for entry in std::fs::read_dir(src)? {
                        let entry = entry?;
                        let ty = entry.file_type()?;
                        if ty.is_dir() {
                            copy_dir_all(&entry.path(), &dst.join(entry.file_name()))?;
                        } else {
                            std::fs::copy(entry.path(), dst.join(entry.file_name()))?;
                        }
                    }
                    Ok(())
                }
                let _ = copy_dir_all(&espeak_src, &espeak_dst);
            }
        }

        let _ = std::fs::remove_dir_all(&extract_dir);
    }

    #[cfg(not(target_os = "windows"))]
    {
        let extract_dir = dir.join("piper_extract");
        let _ = std::fs::create_dir_all(&extract_dir);

        let output = Command::new("tar")
            .args([
                "-xzf",
                &archive_path.to_string_lossy(),
                "-C",
                &extract_dir.to_string_lossy(),
            ])
            .output()
            .map_err(|e| format!("Failed to extract archive: {}", e))?;

        if !output.status.success() {
            return Err(format!(
                "Extraction failed: {}",
                String::from_utf8_lossy(&output.stderr)
            ));
        }

        let final_binary = dir.join(binary_name);
        if let Ok(mut entries) = std::fs::read_dir(&extract_dir) {
            if let Some(Ok(entry)) = entries.next() {
                let piper_dir = entry.path().join("piper");
                let extracted_binary = piper_dir.join(binary_name);
                if extracted_binary.exists() {
                    std::fs::copy(&extracted_binary, &final_binary)
                        .map_err(|e| format!("Failed to copy binary: {}", e))?;

                    #[cfg(unix)]
                    {
                        use std::os::unix::fs::PermissionsExt;
                        std::fs::set_permissions(
                            &final_binary,
                            std::fs::Permissions::from_mode(0o755),
                        )
                        .map_err(|e| format!("Failed to set permissions: {}", e))?;
                    }
                }
            }
        }

        let _ = std::fs::remove_dir_all(&extract_dir);
    }

    let _ = std::fs::remove_file(&archive_path);
    Ok(())
}

pub async fn download_voice(voice_name: &str) -> Result<PathBuf, String> {
    info!("Downloading Piper voice: {}...", voice_name);

    let _ = super::ensure_models_dir();
    let voice_path = get_voice_path(voice_name);
    let json_path = get_voice_json_path(voice_name);

    if voice_path.exists() {
        info!("Voice already downloaded");
        return Ok(voice_path);
    }

    let voices = get_available_voices_all();
    let voice = voices
        .iter()
        .find(|v| v.name == voice_name)
        .ok_or_else(|| format!("Unknown voice: {}", voice_name))?;

    let url = &voice.url;
    info!("Downloading from: {}", url);

    let response = reqwest::get(url)
        .await
        .map_err(|e| format!("Failed to download voice: {}", e))?;

    if !response.status().is_success() {
        return Err(format!(
            "Download failed with status: {}",
            response.status()
        ));
    }

    let total_bytes = response.content_length().unwrap_or(0);

    broadcast_progress(super::DownloadProgress {
        model_id: voice_name.to_string(),
        model_type: "piper".to_string(),
        stage: "downloading".to_string(),
        progress: 0.0,
        bytes_downloaded: 0,
        total_bytes,
    });

    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&voice_path)
        .await
        .map_err(|e| format!("Failed to create voice file: {}", e))?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed to read voice: {}", e))?;
        for part in chunk.chunks(PROGRESS_WRITE_CHUNK_SIZE) {
            file.write_all(part)
                .await
                .map_err(|e| format!("Failed to write voice: {}", e))?;
            downloaded += part.len() as u64;

            broadcast_progress(super::DownloadProgress {
                model_id: voice_name.to_string(),
                model_type: "piper".to_string(),
                stage: "downloading".to_string(),
                progress: if total_bytes > 0 {
                    downloaded as f32 / total_bytes as f32
                } else {
                    0.0
                },
                bytes_downloaded: downloaded,
                total_bytes,
            });
        }
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush voice file: {}", e))?;

    let json_url = format!("{}.json", url);
    info!("Downloading voice config from: {}", json_url);

    if let Ok(response) = reqwest::get(&json_url).await {
        if response.status().is_success() {
            if let Ok(bytes) = response.bytes().await {
                let _ = std::fs::write(&json_path, &bytes);
            }
        }
    }

    broadcast_progress(super::DownloadProgress {
        model_id: voice_name.to_string(),
        model_type: "piper".to_string(),
        stage: "complete".to_string(),
        progress: 1.0,
        bytes_downloaded: downloaded,
        total_bytes: downloaded,
    });

    info!("Voice downloaded successfully");
    Ok(voice_path)
}

pub async fn synthesize(
    text: &str,
    voice_name: Option<&str>,
    output_path: &std::path::Path,
) -> Result<(), String> {
    let binary_path = get_binary_path();
    if !binary_path.exists() {
        return Err("Piper binary not installed. Run voice installation first.".to_string());
    }

    let voice = if let Some(name) = voice_name {
        let voice_path = find_voice_by_name(name);
        info!("TTS: requested voice='{}', path={:?}", name, voice_path);
        voice_path.ok_or_else(|| {
            format!(
                "Voice '{}' not found. Download it first from voice settings.",
                name
            )
        })?
    } else {
        let voice_path = find_downloaded_voice();
        info!("TTS: no voice configured, using fallback: {:?}", voice_path);
        voice_path.ok_or_else(|| "No Piper voice installed. Download a voice first.".to_string())?
    };

    let text_owned = text.to_string();
    let output_path_owned = output_path.to_path_buf();
    let result = tokio::task::spawn_blocking(move || {
        let mut output = Command::new(&binary_path)
            .args([
                "--model",
                &voice.to_string_lossy(),
                "--output_file",
                &output_path_owned.to_string_lossy(),
            ])
            .stdin(std::process::Stdio::piped())
            .spawn()
            .map_err(|e| format!("Failed to run Piper: {}", e))?;

        if let Some(ref mut stdin) = output.stdin {
            use std::io::Write;
            stdin
                .write_all(text_owned.as_bytes())
                .map_err(|e| format!("Failed to write to Piper: {}", e))?;
        }

        output
            .wait_with_output()
            .map_err(|e| format!("Failed to wait for Piper: {}", e))
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    if !result.status.success() {
        return Err(format!(
            "Piper failed: {}",
            String::from_utf8_lossy(&result.stderr)
        ));
    }

    Ok(())
}

fn find_voice_by_name(name: &str) -> Option<PathBuf> {
    let path = get_voice_path(name);
    if path.exists() {
        Some(path)
    } else {
        None
    }
}

pub async fn install_all(lang: &str, voice_name: Option<&str>) -> Result<(), String> {
    install_binary().await?;

    if let Some(name) = voice_name {
        download_voice(name).await?;
        return Ok(());
    }

    let voices = get_available_voices(lang);
    if let Some(voice) = voices.first() {
        download_voice(&voice.name).await?;
    }

    Ok(())
}
