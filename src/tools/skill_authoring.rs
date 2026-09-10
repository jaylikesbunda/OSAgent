//! Runtime skill authoring tools: `skill_create` / `skill_update` / `skill_delete`.
//!
//! These replace the manual `.oskill` bundle workflow for new skills. The
//! agent writes plain `SKILL.md` directories via these tools, the loader
//! hot-reloads, and the skill is immediately usable in the same session via
//! `skill` / `skill_action` — no restart, no zip, no `manifest.toml`.
//!
//! Power is retained: `config`, `actions` (http + script), `token_refresh`
//! and `scripts` use the same schema as before. The simplification is the
//! transport (files, not bundles) and who can author (the agent itself).

use crate::error::{OSAgentError, Result};
use crate::skills::{
    load_existing_parts, save_skill, ConfigField, SkillActionSchema, SkillLoader,
    SkillSaveInput, SkillTokenRefreshSchema,
};
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

fn parse_config_fields(value: Option<&Value>) -> Result<Vec<ConfigField>> {
    let Some(v) = value else { return Ok(Vec::new()) };
    if v.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(v.clone())
        .map_err(|e| OSAgentError::ToolExecution(format!("Invalid 'config' array: {}", e)))
}

fn parse_actions(value: Option<&Value>) -> Result<Vec<SkillActionSchema>> {
    let Some(v) = value else { return Ok(Vec::new()) };
    if v.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(v.clone())
        .map_err(|e| OSAgentError::ToolExecution(format!(
            "Invalid 'actions' array (each needs name + type http|script): {}",
            e
        )))
}

fn parse_token_refresh(value: Option<&Value>) -> Result<Option<SkillTokenRefreshSchema>> {
    let Some(v) = value else { return Ok(None) };
    if v.is_null() {
        return Ok(None);
    }
    serde_json::from_value(v.clone())
        .map_err(|e| OSAgentError::ToolExecution(format!("Invalid 'token_refresh': {}", e)))
        .map(Some)
}

fn parse_scripts(value: Option<&Value>) -> Result<HashMap<String, String>> {
    let Some(v) = value else { return Ok(HashMap::new()) };
    if v.is_null() {
        return Ok(HashMap::new());
    }
    let obj = v.as_object().ok_or_else(|| {
        OSAgentError::ToolExecution("'scripts' must be an object of filename -> content.".to_string())
    })?;
    let mut out = HashMap::new();
    for (k, v) in obj {
        let content = v.as_str().ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Script '{}' content must be a string.", k))
        })?;
        out.insert(k.clone(), content.to_string());
    }
    Ok(out)
}

fn require_string(args: &Value, key: &str, what: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .ok_or_else(|| OSAgentError::ToolExecution(format!("Missing '{}': {}.", key, what)))
}

const CREATE_GUIDANCE: &str = "\n\nGuidance for writing a good skill:\n\
- `instructions` is markdown the agent reads when it calls `skill(name)`. Include: what the skill does, when to use it, step-by-step workflows, which tools to call (e.g. web_fetch, bash, tool_script), argument shapes, and error handling.\n\
- Prefer composing existing tools (web_fetch/web_search/bash/tool_script) in instructions. Use `actions` (http) for REST APIs needing saved secrets, `scripts` + script actions only for logic that cannot be expressed otherwise.\n\
- `config` declares secrets the user fills in Settings > Skills (e.g. API_KEY). Reference them in http actions as {{ config.NAME }} and never ask the user to paste secrets into chat.\n\
- Keep instructions focused and concrete with 1-2 short examples.";

pub struct SkillCreateTool {
    loader: Arc<SkillLoader>,
}

impl SkillCreateTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillCreateTool {
    fn name(&self) -> &str {
        "skill_create"
    }

    fn description(&self) -> &str {
        "Create a new runtime skill from instructions (and optional config/actions/scripts) so it can be used immediately in this session"
    }

    fn when_to_use(&self) -> &str {
        "Use when the user asks for a new reusable capability ('make a skill that...', 'teach OSA to...') or when you notice a repeatable workflow worth saving"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for one-off tasks with no reuse value, or to overwrite an existing skill (use skill_update)"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Save a prompt-only skill for triaging inbox mail".to_string(),
            input: json!({
                "name": "inbox-triage",
                "description": "Triage inbox mail into urgent, follow-up and archive",
                "instructions": "# Inbox Triage\n1. Fetch recent mail.\n2. Classify each as urgent/follow-up/archive.\n3. Summarize with suggested replies."
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Skill id: lowercase letters/numbers/hyphens/underscores, 2-64 chars (e.g. 'inbox-triage')" },
                "description": { "type": "string", "description": "One-line description shown in skill_list (max 500 chars)" },
                "instructions": { "type": "string", "description": "Full markdown instructions the agent follows when the skill is used: purpose, workflows, tool calls, examples" },
                "emoji": { "type": "string", "description": "Optional single emoji for the UI" },
                "config": { "type": "array", "description": "Optional config fields needing user secrets (each: {name, type: string|api_key|password|number|boolean, description, required, default})", "items": { "type": "object" } },
                "actions": { "type": "array", "description": "Optional runtime actions (each: {name, description, type: http|script, ...}). HTTP: {method, url, headers, query, body}. Script: {script: 'scripts/file.py', args}. Templates: {{ config.NAME }}, {{ args.param }}.", "items": { "type": "object" } },
                "token_refresh": { "type": "object", "description": "Optional OAuth refresh block (advanced).", "additionalProperties": true },
                "scripts": { "type": "object", "description": "Optional map of script filename (scripts/*.py|sh|ps1|js) to file content, for script actions.", "additionalProperties": { "type": "string" } }
            },
            "required": ["name", "description", "instructions"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = require_string(&args, "name", "skill id like 'inbox-triage'")?;
        let description = require_string(&args, "description", "one-line skill description")?;
        let instructions = require_string(&args, "instructions", "markdown instructions for the skill")?;
        let emoji = args
            .get("emoji")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty());
        let config = parse_config_fields(args.get("config"))?;
        let actions = parse_actions(args.get("actions"))?;
        let token_refresh = parse_token_refresh(args.get("token_refresh"))?;
        let scripts = parse_scripts(args.get("scripts"))?;

        let target = self.loader.primary_dir();
        std::fs::create_dir_all(&target).map_err(|e| {
            OSAgentError::ToolExecution(format!("Skills directory unavailable: {}", e))
        })?;

        let input = SkillSaveInput {
            description,
            emoji,
            instructions,
            config,
            actions,
            token_refresh,
            scripts,
        };
        save_skill(&target, &name, input, false).map_err(OSAgentError::ToolExecution)?;

        self.loader.load_all()?;
        let skill = self.loader.get(&name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!(
                "Skill '{}' was saved but failed to load. Check SKILL.md frontmatter.",
                name
            ))
        })?;

        let mut out = format!(
            "Skill '{}' created and live now ({} action(s)). Call `skill(name=\"{}\")` to read it or `skill_action` to run an action.",
            skill.name,
            skill.actions.len(),
            skill.name
        );
        if !skill.config_fields.is_empty() {
            let required: Vec<_> = skill
                .config_fields
                .iter()
                .filter(|f| f.required)
                .map(|f| f.name.clone())
                .collect();
            if !required.is_empty() {
                out.push_str(&format!(
                    " It needs user configuration before actions run: {} (Settings > Skills).",
                    required.join(", ")
                ));
            }
        }
        out.push_str(CREATE_GUIDANCE);
        Ok(out)
    }
}

pub struct SkillUpdateTool {
    loader: Arc<SkillLoader>,
}

impl SkillUpdateTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillUpdateTool {
    fn name(&self) -> &str {
        "skill_update"
    }

    fn description(&self) -> &str {
        "Update an existing runtime skill's instructions, config, actions or scripts; applies immediately"
    }

    fn when_to_use(&self) -> &str {
        "Use to fix, extend or improve a skill you just created or one the user asks to change"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use to create a brand-new skill (use skill_create)"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Improve an existing skill's workflow".to_string(),
            input: json!({
                "name": "inbox-triage",
                "instructions": "# Inbox Triage\n1. Fetch recent mail.\n2. Classify as urgent/follow-up/archive with reasons.\n3. Draft replies for urgent items."
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Existing skill name" },
                "description": { "type": "string", "description": "Replacement one-line description (omit to keep current)" },
                "instructions": { "type": "string", "description": "Replacement full markdown instructions (omit to keep current)" },
                "emoji": { "type": "string", "description": "Replacement emoji (omit to keep current, empty string clears)" },
                "config": { "type": "array", "description": "Replacement config field array (omit to keep current, [] clears)", "items": { "type": "object" } },
                "actions": { "type": "array", "description": "Replacement actions array (omit to keep current, [] clears)", "items": { "type": "object" } },
                "token_refresh": { "type": "object", "description": "Replacement token_refresh block, or null to clear (omit to keep current)", "additionalProperties": true },
                "scripts": { "type": "object", "description": "Scripts to add/overwrite by filename (existing files not mentioned are kept)", "additionalProperties": { "type": "string" } }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = require_string(&args, "name", "existing skill name")?;
        self.loader.load_all()?;
        if self.loader.get(&name).is_none() {
            return Err(OSAgentError::ToolExecution(format!(
                "Skill '{}' not found. Use skill_list to browse, skill_create to make it.",
                name
            )));
        }

        let target = self.loader.primary_dir();
        // Merge with existing so partial updates work.
        let (existing_schema, existing_body) =
            load_existing_parts(&target, &name).map_err(OSAgentError::ToolExecution)?;

        let description = args
            .get("description")
            .and_then(|v| v.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .unwrap_or(existing_schema.description);
        let instructions = args
            .get("instructions")
            .and_then(|v| v.as_str())
            .map(|s| s.to_string())
            .filter(|s| !s.trim().is_empty())
            .unwrap_or(existing_body);
        let emoji = if let Some(v) = args.get("emoji") {
            if v.is_null() {
                None
            } else if let Some(s) = v.as_str() {
                if s.trim().is_empty() {
                    None
                } else {
                    Some(s.trim().to_string())
                }
            } else {
                existing_schema.emoji.clone()
            }
        } else {
            existing_schema.emoji.clone()
        };
        let config = if args.get("config").is_some() {
            parse_config_fields(args.get("config"))?
        } else {
            existing_schema.config.clone()
        };
        let actions = if args.get("actions").is_some() {
            parse_actions(args.get("actions"))?
        } else {
            existing_schema.actions.clone()
        };
        let token_refresh = if args.get("token_refresh").is_some() {
            parse_token_refresh(args.get("token_refresh"))?
        } else {
            existing_schema.token_refresh.clone()
        };
        let scripts = parse_scripts(args.get("scripts"))?;

        let input = SkillSaveInput {
            description,
            emoji,
            instructions,
            config,
            actions,
            token_refresh,
            scripts,
        };
        save_skill(&target, &name, input, true).map_err(OSAgentError::ToolExecution)?;
        self.loader.load_all()?;

        Ok(format!(
            "Skill '{}' updated and live now. Call `skill(name=\"{}\")` to verify the new instructions.",
            name, name
        ))
    }
}

pub struct SkillDeleteTool {
    loader: Arc<SkillLoader>,
}

impl SkillDeleteTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillDeleteTool {
    fn name(&self) -> &str {
        "skill_delete"
    }

    fn description(&self) -> &str {
        "Delete a runtime skill by name (removes its directory immediately)"
    }

    fn when_to_use(&self) -> &str {
        "Use when the user asks to remove a skill, or to clear the way before recreating one with a different shape"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use without the user asking (or your own failed create needing a clean slate)"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Remove an obsolete skill".to_string(),
            input: json!({ "name": "inbox-triage" }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": { "type": "string", "description": "Skill name to delete" }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = require_string(&args, "name", "skill name to delete")?;
        let target = self.loader.primary_dir();
        // Prefer deleting from the writable primary dir; fall back to any
        // loaded skill location is out of scope (legacy dirs are managed
        // via Settings UI).
        crate::skills::delete_skill(&target, &name).map_err(OSAgentError::ToolExecution)?;
        // Also drop saved config so a recreated skill starts clean.
        let config_store =
            crate::skills::SkillConfigStore::new(crate::skills::get_config_base_dir());
        let _ = config_store.delete_config(&name);
        self.loader.load_all().ok();
        Ok(format!("Skill '{}' deleted.", name))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parses_optional_arrays() {
        assert!(parse_config_fields(None).unwrap().is_empty());
        assert!(parse_config_fields(Some(&json!(null))).unwrap().is_empty());
        assert!(parse_actions(Some(&json!([]))).unwrap().is_empty());
        assert!(parse_scripts(Some(&json!({"a.py": "print(1)"})))
            .unwrap()
            .contains_key("a.py"));
        assert!(parse_token_refresh(None).unwrap().is_none());
    }

    #[test]
    fn rejects_bad_actions_shape() {
        assert!(parse_actions(Some(&json!([{"name": 1}]))).is_err());
    }
}
