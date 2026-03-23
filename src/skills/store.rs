use crate::skills::bundle::{get_icons_base_dir, get_skills_base_dir, BundleManifest};
use crate::skills::config::{
    get_config_base_dir, parse_frontmatter, ConfigField, MaskedValue, SkillConfig, SkillConfigStore,
};
use serde::Serialize;
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;

#[derive(Debug, Clone, Serialize)]
pub struct SkillInfo {
    pub name: String,
    pub description: String,
    pub version: Option<String>,
    pub author: Option<String>,
    pub emoji: Option<String>,
    pub has_icon: bool,
    pub enabled: bool,
    pub has_config: bool,
    pub config_schema: Vec<ConfigField>,
}

#[derive(Debug, Clone, PartialEq, Serialize)]
pub enum ConfigStatus {
    Complete,
    Missing,
    Partial,
    NotRequired,
}

pub struct SkillStore {
    skills_dir: PathBuf,
    config_store: SkillConfigStore,
    icons_dir: PathBuf,
    cache: RwLock<HashMap<String, SkillInfo>>,
}

impl SkillStore {
    pub fn new() -> Self {
        let skills_dir = get_skills_base_dir();
        let config_store = SkillConfigStore::new(get_config_base_dir());
        let icons_dir = get_icons_base_dir();

        fs::create_dir_all(&skills_dir).ok();
        if let Some(parent) = icons_dir.parent() {
            fs::create_dir_all(parent).ok();
        }

        Self {
            skills_dir,
            config_store,
            icons_dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    pub fn get_skills_dir(&self) -> &PathBuf {
        &self.skills_dir
    }

    pub fn skill_dir(&self, name: &str) -> PathBuf {
        self.skills_dir.join(name)
    }

    pub fn skill_skill_md_path(&self, name: &str) -> PathBuf {
        self.skill_dir(name).join("SKILL.md")
    }

    pub fn skill_icon_path(&self, name: &str) -> Option<PathBuf> {
        let path = self.icons_dir.join(format!("{}.png", name));
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    pub fn list_skills(&self) -> std::io::Result<Vec<SkillInfo>> {
        let mut skills = Vec::new();

        if !self.skills_dir.exists() {
            return Ok(skills);
        }

        for entry in fs::read_dir(&self.skills_dir)? {
            let entry = entry?;
            let path = entry.path();

            if path.is_dir() {
                if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                    if name.starts_with('.') {
                        continue;
                    }

                    if let Ok(skill_info) = self.get_skill_info(name) {
                        skills.push(skill_info);
                    }
                }
            }
        }

        skills.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(skills)
    }

    pub fn get_skill_info(&self, name: &str) -> std::io::Result<SkillInfo> {
        let skill_md_path = self.skill_skill_md_path(name);
        let config = self.config_store.load_config(name).ok();
        let icon_path = self.skill_icon_path(name);

        let (description, emoji, config_schema) = if skill_md_path.exists() {
            if let Ok(content) = fs::read_to_string(&skill_md_path) {
                Self::parse_skill_info_from_md(&content)
            } else {
                (String::new(), None, Vec::new())
            }
        } else {
            (String::new(), None, Vec::new())
        };

        let enabled = config.as_ref().map(|c| c.enabled).unwrap_or(true);
        // has_config is true if the skill declares config fields OR has saved values
        let has_saved = config
            .as_ref()
            .map(|c| !c.settings.is_empty())
            .unwrap_or(false);
        let has_config = !config_schema.is_empty() || has_saved;

        Ok(SkillInfo {
            name: name.to_string(),
            description,
            version: None,
            author: None,
            emoji,
            has_icon: icon_path.is_some(),
            enabled,
            has_config,
            config_schema,
        })
    }

    fn parse_skill_info_from_md(content: &str) -> (String, Option<String>, Vec<ConfigField>) {
        if let Some(schema) = parse_frontmatter(content) {
            let description = if schema.description.is_empty() {
                Self::extract_description_from_body(content)
            } else {
                // Strip surrounding quotes that YAML may leave in plain strings
                schema
                    .description
                    .trim_matches('"')
                    .trim_matches('\'')
                    .to_string()
            };
            return (description, schema.emoji, schema.config);
        }

        // Fallback for skills without frontmatter
        let description = Self::extract_description_from_body(content);
        (description, None, Vec::new())
    }

    fn extract_description_from_body(content: &str) -> String {
        // Skip past frontmatter if present
        let body_start = if content.starts_with("---") {
            content[3..].find("\n---").map(|i| i + 7).unwrap_or(0)
        } else {
            0
        };
        let body = &content[body_start.min(content.len())..];
        for line in body.lines() {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with('-') {
                return trimmed.to_string();
            }
        }
        String::new()
    }

    pub fn get_skill_content(&self, name: &str) -> std::io::Result<String> {
        let path = self.skill_skill_md_path(name);
        if !path.exists() {
            return Err(std::io::Error::new(
                std::io::ErrorKind::NotFound,
                format!("Skill '{}' not found", name),
            ));
        }
        fs::read_to_string(&path)
    }

    pub fn save_skill_content(&self, name: &str, content: &str) -> std::io::Result<()> {
        let dir = self.skill_dir(name);
        fs::create_dir_all(&dir)?;
        let path = self.skill_skill_md_path(name);
        fs::write(&path, content)?;
        self.invalidate_cache(name);
        Ok(())
    }

    pub fn delete_skill(&self, name: &str) -> std::io::Result<()> {
        let dir = self.skill_dir(name);
        if dir.exists() {
            fs::remove_dir_all(&dir)?;
        }
        self.config_store.delete_config(name)?;
        self.invalidate_cache(name);
        Ok(())
    }

    pub fn skill_exists(&self, name: &str) -> bool {
        self.skill_skill_md_path(name).exists()
    }

    pub fn get_env_for_skill(&self, name: &str) -> std::io::Result<HashMap<String, String>> {
        let config = self.config_store.load_config(name)?;
        Ok(config.settings)
    }

    pub fn save_icon(&self, name: &str, icon_data: &[u8]) -> std::io::Result<()> {
        fs::create_dir_all(&self.icons_dir)?;
        let path = self.icons_dir.join(format!("{}.png", name));
        fs::write(&path, icon_data)?;
        self.invalidate_cache(name);
        Ok(())
    }

    fn invalidate_cache(&self, name: &str) {
        if let Ok(mut cache) = self.cache.write() {
            cache.remove(name);
        }
    }
}

impl Default for SkillStore {
    fn default() -> Self {
        Self::new()
    }
}
