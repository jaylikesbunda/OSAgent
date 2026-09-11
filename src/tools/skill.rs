use crate::error::{OSAgentError, Result};
use crate::skills::{
    get_config_base_dir, Skill, SkillActionParameter, SkillActionParameterType, SkillActionRunner,
    SkillActionSchema, SkillConfigStore, SkillLoader, SkillTokenRefreshSchema,
};
use crate::tools::registry::Tool;
use async_trait::async_trait;
use reqwest::Client;
use serde_json::{json, Map, Value};
use std::collections::HashMap;
use std::process::Command;
use std::sync::Arc;
use std::time::Duration;

const SKILL_ACTION_TIMEOUT_SECS: u64 = 30;

pub struct SkillTool {
    loader: Arc<SkillLoader>,
}

impl SkillTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillTool {
    fn name(&self) -> &str {
        "skill"
    }

    fn description(&self) -> &str {
        "Read a skill's full instructions and available runtime actions so you can follow them"
    }

    fn when_to_use(&self) -> &str {
        "Use to read a skill's instructions before using it, or to inspect its actions before calling skill_action"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use if you already know the action you want to execute"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![crate::tools::registry::ToolExample {
            description: "Inspect Spotify skill actions".to_string(),
            input: json!({
                "name": "spotify"
            }),
        }]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "name": {
                    "type": "string",
                    "description": "The installed skill name"
                }
            },
            "required": ["name"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let skill_name = args["name"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'name' parameter".to_string()))?;

        self.loader.load_all()?;

        let skill = self.loader.get(skill_name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Skill not found: {}", skill_name))
        })?;

        Ok(render_skill_summary(&skill))
    }
}

pub struct SkillListTool {
    loader: Arc<SkillLoader>,
}

impl SkillListTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self { loader }
    }
}

#[async_trait]
impl Tool for SkillListTool {
    fn name(&self) -> &str {
        "skill_list"
    }

    fn description(&self) -> &str {
        "List installed skills and their available runtime actions"
    }

    fn when_to_use(&self) -> &str {
        "Use to discover what runtime-capable skills are available"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use if you already know which skill/action to call"
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {}
        })
    }

    async fn execute(&self, _args: Value) -> Result<String> {
        self.loader.load_all()?;

        let mut skills = self.loader.list();
        skills.sort_by(|a, b| a.name.cmp(&b.name));

        if skills.is_empty() {
            return Ok("No skills available.".to_string());
        }

        let mut output = "Available skills:\n".to_string();
        for skill in skills {
            output.push_str(&format!("- **{}**: {}", skill.name, skill.description));

            if let Some(ref meta) = skill.metadata {
                if let Some(ref emoji) = meta.emoji {
                    output.push_str(&format!(" [{}]", emoji));
                }
            }

            if skill.actions.is_empty() {
                output.push_str(" (no runtime actions declared)");
            } else {
                let actions = skill
                    .actions
                    .iter()
                    .filter(|a| a.name != "authorize")
                    .map(format_action_signature)
                    .collect::<Vec<_>>()
                    .join(", ");
                output.push_str(&format!(" (actions: {})", actions));
            }

            output.push('\n');
        }

        Ok(output)
    }
}

pub struct SkillActionTool {
    loader: Arc<SkillLoader>,
    config_store: SkillConfigStore,
    client: Client,
}

impl SkillActionTool {
    pub fn new(loader: Arc<SkillLoader>) -> Self {
        Self {
            loader,
            config_store: SkillConfigStore::new(get_config_base_dir()),
            client: Client::new(),
        }
    }

    async fn execute_http_action(
        &self,
        skill: &Skill,
        action: &SkillActionSchema,
        config: &mut HashMap<String, String>,
        args: &Map<String, Value>,
    ) -> Result<String> {
        let SkillActionRunner::Http {
            method,
            url,
            headers,
            query,
            body,
            body_form,
            ..
        } = &action.runner
        else {
            return Err(OSAgentError::ToolExecution(
                "Action is not an HTTP action".to_string(),
            ));
        };

        let method_str = render_string_template(method, config, args)?;
        let url_str = render_string_template(url, config, args)?;
        let http_method = reqwest::Method::from_bytes(method_str.as_bytes()).map_err(|e| {
            OSAgentError::ToolExecution(format!(
                "Invalid HTTP method for skill '{}', action '{}': {}",
                skill.name, action.name, e
            ))
        })?;

        let mut request = self.client.request(http_method, &url_str);

        for (key, value) in headers {
            request = request.header(key, render_string_template(value, config, args)?);
        }

        if !query.is_empty() {
            let rendered_query: Vec<(String, String)> = query
                .iter()
                .filter_map(|(key, value): (&String, &String)| {
                    let rv = render_string_template(value, config, args).ok()?;
                    if rv.trim().is_empty() {
                        None
                    } else {
                        Some((key.clone(), rv))
                    }
                })
                .collect();
            if !rendered_query.is_empty() {
                request = request.query(&rendered_query);
            }
        }

        if let Some(body) = body {
            let rendered_body = render_json_templates(body, config, args)?;
            request = request.json(&rendered_body);
        } else if let Some(body_form) = body_form {
            let form_pairs: Vec<(String, String)> = body_form
                .iter()
                .filter_map(|(key, value): (&String, &String)| {
                    let rv = render_string_template(value, config, args).ok()?;
                    if rv.trim().is_empty() {
                        None
                    } else {
                        Some((key.clone(), rv))
                    }
                })
                .collect();
            if !form_pairs.is_empty() {
                request = request.form(&form_pairs);
            }
        }

        let response = tokio::time::timeout(
            Duration::from_secs(SKILL_ACTION_TIMEOUT_SECS),
            request.send(),
        )
        .await
        .map_err(|_| OSAgentError::Timeout)??;
        let status = response.status();
        let body_bytes = response.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&body_bytes).trim().to_string();

        if !status.is_success() {
            let message = if text.trim().is_empty() {
                format!("Skill action failed with HTTP {}", status)
            } else {
                format!(
                    "Skill action failed with HTTP {}: {}",
                    status,
                    truncate_output(text.trim(), 1000)
                )
            };
            return Err(OSAgentError::ToolExecution(message));
        }

        if text.trim().is_empty() {
            return Ok(format!("Action '{}' completed successfully.", action.name));
        }

        if let SkillActionRunner::Http {
            response_transform: Some(transform),
            ..
        } = &action.runner
        {
            return self.transform_response(&text, transform);
        }

        Ok(pretty_response_body(&text))
    }

    fn transform_response(&self, text: &str, transform: &str) -> Result<String> {
        let json: Value = serde_json::from_str(text).map_err(|e| {
            OSAgentError::ToolExecution(format!("Response is not valid JSON: {}", e))
        })?;

        let mut val = &json;
        for key in transform.split('.') {
            val = val.get(key).ok_or_else(|| {
                OSAgentError::ToolExecution(format!(
                    "Response transform path '{}' not found in response",
                    transform
                ))
            })?;
        }

        match val {
            Value::Array(items) => {
                if items.is_empty() {
                    return Ok("(no results)".to_string());
                }
                let mut out = String::new();
                for (i, item) in items.iter().enumerate() {
                    out.push_str(&format_item(i, item));
                }
                Ok(out.trim_end().to_string())
            }
            Value::Object(obj) => {
                let mut out = String::new();
                for (k, v) in obj {
                    if let Some(s) = v.as_str() {
                        out.push_str(&format!("{}: {}\n", k, s));
                    } else if v.is_number() || v.is_boolean() {
                        out.push_str(&format!("{}: {}\n", k, v));
                    } else if v.is_object() || v.is_array() {
                        out.push_str(&format!(
                            "{}: {}\n",
                            k,
                            serde_json::to_string_pretty(v).unwrap_or_default()
                        ));
                    }
                }
                Ok(out.trim_end().to_string())
            }
            Value::String(s) => Ok(s.clone()),
            Value::Number(n) => Ok(n.to_string()),
            Value::Bool(b) => Ok(b.to_string()),
            Value::Null => Ok("(null)".to_string()),
        }
    }

    async fn execute_script_action(
        &self,
        skill: &Skill,
        action: &SkillActionSchema,
        config: &HashMap<String, String>,
        args: &Map<String, Value>,
    ) -> Result<String> {
        let SkillActionRunner::Script {
            script,
            args: raw_args,
        } = &action.runner
        else {
            return Err(OSAgentError::ToolExecution(
                "Action is not a script action".to_string(),
            ));
        };

        let script_path = resolve_skill_script(&skill.base_dir, script);
        if !script_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "Skill action script not found: {}",
                script_path.to_string_lossy()
            )));
        }

        let rendered_args = raw_args
            .iter()
            .map(|arg| render_string_template(arg, config, args))
            .collect::<Result<Vec<_>>>()?;
        let env = config.clone();
        let arg_json = Value::Object(args.clone()).to_string();
        let skill_name = skill.name.clone();
        let action_name = action.name.clone();
        let skill_dir = skill.base_dir.clone();
        let action_args = args.clone();

        let output = tokio::task::spawn_blocking(move || {
            let extension = script_path
                .extension()
                .and_then(|ext| ext.to_str())
                .unwrap_or_default()
                .to_ascii_lowercase();

            let mut command = match extension.as_str() {
                "ps1" => {
                    let mut cmd = Command::new(if cfg!(windows) { "powershell" } else { "pwsh" });
                    cmd.arg("-NoProfile")
                        .arg("-ExecutionPolicy")
                        .arg("Bypass")
                        .arg("-File")
                        .arg(&script_path);
                    cmd
                }
                "sh" => {
                    let mut cmd = Command::new("sh");
                    cmd.arg(&script_path);
                    cmd
                }
                "py" => {
                    let mut cmd = Command::new("python");
                    cmd.arg(&script_path);
                    cmd
                }
                "js" => {
                    let mut cmd = Command::new("node");
                    cmd.arg(&script_path);
                    cmd
                }
                _ => Command::new(&script_path),
            };

            command.current_dir(skill_dir);
            command.args(&rendered_args);
            command.envs(&env);
            command.env("OSA_SKILL_NAME", &skill_name);
            command.env("OSA_SKILL_ACTION", &action_name);
            command.env("OSA_SKILL_ARGS_JSON", &arg_json);
            for (key, value) in &action_args {
                command.env(
                    format!("OSA_SKILL_ARG_{}", key.to_ascii_uppercase()),
                    template_value_to_string(value),
                );
            }

            command.output()
        })
        .await??;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr).trim().to_string();
            let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
            let message = if !stderr.is_empty() {
                truncate_output(&stderr, 1000)
            } else if !stdout.is_empty() {
                truncate_output(&stdout, 1000)
            } else {
                format!("Script exited with status {}", output.status)
            };
            return Err(OSAgentError::ToolExecution(message));
        }

        let stdout = String::from_utf8_lossy(&output.stdout).trim().to_string();
        if stdout.is_empty() {
            Ok(format!("Action '{}' completed successfully.", action.name))
        } else {
            Ok(stdout)
        }
    }

    async fn refresh_access_token(
        &self,
        skill_name: &str,
        tr: &SkillTokenRefreshSchema,
        current_config: &HashMap<String, String>,
    ) -> Result<String> {
        let refresh_token = current_config.get(&tr.refresh_token_field).ok_or_else(|| {
            OSAgentError::ToolExecution(format!(
                "Skill '{}' has no refresh token ({} not set). Use the Authorize button in the skill settings first.",
                skill_name, tr.refresh_token_field
            ))
        })?;

        let mut body_map: std::collections::HashMap<String, String> =
            std::collections::HashMap::new();
        body_map.insert("grant_type".to_string(), tr.grant_type.clone());
        body_map.insert("refresh_token".to_string(), refresh_token.clone());

        if !tr.client_id_field.is_empty() {
            if let Some(id) = current_config.get(&tr.client_id_field) {
                body_map.insert("client_id".to_string(), id.clone());
            }
        }
        if !tr.client_secret_field.is_empty() {
            if let Some(secret) = current_config.get(&tr.client_secret_field) {
                body_map.insert("client_secret".to_string(), secret.clone());
            }
        }
        if let Some(extra) = &tr.body {
            for (k, v) in extra {
                if !body_map.contains_key(k) {
                    body_map.insert(k.clone(), v.clone());
                }
            }
        }

        let mut request = self
            .client
            .request(reqwest::Method::POST, &tr.token_url)
            .header("Content-Type", "application/x-www-form-urlencoded");

        if let Some(headers) = &tr.headers {
            for (key, value) in headers {
                request = request.header(key, value);
            }
        }

        let form_pairs: Vec<(String, String)> = body_map
            .into_iter()
            .filter(|(_, v)| !v.is_empty())
            .collect();
        request = request.form(&form_pairs);

        let response = tokio::time::timeout(
            Duration::from_secs(SKILL_ACTION_TIMEOUT_SECS),
            request.send(),
        )
        .await
        .map_err(|_| OSAgentError::Timeout)??;

        let status = response.status();
        let body_bytes = response.bytes().await.unwrap_or_default();
        let text = String::from_utf8_lossy(&body_bytes).trim().to_string();

        if !status.is_success() {
            return Err(OSAgentError::ToolExecution(format!(
                "Token refresh failed with HTTP {}: {}",
                status, text
            )));
        }

        let json: Value = serde_json::from_str(&text).map_err(|e| {
            OSAgentError::ToolExecution(format!("Token refresh response invalid JSON: {}", e))
        })?;

        let access_token = if !tr.response_access_token_path.is_empty() {
            let parts = tr.response_access_token_path.split('.');
            let mut val = &json;
            for key in parts {
                val = val.get(key).ok_or_else(|| {
                    OSAgentError::ToolExecution(format!(
                        "Token refresh response missing path '{}'",
                        tr.response_access_token_path
                    ))
                })?;
            }
            val.as_str()
                .ok_or_else(|| {
                    OSAgentError::ToolExecution(format!(
                        "Token refresh '{}' is not a string",
                        tr.response_access_token_path
                    ))
                })?
                .to_string()
        } else {
            json.get("access_token")
                .and_then(|v| v.as_str())
                .ok_or_else(|| {
                    OSAgentError::ToolExecution(
                        "Token refresh response missing 'access_token' field".to_string(),
                    )
                })?
                .to_string()
        };

        Ok(access_token)
    }
}

#[async_trait]
impl Tool for SkillActionTool {
    fn name(&self) -> &str {
        "skill_action"
    }

    fn description(&self) -> &str {
        "Execute a runtime action declared by an installed skill without exposing its saved secrets to the model"
    }

    fn when_to_use(&self) -> &str {
        "Use when a loaded skill exposes a concrete action like spotify.search_tracks or spotify.pause"
    }

    fn when_not_to_use(&self) -> &str {
        "Don't use if the task is already covered by a built-in tool or the skill has no relevant action"
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Search Spotify tracks".to_string(),
                input: json!({
                    "skill": "spotify",
                    "action": "search_tracks",
                    "args": { "query": "take five" }
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Pause Spotify playback".to_string(),
                input: json!({
                    "skill": "spotify",
                    "action": "pause"
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "skill": {
                    "type": "string",
                    "description": "The installed skill name"
                },
                "action": {
                    "type": "string",
                    "description": "The action name declared by that skill"
                },
                "args": {
                    "type": "object",
                    "description": "Action-specific parameters (e.g., { \"query\": \"search text\" }) - see skill documentation for required parameters",
                    "additionalProperties": true
                }
            },
            "required": ["skill", "action"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let skill_name = args["skill"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'skill' parameter".to_string()))?;
        let action_name = args["action"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'action' parameter".to_string()))?;

        self.loader.load_all()?;

        let skill = self.loader.get(skill_name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Skill not found: {}", skill_name))
        })?;
        let action = skill
            .actions
            .iter()
            .find(|a| a.name == action_name)
            .cloned()
            .ok_or_else(|| {
                OSAgentError::ToolExecution(format!(
                    "Skill '{}' does not define action '{}'",
                    skill_name, action_name
                ))
            })?;

        let mut config = self.config_store.load_config(skill_name).map_err(|e| {
            OSAgentError::ToolExecution(format!("Failed to load skill config: {}", e))
        })?;
        if !config.enabled {
            return Err(OSAgentError::ToolExecution(format!(
                "Skill '{}' is disabled.",
                skill_name
            )));
        }

        let mut action_args = resolve_action_args(&args, &action);

        ensure_skill_is_configured(&skill, &config.settings)?;
        validate_action_args(&action, &mut action_args)?;

        if let Some(ref tr) = skill.token_refresh {
            let access_token_key = &tr.access_token_field;
            let needs_refresh = config
                .settings
                .get(access_token_key)
                .map(|v| v.trim().is_empty())
                .unwrap_or(true);

            if needs_refresh {
                match self
                    .refresh_access_token(skill_name, tr, &config.settings)
                    .await
                {
                    Ok(new_access_token) => {
                        config
                            .settings
                            .insert(access_token_key.clone(), new_access_token.clone());
                        let mut sc = self.config_store.load_config(skill_name).map_err(|e| {
                            OSAgentError::ToolExecution(format!("Failed to reload config: {}", e))
                        })?;
                        sc.settings
                            .insert(access_token_key.clone(), new_access_token);
                        self.config_store
                            .save_config(skill_name, &sc)
                            .map_err(|e| {
                                OSAgentError::ToolExecution(format!(
                                    "Failed to persist refreshed token: {}",
                                    e
                                ))
                            })?;
                    }
                    Err(e) => {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Failed to refresh access token for skill '{}': {}",
                            skill_name, e
                        )));
                    }
                }
            }
        }

        match &action.runner {
            SkillActionRunner::Http { .. } => {
                self.execute_http_action(&skill, &action, &mut config.settings, &action_args)
                    .await
            }
            SkillActionRunner::Script { .. } => {
                self.execute_script_action(&skill, &action, &config.settings, &action_args)
                    .await
            }
        }
    }
}

fn ensure_skill_is_configured(skill: &Skill, settings: &HashMap<String, String>) -> Result<()> {
    let access_token_field = skill
        .token_refresh
        .as_ref()
        .map(|tr| tr.access_token_field.clone());

    for field in &skill.config_fields {
        if field.required {
            if let Some(ref at_field) = access_token_field {
                if &field.name == at_field {
                    continue;
                }
            }

            if settings
                .get(&field.name)
                .map(|value| value.trim().is_empty())
                .unwrap_or(true)
            {
                return Err(OSAgentError::ToolExecution(format!(
                    "Skill '{}' is not configured. Configure it in Settings > Skills before using it.",
                    skill.name
                )));
            }
        }
    }

    Ok(())
}

/// Lenient `args` resolution: models sometimes send `args` as a JSON string,
/// or hoist parameters to the top level (`{"domain": ...}` instead of
/// `{"args": {"domain": ...}}`). Accept those shapes rather than failing.
fn resolve_action_args(call: &Value, action: &SkillActionSchema) -> Map<String, Value> {
    let mut merged = Map::new();

    if let Some(obj) = call.get("args").and_then(Value::as_object) {
        for (key, value) in obj {
            merged.insert(key.clone(), value.clone());
        }
    } else if let Some(text) = call.get("args").and_then(Value::as_str) {
        let trimmed = text.trim();
        if let Ok(Value::Object(obj)) = serde_json::from_str(trimmed) {
            for (key, value) in obj {
                merged.insert(key, value);
            }
        } else if !trimmed.is_empty() {
            // Bare string with a single required string parameter:
            // treat it as that parameter's value.
            let required_strings: Vec<&SkillActionParameter> = action
                .parameters
                .iter()
                .filter(|p| {
                    p.required && p.parameter_type == SkillActionParameterType::String
                })
                .collect();
            if required_strings.len() == 1 && !merged.contains_key(&required_strings[0].name) {
                merged.insert(
                    required_strings[0].name.clone(),
                    Value::String(trimmed.to_string()),
                );
            }
        }
    }

    // Top-level hoisted params: {"skill","action","domain":"example.com"}.
    if let Some(obj) = call.as_object() {
        for parameter in &action.parameters {
            if !merged.contains_key(&parameter.name) {
                if let Some(value) = obj.get(&parameter.name) {
                    if !value.is_null() {
                        merged.insert(parameter.name.clone(), value.clone());
                    }
                }
            }
        }
    }

    merged
}

fn validate_action_args(action: &SkillActionSchema, args: &mut Map<String, Value>) -> Result<()> {
    for parameter in &action.parameters {
        let coerced = match args.get(&parameter.name) {
            None => None,
            Some(value) => Some(
                coerce_param_value(&parameter.parameter_type, value).ok_or_else(|| {
                    OSAgentError::ToolExecution(format!(
                        "Action '{}' argument '{}' has the wrong type.",
                        action.name, parameter.name
                    ))
                })?,
            ),
        };
        match coerced {
            None => {
                if parameter.required {
                    return Err(OSAgentError::ToolExecution(format!(
                        "Action '{}' is missing required argument '{}'.",
                        action.name, parameter.name
                    )));
                }
            }
            Some(value) => {
                args.insert(parameter.name.clone(), value);
            }
        }
    }

    Ok(())
}

/// Coerce one argument value into a parameter type. Returns `None` when the
/// value cannot sensibly convert.
fn coerce_param_value(parameter_type: &SkillActionParameterType, value: &Value) -> Option<Value> {
    match parameter_type {
        SkillActionParameterType::String => match value {
            Value::String(_) => Some(value.clone()),
            Value::Number(n) => Some(Value::String(n.to_string())),
            Value::Bool(b) => Some(Value::String(b.to_string())),
            _ => None,
        },
        SkillActionParameterType::Number => match value {
            Value::Number(_) => Some(value.clone()),
            Value::String(text) => text
                .trim()
                .parse::<f64>()
                .ok()
                .and_then(serde_json::Number::from_f64)
                .map(Value::Number),
            _ => None,
        },
        SkillActionParameterType::Boolean => match value {
            Value::Bool(_) => Some(value.clone()),
            Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
                "true" => Some(Value::Bool(true)),
                "false" => Some(Value::Bool(false)),
                _ => None,
            },
            _ => None,
        },
    }
}

/// Resolve a script action's file. The declared reference may be a bare
/// basename (`stats.py`), a `scripts/`-prefixed path, or `./`-prefixed —
/// all spellings resolve to the same file, with the literal join tried
/// first so explicit layouts keep working.
fn resolve_skill_script(base_dir: &std::path::Path, script: &str) -> std::path::PathBuf {
    let direct = base_dir.join(script);
    if direct.exists() {
        return direct;
    }
    let trimmed = script
        .trim_start_matches("./")
        .trim_start_matches(".\\")
        .trim_start_matches("scripts/")
        .trim_start_matches("scripts\\");
    let basename = trimmed
        .rsplit('/')
        .next()
        .unwrap_or(trimmed)
        .rsplit('\\')
        .next()
        .unwrap_or(trimmed);
    for candidate in [
        base_dir.join("scripts").join(basename),
        base_dir.join(trimmed),
        base_dir.join(basename),
    ] {
        if candidate.exists() {
            return candidate;
        }
    }
    direct
}

fn render_skill_summary(skill: &Skill) -> String {
    let location = skill
        .base_dir
        .display()
        .to_string()
        .replace('/', &std::path::MAIN_SEPARATOR.to_string());
    let mut output = format!(
        "Skill: {}\nDescription: {}\nLocation: {}",
        skill.name, skill.description, location
    );

    if !skill.actions.is_empty() {
        output.push_str("\nActions:\n");
        for action in skill.actions.iter().filter(|a| a.name != "authorize") {
            output.push_str(&format!(
                "- {}: {}\n",
                format_action_signature(action),
                if action.description.is_empty() {
                    "No description"
                } else {
                    action.description.as_str()
                }
            ));
        }
    } else {
        output.push_str("\n(no runtime actions declared — follow the instructions below using your existing tools)");
    }

    if let Some(metadata) = &skill.metadata {
        if let Some(requires) = &metadata.requires {
            let mut requirements = Vec::new();
            if !requires.bins.is_empty() {
                requirements.push(format!("binaries: {}", requires.bins.join(", ")));
            }
            if !requires.files.is_empty() {
                requirements.push(format!("files: {}", requires.files.join(", ")));
            }
            if !requirements.is_empty() {
                output.push_str(&format!(
                    "\nRuntime requirements: {}",
                    requirements.join("; ")
                ));
            }
        }
    }

    // Prompt-style skills carry their behavior as markdown instructions.
    // Return them (bounded) so the agent can actually follow the skill.
    let body = extract_instructions_body(&skill.content);
    if !body.trim().is_empty() {
        const MAX_BODY_CHARS: usize = 6000;
        let mut chars = body.chars();
        let truncated: String = chars.by_ref().take(MAX_BODY_CHARS).collect();
        let rest = chars.next().is_some();
        output.push_str("\n\nInstructions:\n");
        output.push_str(truncated.trim());
        if rest {
            output.push_str("\n\n[...instructions truncated — read the SKILL.md file directly for the remainder]");
        }
    }

    output.push_str("\n\nSkill configuration is managed separately and is not exposed to the model.");
    output
}

/// Strip YAML frontmatter from a SKILL.md so only the markdown instructions remain.
fn extract_instructions_body(skill_md: &str) -> String {
    if skill_md.starts_with("---") {
        if let Some(end) = skill_md[3..].find("\n---") {
            return skill_md[3 + end + 4..].trim_start_matches('\n').to_string();
        }
    }
    skill_md.to_string()
}

fn format_action_signature(action: &SkillActionSchema) -> String {
    if action.parameters.is_empty() {
        return action.name.clone();
    }

    let parameters = action
        .parameters
        .iter()
        .map(format_parameter_signature)
        .collect::<Vec<_>>()
        .join(", ");
    format!("{}({})", action.name, parameters)
}

fn format_parameter_signature(parameter: &SkillActionParameter) -> String {
    if parameter.required {
        parameter.name.clone()
    } else {
        format!("{}?", parameter.name)
    }
}

fn render_string_template(
    template: &str,
    config: &HashMap<String, String>,
    args: &Map<String, Value>,
) -> Result<String> {
    let mut rendered = String::new();
    let mut remaining = template;

    while let Some(start) = remaining.find("{{") {
        rendered.push_str(&remaining[..start]);
        let after_start = &remaining[start + 2..];
        let end = after_start.find("}}").ok_or_else(|| {
            OSAgentError::ToolExecution("Malformed skill action template".to_string())
        })?;
        let expr = after_start[..end].trim();
        rendered.push_str(&resolve_template_expression(expr, config, args)?);
        remaining = &after_start[end + 2..];
    }

    rendered.push_str(remaining);
    Ok(rendered)
}

fn resolve_template_expression(
    expr: &str,
    config: &HashMap<String, String>,
    args: &Map<String, Value>,
) -> Result<String> {
    if let Some(name) = expr.strip_prefix("args.") {
        let value = args.get(name).ok_or_else(|| {
            OSAgentError::ToolExecution(format!("Missing action argument '{}'.", name))
        })?;
        return Ok(template_value_to_string(value));
    }

    if let Some(name) = expr
        .strip_prefix("config.")
        .or_else(|| expr.strip_prefix("skill.env."))
    {
        let value = config.get(name).ok_or_else(|| {
            OSAgentError::ToolExecution(
                "Skill is not configured. Configure it in Settings > Skills before using it."
                    .to_string(),
            )
        })?;
        return Ok(value.clone());
    }

    Err(OSAgentError::ToolExecution(format!(
        "Unsupported skill template expression '{}'.",
        expr
    )))
}

fn render_json_templates(
    value: &Value,
    config: &HashMap<String, String>,
    args: &Map<String, Value>,
) -> Result<Value> {
    match value {
        Value::String(s) => Ok(Value::String(render_string_template(s, config, args)?)),
        Value::Array(items) => Ok(Value::Array(
            items
                .iter()
                .map(|item| render_json_templates(item, config, args))
                .collect::<Result<Vec<_>>>()?,
        )),
        Value::Object(map) => {
            let mut rendered = Map::new();
            for (key, item) in map {
                rendered.insert(key.clone(), render_json_templates(item, config, args)?);
            }
            Ok(Value::Object(rendered))
        }
        _ => Ok(value.clone()),
    }
}

fn template_value_to_string(value: &Value) -> String {
    match value {
        Value::String(s) => s.clone(),
        Value::Null => String::new(),
        Value::Bool(b) => b.to_string(),
        Value::Number(n) => n.to_string(),
        _ => value.to_string(),
    }
}

fn pretty_response_body(body: &str) -> String {
    serde_json::from_str::<Value>(body)
        .ok()
        .and_then(|value| serde_json::to_string_pretty(&value).ok())
        .unwrap_or_else(|| body.trim().to_string())
}

fn truncate_output(text: &str, max_chars: usize) -> String {
    let mut iter = text.chars();
    let truncated = iter.by_ref().take(max_chars).collect::<String>();
    if iter.next().is_some() {
        format!("{}...", truncated)
    } else {
        truncated
    }
}

fn format_item(index: usize, item: &Value) -> String {
    let mut parts = Vec::new();
    if let Some(name) = item.get("name").and_then(Value::as_str) {
        parts.push(name.to_string());
    }
    if let Some(artist) = item.get("artists").and_then(Value::as_array) {
        let names: Vec<_> = artist
            .iter()
            .filter_map(|a| a.get("name").and_then(Value::as_str))
            .collect();
        if !names.is_empty() {
            parts.push(format!("by {}", names.join(", ")));
        }
    }
    if let Some(uri) = item.get("uri").and_then(Value::as_str) {
        parts.push(format!("uri: {}", uri));
    }
    if let Some(album) = item
        .get("album")
        .and_then(|v| v.get("name").and_then(Value::as_str))
    {
        parts.push(format!("album: {}", album));
    }
    if let Some(duration_ms) = item.get("duration_ms").and_then(Value::as_u64) {
        let mins = duration_ms / 60000;
        let secs = (duration_ms % 60000) / 1000;
        parts.push(format!("{}:{:02}", mins, secs));
    }
    if parts.is_empty() {
        return format!(
            "{}. {}\n",
            index + 1,
            serde_json::to_string_pretty(item).unwrap_or_default()
        );
    }
    format!("{}. {}\n", index + 1, parts.join(" | "))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn renders_templates_from_args_and_config() {
        let mut config = HashMap::new();
        config.insert("TOKEN".to_string(), "secret".to_string());
        let args = serde_json::json!({
            "query": "take five"
        })
        .as_object()
        .cloned()
        .expect("args object");

        let rendered = render_string_template(
            "Bearer {{ config.TOKEN }} :: {{ args.query }}",
            &config,
            &args,
        )
        .expect("template should render");

        assert_eq!(rendered, "Bearer secret :: take five");
    }

    #[test]
    fn renders_nested_json_templates() {
        let mut config = HashMap::new();
        config.insert("DEVICE".to_string(), "abc123".to_string());
        let args = serde_json::json!({
            "uri": "spotify:track:123"
        })
        .as_object()
        .cloned()
        .expect("args object");

        let body = json!({
            "device_id": "{{ config.DEVICE }}",
            "uris": ["{{ args.uri }}"]
        });

        let rendered = render_json_templates(&body, &config, &args).expect("json templates render");
        assert_eq!(rendered["device_id"], "abc123");
        assert_eq!(rendered["uris"][0], "spotify:track:123");
    }

    fn recon_action() -> SkillActionSchema {
        SkillActionSchema {
            name: "recon".to_string(),
            description: String::new(),
            parameters: vec![SkillActionParameter {
                name: "domain".to_string(),
                parameter_type: SkillActionParameterType::String,
                description: String::new(),
                required: true,
            }],
            runner: SkillActionRunner::Script {
                script: "scripts/recon.py".to_string(),
                args: vec!["{{ args.domain }}".to_string()],
            },
        }
    }

    #[test]
    fn resolves_stringified_args_object() {
        let action = recon_action();
        let call = json!({
            "skill": "osint-quick",
            "action": "recon",
            "args": "{\"domain\": \"example.com\"}"
        });
        let resolved = resolve_action_args(&call, &action);
        assert_eq!(
            resolved.get("domain").and_then(Value::as_str),
            Some("example.com")
        );
    }

    #[test]
    fn resolves_hoisted_top_level_params() {
        let action = recon_action();
        let call = json!({
            "skill": "osint-quick",
            "action": "recon",
            "domain": "example.com"
        });
        let resolved = resolve_action_args(&call, &action);
        assert_eq!(
            resolved.get("domain").and_then(Value::as_str),
            Some("example.com")
        );
    }

    #[test]
    fn coerces_friendly_type_shapes() {
        let string_action = SkillActionSchema {
            name: "s".to_string(),
            description: String::new(),
            parameters: vec![SkillActionParameter {
                name: "text".to_string(),
                parameter_type: SkillActionParameterType::String,
                description: String::new(),
                required: true,
            }],
            runner: SkillActionRunner::Script {
                script: "scripts/s.py".to_string(),
                args: vec![],
            },
        };
        let mut args = serde_json::json!({"text": 5})
            .as_object()
            .cloned()
            .unwrap();
        validate_action_args(&string_action, &mut args).expect("number coerces to string");
        assert_eq!(args["text"], json!("5"));

        let number_action = SkillActionSchema {
            name: "n".to_string(),
            description: String::new(),
            parameters: vec![SkillActionParameter {
                name: "volume".to_string(),
                parameter_type: SkillActionParameterType::Number,
                description: String::new(),
                required: true,
            }],
            runner: SkillActionRunner::Script {
                script: "scripts/s.py".to_string(),
                args: vec![],
            },
        };
        let mut args = serde_json::json!({"volume": "50"})
            .as_object()
            .cloned()
            .unwrap();
        validate_action_args(&number_action, &mut args).expect("numeric string parses");
        assert_eq!(args["volume"], json!(50.0));

        let mut args = serde_json::json!({"volume": [1, 2]})
            .as_object()
            .cloned()
            .unwrap();
        assert!(validate_action_args(&number_action, &mut args).is_err());
    }
}
