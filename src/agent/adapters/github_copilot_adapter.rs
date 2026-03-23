use crate::agent::provider_adapter::{completion_url, ProviderAdapter};
use crate::config::ProviderConfig;
use crate::oauth::OAuthTokenEntry;

const COPILOT_CHAT_COMPLETIONS_ENDPOINT: &str = "https://api.githubcopilot.com/chat/completions";

pub struct GitHubCopilotAdapter;

impl ProviderAdapter for GitHubCopilotAdapter {
    fn provider_type(&self) -> &str {
        "github-copilot"
    }

    fn resolve_endpoint(&self, config: &ProviderConfig, oauth: Option<&OAuthTokenEntry>) -> String {
        if oauth.is_some() {
            return COPILOT_CHAT_COMPLETIONS_ENDPOINT.to_string();
        }
        completion_url(&config.base_url)
    }

    fn extra_headers(
        &self,
        _config: &ProviderConfig,
        oauth: Option<&OAuthTokenEntry>,
    ) -> Vec<(String, String)> {
        if oauth.is_some() {
            return vec![
                ("Editor-Version".to_string(), "OSAgent/1.0".to_string()),
                ("User-Agent".to_string(), "OSAgent/1.0".to_string()),
                (
                    "Openai-Intent".to_string(),
                    "conversation-edits".to_string(),
                ),
            ];
        }
        Vec::new()
    }
}
