use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum OAuthFlowType {
    Pkce,
    DeviceCode,
    Basic,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct OAuthProviderConfig {
    pub id: &'static str,
    pub name: &'static str,
    pub client_id: &'static str,
    pub authorization_url: &'static str,
    pub token_url: &'static str,
    pub scopes: Vec<&'static str>,
    pub flow_type: OAuthFlowType,
    pub device_code_url: Option<&'static str>,
    pub user_code_length: Option<usize>,
}

pub fn get_oauth_providers() -> Vec<OAuthProviderConfig> {
    vec![
        OAuthProviderConfig {
            id: "openai",
            name: "OpenAI",
            client_id: "",
            authorization_url: "https://oauth.openai.com/authorize",
            token_url: "https://oauth.openai.com/api/token",
            scopes: vec!["api.full-access"],
            flow_type: OAuthFlowType::Pkce,
            device_code_url: None,
            user_code_length: None,
        },
        OAuthProviderConfig {
            id: "anthropic",
            name: "Anthropic",
            client_id: "",
            authorization_url: "https://auth.anthropic.com/oauth/authorize",
            token_url: "https://auth.anthropic.com/oauth/token",
            scopes: vec!["api:read", "api:write"],
            flow_type: OAuthFlowType::Pkce,
            device_code_url: None,
            user_code_length: None,
        },
        OAuthProviderConfig {
            id: "google_ai",
            name: "Google AI",
            client_id: "",
            authorization_url: "https://accounts.google.com/o/oauth2/v2/auth",
            token_url: "https://oauth2.googleapis.com/token",
            scopes: vec!["https://www.googleapis.com/auth/cloud-platform"],
            flow_type: OAuthFlowType::Pkce,
            device_code_url: None,
            user_code_length: None,
        },
        OAuthProviderConfig {
            id: "chutes",
            name: "Chutes",
            client_id: "",
            authorization_url: "https://api.chutes.ai/idp/authorize",
            token_url: "https://api.chutes.ai/idp/token",
            scopes: vec!["openid", "profile", "email"],
            flow_type: OAuthFlowType::Pkce,
            device_code_url: None,
            user_code_length: None,
        },
        OAuthProviderConfig {
            id: "qwen",
            name: "Qwen",
            client_id: "",
            authorization_url: "https://qwen.cloudflare.ai/oauth/authorize",
            token_url: "https://qwen.cloudflare.ai/oauth/token",
            scopes: vec!["openid", "profile", "email"],
            flow_type: OAuthFlowType::DeviceCode,
            device_code_url: Some("https://qwen.cloudflare.ai/oauth/device/code"),
            user_code_length: Some(8),
        },
    ]
}

pub fn get_oauth_provider(id: &str) -> Option<OAuthProviderConfig> {
    get_oauth_providers().into_iter().find(|p| p.id == id)
}

pub fn is_oauth_provider(id: &str) -> bool {
    get_oauth_providers().iter().any(|p| p.id == id)
}

pub fn is_pkce_oauth_provider(id: &str) -> bool {
    get_oauth_provider(id)
        .map(|p| p.flow_type == OAuthFlowType::Pkce)
        .unwrap_or(false)
}

pub fn is_device_code_oauth_provider(id: &str) -> bool {
    get_oauth_provider(id)
        .map(|p| p.flow_type == OAuthFlowType::DeviceCode)
        .unwrap_or(false)
}
