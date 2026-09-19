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
    load_existing_parts, save_skill, ConfigField, SkillActionSchema, SkillLoader, SkillSaveInput,
    SkillTokenRefreshSchema,
};
use crate::tools::registry::Tool;
use async_trait::async_trait;
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::Arc;

fn parse_config_fields(value: Option<&Value>) -> Result<Vec<ConfigField>> {
    let Some(v) = value else {
        return Ok(Vec::new());
    };
    if v.is_null() {
        return Ok(Vec::new());
    }
    serde_json::from_value(v.clone())
        .map_err(|e| OSAgentError::ToolExecution(format!("Invalid 'config' array: {}", e)))
}

fn parse_actions(value: Option<&Value>) -> Result<Vec<SkillActionSchema>> {
    let Some(v) = value else {
        return Ok(Vec::new());
    };
    if v.is_null() {
        return Ok(Vec::new());
    }
    let arr = v.as_array().ok_or_else(|| {
        OSAgentError::ToolExecution(format!(
            "Invalid 'actions': expected an array. Example: {}",
            ACTIONS_EXAMPLE
        ))
    })?;
    let mut normalized = Vec::with_capacity(arr.len());
    for action in arr {
        let mut action = action.clone();
        // Tolerate JSON-Schema style `parameters: {properties: {...}, required: [...]}`.
        if let Some(obj) = action.as_object_mut() {
            normalize_parameters_field(obj)?;
            normalize_script_args_field(obj)?;
        }
        normalized.push(action);
    }
    serde_json::from_value(Value::Array(normalized)).map_err(|e| {
        OSAgentError::ToolExecution(format!(
            "Invalid 'actions' array: {}. Expected: {}. Please rewrite the input so it satisfies the expected schema.",
            e, ACTIONS_EXAMPLE
        ))
    })
}

/// Normalize the `parameters` field of one action object in place:
/// accept the canonical array, or a JSON-Schema style
/// `{properties: {...}, required: [...]}` map.
fn normalize_parameters_field(obj: &mut serde_json::Map<String, Value>) -> Result<()> {
    let action_name = obj
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("?")
        .to_string();
    match obj.get("parameters") {
        None | Some(Value::Null) => {}
        Some(params) if params.is_array() => {}
        Some(params) if params.is_object() => match schema_object_to_params(params) {
            Ok(list) => {
                obj.insert("parameters".to_string(), Value::Array(list));
            }
            Err(e) => {
                return Err(OSAgentError::ToolExecution(format!(
                    "Invalid 'parameters' for action '{}': {}. Expected array like {{\"name\": \"domain\", \"type\": \"string\", \"required\": true}}. Please rewrite the input so it satisfies the expected schema.",
                    action_name, e
                )));
            }
        },
        Some(_) => {
            return Err(OSAgentError::ToolExecution(format!(
                "Invalid 'parameters' for action '{}': expected an array of {{\"name\", \"type\": string|number|boolean, \"required\"}}. Please rewrite the input so it satisfies the expected schema.",
                action_name
            )));
        }
    }
    Ok(())
}

/// Normalize the `args` field of a script action in place. Models often write
/// `args` as a map (`{"url": "description or template"}`) instead of the
/// canonical template array (`["{{{{ args.url }}}}"]`). Accept the map:
/// values containing `{{...}}` are kept as templates, anything else is
/// treated as a description and replaced with a `{{{{ args.key }}}}` template.
/// When `parameters` is absent, derive string parameters from the template
/// references so the action validates.
fn normalize_script_args_field(obj: &mut serde_json::Map<String, Value>) -> Result<()> {
    let action_name = obj
        .get("name")
        .and_then(|n| n.as_str())
        .unwrap_or("?")
        .to_string();
    let is_script = obj
        .get("type")
        .and_then(|t| t.as_str())
        .map(|t| t.eq_ignore_ascii_case("script"))
        .unwrap_or(false);

    if let Some(args) = obj.get("args") {
        if args.is_object() {
            if !is_script {
                return Err(OSAgentError::ToolExecution(format!(
                    "Invalid 'args' for action '{}': only script actions accept 'args', and http actions take headers/query/body instead. Please rewrite the input so it satisfies the expected schema.",
                    action_name
                )));
            }
            let map = args.as_object().expect("checked is_object");
            let mut templates = Vec::with_capacity(map.len());
            let mut derived_params = Vec::with_capacity(map.len());
            for (key, value) in map {
                let template = match value {
                    Value::String(text) if text.contains("{{") => text.clone(),
                    Value::String(text) if text.trim().is_empty() => {
                        format!("{{{{ args.{} }}}}", key)
                    }
                    Value::String(description) => {
                        derived_params.push(serde_json::json!({
                            "name": key,
                            "type": "string",
                            "description": description,
                            "required": true,
                        }));
                        format!("{{{{ args.{} }}}}", key)
                    }
                    _ => format!("{{{{ args.{} }}}}", key),
                };
                templates.push(Value::String(template));
            }
            obj.insert("args".to_string(), Value::Array(templates));
            let params_missing = obj
                .get("parameters")
                .map(|p| p.is_null() || p.as_array().map(|a| a.is_empty()).unwrap_or(false))
                .unwrap_or(true);
            if params_missing && !derived_params.is_empty() {
                obj.insert("parameters".to_string(), Value::Array(derived_params));
            }
        } else if !args.is_array() {
            return Err(OSAgentError::ToolExecution(format!(
                "Invalid 'args' for action '{}': expected an array of templates like [\"{{{{ args.url }}}}\"] or an object mapping parameter names to templates. Please rewrite the input so it satisfies the expected schema.",
                action_name
            )));
        }
    }

    // Fixed convention: a script action's CLI args are its parameters in
    // declared order. When `args` is absent (or an empty array), generate
    // `["{{ args.p1 }}", ...]` automatically so the model never hand-writes
    // the binding. Explicit `args` (old skills, custom ordering) is honored.
    let args_missing = obj
        .get("args")
        .map(|a| a.is_null() || a.as_array().map(|a| a.is_empty()).unwrap_or(false))
        .unwrap_or(true);
    if args_missing && is_script {
        if let Some(params) = obj.get("parameters").and_then(|p| p.as_array()) {
            let generated: Vec<Value> = params
                .iter()
                .filter_map(|p| p.get("name").and_then(|n| n.as_str()))
                .map(|name| Value::String(format!("{{{{ args.{} }}}}", name)))
                .collect();
            if !generated.is_empty() {
                obj.insert("args".to_string(), Value::Array(generated));
            }
        }
    }

    // Derive parameters from `{{ args.name }}` template references when the
    // action declares none, so script actions validate out of the box.
    let params_missing = obj
        .get("parameters")
        .map(|p| p.is_null() || p.as_array().map(|a| a.is_empty()).unwrap_or(false))
        .unwrap_or(true);
    if params_missing {
        let mut refs = Vec::new();
        for source in ["args"] {
            if let Some(Value::Array(items)) = obj.get(source) {
                for item in items {
                    if let Some(text) = item.as_str() {
                        refs.extend(arg_template_refs(text));
                    }
                }
            }
        }
        if !refs.is_empty() {
            let mut seen = std::collections::HashSet::new();
            let derived: Vec<Value> = refs
                .into_iter()
                .filter(|name| seen.insert(name.clone()))
                .map(|name| {
                    serde_json::json!({
                        "name": name,
                        "type": "string",
                        "description": "",
                        "required": true,
                    })
                })
                .collect();
            obj.insert("parameters".to_string(), Value::Array(derived));
        }
    }

    Ok(())
}

/// Surgical per-action edits: merge each patch object into the existing
/// action with the same `name`, leaving every other action and every
/// unmentioned field exactly as they are. Only `name` is required in a patch;
/// any other key overwrites that field (`description`, `parameters`, `args`,
/// `script`, `method`, `url`, `headers`, ...). A `null` value removes an
/// optional field. Unknown action names fail with the valid names listed.
///
/// This is the hard-to-mess-up path for tweaks ("change the check
/// description"): unlike the full `actions` replacement, a patch can never
/// silently flip an action's `type` or drop its `script`/`parameters`.
pub fn apply_action_patches(
    existing: &[SkillActionSchema],
    patches: &[Value],
) -> std::result::Result<Vec<SkillActionSchema>, String> {
    let mut current: Vec<Value> = existing
        .iter()
        .map(|action| {
            serde_json::to_value(action).map_err(|e| format!("Failed to encode action: {}", e))
        })
        .collect::<std::result::Result<_, _>>()?;

    for patch in patches {
        let patch_obj = patch.as_object().ok_or_else(|| {
            "Each action_patch must be an object with at least a 'name' field.".to_string()
        })?;
        let name = patch_obj
            .get("name")
            .and_then(|n| n.as_str())
            .map(|s| s.trim().to_string())
            .filter(|s| !s.is_empty())
            .ok_or_else(|| {
                "Each action_patch needs a 'name' field matching an existing action.".to_string()
            })?;
        let index = current
            .iter()
            .position(|action| action.get("name").and_then(|n| n.as_str()) == Some(&name))
            .ok_or_else(|| {
                let valid: Vec<String> = current
                    .iter()
                    .filter_map(|action| {
                        action.get("name").and_then(|n| n.as_str()).map(|s| s.to_string())
                    })
                    .collect();
                format!(
                    "Skill has no action '{}'. Valid actions: {}. To add a new action, use the full 'actions' array instead.",
                    name,
                    if valid.is_empty() {
                        "(none)".to_string()
                    } else {
                        valid.join(", ")
                    }
                )
            })?;
        let target = current[index]
            .as_object_mut()
            .expect("action serialized as object");
        for (key, value) in patch_obj {
            if key == "name" {
                continue;
            }
            if value.is_null() {
                target.remove(key);
            } else {
                target.insert(key.clone(), value.clone());
            }
        }
        // Reuse the same leniency as creation (map-style parameters/args).
        normalize_parameters_field(target).map_err(|e| e.to_string())?;
        normalize_script_args_field(target).map_err(|e| e.to_string())?;
    }

    current
        .into_iter()
        .map(|action| {
            serde_json::from_value(action).map_err(|e| {
                format!(
                    "Patched action is invalid: {}. Send the complete corrected action via the full 'actions' array instead.",
                    e
                )
            })
        })
        .collect()
}

/// Extract `name` from every `{{ args.name }}` reference in a template string.
fn arg_template_refs(template: &str) -> Vec<String> {
    let mut refs = Vec::new();
    let mut rest = template;
    while let Some(start) = rest.find("{{") {
        let after = &rest[start + 2..];
        if let Some(end) = after.find("}}") {
            let expr = after[..end].trim();
            if let Some(name) = expr.strip_prefix("args.") {
                let name = name.trim();
                if !name.is_empty()
                    && name
                        .chars()
                        .all(|c| c.is_alphanumeric() || c == '_' || c == '-')
                {
                    refs.push(name.to_string());
                }
            }
            rest = &after[end + 2..];
        } else {
            break;
        }
    }
    refs
}

/// Convert `{properties: {domain: {type, description}}, required: [...]}` into
/// `[{name, type, description, required}]`.
fn schema_object_to_params(schema: &Value) -> std::result::Result<Vec<Value>, String> {
    let properties = schema
        .get("properties")
        .and_then(|p| p.as_object())
        .ok_or_else(|| "object must contain a 'properties' map".to_string())?;
    let required: Vec<String> = schema
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let mut out = Vec::new();
    for (name, spec) in properties {
        let param_type = spec
            .get("type")
            .and_then(|t| t.as_str())
            .unwrap_or("string");
        // Normalize integer -> number; anything unknown -> string.
        let normalized_type = match param_type {
            "number" | "integer" => "number",
            "boolean" => "boolean",
            _ => "string",
        };
        let description = spec
            .get("description")
            .and_then(|d| d.as_str())
            .unwrap_or("");
        out.push(serde_json::json!({
            "name": name,
            "type": normalized_type,
            "description": description,
            "required": required.iter().any(|r| r == name),
        }));
    }
    Ok(out)
}

const ACTIONS_EXAMPLE: &str = r#"[{"name": "recon", "description": "Run recon", "type": "http", "method": "GET", "url": "https://api.example.com/recon?q={{ args.domain }}", "parameters": [{"name": "domain", "type": "string", "description": "Target domain", "required": true}]}] for HTTP, or [{"name": "clean", "description": "Clean CSV", "type": "script", "script": "scripts/clean.py", "parameters": [{"name": "path", "type": "string", "required": true}]}] for scripts"#;

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
    let Some(v) = value else {
        return Ok(HashMap::new());
    };
    if v.is_null() {
        return Ok(HashMap::new());
    }
    let obj = v.as_object().ok_or_else(|| {
        OSAgentError::ToolExecution(
            "'scripts' must be an object of filename -> content.".to_string(),
        )
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
- Prefer a script action (python stdlib) over HTTP actions: scripts read config secrets from env vars (e.g. `os.environ[\"API_KEY\"]`) and need no template syntax. Only write an `http` action for a trivial single REST call with saved secrets.\n\
- Script contract (fixed, nothing to declare): each parameter arrives as CLI args in `parameters` order (`sys.argv[1]`, `sys.argv[2]`, ...) AND the full object as JSON in `OSA_SKILL_ARGS_JSON` plus per-arg `OSA_SKILL_ARG_<NAME>` env vars. Never declare `args` — it is generated from `parameters` automatically.\n\
- An action is just `{name, description, script: \"scripts/file.py\", parameters: [{name, required}]}` — parameter `type` defaults to string. Keep `config` for secrets the user fills in Settings > Skills, and never ask the user to paste secrets into chat.\n\
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
        "Create a new runtime skill from instructions so it can be used immediately in this session. For executable actions, include the action definition and its script source in the same call; skill_create writes the files itself."
    }

    fn when_to_use(&self) -> &str {
        "Use when the user asks for a new reusable capability ('make a skill that...', 'teach OSA to...') or when you notice a repeatable workflow worth saving"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use for one-off tasks with no reuse value, or to overwrite an existing skill (use skill_update)"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Save a prompt-only skill for triaging inbox mail".to_string(),
                input: json!({
                    "name": "inbox-triage",
                    "description": "Triage inbox mail into urgent, follow-up and archive",
                    "instructions": "# Inbox Triage\n1. Fetch recent mail.\n2. Classify each as urgent/follow-up/archive.\n3. Summarize with suggested replies."
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Create a script-backed runtime action".to_string(),
                input: json!({
                    "name": "text-normalizer",
                    "description": "Normalize text for downstream processing",
                    "instructions": "# Text Normalizer\nUse the normalize action for reusable text cleanup.",
                    "actions": [{
                        "name": "normalize",
                        "description": "Trim whitespace and lowercase text",
                        "type": "script",
                        "script": "scripts/normalize.py",
                        "parameters": [{ "name": "text", "type": "string", "required": true }]
                    }],
                    "scripts": {
                        "normalize.py": "import sys\nprint(sys.argv[1].strip().lower())\n"
                    }
                }),
            },
        ]
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
                "actions": { "type": "array", "description": "Optional runtime actions. If the user asks for an executable action, include it here and provide its implementation in the scripts map in this same call. Script action shape: {name, description, type: 'script', script: 'scripts/file.py', parameters: [{name, required}]} — CLI args and type are automatic, omit them. Only use {type: http, method, url, ...} for a trivial single REST call; prefer scripts.", "items": { "type": "object" } },
                "token_refresh": { "type": "object", "description": "Optional OAuth refresh block (advanced).", "additionalProperties": true },
                "scripts": { "type": "object", "description": "Map of script filename (scripts/*.py|sh|ps1|js) to source code for script actions. Include the complete script here; skill_create writes it automatically. Do not use shell or file tools to create the script.", "additionalProperties": { "type": "string" } }
            },
            "required": ["name", "description", "instructions"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = require_string(&args, "name", "skill id like 'inbox-triage'")?;
        let description = require_string(&args, "description", "one-line skill description")?;
        let instructions =
            require_string(&args, "instructions", "markdown instructions for the skill")?;
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
        if skill.actions.is_empty() {
            out.push_str(" This is a prompt-only skill: no runnable skill_action actions were created. For an executable action, use skill_update with an actions entry and its complete source in scripts.");
        }
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
        "Update an existing runtime skill's instructions, config, actions or scripts; applies immediately. Prefer action_patches for small action tweaks"
    }

    fn when_to_use(&self) -> &str {
        "Use to fix, extend or improve a skill. For small action tweaks (rename, reword a description) use action_patches with just {name + changed fields}; only send the full actions array when restructuring"
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
                "actions": { "type": "array", "description": "FULL replacement of the actions array (omit to keep current, [] clears all). Dangerous: you must resend every action completely. Prefer action_patches for tweaks.", "items": { "type": "object" } },
                "action_patches": { "type": "array", "description": "Surgical per-action edits, merged by name; all other actions and unmentioned fields are left untouched. Each item needs 'name' plus only the fields to change, e.g. [{\"name\": \"check\", \"description\": \"new text\"}]. Cannot add new actions (use actions for that).", "items": { "type": "object" } },
                "token_refresh": { "type": "object", "description": "Replacement token_refresh block, or null to clear (omit to keep current)", "additionalProperties": true },
                "scripts": { "type": "object", "description": "Scripts to add/overwrite by filename (existing files not mentioned are kept)", "additionalProperties": { "type": "string" } }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let name = require_string(&args, "name", "existing skill name")?;
        self.loader.load_all()?;
        let existing_skill = self.loader.get(&name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!(
                "Skill '{}' not found. Use skill_list to browse, skill_create to make it.",
                name
            ))
        })?;

        let target = self.loader.primary_dir();
        // Merge with existing so partial updates work. The skill may live in
        // a fallback dir (legacy installs); read from its actual location,
        // then save to primary (migrating it so the UI and agent converge).
        let (existing_schema, existing_body) = load_existing_parts(&target, &name)
            .or_else(|_| {
                existing_skill
                    .base_dir
                    .parent()
                    .map(|root| load_existing_parts(root, &name))
                    .unwrap_or_else(|| Err(format!("Skill '{}' not found.", name)))
            })
            .map_err(OSAgentError::ToolExecution)?;
        // Deleted skills leave a loaded snapshot behind; if the SKILL.md is
        // gone from every root, refuse instead of resurrecting it.
        {
            let primary_md = target.join(&name).join("SKILL.md");
            let actual_md = existing_skill.base_dir.join("SKILL.md");
            if !primary_md.exists() && !actual_md.exists() {
                return Err(OSAgentError::ToolExecution(format!(
                    "Skill '{}' not found on disk (it may have been deleted). Use skill_create to recreate it.",
                    name
                )));
            }
        }

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
        let actions = match (args.get("actions"), args.get("action_patches")) {
            (Some(_), Some(_)) => {
                return Err(OSAgentError::ToolExecution(
                    "Pass either 'actions' (full replacement) or 'action_patches' (surgical edit), not both. For small tweaks prefer 'action_patches'.".to_string(),
                ));
            }
            (Some(_), None) => parse_actions(args.get("actions"))?,
            (None, Some(patches)) => {
                let list = patches.as_array().ok_or_else(|| {
                    OSAgentError::ToolExecution(
                        "'action_patches' must be an array like [{\"name\": \"check\", \"description\": \"new text\"}].".to_string(),
                    )
                })?;
                apply_action_patches(&existing_schema.actions, list)
                    .map_err(OSAgentError::ToolExecution)?
            }
            (None, None) => existing_schema.actions.clone(),
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
        self.loader.load_all().ok();
        let target = self.loader.primary_dir();
        // Delete from the actual on-disk location when known (primary or
        // legacy fallback), plus a best-effort pass on primary so renames
        // and half-written dirs don't linger.
        let mut deleted_any = false;
        if let Some(skill) = self.loader.get(&name) {
            let dir = skill.base_dir.clone();
            if dir.join("SKILL.md").exists() || dir.exists() {
                std::fs::remove_dir_all(&dir).map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to delete skill '{}': {}", name, e))
                })?;
                deleted_any = true;
            }
        }
        if crate::skills::delete_skill(&target, &name).is_ok() {
            deleted_any = true;
        }
        if !deleted_any {
            return Err(OSAgentError::ToolExecution(format!(
                "Skill '{}' not found.",
                name
            )));
        }
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

    #[test]
    fn accepts_json_schema_style_parameters() {
        let actions = json!([{
            "name": "recon",
            "description": "Run recon",
            "type": "http",
            "method": "GET",
            "url": "https://api.example.com/recon?q={{ args.domain }}",
            "parameters": {
                "properties": {
                    "domain": {"type": "string", "description": "Target domain"}
                },
                "required": ["domain"]
            }
        }]);
        let parsed = parse_actions(Some(&actions)).expect("should normalize");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].parameters.len(), 1);
        assert_eq!(parsed[0].parameters[0].name, "domain");
        assert!(parsed[0].parameters[0].required);
    }

    #[test]
    fn accepts_map_style_script_args() {
        // The shape the model kept writing for link-checker.
        let actions = json!([{
            "name": "check",
            "description": "Check links",
            "type": "script",
            "script": "scripts/link_checker.py",
            "args": {"url": "The URL to check for broken links"}
        }]);
        let parsed = parse_actions(Some(&actions)).expect("should normalize");
        assert_eq!(parsed.len(), 1);
        assert_eq!(parsed[0].parameters.len(), 1);
        assert_eq!(parsed[0].parameters[0].name, "url");
        match &parsed[0].runner {
            crate::skills::SkillActionRunner::Script { args, .. } => {
                assert_eq!(args, &vec!["{{ args.url }}".to_string()]);
            }
            other => panic!("expected script runner, got {:?}", other),
        }
    }

    #[test]
    fn derives_parameters_from_args_templates() {
        let actions = json!([{
            "name": "check",
            "description": "Check links",
            "type": "script",
            "script": "scripts/link_checker.py",
            "args": ["{{ args.url }}"]
        }]);
        let parsed = parse_actions(Some(&actions)).expect("should derive");
        assert_eq!(parsed[0].parameters.len(), 1);
        assert_eq!(parsed[0].parameters[0].name, "url");
    }

    #[test]
    fn generates_args_from_parameters() {
        // The model never writes `args`: CLI bindings come from parameters order.
        let actions = json!([{
            "name": "stats",
            "description": "Stats",
            "type": "script",
            "script": "scripts/stats.py",
            "parameters": [{"name": "text", "required": true}]
        }]);
        let parsed = parse_actions(Some(&actions)).expect("should generate");
        match &parsed[0].runner {
            crate::skills::SkillActionRunner::Script { args, .. } => {
                assert_eq!(args, &vec!["{{ args.text }}".to_string()]);
            }
            other => panic!("expected script runner, got {:?}", other),
        }
        // Minimal parameter shape defaults to string.
        assert_eq!(
            parsed[0].parameters[0].parameter_type,
            crate::skills::SkillActionParameterType::String
        );
    }

    fn link_check_action() -> SkillActionSchema {
        serde_json::from_value(json!({
            "name": "check",
            "description": "Check all links",
            "type": "script",
            "script": "scripts/link_checker.py",
            "args": ["{{ args.url }}"],
            "parameters": [{"name": "url", "type": "string", "description": "URL", "required": true}]
        }))
        .expect("fixture action")
    }

    #[test]
    fn action_patch_changes_only_listed_fields() {
        let patched = apply_action_patches(
            &[link_check_action()],
            &[json!({"name": "check", "description": "New text"})],
        )
        .expect("patch should apply");
        assert_eq!(patched.len(), 1);
        assert_eq!(patched[0].description, "New text");
        // Untouched fields survive: no silent type flip, script kept.
        match &patched[0].runner {
            crate::skills::SkillActionRunner::Script { script, args } => {
                assert_eq!(script, "scripts/link_checker.py");
                assert_eq!(args, &vec!["{{ args.url }}".to_string()]);
            }
            other => panic!("type must stay script, got {:?}", other),
        }
        assert_eq!(patched[0].parameters.len(), 1);
        assert_eq!(patched[0].parameters[0].name, "url");
    }

    #[test]
    fn action_patch_unknown_name_lists_valid() {
        let err = apply_action_patches(&[link_check_action()], &[json!({"name": "nope"})])
            .expect_err("should fail");
        assert!(
            err.contains("check"),
            "error should list valid names: {}",
            err
        );
    }
}
