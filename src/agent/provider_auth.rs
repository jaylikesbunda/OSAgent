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

        // Check for GCP access token (ADC support)
        let gcp_token = std::env::var("GCLOUD_ACCESS_TOKEN")
            .ok()
            .filter(|t| !t.is_empty());

        // Check for service account JSON file path
        let sa_file = std::env::var("GOOGLE_APPLICATION_CREDENTIALS")
            .ok()
            .filter(|p| !p.is_empty());

        let has_creds = !project.is_empty()
            || !config.api_key.is_empty()
            || gcp_token.is_some()
            || sa_file.is_some();

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

        let mut headers: Vec<(String, String)> = Vec::new();
        if let Some(token) = gcp_token {
            headers.push(("Authorization".to_string(), format!("Bearer {}", token)));
        }

        ProviderAuthResult {
            autoload: has_creds,
            extra_headers: headers,
            extra_options: json!({}),
            api_key_override: None,
            base_url_override: base_url,
        }
    }

    fn azure(config: &ProviderConfig) -> ProviderAuthResult {
        let key = if config.api_key.is_empty() {
            std::env::var("AZURE_OPENAI_KEY")
                .or_else(|_| std::env::var("AZURE_OPENAI_API_KEY"))
                .unwrap_or_default()
        } else {
            config.api_key.clone()
        };

        let base_url = if config.base_url.contains("{{") || config.base_url.is_empty() {
            if let Ok(endpoint) = std::env::var("AZURE_OPENAI_ENDPOINT") {
                Some(endpoint.trim_end_matches('/').to_string())
            } else {
                let resource = std::env::var("AZURE_OPENAI_RESOURCE")
                    .or_else(|_| std::env::var("AZURE_RESOURCE_NAME"))
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

    fn load_aws_profile(profile: &str) -> Option<(String, String, Option<String>)> {
        let aws_dir = dirs_next::home_dir().map(|d| d.join(".aws"));
        let cred_path = aws_dir.as_ref()?.join("credentials");
        let content = std::fs::read_to_string(cred_path).ok()?;
        let mut in_profile = false;
        let mut access_key = String::new();
        let mut secret_key = String::new();
        let mut session_token: Option<String> = None;

        for line in content.lines() {
            let line = line.trim();
            if line.starts_with('[') && line.ends_with(']') {
                let name = &line[1..line.len() - 1];
                in_profile = name == profile;
                continue;
            }
            if in_profile {
                if let Some(val) = line.strip_prefix("aws_access_key_id =") {
                    access_key = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("aws_secret_access_key =") {
                    secret_key = val.trim().to_string();
                } else if let Some(val) = line.strip_prefix("aws_session_token =") {
                    session_token = Some(val.trim().to_string());
                }
            }
        }

        if !access_key.is_empty() && !secret_key.is_empty() {
            Some((access_key, secret_key, session_token))
        } else {
            None
        }
    }

    fn amazon_bedrock(config: &ProviderConfig) -> ProviderAuthResult {
        let region = std::env::var("AWS_REGION")
            .or_else(|_| std::env::var("AWS_DEFAULT_REGION"))
            .unwrap_or_else(|_| "us-east-1".to_string());

        // Try credential chain: env vars > profile > config api_key
        let (access_key, secret_key, session_token) = {
            let ak = std::env::var("AWS_ACCESS_KEY_ID").unwrap_or_default();
            let sk = std::env::var("AWS_SECRET_ACCESS_KEY").unwrap_or_default();
            let st = std::env::var("AWS_SESSION_TOKEN").ok();

            if !ak.is_empty() && !sk.is_empty() {
                (ak, sk, st)
            } else {
                let profile =
                    std::env::var("AWS_PROFILE").unwrap_or_else(|_| "default".to_string());
                if let Some((ak, sk, st)) = Self::load_aws_profile(&profile) {
                    (ak, sk, st)
                } else {
                    (String::new(), String::new(), None)
                }
            }
        };

        let has_creds =
            !access_key.is_empty() && !secret_key.is_empty() || !config.api_key.is_empty();

        // Support cross-region inference prefix
        let base_url = if config.base_url.contains("{{") {
            let endpoint = if region.starts_with("us.")
                || region.starts_with("eu.")
                || region.starts_with("apac.")
                || region.starts_with("au.")
                || region.starts_with("jp.")
            {
                format!("bedrock-runtime.{}.amazonaws.com", region)
            } else {
                format!("bedrock-runtime.{}.amazonaws.com", region)
            };
            Some(format!("https://{}", endpoint))
        } else if !config.base_url.is_empty() {
            Some(config.base_url.clone())
        } else {
            None
        };

        ProviderAuthResult {
            autoload: has_creds,
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
