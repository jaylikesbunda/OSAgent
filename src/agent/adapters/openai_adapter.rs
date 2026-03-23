use crate::agent::provider_adapter::{completion_url, ProviderAdapter, RequestMode};
use crate::config::ProviderConfig;
use crate::oauth::OAuthTokenEntry;

const OPENAI_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";

pub struct OpenAIAdapter;

impl ProviderAdapter for OpenAIAdapter {
    fn provider_type(&self) -> &str {
        "openai"
    }

    fn resolve_endpoint(&self, config: &ProviderConfig, oauth: Option<&OAuthTokenEntry>) -> String {
        if oauth.is_some() {
            return OPENAI_CODEX_API_ENDPOINT.to_string();
        }
        completion_url(&config.base_url)
    }

    fn request_mode(
        &self,
        _config: &ProviderConfig,
        oauth: Option<&OAuthTokenEntry>,
        _model: &str,
    ) -> RequestMode {
        if oauth.is_some() {
            return RequestMode::Responses;
        }
        RequestMode::ChatCompletions
    }

    fn extra_headers(
        &self,
        _config: &ProviderConfig,
        oauth: Option<&OAuthTokenEntry>,
    ) -> Vec<(String, String)> {
        let Some(entry) = oauth else {
            return Vec::new();
        };

        let mut headers = vec![("originator".to_string(), "osagent".to_string())];
        if let Some(account_id) = entry.account_id.clone() {
            headers.push(("ChatGPT-Account-Id".to_string(), account_id));
        }
        headers
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uses_responses_mode_with_oauth() {
        let adapter = OpenAIAdapter;
        let cfg = ProviderConfig::default();
        let oauth = OAuthTokenEntry {
            access_token: "tok".to_string(),
            refresh_token: None,
            expires_at: None,
            scopes: None,
            account_id: None,
        };

        assert_eq!(
            adapter.request_mode(&cfg, Some(&oauth), "gpt-5"),
            RequestMode::Responses
        );
        assert_eq!(
            adapter.request_mode(&cfg, None, "gpt-5"),
            RequestMode::ChatCompletions
        );
    }
}
