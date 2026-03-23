pub mod piper;
pub mod whisper;

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
        let (tx, _) = broadcast::channel(100);
        tx
    };
}

pub fn get_progress_receiver() -> broadcast::Receiver<DownloadProgress> {
    PROGRESS_TX.subscribe()
}

pub fn broadcast_progress(progress: DownloadProgress) {
    let _ = PROGRESS_TX.send(progress);
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
