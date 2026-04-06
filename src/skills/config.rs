use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillConfigSchema {
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub emoji: Option<String>,
    #[serde(default)]
    pub icon_url: Option<String>,
    #[serde(default)]
    pub requires: SkillRequirements,
    #[serde(default)]
    pub config: Vec<ConfigField>,
    #[serde(default)]
    pub actions: Vec<SkillActionSchema>,
    #[serde(default)]
    pub token_refresh: Option<SkillTokenRefreshSchema>,
}

/// Parse the YAML frontmatter block from a SKILL.md file.
/// Returns None if no frontmatter is present or parsing fails.
pub fn parse_frontmatter(content: &str) -> Option<SkillConfigSchema> {
    if !content.starts_with("---") {
        return None;
    }
    let after_open = &content[3..];
    // Find the closing ---
    let end = after_open.find("\n---")?;
    let yaml = &after_open[..end];
    serde_yaml::from_str(yaml).ok()
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct SkillRequirements {
    #[serde(default)]
    pub bins: Vec<String>,
    #[serde(default)]
    pub files: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConfigField {
    pub name: String,
    #[serde(rename = "type")]
    pub field_type: ConfigFieldType,
    pub description: String,
    #[serde(default)]
    pub required: bool,
    #[serde(default)]
    pub default: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillActionSchema {
    pub name: String,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub parameters: Vec<SkillActionParameter>,
    #[serde(flatten)]
    pub runner: SkillActionRunner,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillActionParameter {
    pub name: String,
    #[serde(rename = "type")]
    pub parameter_type: SkillActionParameterType,
    #[serde(default)]
    pub description: String,
    #[serde(default)]
    pub required: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
pub enum SkillActionParameterType {
    String,
    Number,
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum SkillActionRunner {
    Http {
        method: String,
        url: String,
        #[serde(default)]
        headers: HashMap<String, String>,
        #[serde(default)]
        query: HashMap<String, String>,
        #[serde(default)]
        body: Option<serde_json::Value>,
        #[serde(default)]
        body_form: Option<HashMap<String, String>>,
        #[serde(default)]
        response_transform: Option<String>,
    },
    Script {
        script: String,
        #[serde(default)]
        args: Vec<String>,
    },
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillTokenRefreshSchema {
    pub token_url: String,
    #[serde(default = "default_grant_type")]
    pub grant_type: String,
    pub refresh_token_field: String,
    pub access_token_field: String,
    #[serde(default)]
    pub client_id_field: String,
    #[serde(default)]
    pub client_secret_field: String,
    #[serde(default)]
    pub body: Option<HashMap<String, String>>,
    #[serde(default)]
    pub headers: Option<HashMap<String, String>>,
    #[serde(default)]
    pub response_access_token_path: String,
    #[serde(default)]
    pub response_refresh_token_path: String,
    #[serde(default)]
    pub token_expiry_seconds: Option<u64>,
    #[serde(default)]
    pub authorize_url: Option<String>,
    #[serde(default)]
    pub scopes: Option<String>,
    #[serde(default = "default_callback_port")]
    pub callback_port: u16,
    #[serde(default = "default_redirect_path")]
    pub redirect_path: String,
}

fn default_grant_type() -> String {
    "refresh_token".to_string()
}

fn default_callback_port() -> u16 {
    8888
}

fn default_redirect_path() -> String {
    "/callback".to_string()
}

impl SkillTokenRefreshSchema {
    pub fn redirect_uri(&self) -> String {
        format!(
            "http://127.0.0.1:{}{}",
            self.callback_port, self.redirect_path
        )
    }

    pub fn supports_native_oauth(&self) -> bool {
        self.authorize_url.is_some()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "snake_case")]
#[derive(Default)]
pub enum ConfigFieldType {
    #[default]
    String,
    ApiKey,
    Password,
    Number,
    Boolean,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SkillConfig {
    pub settings: HashMap<String, String>,
    #[serde(default = "default_enabled")]
    pub enabled: bool,
}

fn default_enabled() -> bool {
    true
}

impl Default for SkillConfig {
    fn default() -> Self {
        Self {
            settings: HashMap::new(),
            enabled: true,
        }
    }
}

#[derive(Debug)]
pub struct SkillConfigStore {
    base_dir: PathBuf,
}

impl SkillConfigStore {
    pub fn new(base_dir: PathBuf) -> Self {
        Self { base_dir }
    }

    pub fn get_config_dir(&self) -> PathBuf {
        self.base_dir.clone()
    }

    pub fn skill_config_path(&self, skill_name: &str) -> PathBuf {
        self.base_dir.join(skill_name).join("config.toml")
    }

    pub fn skill_icon_path(&self, skill_name: &str) -> Option<PathBuf> {
        let path = self.base_dir.join(skill_name).join("icon.png");
        if path.exists() {
            Some(path)
        } else {
            None
        }
    }

    pub fn load_config(&self, skill_name: &str) -> std::io::Result<SkillConfig> {
        let path = self.skill_config_path(skill_name);
        if !path.exists() {
            return Ok(SkillConfig::default());
        }
        let content = fs::read_to_string(&path)?;
        let config: SkillConfig = toml::from_str(&content)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        Ok(config)
    }

    pub fn save_config(&self, skill_name: &str, config: &SkillConfig) -> std::io::Result<()> {
        let skill_dir = self.base_dir.join(skill_name);
        fs::create_dir_all(&skill_dir)?;
        let path = self.skill_config_path(skill_name);
        let content = toml::to_string_pretty(config)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        fs::write(&path, content)
    }

    pub fn delete_config(&self, skill_name: &str) -> std::io::Result<()> {
        let skill_dir = self.base_dir.join(skill_name);
        if skill_dir.exists() {
            fs::remove_dir_all(&skill_dir)?;
        }
        Ok(())
    }

    pub fn list_skills_with_config(&self) -> std::io::Result<Vec<String>> {
        if !self.base_dir.exists() {
            return Ok(Vec::new());
        }
        let mut skills = Vec::new();
        for entry in fs::read_dir(&self.base_dir)? {
            let entry = entry?;
            if entry.path().is_dir() {
                if let Some(name) = entry.path().file_name().and_then(|n| n.to_str()) {
                    skills.push(name.to_string());
                }
            }
        }
        Ok(skills)
    }

    pub fn get_masked_config(
        &self,
        skill_name: &str,
    ) -> std::io::Result<HashMap<String, MaskedValue>> {
        let config = self.load_config(skill_name)?;
        let mut masked = HashMap::new();
        for (key, value) in config.settings {
            masked.insert(key, MaskedValue::from_value(&value));
        }
        masked.insert("enabled".to_string(), MaskedValue::Bool(config.enabled));
        Ok(masked)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum MaskedValue {
    String(String),
    Bool(bool),
    Number(f64),
}

impl MaskedValue {
    pub fn from_value(value: &str) -> Self {
        if let Ok(num) = value.parse::<f64>() {
            MaskedValue::Number(num)
        } else if value == "true" || value == "false" {
            MaskedValue::Bool(value.parse::<bool>().unwrap_or(false))
        } else {
            MaskedValue::String(value.to_string())
        }
    }

    pub fn masked(&self) -> String {
        match self {
            MaskedValue::String(s) if s.len() > 8 => {
                format!("{}...{}", &s[..4], &s[s.len() - 4..])
            }
            MaskedValue::String(s) if !s.is_empty() => "***".to_string(),
            MaskedValue::String(_) => "".to_string(),
            MaskedValue::Bool(b) => b.to_string(),
            MaskedValue::Number(n) => n.to_string(),
        }
    }

    pub fn is_api_key(&self) -> bool {
        matches!(self, MaskedValue::String(s) if s.len() > 10)
    }

    pub fn as_string(&self) -> String {
        match self {
            MaskedValue::String(s) => s.clone(),
            MaskedValue::Bool(b) => b.to_string(),
            MaskedValue::Number(n) => n.to_string(),
        }
    }
}

pub fn get_config_base_dir() -> PathBuf {
    let data_dir = std::env::var("OSAGENT_DATA_DIR")
        .unwrap_or_else(|_| std::env::var("OSAGENT_WORKSPACE").unwrap_or_else(|_| ".".to_string()));
    PathBuf::from(data_dir).join("skills-config")
}
