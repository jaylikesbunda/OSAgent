//! Separate API-key store (`auth.toml` next to `config.toml`).
//!
//! Secrets stay out of the shareable config file (opencode-style
//! `auth.json` split): provider keys live here with 0600 permissions,
//! `config.toml` only names providers, endpoints and models. Resolution
//! order per request is explicit config keys → auth file → env vars.

use crate::error::{OSAgentError, Result};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

/// On-disk shape: one table per provider id.
/// ```toml
/// [openrouter]
/// api_keys = ["sk-or-..."]
/// ```
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct AuthFile {
    #[serde(flatten)]
    entries: HashMap<String, AuthEntry>,
}

#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
struct AuthEntry {
    #[serde(default)]
    api_keys: Vec<String>,
}

pub fn auth_store_path(config_dir: &Path) -> PathBuf {
    config_dir.join("auth.toml")
}

/// Load provider-id → keys. Missing file means no stored keys.
pub fn load_auth_keys(config_dir: &Path) -> HashMap<String, Vec<String>> {
    let path = auth_store_path(config_dir);
    let raw = match std::fs::read_to_string(&path) {
        Ok(raw) => raw,
        Err(_) => return HashMap::new(),
    };
    let file: AuthFile = match toml::from_str(&raw) {
        Ok(file) => file,
        Err(e) => {
            tracing::warn!("auth.toml is unusable ({}); ignoring stored keys", e);
            return HashMap::new();
        }
    };
    file.entries
        .into_iter()
        .map(|(id, entry)| {
            let keys: Vec<String> = entry
                .api_keys
                .into_iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            (id, keys)
        })
        .filter(|(_, keys)| !keys.is_empty())
        .collect()
}

/// Persist provider-id → keys with owner-only permissions.
pub fn save_auth_keys(config_dir: &Path, keys: &HashMap<String, Vec<String>>) -> Result<()> {
    let cleaned: HashMap<String, AuthEntry> = keys
        .iter()
        .filter_map(|(id, list)| {
            let distinct: Vec<String> = list
                .iter()
                .map(|k| k.trim().to_string())
                .filter(|k| !k.is_empty())
                .collect::<std::collections::HashSet<_>>()
                .into_iter()
                .collect();
            if id.trim().is_empty() || distinct.is_empty() {
                None
            } else {
                Some((id.clone(), AuthEntry { api_keys: distinct }))
            }
        })
        .collect();
    let data = toml::to_string_pretty(&AuthFile { entries: cleaned })
        .map_err(|e| OSAgentError::Config(format!("Failed to serialize auth store: {}", e)))?;
    std::fs::create_dir_all(config_dir).map_err(OSAgentError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(config_dir, std::fs::Permissions::from_mode(0o700));
    }
    std::fs::write(auth_store_path(config_dir), data).map_err(OSAgentError::Io)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(
            auth_store_path(config_dir),
            std::fs::Permissions::from_mode(0o600),
        );
    }
    Ok(())
}

/// Move inline config keys into the auth store. Idempotent: values already
/// stored are deduped, so a failed config save just re-migrates next load.
pub fn migrate_inline_keys_to_store(
    config_dir: &Path,
    providers: &mut [crate::config::ProviderConfig],
) -> bool {
    let mut store = load_auth_keys(config_dir);
    let mut moved = false;
    for provider in providers.iter_mut() {
        let id = provider.effective_id().to_string();
        let mut inline = Vec::new();
        if !provider.api_key.trim().is_empty() {
            inline.push(provider.api_key.clone());
        }
        inline.extend(provider.api_keys.iter().cloned());
        if inline.is_empty() {
            continue;
        }
        let bucket = store.entry(id).or_default();
        for key in inline {
            let trimmed = key.trim().to_string();
            if !trimmed.is_empty() && !bucket.iter().any(|k| k == &trimmed) {
                bucket.push(trimmed);
                moved = true;
            }
        }
        provider.api_key.clear();
        provider.api_keys.clear();
    }
    if moved {
        if let Err(e) = save_auth_keys(config_dir, &store) {
            tracing::warn!("could not persist auth.toml: {}", e);
            return false;
        }
        tracing::warn!(
            "moved provider API keys from config.toml to auth.toml (0600); config.toml is now shareable"
        );
    }
    moved
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_keys_per_provider() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut keys = HashMap::new();
        keys.insert(
            "openrouter".to_string(),
            vec!["sk-or-1".to_string(), "sk-or-2".to_string()],
        );
        save_auth_keys(dir.path(), &keys).expect("save");
        let loaded = load_auth_keys(dir.path());
        for key in ["sk-or-1", "sk-or-2"] {
            assert!(loaded["openrouter"].contains(&key.to_string()));
        }
    }

    #[test]
    fn missing_file_loads_empty() {
        let dir = tempfile::tempdir().expect("tempdir");
        assert!(load_auth_keys(dir.path()).is_empty());
    }

    #[test]
    fn migration_moves_and_clears_inline_keys() {
        let dir = tempfile::tempdir().expect("tempdir");
        let mut providers = vec![crate::config::ProviderConfig {
            provider_type: "openrouter".to_string(),
            api_key: "sk-or-inline".to_string(),
            api_keys: vec!["sk-or-extra".to_string()],
            ..Default::default()
        }];
        assert!(migrate_inline_keys_to_store(dir.path(), &mut providers));
        assert!(providers[0].api_key.is_empty());
        assert!(providers[0].api_keys.is_empty());
        let stored = load_auth_keys(dir.path());
        assert!(stored["openrouter"].contains(&"sk-or-inline".to_string()));
        assert!(stored["openrouter"].contains(&"sk-or-extra".to_string()));
        // Second run is a no-op.
        assert!(!migrate_inline_keys_to_store(dir.path(), &mut providers));
    }
}
