use crate::error::{OSAgentError, Result};
use crate::skills::config::{
    parse_frontmatter, ConfigField, SkillActionSchema, SkillTokenRefreshSchema,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use std::sync::RwLock;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub requires: Option<SkillRequirements>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillRequirements {
    #[serde(default)]
    pub bins: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Skill {
    pub name: String,
    pub description: String,
    pub content: String,
    pub base_dir: PathBuf,
    pub config_fields: Vec<ConfigField>,
    pub actions: Vec<SkillActionSchema>,
    pub token_refresh: Option<SkillTokenRefreshSchema>,
    pub scripts: HashMap<String, PathBuf>,
    pub references: HashMap<String, PathBuf>,
    pub metadata: Option<SkillMetadata>,
}

impl Skill {
    pub fn get_script_path(&self, script_name: &str) -> Option<PathBuf> {
        self.scripts.get(script_name).cloned()
    }

    pub fn get_reference_path(&self, reference_name: &str) -> Option<PathBuf> {
        self.references.get(reference_name).cloned()
    }

    pub fn list_scripts(&self) -> Vec<&String> {
        self.scripts.keys().collect()
    }

    pub fn list_references(&self) -> Vec<&String> {
        self.references.keys().collect()
    }
}

pub struct SkillLoader {
    skills_dir: PathBuf,
    additional_skills_dirs: Vec<PathBuf>,
    skills: RwLock<HashMap<String, Skill>>,
}

impl SkillLoader {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            additional_skills_dirs: Vec::new(),
            skills: RwLock::new(HashMap::new()),
        }
    }

    pub fn with_workspace_skills(mut self, workspace_dir: PathBuf) -> Self {
        self.additional_skills_dirs
            .push(workspace_dir.join(".osagent").join("skills"));
        self
    }

    pub fn with_additional_skills_dir(mut self, dir: PathBuf) -> Self {
        self.additional_skills_dirs.push(dir);
        self
    }

    pub fn load_all(&self) -> Result<Vec<String>> {
        let mut loaded_skills = HashMap::new();

        for dir in &self.additional_skills_dirs {
            self.load_from_dir(dir, &mut loaded_skills)?;
        }

        self.load_from_dir(&self.skills_dir, &mut loaded_skills)?;

        let mut skills = self.skills.write().unwrap();
        *skills = loaded_skills;

        Ok(skills.keys().cloned().collect())
    }

    fn load_from_dir(
        &self,
        dir: &PathBuf,
        loaded_skills: &mut HashMap<String, Skill>,
    ) -> Result<()> {
        if !dir.exists() {
            return Ok(());
        }

        let entries = fs::read_dir(dir).map_err(|e| {
            OSAgentError::Io(std::io::Error::other(format!(
                "Failed to read skills directory: {}",
                e
            )))
        })?;

        for entry in entries {
            let entry = entry.map_err(|e| {
                OSAgentError::Io(std::io::Error::other(format!(
                    "Failed to read directory entry: {}",
                    e
                )))
            })?;

            let skill_dir = entry.path();
            if skill_dir.is_dir() {
                match self.load_skill(&skill_dir) {
                    Ok(Some(skill)) => {
                        info!("Loaded skill: {}", skill.name);
                        loaded_skills.insert(skill.name.clone(), skill);
                    }
                    Ok(None) => {
                        warn!("No SKILL.md found in {:?}", skill_dir);
                    }
                    Err(e) => {
                        warn!("Failed to load skill from {:?}: {}", skill_dir, e);
                    }
                }
            }
        }

        Ok(())
    }

    fn load_skill(&self, skill_dir: &PathBuf) -> Result<Option<Skill>> {
        let skill_md = skill_dir.join("SKILL.md");
        if !skill_md.exists() {
            return Ok(None);
        }

        let content = fs::read_to_string(&skill_md).map_err(|e| {
            OSAgentError::Io(std::io::Error::other(format!(
                "Failed to read SKILL.md: {}",
                e
            )))
        })?;

        let (name, description, metadata, config_fields, actions, token_refresh, body) =
            parse_skill_md(&content);

        let mut scripts = HashMap::new();
        let mut references = HashMap::new();

        let scripts_dir = skill_dir.join("scripts");
        if scripts_dir.exists() {
            if let Ok(entries) = fs::read_dir(&scripts_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(filename) = path.file_name() {
                            if let Some(filename_str) = filename.to_str() {
                                let script_name = filename_str
                                    .strip_suffix(".sh")
                                    .or_else(|| filename_str.strip_suffix(".js"))
                                    .or_else(|| filename_str.strip_suffix(".py"))
                                    .unwrap_or(filename_str);
                                scripts.insert(script_name.to_string(), path);
                            }
                        }
                    }
                }
            }
        }

        let refs_dir = skill_dir.join("references");
        if refs_dir.exists() {
            if let Ok(entries) = fs::read_dir(&refs_dir) {
                for entry in entries.flatten() {
                    let path = entry.path();
                    if path.is_file() {
                        if let Some(filename) = path.file_name() {
                            if let Some(filename_str) = filename.to_str() {
                                let ref_name = filename_str
                                    .strip_suffix(".md")
                                    .or_else(|| filename_str.strip_suffix(".txt"))
                                    .unwrap_or(filename_str);
                                references.insert(ref_name.to_string(), path);
                            }
                        }
                    }
                }
            }
        }

        Ok(Some(Skill {
            name,
            description,
            content: body,
            base_dir: skill_dir.clone(),
            config_fields,
            actions,
            token_refresh,
            scripts,
            references,
            metadata,
        }))
    }

    pub fn get(&self, name: &str) -> Option<Skill> {
        self.skills.read().unwrap().get(name).cloned()
    }

    pub fn list(&self) -> Vec<Skill> {
        self.skills.read().unwrap().values().cloned().collect()
    }

    pub fn names(&self) -> Vec<String> {
        self.skills.read().unwrap().keys().cloned().collect()
    }
}

fn parse_skill_md(
    content: &str,
) -> (
    String,
    String,
    Option<SkillMetadata>,
    Vec<ConfigField>,
    Vec<SkillActionSchema>,
    Option<SkillTokenRefreshSchema>,
    String,
) {
    let mut name = String::new();
    let mut description = String::new();
    let mut metadata: Option<SkillMetadata> = None;
    let mut config_fields = Vec::new();
    let mut actions = Vec::new();
    let mut token_refresh = None;
    let mut body_start = 0;

    if let Some(schema) = parse_frontmatter(content) {
        name = schema.name;
        description = if schema.description.is_empty() {
            "No description".to_string()
        } else {
            schema
                .description
                .trim_matches('"')
                .trim_matches('\'')
                .to_string()
        };
        let requires = if schema.requires.bins.is_empty() && schema.requires.files.is_empty() {
            None
        } else {
            Some(SkillRequirements {
                bins: schema.requires.bins,
                files: schema.requires.files,
            })
        };
        metadata = Some(SkillMetadata {
            emoji: schema.emoji,
            icon_url: schema.icon_url,
            requires,
        });
        config_fields = schema.config;
        actions = schema.actions;
        token_refresh = schema.token_refresh;

        if let Some(end) = content[3..].find("---\n") {
            body_start = end + 6;
        }
    }

    // If no frontmatter, extract name from first heading
    if name.is_empty() {
        for line in content.lines() {
            let trimmed = line.trim();
            if trimmed.starts_with("# ") {
                name = trimmed[2..].to_string();
                body_start = content.find(trimmed).unwrap_or(0) + trimmed.len();
                break;
            }
        }
    }

    // Extract description from content after name
    if description.is_empty() {
        let body = &content[body_start.min(content.len())..];
        for line in body.lines().take(3) {
            let trimmed = line.trim();
            if !trimmed.is_empty() && !trimmed.starts_with('#') && !trimmed.starts_with("-") {
                description = trimmed.to_string();
                break;
            }
        }
    }

    // Body is the full content for the skill
    let body = content.to_string();

    (
        name,
        description,
        metadata,
        config_fields,
        actions,
        token_refresh,
        body,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn test_parse_skill_md_with_native_oauth() {
        let content = r#"---
name: spotify
description: "Control Spotify"
emoji: "🎵"
config:
  - name: SPOTIFY_CLIENT_ID
    type: api_key
    description: "Client ID"
    required: true
token_refresh:
  token_url: "https://accounts.spotify.com/api/token"
  grant_type: "refresh_token"
  refresh_token_field: "SPOTIFY_REFRESH_TOKEN"
  access_token_field: "SPOTIFY_ACCESS_TOKEN"
  client_id_field: "SPOTIFY_CLIENT_ID"
  client_secret_field: "SPOTIFY_CLIENT_SECRET"
  authorize_url: "https://accounts.spotify.com/authorize"
  scopes: "user-modify-playback-state user-read-playback-state"
  callback_port: 8888
  redirect_path: "/callback"
actions:
  - name: status
    description: Get status
    type: http
    method: GET
    url: "https://api.spotify.com/v1/me/player"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
---
# Spotify
"#;
        let schema = crate::skills::config::parse_frontmatter(content).expect("should parse");
        let tr = schema.token_refresh.expect("should have token_refresh");
        assert_eq!(
            tr.authorize_url.as_deref(),
            Some("https://accounts.spotify.com/authorize")
        );
        assert_eq!(tr.callback_port, 8888);
        assert!(tr.supports_native_oauth());
        assert!(!schema.actions.iter().any(|a| a.name == "authorize"));
    }

    #[test]
    fn test_parse_skill_md() {
        let content = r#"---
name: github
description: GitHub operations
emoji: "📦"
requires:
  bins: ["gh"]
actions:
  - name: status
    description: Show GitHub status
    type: script
    script: scripts/status.ps1
---

# GitHub Skill

Use `gh` CLI for GitHub operations.
"#;
        let (name, desc, meta, config_fields, actions, _token_refresh, body) =
            parse_skill_md(content);
        assert_eq!(name, "github");
        assert_eq!(desc, "GitHub operations");
        assert!(meta.is_some());
        assert!(config_fields.is_empty());
        assert_eq!(actions.len(), 1);
        assert!(body.contains("GitHub Skill"));
    }

    #[test]
    fn loads_from_additional_directory_before_primary_directory() {
        let temp = TempDir::new().expect("temp dir");
        let primary_dir = temp.path().join("primary");
        let fallback_dir = temp.path().join("fallback");

        std::fs::create_dir_all(primary_dir.join("spotify")).expect("create primary skill dir");
        std::fs::create_dir_all(fallback_dir.join("spotify")).expect("create fallback skill dir");

        std::fs::write(
            primary_dir.join("spotify").join("SKILL.md"),
            "---\nname: spotify\ndescription: Primary skill\n---\n",
        )
        .expect("write primary skill");
        std::fs::write(
            fallback_dir.join("spotify").join("SKILL.md"),
            "---\nname: spotify\ndescription: Fallback skill\n---\n",
        )
        .expect("write fallback skill");

        let loader = SkillLoader::new(primary_dir).with_additional_skills_dir(fallback_dir);
        loader.load_all().expect("load skills");

        let skill = loader.get("spotify").expect("skill should exist");
        assert_eq!(skill.description, "Primary skill");
    }
}
