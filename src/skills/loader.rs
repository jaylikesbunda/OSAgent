use crate::error::{OSAgentError, Result};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;
use tracing::{info, warn};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillMetadata {
    #[serde(default)]
    pub emoji: Option<String>,
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
    workspace_skills_dir: Option<PathBuf>,
    skills: HashMap<String, Skill>,
}

impl SkillLoader {
    pub fn new(skills_dir: PathBuf) -> Self {
        Self {
            skills_dir,
            workspace_skills_dir: None,
            skills: HashMap::new(),
        }
    }

    pub fn with_workspace_skills(mut self, workspace_dir: PathBuf) -> Self {
        self.workspace_skills_dir = Some(workspace_dir.join(".osagent").join("skills"));
        self
    }

    pub fn load_all(&mut self) -> Result<Vec<String>> {
        self.skills.clear();

        let skills_dir = self.skills_dir.clone();
        self.load_from_dir(&skills_dir)?;

        if let Some(ref workspace_dir) = self.workspace_skills_dir {
            if workspace_dir.exists() {
                let workspace_dir_clone = workspace_dir.clone();
                self.load_from_dir(&workspace_dir_clone)?;
            }
        }

        Ok(self.skills.keys().cloned().collect())
    }

    fn load_from_dir(&mut self, dir: &PathBuf) -> Result<()> {
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
                        self.skills.insert(skill.name.clone(), skill);
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

        let (name, description, metadata, body) = parse_skill_md(&content);

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
            scripts,
            references,
            metadata,
        }))
    }

    pub fn get(&self, name: &str) -> Option<&Skill> {
        self.skills.get(name)
    }

    pub fn list(&self) -> Vec<&Skill> {
        self.skills.values().collect()
    }

    pub fn names(&self) -> Vec<&String> {
        self.skills.keys().collect()
    }
}

fn parse_skill_md(content: &str) -> (String, String, Option<SkillMetadata>, String) {
    let mut name = String::new();
    let mut description = String::new();
    let mut metadata: Option<SkillMetadata> = None;
    let mut body_start = 0;

    if content.starts_with("---") {
        if let Some(end) = content[3..].find("---\n") {
            let frontmatter = &content[3..end + 3];
            body_start = end + 6;

            let lines = frontmatter.lines();
            let mut name_line: Option<String> = None;
            let mut desc_line: Option<String> = None;
            let mut emoji: Option<String> = None;
            let mut requires_bins: Vec<String> = Vec::new();

            for line in lines {
                let line = line.trim();
                if line.starts_with("name:") {
                    name_line = Some(line[5..].trim().to_string());
                } else if line.starts_with("description:") {
                    desc_line = Some(line[12..].trim().to_string());
                } else if line.starts_with("emoji:") {
                    emoji = Some(line[6..].trim().to_string());
                } else if line.starts_with("bins:") {
                    // Simple array parsing
                    if let Some(rest) = line[5..].strip_prefix('[') {
                        if let Some(inner) = rest.strip_suffix(']') {
                            for item in inner.split(',') {
                                let item = item.trim().trim_matches('"').trim_matches('\'');
                                if !item.is_empty() {
                                    requires_bins.push(item.to_string());
                                }
                            }
                        }
                    }
                }
            }

            name = name_line.unwrap_or_default();
            description = desc_line.unwrap_or_else(|| "No description".to_string());

            let requires = if requires_bins.is_empty() {
                None
            } else {
                Some(SkillRequirements {
                    bins: requires_bins,
                    files: Vec::new(),
                })
            };

            metadata = Some(SkillMetadata { emoji, requires });
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

    (name, description, metadata, body)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_parse_skill_md() {
        let content = r#"---
name: github
description: GitHub operations
metadata:
  osa:
    emoji: "📦"
    requires:
      bins: ["gh"]
---

# GitHub Skill

Use `gh` CLI for GitHub operations.
"#;
        let (name, desc, meta, body) = parse_skill_md(content);
        assert_eq!(name, "github");
        assert_eq!(desc, "GitHub operations");
        assert!(meta.is_some());
        assert!(body.contains("GitHub Skill"));
    }
}
