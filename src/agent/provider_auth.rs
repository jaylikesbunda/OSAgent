use crate::agent::provider_presets::get_preset;
use crate::config::ProviderConfig;
use serde_json::{json, Value};
use tracing::info;

pub struct ProviderAuth;

#[derive(Debug, Clone)]
pub struct ProviderAuthResult {
    pub autoload: bool,
    pub extra_headers: Vec<(String, String)>,
    pub extra_options: Value,
    pub api_key_override: Option<String>,
    pub base_url_override: Option<String>,
}

impl ProviderAuth {
    pub fn configure(provider_type: &str, config: &ProviderConfig) -> ProviderAuthResult {
        match provider_type {
            "openrouter" => Self::openrouter(config),
            "anthropic" => Self::anthropic(config),
            "openai" => Self::openai(config),
            "google-vertex" => Self::google_vertex(config),
            "azure" => Self::azure(config),
            "github-copilot" => Self::github_copilot(config),
            "amazon-bedrock" => Self::amazon_bedrock(config),
            "groq" => Self::groq(config),
            "cerebras" => Self::cerebras(config),
            "xai" => Self::xai(config),
            "ollama" => Self::ollama(config),
            "deepseek" => Self::deepseek(config),
            "togetherai" => Self::togetherai(config),
            "mistral" => Self::mistral(config),
            _ => ProviderAuthResult {
                autoload: !config.api_key.is_empty(),
                extra_headers: vec![],
                extra_options: json!({}),
                api_key_override: None,
                base_url_override: None,
            },
        }
    }

    fn openrouter(config: &ProviderConfig) -> ProviderAuthResult {
        let headers = vec![
            (
                "HTTP-Referer".to_string(),
                "https://osagent.local".to_string(),
            ),
            ("X-Title".to_string(), "OSAgent".to_string()),
        ];
        ProviderAuthResult {
            autoload: !config.api_key.is_empty(),
            extra_headers: headers,
            extra_options: json!({}),
            api_key_override: None,
            base_url_override: None,
        }
    }

    fn anthropic(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("ANTHROPIC_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({
                "anthropic-beta": "interleaved-thinking-2025-05-14"
            }),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn openai(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("OPENAI_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn google_vertex(config: &ProviderConfig) -> ProviderAuthResult {
        let project = std::env::var("GOOGLE_CLOUD_PROJECT")
            .or_else(|_| std::env::var("GCP_PROJECT"))
            .or_else(|_| std::env::var("GCLOUD_PROJECT"))
            .unwrap_or_default();

        let location = std::env::var("GOOGLE_CLOUD_LOCATION")
            .or_else(|_| std::env::var("VERTEX_LOCATION"))
            .unwrap_or_else(|_| "us-central1".to_string());

        let autoload = !project.is_empty() || !config.api_key.is_empty();

        let base_url = if !project.is_empty() {
            let endpoint = if location == "global" {
                "aiplatform.googleapis.com".to_string()
            } else {
                format!("{}-aiplatform.googleapis.com", location)
            };
            Some(format!(
                "https://{}/v1beta1/projects/{}/locations/{}",
                endpoint, project, location
            ))
        } else if !config.base_url.is_empty() && !config.base_url.contains("{{") {
            Some(config.base_url.clone())
        } else {
            None
        };

        ProviderAuthResult {
            autoload,
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: None,
            base_url_override: base_url,
        }
    }

    fn azure(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("AZURE_OPENAI_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };

        let base_url = if config.base_url.contains("{{") {
            if let Ok(endpoint) = std::env::var("AZURE_OPENAI_ENDPOINT") {
                Some(endpoint.trim_end_matches('/').to_string())
            } else {
                let resource = std::env::var("AZURE_OPENAI_RESOURCE")
                    .unwrap_or_else(|_| "resource".to_string());
                let deployment = std::env::var("AZURE_OPENAI_DEPLOYMENT")
                    .unwrap_or_else(|_| "gpt-4o".to_string());
                Some(format!(
                    "https://{}.openai.azure.com/openai/deployments/{}",
                    resource, deployment
                ))
            }
        } else if !config.base_url.is_empty() {
            Some(config.base_url.clone())
        } else {
            None
        };

        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![("api-key".to_string(), key.clone())],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: base_url,
        }
    }

    fn github_copilot(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("GITHUB_TOKEN").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        let headers = vec![
            ("Editor-Version".to_string(), "OSAgent/1.0".to_string()),
            ("User-Agent".to_string(), "OSAgent/1.0".to_string()),
        ];
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: headers,
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: if config.base_url.is_empty() || config.base_url.contains("{{") {
                Some("https://api.githubcopilot.com".to_string())
            } else {
                None
            },
        }
    }

    fn amazon_bedrock(config: &ProviderConfig) -> ProviderAuthResult {
        let access_key = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default();
        let secret_key = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default();
        let session_token = std::env::var("AWS_SESSION_TOKEN").ok();
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        let autoload = !access_key.is_empty()
            || !secret_key.is_empty()
            || !config.api_key.is_empty()
            || session_token.is_some();

        let base_url = if config.base_url.contains("{{") {
            Some(format!("https://bedrock-runtime.{}.amazonaws.com", region))
        } else if !config.base_url.is_empty() {
            Some(config.base_url.clone())
        } else {
            None
        };

        ProviderAuthResult {
            autoload,
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: None,
            base_url_override: base_url,
        }
    }

    fn groq(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("GROQ_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn cerebras(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("CEREBRAS_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        let headers = vec![(
            "X-Cerebras-3rd-Party-Integration".to_string(),
            "osagent".to_string(),
        )];
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: headers,
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn xai(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("XAI_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn ollama(config: &ProviderConfig) -> ProviderAuthResult {
        let base_url = if config.base_url.is_empty() || config.base_url.contains("localhost") {
            Some("http://localhost:11434/v1".to_string())
        } else {
            None
        };
        ProviderAuthResult {
            autoload: true,
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: Some("ollama".to_string()),
            base_url_override: base_url,
        }
    }

    fn deepseek(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("DEEPSEEK_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn togetherai(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("TOGETHER_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }

    fn mistral(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("MISTRAL_API_KEY").unwrap_or_default()
        } else {
            config.api_key.clone()
        };
        ProviderAuthResult {
            autoload: !key.is_empty(),
            extra_headers: vec![],
            extra_options: json!({}),
            api_key_override: if key.is_empty() { None } else { Some(key) },
            base_url_override: None,
        }
    }
}

pub fn resolve_provider_config(mut config: ProviderConfig) -> ProviderConfig {
    let preset = get_preset(&config.provider_type);
    let auth = ProviderAuth::configure(&config.provider_type, &config);

    if config.base_url.is_empty() || config.base_url.contains("{{") {
        if let Some(url) = auth.base_url_override {
            config.base_url = url;
        } else if let Some(ref p) = preset {
            if !p.base_url.contains("{{") {
                config.base_url = p.base_url.clone();
            }
        }
    }

    if config.api_key.is_empty() {
        if let Some(key) = auth.api_key_override {
            config.api_key = key;
        } else {
            config.api_key =
                crate::agent::provider_presets::resolve_env_api_key(&config.provider_type)
                    .unwrap_or_default();
        }
    }

    config
}

pub fn get_extra_headers(provider_type: &str, config: &ProviderConfig) -> Vec<(String, String)> {
    ProviderAuth::configure(provider_type, config).extra_headers
}
