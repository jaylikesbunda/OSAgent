use crate::error::OSAgentError;
use crate::skills::config::{ConfigField, MaskedValue, SkillConfigStore};
use crate::skills::installer::{InstallResult, SkillInstaller};
use crate::skills::store::{SkillInfo, SkillStore};
use std::collections::HashMap;
use std::sync::Arc;

pub struct SkillService {
    store: Arc<SkillStore>,
    installer: Arc<SkillInstaller>,
    config_store: Arc<SkillConfigStore>,
}

impl SkillService {
    pub fn new() -> Self {
        let store = Arc::new(SkillStore::new());
        let installer = Arc::new(SkillInstaller::new());
        let config_store = Arc::new(SkillConfigStore::new(
            crate::skills::config::get_config_base_dir(),
        ));

        Self {
            store,
            installer,
            config_store,
        }
    }

    pub fn list_skills(&self) -> Result<Vec<SkillInfo>, OSAgentError> {
        self.store
            .list_skills()
            .map_err(|e| OSAgentError::Unknown(format!("Failed to list skills: {}", e)))
    }

    pub fn get_skill(&self, name: &str) -> Result<SkillInfo, OSAgentError> {
        self.store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill: {}", e)))
    }

    pub fn get_skill_content(&self, name: &str) -> Result<String, OSAgentError> {
        self.store
            .get_skill_content(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill content: {}", e)))
    }

    pub fn install_skill(&self, bundle_data: &[u8]) -> Result<InstallResult, OSAgentError> {
        self.installer.install_from_bundle(bundle_data)
    }

    pub fn uninstall_skill(&self, name: &str) -> Result<(), OSAgentError> {
        self.installer.uninstall(name)
    }

    pub fn upgrade_skill(
        &self,
        name: &str,
        bundle_data: &[u8],
    ) -> Result<InstallResult, OSAgentError> {
        self.installer.upgrade_from_bundle(name, bundle_data)
    }

    pub fn export_skill(&self, name: &str) -> Result<Vec<u8>, OSAgentError> {
        self.installer.export_skill(name)
    }

    pub fn get_config(&self, name: &str) -> Result<HashMap<String, MaskedValue>, OSAgentError> {
        self.config_store
            .get_masked_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get config: {}", e)))
    }

    pub fn save_config(
        &self,
        name: &str,
        settings: HashMap<String, String>,
    ) -> Result<(), OSAgentError> {
        let mut config = self
            .config_store
            .load_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to load config: {}", e)))?;

        for (key, value) in settings {
            if !value.is_empty() {
                config.settings.insert(key, value);
            } else {
                config.settings.remove(&key);
            }
        }

        self.config_store
            .save_config(name, &config)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to save config: {}", e)))
    }

    pub fn delete_config_value(&self, name: &str, key: &str) -> Result<(), OSAgentError> {
        let mut config = self
            .config_store
            .load_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to load config: {}", e)))?;
        config.settings.remove(key);
        self.config_store
            .save_config(name, &config)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to save config: {}", e)))
    }

    pub fn set_skill_enabled(&self, name: &str, enabled: bool) -> Result<(), OSAgentError> {
        let mut config = self
            .config_store
            .load_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to load config: {}", e)))?;
        config.enabled = enabled;
        self.config_store
            .save_config(name, &config)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to save config: {}", e)))
    }

    pub fn is_skill_enabled(&self, name: &str) -> bool {
        self.config_store
            .load_config(name)
            .map(|c| c.enabled)
            .unwrap_or(true)
    }

    pub fn get_skill_env(&self, name: &str) -> Result<HashMap<String, String>, OSAgentError> {
        if !self.is_skill_enabled(name) {
            return Ok(HashMap::new());
        }
        self.store
            .get_env_for_skill(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get env: {}", e)))
    }

    pub fn skill_exists(&self, name: &str) -> bool {
        self.store.skill_exists(name)
    }

    pub fn reload_skill(&self, name: &str) -> Result<SkillInfo, OSAgentError> {
        if !self.store.skill_exists(name) {
            return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
        }
        self.store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill: {}", e)))
    }

    pub fn reload_all(&self) -> Result<Vec<SkillInfo>, OSAgentError> {
        self.store
            .list_skills()
            .map_err(|e| OSAgentError::Unknown(format!("Failed to list skills: {}", e)))
    }
}

impl Default for SkillService {
    fn default() -> Self {
        Self::new()
    }
}

#[derive(Debug, serde::Serialize)]
pub struct SkillDetails {
    #[serde(flatten)]
    pub info: SkillInfo,
    pub config: HashMap<String, MaskedValue>,
    pub config_schema: Vec<ConfigField>,
    pub content: String,
}

impl SkillService {
    pub fn get_skill_details(&self, name: &str) -> Result<SkillDetails, OSAgentError> {
        let info = self
            .store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill info: {}", e)))?;
        let config = self.get_config(name)?;
        let config_schema = info.config_schema.clone();
        let content = self
            .store
            .get_skill_content(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get content: {}", e)))?;

        Ok(SkillDetails {
            info,
            config,
            config_schema,
            content,
        })
    }
}
