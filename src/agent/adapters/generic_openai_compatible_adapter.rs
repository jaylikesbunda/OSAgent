use crate::agent::provider_adapter::ProviderAdapter;

pub struct GenericOpenAICompatibleAdapter;

impl ProviderAdapter for GenericOpenAICompatibleAdapter {
    fn provider_type(&self) -> &str {
        "openai-compatible"
    }
}
