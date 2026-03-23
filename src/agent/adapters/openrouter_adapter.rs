use crate::agent::provider_adapter::ProviderAdapter;

pub struct OpenRouterAdapter;

impl ProviderAdapter for OpenRouterAdapter {
    fn provider_type(&self) -> &str {
        "openrouter"
    }
}
