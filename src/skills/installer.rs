use crate::error::OSAgentError;
use crate::skills::bundle::{get_icons_base_dir, get_skills_base_dir, BundleManifest, SkillBundle};
use crate::skills::config::{get_config_base_dir, SkillConfigStore};
use crate::skills::store::{SkillInfo, SkillStore};
use std::path::PathBuf;

pub struct SkillInstaller {
    skills_dir: PathBuf,
    icons_dir: PathBuf,
    config_store: SkillConfigStore,
    store: SkillStore,
}

impl SkillInstaller {
    pub fn new() -> Self {
        let skills_dir = get_skills_base_dir();
        let icons_dir = get_icons_base_dir();
        let config_store = SkillConfigStore::new(get_config_base_dir());
        let store = SkillStore::new();

        Self {
            skills_dir,
            icons_dir,
            config_store,
            store,
        }
    }

    pub fn install_from_bundle(&self, bundle_data: &[u8]) -> Result<InstallResult, OSAgentError> {
        SkillBundle::validate(bundle_data)
            .map_err(|e| OSAgentError::Unknown(format!("Invalid bundle: {}", e)))?;

        let manifest = SkillBundle::read_manifest(bundle_data)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to read manifest: {}", e)))?;

        let target_dir = self.skills_dir.join(&manifest.name);

        if target_dir.exists() {
            return Err(OSAgentError::Unknown(format!(
                "Skill '{}' is already installed. Uninstall it first.",
                manifest.name
            )));
        }

        SkillBundle::unpack(bundle_data, &target_dir)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to unpack bundle: {}", e)))?;

        if let Some(ref icon_name) = manifest.icon {
            let icon_source = target_dir.join(icon_name);
            if icon_source.exists() {
                if let Ok(icon_data) = std::fs::read(&icon_source) {
                    let _ = self.save_icon(&manifest.name, &icon_data);
                    let _ = std::fs::remove_file(icon_source);
                }
            }
        }

        let skill_info = self
            .store
            .get_skill_info(&manifest.name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill info: {}", e)))?;

        let skill_name = manifest.name.clone();
        Ok(InstallResult {
            name: manifest.name,
            version: manifest.version,
            description: manifest.description,
            icon_path: self.store.skill_icon_path(&skill_name),
            skill_info,
        })
    }

    pub fn uninstall(&self, name: &str) -> Result<(), OSAgentError> {
        if !self.store.skill_exists(name) {
            return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
        }

        self.store
            .delete_skill(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to delete skill: {}", e)))?;

        let icon_path = self.icons_dir.join(format!("{}.png", name));
        if icon_path.exists() {
            let _ = std::fs::remove_file(&icon_path);
        }

        self.config_store
            .delete_config(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to delete config: {}", e)))?;

        Ok(())
    }

    pub fn upgrade_from_bundle(
        &self,
        name: &str,
        bundle_data: &[u8],
    ) -> Result<InstallResult, OSAgentError> {
        if !self.store.skill_exists(name) {
            return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
        }

        let manifest = SkillBundle::read_manifest(bundle_data)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to read manifest: {}", e)))?;

        if manifest.name != name {
            return Err(OSAgentError::Unknown(format!(
                "Bundle name '{}' does not match skill name '{}'",
                manifest.name, name
            )));
        }

        let backup_dir = self.skills_dir.join(format!("{}.backup", name));
        let skill_dir = self.skills_dir.join(name);

        if skill_dir.exists() {
            let _ = std::fs::rename(&skill_dir, &backup_dir);
        }

        match SkillBundle::unpack(bundle_data, &self.skills_dir) {
            Ok(_) => {
                if let Some(ref icon_name) = manifest.icon {
                    let icon_source = skill_dir.join(icon_name);
                    if icon_source.exists() {
                        if let Ok(icon_data) = std::fs::read(&icon_source) {
                            let _ = self.save_icon(name, &icon_data);
                            let _ = std::fs::remove_file(icon_source);
                        }
                    }
                }

                if backup_dir.exists() {
                    let _ = std::fs::remove_dir_all(&backup_dir);
                }

                let skill_info = self.store.get_skill_info(name).map_err(|e| {
                    OSAgentError::Unknown(format!("Failed to get skill info: {}", e))
                })?;

                Ok(InstallResult {
                    name: manifest.name,
                    version: manifest.version,
                    description: manifest.description,
                    icon_path: self.store.skill_icon_path(name),
                    skill_info,
                })
            }
            Err(e) => {
                let _ = std::fs::remove_dir_all(&skill_dir);
                let _ = std::fs::rename(&backup_dir, &skill_dir);
                Err(OSAgentError::Unknown(format!(
                    "Failed to install skill: {}",
                    e
                )))
            }
        }
    }

    pub fn export_skill(&self, name: &str) -> Result<Vec<u8>, OSAgentError> {
        if !self.store.skill_exists(name) {
            return Err(OSAgentError::Unknown(format!("Skill '{}' not found", name)));
        }

        let manifest = self.create_manifest_for_skill(name)?;
        let temp_dir = std::env::temp_dir().join(format!("oskill_export_{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&temp_dir)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to create temp dir: {}", e)))?;

        let manifest_path = temp_dir.join("manifest.toml");
        let manifest_toml = toml::to_string_pretty(&manifest)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to serialize manifest: {}", e)))?;
        std::fs::write(&manifest_path, manifest_toml)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to write manifest: {}", e)))?;

        let skill_dir = self.store.skill_dir(name);
        if let Ok(content) = self.store.get_skill_content(name) {
            let _ = std::fs::write(skill_dir.join("SKILL.md"), &content);
        }

        if let Some(icon_path) = self.store.skill_icon_path(name) {
            if let Ok(icon_data) = std::fs::read(&icon_path) {
                let _ = std::fs::write(temp_dir.join("icon.png"), &icon_data);
            }
        }

        let zip_path = temp_dir.join(format!("{}.oskill", name));
        SkillBundle::pack(&temp_dir, &zip_path)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to create zip: {}", e)))?;

        let data = std::fs::read(&zip_path)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to read zip: {}", e)))?;

        let _ = std::fs::remove_dir_all(&temp_dir);

        Ok(data)
    }

    fn create_manifest_for_skill(&self, name: &str) -> Result<BundleManifest, OSAgentError> {
        let skill_info = self
            .store
            .get_skill_info(name)
            .map_err(|e| OSAgentError::Unknown(format!("Failed to get skill info: {}", e)))?;
        Ok(BundleManifest {
            name: name.to_string(),
            version: skill_info.version.unwrap_or_else(|| "1.0.0".to_string()),
            description: skill_info.description,
            author: skill_info.author,
            icon: Some("icon.png".to_string()),
        })
    }

    fn save_icon(&self, name: &str, icon_data: &[u8]) -> std::io::Result<()> {
        std::fs::create_dir_all(&self.icons_dir)?;
        let path = self.icons_dir.join(format!("{}.png", name));
        std::fs::write(&path, icon_data)
    }
}

impl Default for SkillInstaller {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::skills::loader::SkillLoader;
    use tempfile::TempDir;

    fn example_bundle_bytes(name: &str) -> Vec<u8> {
        let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
            .join("examples")
            .join("skills")
            .join(format!("{}.oskill", name));
        std::fs::read(path).expect("example bundle should exist")
    }

    #[test]
    fn installs_and_loads_example_skill_bundle() {
        let temp = TempDir::new().expect("temp dir");
        std::env::set_var("OSAGENT_DATA_DIR", temp.path());

        let installer = SkillInstaller::new();
        let bundle = example_bundle_bytes("github");
        let result = installer
            .install_from_bundle(&bundle)
            .expect("install should succeed");

        assert_eq!(result.name, "github");
        assert!(installer.store.skill_exists("github"));

        let skill_info = installer
            .store
            .get_skill_info("github")
            .expect("skill info should load");
        assert_eq!(skill_info.name, "github");
        assert_eq!(skill_info.emoji.as_deref(), Some("🐙"));
        assert!(skill_info.has_config);

        let mut loader = SkillLoader::new(temp.path().join("skills"));
        let loaded = loader.load_all().expect("loader should succeed");
        assert!(loaded.iter().any(|name| name == "github"));

        std::env::remove_var("OSAGENT_DATA_DIR");
    }
}

#[derive(Debug, serde::Serialize)]
pub struct InstallResult {
    pub name: String,
    pub version: String,
    pub description: String,
    pub icon_path: Option<PathBuf>,
    #[serde(flatten)]
    pub skill_info: SkillInfo,
}
