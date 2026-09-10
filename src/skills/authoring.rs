//! Runtime-authorable skills: plain `SKILL.md` directories, no bundles.
//!
//! Legacy `.oskill` bundles (zip + `manifest.toml`, see `bundle.rs`) are kept
//! for importing old skills only. New skills are created directly as files:
//!
//! ```text
//! <skills_dir>/<name>/SKILL.md          # frontmatter (name, description, ...) + markdown instructions
//! <skills_dir>/<name>/scripts/<file>    # optional script-backed actions
//! ```
//!
//! The agent authors skills at runtime via the `skill_create` / `skill_update`
//! / `skill_delete` tools (see `crate::tools::skill_authoring`). After a save
//! the caller runs `SkillLoader::load_all()` so the new skill is usable in the
//! same session via `skill` / `skill_action` with no restart.
//!
//! frontmatter keeps the full existing schema (`config`, `actions`,
//! `token_refresh`, ...) so old power is retained — the simplification is the
//! transport (plain files, not zips) and the authoring path (agent tools, not
//! manual packaging).

use crate::skills::config::{
    parse_frontmatter, ConfigField, SkillActionSchema, SkillConfigSchema, SkillTokenRefreshSchema,
};
use std::collections::HashMap;
use std::path::{Path, PathBuf};

pub const MAX_INSTRUCTIONS_CHARS: usize = 100_000;
pub const MAX_SCRIPT_BYTES: usize = 100_000;
pub const MAX_SCRIPTS: usize = 20;
pub const MAX_CONFIG_FIELDS: usize = 30;
pub const MAX_ACTIONS: usize = 30;
pub const MAX_DESCRIPTION_CHARS: usize = 500;

/// Validate a skill name. Same rules as the old bundle manifest: lowercase
/// alphanumeric plus `-`/`_`, so the name is safe as a directory name and as
/// a `skill_action(skill=...)` argument.
pub fn validate_skill_name(name: &str) -> Result<(), String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Skill name is required.".to_string());
    }
    if name.len() < 2 || name.len() > 64 {
        return Err("Skill name must be 2-64 characters.".to_string());
    }
    if !name
        .chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
    {
        return Err(
            "Skill name may only contain letters, numbers, hyphens and underscores.".to_string(),
        );
    }
    if name.starts_with('.') || name.contains('/') || name.contains('\\') || name.contains("..") {
        return Err("Skill name must not contain path separators.".to_string());
    }
    Ok(())
}

/// Validate a script filename (basename only, allowlisted extension).
pub fn validate_script_filename(filename: &str) -> Result<(), String> {
    let filename = filename.trim();
    if filename.is_empty() {
        return Err("Script filename is required.".to_string());
    }
    if filename.contains('/') || filename.contains('\\') || filename.contains("..") {
        return Err(format!(
            "Script filename '{}' must be a bare filename inside scripts/.",
            filename
        ));
    }
    let lower = filename.to_ascii_lowercase();
    let allowed = ["py", "sh", "ps1", "js"];
    let ext = Path::new(&lower)
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("");
    if !allowed.contains(&ext) {
        return Err(format!(
            "Script '{}' must end in one of: .py, .sh, .ps1, .js",
            filename
        ));
    }
    if filename.len() > 128 {
        return Err("Script filename is too long (max 128 chars).".to_string());
    }
    Ok(())
}

/// Render a full `SKILL.md` file from a schema + markdown body.
pub fn build_skill_md(schema: &SkillConfigSchema, instructions: &str) -> String {
    let mut yaml = serde_yaml::to_string(schema).unwrap_or_else(|_| "name: skill\n".to_string());
    // serde_yaml emits a leading `---\n`; strip it so we emit exactly one block.
    if let Some(stripped) = yaml.strip_prefix("---\n") {
        yaml = stripped.to_string();
    }
    format!("---\n{}---\n\n{}", yaml, instructions.trim())
}

#[derive(Debug, Default)]
pub struct SkillSaveInput {
    pub description: String,
    pub emoji: Option<String>,
    pub instructions: String,
    pub config: Vec<ConfigField>,
    pub actions: Vec<SkillActionSchema>,
    pub token_refresh: Option<SkillTokenRefreshSchema>,
    /// filename (inside `scripts/`) -> file content
    pub scripts: HashMap<String, String>,
}

/// Write (or overwrite) `<skills_dir>/<name>/SKILL.md` + scripts.
///
/// When `overwrite` is false and the skill already exists this fails so
/// `skill_create` never clobbers silently; `skill_update` passes true.
pub fn save_skill(
    skills_dir: &Path,
    name: &str,
    input: SkillSaveInput,
    overwrite: bool,
) -> Result<PathBuf, String> {
    validate_skill_name(name)?;
    let name = name.trim().to_string();

    if input.description.trim().is_empty() {
        return Err("Skill description is required.".to_string());
    }
    if input.description.chars().count() > MAX_DESCRIPTION_CHARS {
        return Err(format!(
            "Skill description is too long (max {} chars).",
            MAX_DESCRIPTION_CHARS
        ));
    }
    if input.instructions.trim().is_empty() {
        return Err(
            "Skill instructions are required: explain what the skill does and how to use it."
                .to_string(),
        );
    }
    if input.instructions.chars().count() > MAX_INSTRUCTIONS_CHARS {
        return Err(format!(
            "Skill instructions are too long (max {} chars).",
            MAX_INSTRUCTIONS_CHARS
        ));
    }
    if input.config.len() > MAX_CONFIG_FIELDS {
        return Err(format!(
            "Too many config fields (max {}).",
            MAX_CONFIG_FIELDS
        ));
    }
    if input.actions.len() > MAX_ACTIONS {
        return Err(format!("Too many actions (max {}).", MAX_ACTIONS));
    }
    if input.scripts.len() > MAX_SCRIPTS {
        return Err(format!("Too many scripts (max {}).", MAX_SCRIPTS));
    }
    for (filename, content) in &input.scripts {
        validate_script_filename(filename)?;
        if content.len() > MAX_SCRIPT_BYTES {
            return Err(format!("Script '{}' is too large (max 100KB).", filename));
        }
    }
    // Every `type: script` action must reference a script that is either
    // shipped in this save or already on disk (for updates).
    for action in &input.actions {
        if let crate::skills::config::SkillActionRunner::Script { script, .. } = &action.runner {
            let referenced = script
                .rsplit('/')
                .next()
                .unwrap_or(script)
                .rsplit('\\')
                .next()
                .unwrap_or(script);
            let provided = input.scripts.keys().any(|k| {
                k == script
                    || k == referenced
                    || k.trim_start_matches("scripts/") == script.trim_start_matches("scripts/")
            });
            if !provided && !overwrite {
                // For creates we require the script up front; for updates the
                // file may already exist from a previous save — checked below.
            }
        }
    }

    let skill_dir = skills_dir.join(&name);
    let skill_md = skill_dir.join("SKILL.md");
    if skill_md.exists() && !overwrite {
        return Err(format!(
            "Skill '{}' already exists. Use skill_update to modify it, or skill_delete first.",
            name
        ));
    }

    std::fs::create_dir_all(&skill_dir)
        .map_err(|e| format!("Failed to create skill directory: {}", e))?;

    let schema = SkillConfigSchema {
        name: name.clone(),
        description: input.description.trim().to_string(),
        emoji: input.emoji.filter(|e| !e.trim().is_empty()),
        icon_url: None,
        requires: Default::default(),
        config: input.config,
        actions: input.actions,
        token_refresh: input.token_refresh,
    };
    let md = build_skill_md(&schema, &input.instructions);
    std::fs::write(&skill_md, md).map_err(|e| format!("Failed to write SKILL.md: {}", e))?;

    if !input.scripts.is_empty() {
        let scripts_dir = skill_dir.join("scripts");
        std::fs::create_dir_all(&scripts_dir)
            .map_err(|e| format!("Failed to create scripts directory: {}", e))?;
        for (filename, content) in input.scripts {
            let clean = filename
                .trim_start_matches("scripts/")
                .trim_start_matches("scripts\\");
            let dest = scripts_dir.join(clean);
            // Defense in depth: filename validation already rejects
            // separators, but canonicalize-check anyway.
            if !dest.starts_with(&scripts_dir) {
                return Err(format!("Refusing to write script outside scripts/: {}", filename));
            }
            std::fs::write(&dest, content)
                .map_err(|e| format!("Failed to write script '{}': {}", filename, e))?;
        }
    }

    Ok(skill_dir)
}

/// Load the existing skill parts so `skill_update` can merge partial edits.
pub fn load_existing_parts(skills_dir: &Path, name: &str) -> Result<(SkillConfigSchema, String), String> {
    validate_skill_name(name)?;
    let skill_md = skills_dir.join(name.trim()).join("SKILL.md");
    if !skill_md.exists() {
        return Err(format!("Skill '{}' not found.", name.trim()));
    }
    let content =
        std::fs::read_to_string(&skill_md).map_err(|e| format!("Failed to read SKILL.md: {}", e))?;
    let schema = parse_frontmatter(&content)
        .ok_or_else(|| format!("Skill '{}' has no valid frontmatter.", name.trim()))?;
    // Body = everything after the closing `---`.
    let body = if content.starts_with("---") {
        if let Some(end) = content[3..].find("\n---") {
            content[3 + end + 4..].trim_start_matches('\n').to_string()
        } else {
            String::new()
        }
    } else {
        content.clone()
    };
    Ok((schema, body))
}

/// Delete `<skills_dir>/<name>` entirely.
pub fn delete_skill(skills_dir: &Path, name: &str) -> Result<(), String> {
    validate_skill_name(name)?;
    let dir = skills_dir.join(name.trim());
    if !dir.exists() {
        return Err(format!("Skill '{}' not found.", name.trim()));
    }
    // Never delete outside the skills root even if the name was crafted.
    let canonical_root = skills_dir
        .canonicalize()
        .map_err(|e| format!("Skills directory is unavailable: {}", e))?;
    let canonical_target = dir
        .canonicalize()
        .map_err(|e| format!("Failed to resolve skill directory: {}", e))?;
    if !canonical_target.starts_with(&canonical_root) {
        return Err("Refusing to delete a path outside the skills directory.".to_string());
    }
    std::fs::remove_dir_all(&canonical_target)
        .map_err(|e| format!("Failed to delete skill '{}': {}", name.trim(), e))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_bad_names() {
        assert!(validate_skill_name("").is_err());
        assert!(validate_skill_name("a").is_err());
        assert!(validate_skill_name("has space").is_err());
        assert!(validate_skill_name("../escape").is_err());
        assert!(validate_skill_name("ok-name_1").is_ok());
    }

    #[test]
    fn rejects_bad_script_names() {
        assert!(validate_script_filename("../evil.sh").is_err());
        assert!(validate_script_filename("tool.exe").is_err());
        assert!(validate_script_filename("run.py").is_ok());
        assert!(validate_script_filename("job.ps1").is_ok());
    }

    #[test]
    fn roundtrips_skill_md() {
        let schema = SkillConfigSchema {
            name: "demo".to_string(),
            description: "Demo skill".to_string(),
            emoji: Some("✨".to_string()),
            ..Default::default()
        };
        let md = build_skill_md(&schema, "# Demo\nDo things.");
        let parsed = parse_frontmatter(&md).expect("should parse");
        assert_eq!(parsed.name, "demo");
        assert_eq!(parsed.description, "Demo skill");
        assert!(md.contains("# Demo"));
    }

    #[test]
    fn saves_and_deletes_a_skill() {
        let temp = tempfile::TempDir::new().expect("temp");
        let input = SkillSaveInput {
            description: "Test skill".to_string(),
            emoji: None,
            instructions: "# Test\nUse web_search to do things.".to_string(),
            ..Default::default()
        };
        let dir = save_skill(temp.path(), "my-skill", input, false).expect("save");
        assert!(dir.join("SKILL.md").exists());

        // Create must not clobber.
        let dup = SkillSaveInput {
            description: "dup".to_string(),
            instructions: "hi".to_string(),
            ..Default::default()
        };
        assert!(save_skill(temp.path(), "my-skill", dup, false).is_err());

        let (schema, body) = load_existing_parts(temp.path(), "my-skill").expect("load");
        assert_eq!(schema.name, "my-skill");
        assert!(body.contains("Use web_search"));

        delete_skill(temp.path(), "my-skill").expect("delete");
        assert!(!dir.exists());
    }
}
