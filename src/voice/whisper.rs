use futures::StreamExt;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::process::Command;
use tokio::io::AsyncWriteExt;
use tracing::info;

use super::{broadcast_progress, get_models_dir, InstalledModel, ModelInfo};

const WHISPER_CPP_VERSION: &str = "1.8.3";
const PROGRESS_WRITE_CHUNK_SIZE: usize = 256 * 1024;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WhisperStatus {
    pub binary_installed: bool,
    pub model_name: Option<String>,
    pub model_path: Option<String>,
    /// Whether the persistent server binary is present. The UI only streams
    /// partial transcripts when it is: without a resident model every partial
    /// would reload hundreds of megabytes from disk.
    #[serde(default)]
    pub server_available: bool,
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

#[cfg(target_os = "windows")]
fn required_runtime_files() -> Vec<&'static str> {
    vec![
        "whisper.dll",
        "ggml.dll",
        "ggml-base.dll",
        "ggml-cpu.dll",
        "SDL2.dll",
    ]
}

#[cfg(not(target_os = "windows"))]
fn required_runtime_files() -> Vec<&'static str> {
    vec![]
}

fn is_runtime_installed() -> bool {
    let dir = get_models_dir();
    let binary_path = get_binary_path();
    binary_path.exists()
        && required_runtime_files()
            .into_iter()
            .all(|file| dir.join(file).exists())
}

fn get_model_path(model: &WhisperModel) -> PathBuf {
    get_models_dir().join(format!("ggml-{}.bin", model.id()))
}

fn get_custom_model_path(model_id: &str) -> PathBuf {
    get_models_dir().join(format!("ggml-{}.bin", model_id))
}

pub fn get_status() -> WhisperStatus {
    let binary_installed = is_runtime_installed();

    let model_path = find_downloaded_model();

    WhisperStatus {
        binary_installed,
        model_name: model_path.as_ref().map(|p| {
            p.file_name()
                .map(|f| f.to_string_lossy().to_string())
                .unwrap_or_default()
        }),
        model_path: model_path.map(|p| p.to_string_lossy().to_string()),
        server_available: super::whisper_server::is_available(),
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

    if let Ok(entries) = std::fs::read_dir(&dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().map(|ext| ext == "bin").unwrap_or(false) {
                return Some(path);
            }
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

    let mut models = vec![
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
    ];

    for model in installed {
        if ["tiny", "base", "small", "medium"].contains(&model.id.as_str()) {
            continue;
        }

        models.push(ModelInfo {
            id: model.id.clone(),
            model_type: "whisper".to_string(),
            name: model.name.clone(),
            size_mb: ((model.size_bytes as f64) / (1024.0 * 1024.0)).ceil() as u64,
            lang: None,
            quality: Some("custom".to_string()),
            url: String::new(),
            installed: true,
        });
    }

    models
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
                    let display_name = id
                        .split(['-', '_'])
                        .filter(|part| !part.is_empty())
                        .map(|part| {
                            let mut chars = part.chars();
                            match chars.next() {
                                Some(first) => {
                                    let mut out = first.to_uppercase().to_string();
                                    out.push_str(chars.as_str());
                                    out
                                }
                                None => String::new(),
                            }
                        })
                        .collect::<Vec<String>>()
                        .join(" ");

                    models.push(InstalledModel {
                        id,
                        model_type: "whisper".to_string(),
                        name: format!("Whisper {}", display_name),
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
    if is_runtime_installed() {
        info!("Whisper binary already installed");
        return Ok(());
    }

    if binary_path.exists() {
        info!("Whisper binary is missing runtime files, reinstalling");
    }

    #[cfg(target_os = "windows")]
    {
        let url = format!(
            "https://github.com/ggml-org/whisper.cpp/releases/download/v{}/whisper-bin-x64.zip",
            WHISPER_CPP_VERSION
        );
        download_and_extract_binary(&url, &dir, "whisper.exe").await?;
        info!("Whisper binary installed successfully");
        Ok(())
    }

    #[cfg(target_os = "macos")]
    {
        Err("Automatic Whisper installation not available for macOS. Please install whisper.cpp manually from https://github.com/ggml-org/whisper.cpp".to_string())
    }

    #[cfg(target_os = "linux")]
    {
        Err("Automatic Whisper installation not available for Linux. Please install whisper.cpp manually from https://github.com/ggml-org/whisper.cpp".to_string())
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
    {
        Err("Unsupported platform for automatic Whisper installation. Please install whisper.cpp manually.".to_string())
    }
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
    let archive_path = dir.join("whisper_archive.zip");
    #[cfg(not(target_os = "windows"))]
    let archive_path = dir.join("whisper_archive.tar.gz");
    let total_bytes = response.content_length().unwrap_or(0);
    let mut downloaded = 0u64;
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&archive_path)
        .await
        .map_err(|e| format!("Failed to create archive: {}", e))?;

    broadcast_progress(super::DownloadProgress {
        model_id: "whisper-binary".to_string(),
        model_type: "whisper".to_string(),
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
                model_id: "whisper-binary".to_string(),
                model_type: "whisper".to_string(),
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
        model_id: "whisper-binary".to_string(),
        model_type: "whisper".to_string(),
        stage: "extracting".to_string(),
        progress: 1.0,
        bytes_downloaded: downloaded,
        total_bytes,
    });

    info!("Extracting binary...");

    #[cfg(target_os = "windows")]
    {
        super::verify_download_complete(&archive_path, total_bytes, "Whisper runtime")?;

        let extract_dir = dir.join("whisper_extract");
        let _ = std::fs::remove_dir_all(&extract_dir);
        super::extract_zip_with_progress(&archive_path, &extract_dir, "whisper-binary", "whisper")?;

        // The archive nests everything under Release/, but don't depend on that
        // layout: search the tree, and accept the names whisper.cpp has shipped.
        let extracted_binary = ["whisper-cli.exe", "whisper.exe", "main.exe"]
            .iter()
            .find_map(|name| super::find_file_recursive(&extract_dir, name))
            .ok_or_else(|| {
                "Whisper archive did not contain a usable binary (whisper-cli.exe, whisper.exe or main.exe)"
                    .to_string()
            })?;
        let release_dir = extracted_binary
            .parent()
            .ok_or_else(|| "Could not determine Whisper extraction directory".to_string())?
            .to_path_buf();

        let final_binary = dir.join(binary_name);
        std::fs::copy(&extracted_binary, &final_binary)
            .map_err(|e| format!("Failed to copy binary: {}", e))?;

        let entries = std::fs::read_dir(&release_dir)
            .map_err(|_| "Failed to inspect extracted Whisper runtime files".to_string())?;
        for entry in entries.flatten() {
            let path = entry.path();
            if !path.is_file() {
                continue;
            }

            let Some(file_name) = path.file_name() else {
                continue;
            };

            if path == extracted_binary {
                continue;
            }

            let dest = dir.join(file_name);
            std::fs::copy(&path, &dest).map_err(|e| {
                format!(
                    "Failed to copy runtime file '{}' : {}",
                    file_name.to_string_lossy(),
                    e
                )
            })?;
        }

        for required in required_runtime_files() {
            if !dir.join(required).exists() {
                return Err(format!(
                    "Whisper runtime install is incomplete: missing {} after extraction",
                    required
                ));
            }
        }

        let _ = std::fs::remove_dir_all(&extract_dir);
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

    let mut downloaded = 0u64;
    let mut last_pct = -1i64;
    let mut stream = response.bytes_stream();
    let mut file = tokio::fs::File::create(&model_path)
        .await
        .map_err(|e| format!("Failed to create model file: {}", e))?;

    while let Some(chunk) = stream.next().await {
        let chunk = chunk.map_err(|e| format!("Failed to read model: {}", e))?;
        for part in chunk.chunks(PROGRESS_WRITE_CHUNK_SIZE) {
            file.write_all(part)
                .await
                .map_err(|e| format!("Failed to write model: {}", e))?;
            downloaded += part.len() as u64;

            if super::should_emit_progress(&mut last_pct, downloaded, total_bytes) {
                broadcast_progress(super::DownloadProgress {
                    model_id: model_id.to_string(),
                    model_type: "whisper".to_string(),
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
    }

    file.flush()
        .await
        .map_err(|e| format!("Failed to flush model file: {}", e))?;

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

/// Prefers the English-only build of a model when the configured language is
/// English. `tiny.en` and `base.en` are meaningfully faster and more accurate
/// on English than their multilingual counterparts at the same size.
///
/// Only a preference: the caller falls back to the requested model when the
/// `.en` variant is not downloaded.
fn prefer_english_variant(model_id: &str, language: Option<&str>) -> String {
    let is_english = language
        .map(|lang| {
            let lang = lang.to_lowercase();
            lang == "en" || lang.starts_with("en-") || lang.starts_with("en_")
        })
        .unwrap_or(false);

    if !is_english || model_id.ends_with(".en") {
        return model_id.to_string();
    }

    // medium/large have .en builds too, but only these are offered for download.
    match model_id {
        "tiny" | "base" | "small" => format!("{}.en", model_id),
        other => other.to_string(),
    }
}

/// Threads to give whisper.cpp. Uses physical cores where we can tell, capped
/// so a very large machine does not spend more time on thread overhead than
/// inference.
fn transcribe_threads() -> usize {
    std::thread::available_parallelism()
        .map(|n| (n.get() / 2).clamp(4, 16))
        .unwrap_or(4)
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

    let model_path = if let Some(id) = model_id {
        let requested = prefer_english_variant(id, language);
        find_model_by_id(&requested)
            .or_else(|| find_model_by_id(id))
            .ok_or_else(|| {
                format!(
                    "Selected Whisper model '{}' is not installed. Download it first from Voice settings.",
                    id
                )
            })?
    } else {
        find_downloaded_model()
            .ok_or_else(|| "No Whisper model installed. Download a model first.".to_string())?
    };

    let threads = transcribe_threads();

    // Prefer the resident server: the CLI reloads the entire model for every
    // utterance, which dominates latency on short input. Any failure falls
    // through to the CLI below, so this can never make transcription
    // unavailable — only slower.
    if super::whisper_server::is_available() {
        match super::whisper_server::transcribe(audio_path, language, &model_path, threads).await {
            Ok(text) => return Ok(text),
            Err(err) => {
                tracing::warn!("whisper-server transcription failed, using CLI: {}", err);
                super::whisper_server::mark_unhealthy();
            }
        }
    }

    let mut args = vec![
        "-f".to_string(),
        audio_path.to_string_lossy().to_string(),
        "-m".to_string(),
        model_path.to_string_lossy().to_string(),
        "-nt".to_string(),
        // Physical cores. whisper.cpp's own default is conservative, and
        // transcription is the only thing running at this point.
        "-t".to_string(),
        threads.to_string(),
        // Greedy instead of the default 5-beam search. For short conversational
        // utterances the accuracy difference is negligible and this is roughly
        // beam-count faster.
        "-bs".to_string(),
        "1".to_string(),
    ];

    if let Some(lang) = language {
        args.push("-l".to_string());
        args.push(lang.to_string());
    }

    let output = tokio::task::spawn_blocking(move || {
        Command::new(&binary_path)
            .args(&args)
            .output()
            .map_err(|e| format!("Failed to run Whisper: {}", e))
    })
    .await
    .map_err(|e| format!("spawn_blocking error: {}", e))??;

    if !output.status.success() {
        return Err(format!(
            "Whisper failed: {}",
            String::from_utf8_lossy(&output.stderr)
        ));
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    Ok(parse_transcript(&stdout))
}

/// Joins every transcribed segment.
///
/// whisper.cpp prints one line per segment. The previous parser looked for a
/// `]` to skip a timestamp prefix, but `-nt` removes timestamps entirely, so it
/// never matched and fell through to returning only the *last* line — silently
/// dropping everything before it on any utterance long enough to be split into
/// more than one segment.
///
/// Timestamps are still stripped when present, so this stays correct if `-nt`
/// is ever dropped.
fn parse_transcript(stdout: &str) -> String {
    let mut parts: Vec<&str> = Vec::new();

    for line in stdout.lines() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }

        // Progress and diagnostics go to stdout on some builds.
        if line.starts_with("whisper_")
            || line.starts_with("main:")
            || line.starts_with("system_info:")
        {
            continue;
        }

        let text = if line.starts_with('[') {
            match line.split_once(']') {
                Some((_, rest)) => rest.trim(),
                None => continue,
            }
        } else {
            line
        };

        if !text.is_empty() {
            parts.push(text);
        }
    }

    parts.join(" ")
}

pub async fn install_all(model: WhisperModel) -> Result<(), String> {
    install_binary().await?;
    download_model(model.id()).await?;
    Ok(())
}

#[cfg(test)]
mod transcript_parse_tests {
    use super::*;

    /// The regression this replaces: with -nt there are no timestamps, the old
    /// parser matched nothing, and it returned only the final line.
    #[test]
    fn joins_every_segment_without_timestamps() {
        let stdout = " I updated the storage layer.\n It should be faster now.\n Let me know.\n";
        assert_eq!(
            parse_transcript(stdout),
            "I updated the storage layer. It should be faster now. Let me know."
        );
    }

    #[test]
    fn single_segment_is_unchanged() {
        assert_eq!(parse_transcript(" hello there\n"), "hello there");
    }

    /// Still correct if -nt is ever removed.
    #[test]
    fn strips_timestamps_when_present() {
        let stdout = "[00:00:00.000 --> 00:00:02.000]   first part\n\
                      [00:00:02.000 --> 00:00:04.000]   second part\n";
        assert_eq!(parse_transcript(stdout), "first part second part");
    }

    #[test]
    fn skips_diagnostics_and_blank_lines() {
        let stdout = "whisper_init_from_file_with_params_no_state: loading model\n\
                      system_info: n_threads = 8\n\
                      \n\
                      the actual words\n\
                      \n";
        assert_eq!(parse_transcript(stdout), "the actual words");
    }

    #[test]
    fn empty_output_yields_empty_string() {
        assert_eq!(parse_transcript(""), "");
        assert_eq!(parse_transcript("\n\n"), "");
    }

    #[test]
    fn prefers_english_build_only_for_english() {
        assert_eq!(prefer_english_variant("base", Some("en")), "base.en");
        assert_eq!(prefer_english_variant("tiny", Some("en-US")), "tiny.en");
        assert_eq!(prefer_english_variant("base", Some("fr")), "base");
        assert_eq!(prefer_english_variant("base", None), "base");
        // Already English-only, and models without a .en build.
        assert_eq!(prefer_english_variant("base.en", Some("en")), "base.en");
        assert_eq!(prefer_english_variant("medium", Some("en")), "medium");
    }

    #[test]
    fn thread_count_is_sane() {
        let threads = transcribe_threads();
        assert!(
            (4..=16).contains(&threads),
            "unexpected thread count {threads}"
        );
    }
}
