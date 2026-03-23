use std::sync::Arc;

use crate::config::ProviderConfig;
use crate::oauth::OAuthTokenEntry;
use crate::storage::models::Message;

use super::adapters::generic_openai_compatible_adapter::GenericOpenAICompatibleAdapter;
use super::adapters::github_copilot_adapter::GitHubCopilotAdapter;
use super::adapters::openai_adapter::OpenAIAdapter;
use super::adapters::openrouter_adapter::OpenRouterAdapter;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RequestMode {
    ChatCompletions,
    Responses,
    Custom,
}

pub trait ProviderAdapter: Send + Sync {
    fn provider_type(&self) -> &str;

    fn resolve_endpoint(
        &self,
        config: &ProviderConfig,
        _oauth: Option<&OAuthTokenEntry>,
    ) -> String {
        completion_url(&config.base_url)
    }

    fn request_mode(
        &self,
        _config: &ProviderConfig,
        _oauth: Option<&OAuthTokenEntry>,
        _model: &str,
    ) -> RequestMode {
        RequestMode::ChatCompletions
    }

    fn extra_headers(
        &self,
        _config: &ProviderConfig,
        _oauth: Option<&OAuthTokenEntry>,
    ) -> Vec<(String, String)> {
        Vec::new()
    }

    fn transform_messages(&self, messages: &[Message], config: &ProviderConfig) -> Vec<Message> {
        crate::agent::provider_transforms::transform_messages(messages, config)
    }

    fn transform_schema(&self, schema: serde_json::Value, _model: &str) -> serde_json::Value {
        schema
    }

    fn default_options(&self, provider_type: &str, model: &str) -> serde_json::Value {
        crate::agent::provider_transforms::get_provider_specific_options(provider_type, model)
    }
}

pub fn create_provider_adapter(provider_type: &str) -> Arc<dyn ProviderAdapter> {
    match provider_type {
        "openai" => Arc::new(OpenAIAdapter),
        "github-copilot" => Arc::new(GitHubCopilotAdapter),
        "openrouter" => Arc::new(OpenRouterAdapter),
        _ => Arc::new(GenericOpenAICompatibleAdapter),
    }
}

pub fn completion_url(base_url: &str) -> String {
    let trimmed = base_url.trim_end_matches('/');
    if trimmed.ends_with("/chat/completions") {
        trimmed.to_string()
    } else {
        format!("{}/chat/completions", trimmed)
    }
}
