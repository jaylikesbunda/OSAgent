use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TtsConfig {
    pub enabled: bool,
    pub piper_path: String,
    pub voice: String,
    pub rate: f32,
}

impl Default for TtsConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            piper_path: "piper".to_string(),
            voice: "en_US-lessac-medium".to_string(),
            rate: 1.0,
        }
    }
}

pub struct TtsEngine {
    config: TtsConfig,
    available: bool,
}

impl TtsEngine {
    pub fn new(config: TtsConfig) -> Self {
        let available = if cfg!(windows) {
            std::process::Command::new("where")
                .arg(&config.piper_path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        } else {
            std::process::Command::new("which")
                .arg(&config.piper_path)
                .output()
                .map(|o| o.status.success())
                .unwrap_or(false)
        };

        Self { available, config }
    }

    pub fn is_available(&self) -> bool {
        self.available
    }

    pub async fn synthesize_wav(&self, _text: &str) -> crate::error::Result<Vec<u8>> {
        if !self.available {
            return Err(crate::error::OSAgentError::Tts(
                "TTS not available".to_string(),
            ));
        }
        Err(crate::error::OSAgentError::Tts(
            "Server-side TTS not implemented - use browser TTS".to_string(),
        ))
    }
}
