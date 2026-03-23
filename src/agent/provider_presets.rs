use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub env_vars: Vec<String>,
    pub description: String,
    pub models: Vec<ModelPreset>,
    pub api_key_url: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelPreset {
    pub id: String,
    pub name: String,
    pub context_window: usize,
    pub supports_tools: bool,
    pub supports_vision: bool,
    pub category: String,
}

pub fn get_presets() -> Vec<ProviderPreset> {
    vec![
        ProviderPreset {
            id: "openrouter".to_string(),
            name: "OpenRouter".to_string(),
            base_url: "https://openrouter.ai/api/v1".to_string(),
            env_vars: vec!["OPENROUTER_API_KEY".to_string()],
            description: "Multi-provider aggregator with access to 200+ models".to_string(),
            api_key_url: Some("https://openrouter.ai/keys".to_string()),
            models: vec![
                ModelPreset {
                    id: "anthropic/claude-sonnet-4".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "anthropic/claude-3.5-sonnet".to_string(),
                    name: "Claude 3.5 Sonnet".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "anthropic/claude-3-opus".to_string(),
                    name: "Claude 3 Opus".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "anthropic/claude-3-haiku".to_string(),
                    name: "Claude 3 Haiku".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "openai/gpt-4.1".to_string(),
                    name: "GPT-4.1".to_string(),
                    context_window: 1_047_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "openai/gpt-4.1-mini".to_string(),
                    name: "GPT-4.1 Mini".to_string(),
                    context_window: 1_047_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "openai/gpt-4.1-nano".to_string(),
                    name: "GPT-4.1 Nano".to_string(),
                    context_window: 1_047_576,
                    supports_tools: true,
                    supports_vision: false,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "openai/gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                    context_window: 128_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "openai/gpt-4o-mini".to_string(),
                    name: "GPT-4o Mini".to_string(),
                    context_window: 128_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "google/gemini-2.5-pro-preview".to_string(),
                    name: "Gemini 2.5 Pro".to_string(),
                    context_window: 1_048_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "google/gemini-2.5-flash-preview".to_string(),
                    name: "Gemini 2.5 Flash".to_string(),
                    context_window: 1_048_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "google/gemini-2.0-flash-001".to_string(),
                    name: "Gemini 2.0 Flash".to_string(),
                    context_window: 1_048_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "meta-llama/llama-3.1-405b-instruct".to_string(),
                    name: "Llama 3.1 405B".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "open".to_string(),
                },
                ModelPreset {
                    id: "mistralai/mistral-large-2411".to_string(),
                    name: "Mistral Large".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "deepseek/deepseek-r1".to_string(),
                    name: "DeepSeek R1".to_string(),
                    context_window: 131_072,
                    supports_tools: false,
                    supports_vision: false,
                    category: "reasoning".to_string(),
                },
                ModelPreset {
                    id: "deepseek/deepseek-chat".to_string(),
                    name: "DeepSeek V3".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "x-ai/grok-3".to_string(),
                    name: "Grok 3".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "qwen/qwen3-235b-a22b".to_string(),
                    name: "Qwen3 235B".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "open".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "openai".to_string(),
            name: "OpenAI".to_string(),
            base_url: "https://api.openai.com/v1".to_string(),
            env_vars: vec!["OPENAI_API_KEY".to_string()],
            description: "OpenAI API direct access".to_string(),
            api_key_url: Some("https://platform.openai.com/api-keys".to_string()),
            models: vec![
                ModelPreset {
                    id: "gpt-4.1".to_string(),
                    name: "GPT-4.1".to_string(),
                    context_window: 1_047_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "gpt-4.1-mini".to_string(),
                    name: "GPT-4.1 Mini".to_string(),
                    context_window: 1_047_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "gpt-4.1-nano".to_string(),
                    name: "GPT-4.1 Nano".to_string(),
                    context_window: 1_047_576,
                    supports_tools: true,
                    supports_vision: false,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "gpt-4o".to_string(),
                    name: "GPT-4o".to_string(),
                    context_window: 128_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "gpt-4o-mini".to_string(),
                    name: "GPT-4o Mini".to_string(),
                    context_window: 128_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "o3-mini".to_string(),
                    name: "o3-mini".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: false,
                    category: "reasoning".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "anthropic".to_string(),
            name: "Anthropic".to_string(),
            base_url: "https://api.anthropic.com/v1".to_string(),
            env_vars: vec!["ANTHROPIC_API_KEY".to_string()],
            description: "Anthropic API direct access".to_string(),
            api_key_url: Some("https://console.anthropic.com/settings/keys".to_string()),
            models: vec![
                ModelPreset {
                    id: "claude-sonnet-4-20250514".to_string(),
                    name: "Claude Sonnet 4".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "claude-3-5-sonnet-20241022".to_string(),
                    name: "Claude 3.5 Sonnet".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "claude-3-opus-20240229".to_string(),
                    name: "Claude 3 Opus".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "claude-3-haiku-20240307".to_string(),
                    name: "Claude 3 Haiku".to_string(),
                    context_window: 200_000,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "google".to_string(),
            name: "Google AI".to_string(),
            base_url: "https://generativelanguage.googleapis.com/v1beta/openai".to_string(),
            env_vars: vec!["GOOGLE_API_KEY".to_string()],
            description: "Google Gemini API (OpenAI-compatible endpoint)".to_string(),
            api_key_url: Some("https://aistudio.google.com/apikey".to_string()),
            models: vec![
                ModelPreset {
                    id: "gemini-2.5-pro-preview-05-06".to_string(),
                    name: "Gemini 2.5 Pro".to_string(),
                    context_window: 1_048_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "gemini-2.5-flash-preview-05-20".to_string(),
                    name: "Gemini 2.5 Flash".to_string(),
                    context_window: 1_048_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "gemini-2.0-flash-001".to_string(),
                    name: "Gemini 2.0 Flash".to_string(),
                    context_window: 1_048_576,
                    supports_tools: true,
                    supports_vision: true,
                    category: "popular".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "ollama".to_string(),
            name: "Ollama (Local)".to_string(),
            base_url: "http://localhost:11434/v1".to_string(),
            env_vars: vec![],
            description: "Run models locally with Ollama".to_string(),
            api_key_url: None,
            models: vec![
                ModelPreset {
                    id: "llama3.1:70b".to_string(),
                    name: "Llama 3.1 70B".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "qwen3:32b".to_string(),
                    name: "Qwen3 32B".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "popular".to_string(),
                },
                ModelPreset {
                    id: "mistral:7b".to_string(),
                    name: "Mistral 7B".to_string(),
                    context_window: 32_768,
                    supports_tools: true,
                    supports_vision: false,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "codellama:13b".to_string(),
                    name: "CodeLlama 13B".to_string(),
                    context_window: 16_384,
                    supports_tools: true,
                    supports_vision: false,
                    category: "code".to_string(),
                },
                ModelPreset {
                    id: "deepseek-r1:14b".to_string(),
                    name: "DeepSeek R1 14B".to_string(),
                    context_window: 131_072,
                    supports_tools: false,
                    supports_vision: false,
                    category: "reasoning".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "groq".to_string(),
            name: "Groq".to_string(),
            base_url: "https://api.groq.com/openai/v1".to_string(),
            env_vars: vec!["GROQ_API_KEY".to_string()],
            description: "Ultra-fast inference with Groq".to_string(),
            api_key_url: Some("https://console.groq.com/keys".to_string()),
            models: vec![
                ModelPreset {
                    id: "llama-3.3-70b-versatile".to_string(),
                    name: "Llama 3.3 70B".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "llama-3.1-8b-instant".to_string(),
                    name: "Llama 3.1 8B".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "fast".to_string(),
                },
                ModelPreset {
                    id: "mixtral-8x7b-32768".to_string(),
                    name: "Mixtral 8x7B".to_string(),
                    context_window: 32_768,
                    supports_tools: true,
                    supports_vision: false,
                    category: "popular".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            env_vars: vec!["DEEPSEEK_API_KEY".to_string()],
            description: "DeepSeek API direct access".to_string(),
            api_key_url: Some("https://platform.deepseek.com/api_keys".to_string()),
            models: vec![
                ModelPreset {
                    id: "deepseek-r1".to_string(),
                    name: "DeepSeek R1".to_string(),
                    context_window: 131_072,
                    supports_tools: false,
                    supports_vision: false,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "deepseek-chat".to_string(),
                    name: "DeepSeek V3".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "popular".to_string(),
                },
            ],
        },
        ProviderPreset {
            id: "xai".to_string(),
            name: "xAI".to_string(),
            base_url: "https://api.x.ai/v1".to_string(),
            env_vars: vec!["XAI_API_KEY".to_string()],
            description: "xAI Grok API".to_string(),
            api_key_url: Some("https://console.x.ai/keys".to_string()),
            models: vec![
                ModelPreset {
                    id: "grok-3".to_string(),
                    name: "Grok 3".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "recommended".to_string(),
                },
                ModelPreset {
                    id: "grok-3-mini".to_string(),
                    name: "Grok 3 Mini".to_string(),
                    context_window: 131_072,
                    supports_tools: true,
                    supports_vision: false,
                    category: "fast".to_string(),
                },
            ],
        },
    ]
}

pub fn get_preset(id: &str) -> Option<ProviderPreset> {
    get_presets().into_iter().find(|p| p.id == id)
}

pub fn detect_env_providers() -> Vec<ProviderPreset> {
    let mut detected = Vec::new();
    for preset in get_presets() {
        for env_var in &preset.env_vars {
            if let Ok(val) = std::env::var(env_var) {
                if !val.is_empty() {
                    detected.push(preset.clone());
                    break;
                }
            }
        }
    }
    detected
}

pub fn resolve_env_api_key(preset_id: &str) -> Option<String> {
    let preset = get_preset(preset_id)?;
    for env_var in &preset.env_vars {
        if let Ok(val) = std::env::var(env_var) {
            if !val.is_empty() {
                return Some(val);
            }
        }
    }
    None
}

pub fn get_all_models() -> Vec<(String, ModelPreset)> {
    let mut all = Vec::new();
    for preset in get_presets() {
        for model in &preset.models {
            all.push((preset.id.clone(), model.clone()));
        }
    }
    all
}

pub fn lookup_model(provider_id: &str, model_id: &str) -> Option<ModelPreset> {
    let preset = get_preset(provider_id)?;
    preset.models.into_iter().find(|m| m.id == model_id)
}

pub fn search_models(query: &str) -> Vec<(String, ModelPreset)> {
    let query = query.to_lowercase();
    get_all_models()
        .into_iter()
        .filter(|(provider_id, model)| {
            model.id.to_lowercase().contains(&query)
                || model.name.to_lowercase().contains(&query)
                || provider_id.to_lowercase().contains(&query)
        })
        .collect()
}
