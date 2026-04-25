use crate::agent::provider_adapter::ProviderAdapter;
use crate::config::ProviderConfig;
use crate::oauth::OAuthTokenEntry;

pub struct OpenRouterAdapter;

impl ProviderAdapter for OpenRouterAdapter {
    fn provider_type(&self) -> &str {
        "openrouter"
    }

    fn extra_headers(
        &self,
        config: &ProviderConfig,
        _oauth: Option<&OAuthTokenEntry>,
    ) -> Vec<(String, String)> {
        let model = config.model.to_lowercase();
        if model.contains("claude") {
            // Enable Anthropic prompt caching via OpenRouter.
            // The PromptCache in prompt.rs ensures the system prefix is stable across
            // turns, which is the prerequisite for cache hits.
            vec![(
                "anthropic-beta".to_string(),
                "prompt-caching-2024-07-31".to_string(),
            )]
        } else {
            Vec::new()
        }
    }
}
