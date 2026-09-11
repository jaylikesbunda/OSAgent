use crate::skills::bundle::{get_icons_base_dir, get_skills_base_dir};
use crate::skills::config::{
    get_config_base_dir, parse_frontmatter, ConfigField, SkillConfigStore,
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
    pub icon_url: Option<String>,
    pub has_icon: bool,
    pub enabled: bool,
    pub has_config: bool,
    pub config_schema: Vec<ConfigField>,
}

pub struct SkillStore {
    /// Search roots, primary first. The primary is the configured
    /// `tools.skills.directory` (where agent tools + UI saves write); the
    /// legacy `get_skills_base_dir()` follows so old installs keep working.
    /// Primary wins on name collisions, mirroring `SkillLoader`.
    roots: Vec<PathBuf>,
    config_store: SkillConfigStore,
    icons_dir: PathBuf,
    cache: RwLock<HashMap<String, SkillInfo>>,
}

fn dedup_roots(roots: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in roots {
        if !out.iter().any(|existing| existing == &root) {
            out.push(root);
        }
    }
    out
}

fn config_skills_dir() -> PathBuf {
    PathBuf::from(shellexpand::tilde("~/.osagent/skills").to_string())
}

impl SkillStore {
    pub fn new() -> Self {
        // Default preserves legacy behavior: primary = legacy base dir so
        // existing installs, bundle imports and tests keep working.
        // Production wires `with_primary(configured_dir)` instead.
        Self::with_roots(vec![get_skills_base_dir(), config_skills_dir()])
    }

    /// Primary = caller-provided dir (e.g. configured `tools.skills.directory`).
    pub fn with_primary(primary: PathBuf) -> Self {
        Self::with_roots(vec![primary, get_skills_base_dir()])
    }

    fn with_roots(roots: Vec<PathBuf>) -> Self {
        let roots = dedup_roots(roots);
        let config_store = SkillConfigStore::new(get_config_base_dir());
        let icons_dir = get_icons_base_dir();

        for root in &roots {
            fs::create_dir_all(root).ok();
        }
        if let Some(parent) = icons_dir.parent() {
            fs::create_dir_all(parent).ok();
        }

        Self {
            roots,
            config_store,
            icons_dir,
            cache: RwLock::new(HashMap::new()),
        }
    }

    /// Writable root: new installs/saves go here so agent tools and the
    /// Settings UI converge on one location.
    pub fn primary_root(&self) -> PathBuf {
        self.roots
            .first()
            .cloned()
            .unwrap_or_else(get_skills_base_dir)
    }

    /// Resolve the on-disk directory for a skill, primary first.
    fn resolve_dir(&self, name: &str) -> Option<PathBuf> {
        for root in &self.roots {
            let dir = root.join(name);
            if dir.join("SKILL.md").exists() {
                return Some(dir);
            }
        }
        None
    }

    pub fn skill_dir(&self, name: &str) -> PathBuf {
        self.resolve_dir(name).unwrap_or_else(|| self.primary_root().join(name))
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
        let mut seen = std::collections::HashSet::new();
        let mut skills = Vec::new();

        for root in &self.roots {
            if !root.exists() {
                continue;
            }
            for entry in fs::read_dir(root)? {
                let entry = entry?;
                let path = entry.path();

                if path.is_dir() {
                    if let Some(name) = path.file_name().and_then(|n| n.to_str()) {
                        if name.starts_with('.') || !seen.insert(name.to_string()) {
                            continue;
                        }

                        if let Ok(skill_info) = self.get_skill_info(name) {
                            skills.push(skill_info);
                        }
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

        let (description, emoji, icon_url, config_schema) = if skill_md_path.exists() {
            if let Ok(content) = fs::read_to_string(&skill_md_path) {
                Self::parse_skill_info_from_md(&content)
            } else {
                (String::new(), None, None, Vec::new())
            }
        } else {
            (String::new(), None, None, Vec::new())
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
            icon_url,
            has_icon: icon_path.is_some(),
            enabled,
            has_config,
            config_schema,
        })
    }

    fn parse_skill_info_from_md(
        content: &str,
    ) -> (String, Option<String>, Option<String>, Vec<ConfigField>) {
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
            return (description, schema.emoji, schema.icon_url, schema.config);
        }

        // Fallback for skills without frontmatter
        let description = Self::extract_description_from_body(content);
        (description, None, None, Vec::new())
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

    pub fn delete_skill(&self, name: &str) -> std::io::Result<()> {
        let mut removed = false;
        for root in &self.roots {
            let dir = root.join(name);
            if dir.exists() {
                fs::remove_dir_all(&dir)?;
                removed = true;
            }
        }
        let _ = removed;
        self.config_store.delete_config(name)?;
        self.invalidate_cache(name);
        Ok(())
    }

    pub fn skill_exists(&self, name: &str) -> bool {
        self.resolve_dir(name).is_some()
    }

    pub fn get_env_for_skill(&self, name: &str) -> std::io::Result<HashMap<String, String>> {
        let config = self.config_store.load_config(name)?;
        Ok(config.settings)
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
