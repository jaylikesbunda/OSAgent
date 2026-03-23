use aes_gcm::{
    aead::{Aead, KeyInit},
    Aes256Gcm, Nonce,
};
use base64::{engine::general_purpose::STANDARD as BASE64, Engine};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD as BASE64_URL, Engine as _};
use rand::Rng;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use thiserror::Error;

#[derive(Debug, Clone)]
pub struct PkcePair {
    pub verifier: String,
    pub challenge: String,
}

pub fn generate_pkce_pair() -> PkcePair {
    let mut bytes = [0u8; 32];
    rand::thread_rng().fill(&mut bytes);
    let verifier = BASE64_URL.encode(bytes);
    let mut hasher = Sha256::new();
    hasher.update(verifier.as_bytes());
    let hash = hasher.finalize();
    let challenge = BASE64_URL.encode(hash);
    PkcePair {
        verifier,
        challenge,
    }
}

pub fn generate_oauth_state() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill(&mut bytes);
    BASE64_URL.encode(bytes)
}

#[derive(Error, Debug)]
pub enum OAuthStorageError {
    #[error("IO error: {0}")]
    Io(#[from] std::io::Error),
    #[error("Serialization error: {0}")]
    Serialization(#[from] serde_json::Error),
    #[error("Encryption error: {0}")]
    Encryption(String),
    #[error("Decryption error: {0}")]
    Decryption(String),
    #[error("No encryption key available")]
    NoKey,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthTokenEntry {
    pub access_token: String,
    pub refresh_token: Option<String>,
    pub expires_at: Option<i64>,
    pub scopes: Option<Vec<String>>,
    #[serde(default)]
    pub account_id: Option<String>,
}

pub fn parse_jwt_claims(token: &str) -> Option<Value> {
    let payload = token.split('.').nth(1)?;
    let bytes = BASE64_URL.decode(payload).ok()?;
    serde_json::from_slice(&bytes).ok()
}

fn account_id_from_claims(claims: &Value) -> Option<String> {
    claims
        .get("chatgpt_account_id")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| {
            claims
                .get("https://api.openai.com/auth")
                .and_then(|v| v.get("chatgpt_account_id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
        .or_else(|| {
            claims
                .get("organizations")
                .and_then(Value::as_array)
                .and_then(|items| items.first())
                .and_then(|item| item.get("id"))
                .and_then(Value::as_str)
                .map(str::to_string)
        })
}

pub fn extract_account_id(id_token: Option<&str>, access_token: Option<&str>) -> Option<String> {
    id_token
        .and_then(parse_jwt_claims)
        .as_ref()
        .and_then(account_id_from_claims)
        .or_else(|| {
            access_token
                .and_then(parse_jwt_claims)
                .as_ref()
                .and_then(account_id_from_claims)
        })
}

#[derive(Debug, Clone, Serialize, Deserialize)]
struct EncryptedStorage {
    pub entries: HashMap<String, String>,
    pub nonce: String,
}

pub struct OAuthStorage {
    storage_path: PathBuf,
    encryption_key: Option<[u8; 32]>,
}

impl OAuthStorage {
    pub fn new(storage_path: PathBuf) -> Self {
        let encryption_key = Self::derive_key();
        Self {
            storage_path,
            encryption_key,
        }
    }

    fn derive_key() -> Option<[u8; 32]> {
        let machine_id = Self::get_machine_identifier()?;
        let mut hasher = Sha256::new();
        hasher.update(machine_id.as_bytes());
        hasher.update(b"osagent_oauth_salt_v1");
        let result = hasher.finalize();
        let mut key = [0u8; 32];
        key.copy_from_slice(&result[..32]);
        Some(key)
    }

    fn get_machine_identifier() -> Option<String> {
        #[cfg(target_os = "windows")]
        {
            std::env::var("COMPUTERNAME").ok()
        }
        #[cfg(target_os = "macos")]
        {
            std::env::var("HOSTNAME").ok().or_else(|| {
                std::process::Command::new("scutil")
                    .args(["--get", "LocalHostName"])
                    .output()
                    .ok()
                    .and_then(|o| String::from_utf8(o.stdout).ok())
                    .map(|s| s.trim().to_string())
            })
        }
        #[cfg(target_os = "linux")]
        {
            std::env::var("HOSTNAME").ok().or_else(|| {
                std::fs::read_to_string("/etc/machine-id")
                    .ok()
                    .map(|s| s.trim().to_string())
            })
        }
        #[cfg(not(any(target_os = "windows", target_os = "macos", target_os = "linux")))]
        {
            std::env::var("HOSTNAME").ok()
        }
    }

    fn encrypt(&self, data: &str) -> Result<String, OAuthStorageError> {
        let key = self.encryption_key.ok_or(OAuthStorageError::NoKey)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| OAuthStorageError::Encryption(e.to_string()))?;

        let mut nonce_bytes = [0u8; 12];
        rand::thread_rng().fill(&mut nonce_bytes);
        let nonce = Nonce::from_slice(&nonce_bytes);

        let ciphertext = cipher
            .encrypt(nonce, data.as_bytes())
            .map_err(|e| OAuthStorageError::Encryption(e.to_string()))?;

        let mut combined = nonce_bytes.to_vec();
        combined.extend(ciphertext);

        Ok(BASE64.encode(&combined))
    }

    fn decrypt(&self, encrypted: &str) -> Result<String, OAuthStorageError> {
        let key = self.encryption_key.ok_or(OAuthStorageError::NoKey)?;
        let cipher = Aes256Gcm::new_from_slice(&key)
            .map_err(|e| OAuthStorageError::Decryption(e.to_string()))?;

        let combined = BASE64
            .decode(encrypted)
            .map_err(|e| OAuthStorageError::Decryption(e.to_string()))?;

        if combined.len() < 12 {
            return Err(OAuthStorageError::Decryption("Data too short".to_string()));
        }

        let (nonce_bytes, ciphertext) = combined.split_at(12);
        let nonce = Nonce::from_slice(nonce_bytes);

        let plaintext = cipher
            .decrypt(nonce, ciphertext)
            .map_err(|e| OAuthStorageError::Decryption(e.to_string()))?;

        String::from_utf8(plaintext).map_err(|e| OAuthStorageError::Decryption(e.to_string()))
    }

    pub fn load(&self) -> Result<HashMap<String, OAuthTokenEntry>, OAuthStorageError> {
        if !self.storage_path.exists() {
            return Ok(HashMap::new());
        }

        let encrypted_data = fs::read_to_string(&self.storage_path)?;

        if self.encryption_key.is_none() {
            return Err(OAuthStorageError::NoKey);
        }

        let entries: HashMap<String, OAuthTokenEntry> = if encrypted_data.starts_with('{') {
            serde_json::from_str(&encrypted_data)?
        } else {
            let decrypted = self.decrypt(&encrypted_data)?;
            serde_json::from_str(&decrypted)?
        };

        Ok(entries)
    }

    pub fn save(
        &self,
        entries: &HashMap<String, OAuthTokenEntry>,
    ) -> Result<(), OAuthStorageError> {
        if let Some(parent) = self.storage_path.parent() {
            fs::create_dir_all(parent)?;
        }

        let json = serde_json::to_string_pretty(entries)?;

        let data_to_write = if self.encryption_key.is_some() {
            self.encrypt(&json)?
        } else {
            json
        };

        fs::write(&self.storage_path, data_to_write)?;

        Ok(())
    }

    pub fn get_token(
        &self,
        provider_id: &str,
    ) -> Result<Option<OAuthTokenEntry>, OAuthStorageError> {
        let entries = self.load()?;
        Ok(entries.get(provider_id).cloned())
    }

    pub fn set_token(
        &self,
        provider_id: &str,
        entry: OAuthTokenEntry,
    ) -> Result<(), OAuthStorageError> {
        let mut entries = self.load()?;
        entries.insert(provider_id.to_string(), entry);
        self.save(&entries)
    }

    pub fn remove_token(&self, provider_id: &str) -> Result<(), OAuthStorageError> {
        let mut entries = self.load()?;
        entries.remove(provider_id);
        self.save(&entries)
    }

    pub fn clear(&self) -> Result<(), OAuthStorageError> {
        if self.storage_path.exists() {
            fs::remove_file(&self.storage_path)?;
        }
        Ok(())
    }
}

pub fn get_oauth_storage_path(config_dir: &PathBuf) -> PathBuf {
    config_dir.join("oauth_tokens.json")
}

pub fn create_oauth_storage(config_dir: &PathBuf) -> OAuthStorage {
    OAuthStorage::new(get_oauth_storage_path(config_dir))
}

pub mod provider;
