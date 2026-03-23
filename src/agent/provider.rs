use crate::config::ProviderConfig;
use crate::error::{OSAgentError, Result};
use crate::oauth::{extract_account_id, OAuthStorage, OAuthTokenEntry};
use crate::storage::models::{Message, ToolCall};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::model_catalog::ModelCatalog;

const MAX_RETRIES: u32 = 4;
const BASE_RETRY_DELAY_SECS: u64 = 2;
const MAX_TOOL_SCHEMA_TOKENS: usize = 8_000;
const OPENAI_CODEX_API_ENDPOINT: &str = "https://chatgpt.com/backend-api/codex/responses";
const OPENAI_CODEX_CLIENT_ID: &str = "app_EMoamEEZ73f0CkXaXp7hrann";

#[async_trait]
pub trait Provider: Send + Sync {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse>;

    #[allow(dead_code)]
    async fn complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent>>>;

    async fn model_context_window(&self) -> Option<usize>;
    async fn current_model(&self) -> String;
    async fn set_model(&self, model: String);
    fn provider_type(&self) -> &str;
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub tool_type: String,
    pub function: ToolFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolFunction {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProviderResponse {
    pub content: Option<String>,
    pub tool_calls: Option<Vec<ToolCall>>,
    pub finish_reason: String,
    #[serde(default)]
    pub retry_count: u32,
    #[serde(default)]
    pub context_compressed: bool,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct TokenUsage {
    pub input: usize,
    pub output: usize,
    pub total: usize,
    #[serde(default)]
    pub cached_read: Option<usize>,
    #[serde(default)]
    pub cached_write: Option<usize>,
    #[serde(default)]
    pub reasoning: Option<usize>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub content: Option<String>,
    pub tool_call: Option<ToolCall>,
    pub done: bool,
}

struct ResolvedRequestAuth {
    request_url: String,
    api_key: String,
    extra_headers: Vec<(String, String)>,
}

pub struct OpenAICompatibleProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    oauth_storage: Option<OAuthStorage>,
    context_window: RwLock<Option<usize>>,
    context_window_attempted: RwLock<bool>,
    model_override: RwLock<Option<String>>,
    catalog: Option<Arc<ModelCatalog>>,
}

impl OpenAICompatibleProvider {
    pub fn new(config: ProviderConfig) -> Result<Self> {
        Self::with_catalog(config, None)
    }

    pub fn with_catalog(
        config: ProviderConfig,
        catalog: Option<Arc<ModelCatalog>>,
    ) -> Result<Self> {
        Self::with_catalog_and_oauth(config, catalog, None)
    }

    pub fn with_catalog_and_oauth(
        config: ProviderConfig,
        catalog: Option<Arc<ModelCatalog>>,
        oauth_storage: Option<OAuthStorage>,
    ) -> Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(Duration::from_secs(3600))
            .build()
            .map_err(OSAgentError::Http)?;

        let api_key_preview = if config.api_key.is_empty() {
            "(EMPTY)".to_string()
        } else if config.api_key.len() > 10 {
            format!(
                "{}...{} ({} chars)",
                &config.api_key[..7],
                &config.api_key[config.api_key.len() - 4..],
                config.api_key.len()
            )
        } else {
            format!("({} chars)", config.api_key.len())
        };

        tracing::info!(
            "Provider initialized: type={}, base_url={}, model={}, api_key={}",
            config.provider_type,
            config.base_url,
            config.model,
            api_key_preview
        );

        Ok(Self {
            client,
            config,
            oauth_storage,
            context_window: RwLock::new(None),
            context_window_attempted: RwLock::new(false),
            model_override: RwLock::new(None),
            catalog,
        })
    }

    fn build_messages(&self, messages: &[Message]) -> Vec<serde_json::Value> {
        messages
            .iter()
            .map(|msg| {
                let mut value = serde_json::json!({
                    "role": msg.role,
                });

                if msg.role == "tool" {
                    value["content"] = serde_json::json!(msg.content);
                    if let Some(tool_call_id) = &msg.tool_call_id {
                        value["tool_call_id"] = serde_json::json!(tool_call_id);
                    }
                } else if msg.role == "assistant" {
                    if let Some(tool_calls) = &msg.tool_calls {
                        let formatted_calls: Vec<serde_json::Value> = tool_calls
                            .iter()
                            .map(|tc| {
                                serde_json::json!({
                                    "id": tc.id,
                                    "type": "function",
                                    "function": {
                                        "name": tc.name,
                                        "arguments": serde_json::to_string(&tc.arguments).unwrap_or_default()
                                    }
                                })
                            })
                            .collect();
                        value["tool_calls"] = serde_json::json!(formatted_calls);
                    }
                    // Always use empty string, never null for content
                    value["content"] = serde_json::json!(msg.content);
                } else {
                    value["content"] = serde_json::json!(msg.content);
                }

                value
            })
            .collect()
    }
}

impl OpenAICompatibleProvider {
    fn oauth_enabled(&self) -> bool {
        self.config.auth_type.as_deref() == Some("oauth")
    }

    fn completion_url(base_url: &str) -> String {
        let trimmed = base_url.trim_end_matches('/');
        if trimmed.ends_with("/chat/completions") {
            trimmed.to_string()
        } else {
            format!("{}/chat/completions", trimmed)
        }
    }

    fn oauth_entry(&self) -> Result<Option<OAuthTokenEntry>> {
        let Some(storage) = &self.oauth_storage else {
            return Ok(None);
        };
        storage
            .get_token(&self.config.provider_type)
            .map_err(|e| OSAgentError::Provider(format!("Failed to load OAuth token: {}", e)))
    }

    fn save_oauth_entry(&self, entry: OAuthTokenEntry) -> Result<()> {
        let Some(storage) = &self.oauth_storage else {
            return Ok(());
        };
        storage
            .set_token(&self.config.provider_type, entry)
            .map_err(|e| OSAgentError::Provider(format!("Failed to save OAuth token: {}", e)))
    }

    fn oauth_expired(entry: &OAuthTokenEntry) -> bool {
        entry
            .expires_at
            .map(|expires_at| expires_at <= chrono::Utc::now().timestamp() + 30)
            .unwrap_or(false)
    }

    async fn refresh_openai_oauth(&self, entry: &OAuthTokenEntry) -> Result<OAuthTokenEntry> {
        let refresh_token = entry.refresh_token.as_ref().ok_or_else(|| {
            OSAgentError::Provider("OpenAI OAuth token is missing a refresh token".to_string())
        })?;

        #[derive(Deserialize)]
        struct TokenResponse {
            access_token: String,
            #[serde(default)]
            refresh_token: String,
            #[serde(default)]
            expires_in: Option<i64>,
            #[serde(default)]
            id_token: Option<String>,
        }

        let response = self
            .client
            .post("https://auth.openai.com/oauth/token")
            .header("Content-Type", "application/x-www-form-urlencoded")
            .body(
                [
                    ("grant_type", "refresh_token"),
                    ("refresh_token", refresh_token.as_str()),
                    ("client_id", OPENAI_CODEX_CLIENT_ID),
                ]
                .iter()
                .map(|(k, v)| format!("{}={}", urlencoding::encode(k), urlencoding::encode(v)))
                .collect::<Vec<_>>()
                .join("&"),
            )
            .send()
            .await
            .map_err(OSAgentError::Http)?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(OSAgentError::Provider(format!(
                "Failed to refresh OpenAI OAuth token: {} {}",
                status,
                body
            )));
        }

        let data: TokenResponse = response.json().await.map_err(OSAgentError::Http)?;
        let refreshed = OAuthTokenEntry {
            access_token: data.access_token.clone(),
            refresh_token: if data.refresh_token.trim().is_empty() {
                entry.refresh_token.clone()
            } else {
                Some(data.refresh_token)
            },
            expires_at: data
                .expires_in
                .map(|secs| chrono::Utc::now().timestamp() + secs),
            scopes: entry.scopes.clone(),
            account_id: extract_account_id(data.id_token.as_deref(), Some(&data.access_token))
                .or_else(|| entry.account_id.clone()),
        };
        self.save_oauth_entry(refreshed.clone())?;
        Ok(refreshed)
    }

    async fn resolve_oauth_entry(&self) -> Result<Option<OAuthTokenEntry>> {
        let Some(entry) = self.oauth_entry()? else {
            return Ok(None);
        };

        if self.config.provider_type == "openai" && Self::oauth_expired(&entry) {
            return self.refresh_openai_oauth(&entry).await.map(Some);
        }

        Ok(Some(entry))
    }

    async fn resolve_request_auth(&self) -> Result<ResolvedRequestAuth> {
        let mut request_url = Self::completion_url(&self.config.base_url);
        let mut api_key = self.config.api_key.clone();
        let mut extra_headers = Vec::new();

        if let Some(headers) = &self.config.custom_headers {
            for (key, value) in headers {
                extra_headers.push((key.clone(), value.clone()));
            }
        }

        if !self.oauth_enabled() {
            return Ok(ResolvedRequestAuth {
                request_url,
                api_key,
                extra_headers,
            });
        }

        let Some(entry) = self.resolve_oauth_entry().await? else {
            return Ok(ResolvedRequestAuth {
                request_url,
                api_key,
                extra_headers,
            });
        };

        api_key = entry.access_token.clone();
        match self.config.provider_type.as_str() {
            "openai" => {
                request_url = OPENAI_CODEX_API_ENDPOINT.to_string();
                if let Some(account_id) = entry.account_id.clone() {
                    extra_headers.push(("ChatGPT-Account-Id".to_string(), account_id));
                }
                extra_headers.push(("originator".to_string(), "osagent".to_string()));
            }
            "github-copilot" => {
                request_url = "https://api.githubcopilot.com/chat/completions".to_string();
                extra_headers.push(("Editor-Version".to_string(), "OSAgent/1.0".to_string()));
                extra_headers.push(("User-Agent".to_string(), "OSAgent/1.0".to_string()));
                extra_headers.push((
                    "Openai-Intent".to_string(),
                    "conversation-edits".to_string(),
                ));
            }
            _ => {}
        }

        Ok(ResolvedRequestAuth {
            request_url,
            api_key,
            extra_headers,
        })
    }

    fn should_send_tools(messages: &[Message], tools: &[ToolDefinition]) -> bool {
        if tools.is_empty() {
            return false;
        }

        messages.iter().any(|msg| {
            msg.role == "user"
                || msg
                    .tool_calls
                    .as_ref()
                    .map(|calls| !calls.is_empty())
                    .unwrap_or(false)
        })
    }

    fn estimated_tools_tokens(tools: &[ToolDefinition]) -> usize {
        let serialized = serde_json::to_string(tools).unwrap_or_default();
        (serialized.chars().count() / 4).max(1)
    }

    fn should_trim_tools(messages: &[Message], tools: &[ToolDefinition]) -> bool {
        if !Self::should_send_tools(messages, tools) {
            return false;
        }

        Self::estimated_tools_tokens(tools) > MAX_TOOL_SCHEMA_TOKENS
    }

    fn retry_delay_for_attempt(attempt: u32, error: &OSAgentError) -> Duration {
        if error.is_rate_limited() {
            Duration::from_secs(8 * attempt as u64)
        } else {
            Duration::from_secs(BASE_RETRY_DELAY_SECS * attempt as u64)
        }
    }

    fn trim_message_content(content: &str, max_chars: usize) -> String {
        if content.chars().count() <= max_chars {
            return content.to_string();
        }

        let prefix_len = max_chars.saturating_sub(64);
        let prefix: String = content.chars().take(prefix_len).collect();
        let original_len = content.chars().count();
        format!(
            "{}\n...[truncated {} chars to fit provider context]",
            prefix,
            original_len.saturating_sub(prefix_len)
        )
    }

    fn compress_for_context_limit(
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> (Vec<Message>, bool) {
        let mut compressed = messages.to_vec();
        let mut changed = false;

        if compressed.len() > 12 {
            let keep_head = 3usize.min(compressed.len());
            let keep_tail = 6usize.min(compressed.len().saturating_sub(keep_head));

            if compressed.len() > keep_head + keep_tail {
                let middle = &compressed[keep_head..compressed.len() - keep_tail];
                let summary = middle
                    .iter()
                    .filter_map(|msg| {
                        let content = msg.content.trim();
                        if content.is_empty() {
                            None
                        } else {
                            let preview =
                                Self::trim_message_content(content, 220).replace('\n', " ");
                            Some(format!("- {}: {}", msg.role, preview))
                        }
                    })
                    .take(24)
                    .collect::<Vec<_>>()
                    .join("\n");

                let summary_message = Message::assistant(
                    if summary.is_empty() {
                        "Earlier conversation compressed to fit provider context limits."
                            .to_string()
                    } else {
                        format!(
                            "Earlier conversation compressed to fit provider context limits:\n{}",
                            summary
                        )
                    },
                    None,
                );

                let mut next = Vec::new();
                next.extend_from_slice(&compressed[..keep_head]);
                next.push(summary_message);
                next.extend_from_slice(&compressed[compressed.len() - keep_tail..]);
                compressed = next;
                changed = true;
            }
        }

        if Self::should_trim_tools(&compressed, tools) {
            changed = true;
        }

        (compressed, changed)
    }

    async fn fetch_openrouter_context_window(&self) -> Option<usize> {
        if !self.config.base_url.contains("openrouter.ai") {
            return None;
        }

        let mut req = self
            .client
            .get(format!("{}/models", self.config.base_url))
            .header("Content-Type", "application/json")
            .timeout(Duration::from_secs(30));

        if !self.config.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", self.config.api_key));
        }

        req = req
            .header("HTTP-Referer", "https://osagent.local")
            .header("X-Title", "OSA");

        let response = req.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }

        let response_json: serde_json::Value = response.json().await.ok()?;
        let data = response_json.get("data")?.as_array()?;
        let model_id = self.current_model().await;

        for model in data {
            let id = model.get("id").and_then(|v| v.as_str());
            if id != Some(model_id.as_str()) {
                continue;
            }

            let context = model
                .get("context_length")
                .and_then(|v| v.as_u64())
                .or_else(|| model.get("max_context_length").and_then(|v| v.as_u64()))
                .or_else(|| model.get("context_window").and_then(|v| v.as_u64()));

            if let Some(value) = context {
                return Some(value as usize);
            }
        }

        None
    }

    async fn complete_with_retry(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse> {
        let mut last_error: Option<OSAgentError> = None;
        let mut current_messages = messages.to_vec();
        let mut context_compressed = false;

        for attempt in 1..=MAX_RETRIES {
            match self.do_complete(&current_messages, tools).await {
                Ok(mut response) => {
                    response.retry_count = attempt.saturating_sub(1);
                    response.context_compressed = context_compressed;
                    return Ok(response);
                }
                Err(e) => {
                    if e.is_context_limit() && attempt < MAX_RETRIES {
                        let (compressed_messages, changed) =
                            Self::compress_for_context_limit(&current_messages, tools);
                        if changed {
                            warn!(
                                "Provider request exceeded context (attempt {}/{}). Compressing messages and retrying.",
                                attempt,
                                MAX_RETRIES
                            );
                            current_messages = compressed_messages;
                            context_compressed = true;
                            last_error = Some(e);
                            continue;
                        }
                    }

                    if e.is_retryable() && attempt < MAX_RETRIES {
                        let delay = Self::retry_delay_for_attempt(attempt, &e);
                        warn!(
                            "Provider request failed (attempt {}/{}): {}. Retrying in {}s...",
                            attempt,
                            MAX_RETRIES,
                            e,
                            delay.as_secs()
                        );
                        last_error = Some(e);
                        tokio::time::sleep(delay).await;
                        continue;
                    }

                    return Err(e);
                }
            }
        }

        Err(last_error
            .unwrap_or_else(|| OSAgentError::Provider("Max retries exceeded".to_string())))
    }

    async fn do_complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse> {
        let model = self.current_model().await;
        let mut request_body = serde_json::json!({
            "model": model,
            "messages": self.build_messages(messages),
        });

        let include_tools =
            Self::should_send_tools(messages, tools) && !Self::should_trim_tools(messages, tools);
        if include_tools {
            request_body["tools"] = serde_json::to_value(tools).unwrap();
            request_body["tool_choice"] = serde_json::json!("auto");
        } else if !tools.is_empty() {
            info!(
                "Skipping tool schema in provider request to reduce context size (estimated {} tokens)",
                Self::estimated_tools_tokens(tools)
            );
        }

        tracing::debug!(
            "Request body: {}",
            serde_json::to_string_pretty(&request_body).unwrap_or_default()
        );

        let request_auth = self.resolve_request_auth().await?;
        let api_key = &request_auth.api_key;
        let api_key_preview = if api_key.len() > 10 {
            format!("{}...{}", &api_key[..7], &api_key[api_key.len() - 4..])
        } else if api_key.is_empty() {
            "(empty)".to_string()
        } else {
            "(too short)".to_string()
        };

        info!(
            "Sending request to {} with model {}, API key: {}",
            request_auth.request_url, self.config.model, api_key_preview
        );

        let response = {
            let mut req = self
                .client
                .post(&request_auth.request_url)
                .header("Content-Type", "application/json")
                .timeout(Duration::from_secs(3600));

            if !api_key.is_empty() {
                req = req.header("Authorization", format!("Bearer {}", api_key));
            } else {
                warn!("API key is empty - request will likely fail with 401");
            }

            if self.config.base_url.contains("openrouter.ai") {
                req = req
                    .header("HTTP-Referer", "https://osagent.local")
                    .header("X-Title", "OSA");
            }

            for (key, value) in &request_auth.extra_headers {
                req = req.header(key, value);
            }

            req.json(&request_body).send().await.map_err(|e| {
                error!("HTTP request failed: {}", e);
                OSAgentError::Http(e)
            })?
        };

        if !response.status().is_success() {
            let status = response.status();
            let error_text = response.text().await.unwrap_or_default();
            let error_msg = format!("API request failed ({}): {}", status, error_text);
            error!("{}", error_msg);
            return Err(OSAgentError::Provider(error_msg));
        }

        let response_text = response.text().await.map_err(|e| {
            error!("Failed to read response body: {}", e);
            OSAgentError::Parse(format!("Failed to read response body: {}", e))
        })?;

        let response_json: serde_json::Value =
            serde_json::from_str(&response_text).map_err(|e| {
                error!(
                    "Failed to parse JSON response: {} | Body: {}",
                    e, response_text
                );
                OSAgentError::Parse(format!(
                    "Failed to parse JSON: {} | Body: {}",
                    e, response_text
                ))
            })?;

        tracing::debug!(
            "Raw API response: {}",
            serde_json::to_string_pretty(&response_json).unwrap_or_default()
        );

        if let Some(error) = response_json.get("error") {
            let error_msg = error["message"].as_str().unwrap_or("Unknown error");
            let error_code = error["code"].as_i64().unwrap_or(0);
            let full_error = format!("API error ({}): {}", error_code, error_msg);
            error!("{}", full_error);
            return Err(OSAgentError::Provider(full_error));
        }

        let choice = &response_json["choices"][0];
        let message = &choice["message"];

        let content = message["content"].as_str().map(|s| s.to_string());

        info!(
            "Parsed response - content: {:?}, finish_reason: {}",
            content.as_ref().map(|c| {
                if c.chars().count() > 100 {
                    format!("{}...", c.chars().take(100).collect::<String>())
                } else {
                    c.clone()
                }
            }),
            choice["finish_reason"]
        );

        let tool_calls = message["tool_calls"].as_array().map(|calls| {
            calls
                .iter()
                .filter_map(|call| {
                    let id = call["id"].as_str()?.to_string();
                    let name = call["function"]["name"].as_str()?.to_string();
                    let arguments = call["function"]["arguments"]
                        .as_str()
                        .and_then(|s| serde_json::from_str(s).ok())
                        .unwrap_or(serde_json::json!({}));

                    Some(ToolCall {
                        id,
                        name,
                        arguments,
                    })
                })
                .collect()
        });

        let finish_reason = choice["finish_reason"]
            .as_str()
            .unwrap_or("stop")
            .to_string();

        let usage = response_json.get("usage").map(|usage_obj| TokenUsage {
            input: usage_obj["prompt_tokens"].as_u64().unwrap_or(0) as usize,
            output: usage_obj["completion_tokens"].as_u64().unwrap_or(0) as usize,
            total: usage_obj["total_tokens"].as_u64().unwrap_or(0) as usize,
            cached_read: usage_obj
                .get("cached_tokens")
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            cached_write: None,
            reasoning: usage_obj
                .get("completion_tokens_details")
                .and_then(|v| v.get("reasoning_tokens"))
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
        });

        Ok(ProviderResponse {
            content,
            tool_calls,
            finish_reason,
            retry_count: 0,
            context_compressed: false,
            usage,
        })
    }
}

#[async_trait]
impl Provider for OpenAICompatibleProvider {
    async fn complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse> {
        self.complete_with_retry(messages, tools).await
    }

    async fn complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent>>> {
        let request_auth = self.resolve_request_auth().await?;
        let request_body = serde_json::json!({
            "model": self.config.model,
            "messages": self.build_messages(messages),
            "tools": tools,
            "tool_choice": "auto",
            "stream": true,
        });

        let mut req = self
            .client
            .post(&request_auth.request_url)
            .header("Content-Type", "application/json")
            .json(&request_body);

        if !request_auth.api_key.is_empty() {
            req = req.header(
                "Authorization",
                format!("Bearer {}", request_auth.api_key.as_str()),
            );
        }

        for (key, value) in &request_auth.extra_headers {
            req = req.header(key, value);
        }

        let event_source = req
            .eventsource()
            .map_err(|e| OSAgentError::Parse(e.to_string()))?;

        let stream = event_source
            .take_while(|event| {
                futures::future::ready(match event {
                    Ok(Event::Message(_)) => true,
                    _ => false,
                })
            })
            .filter_map(|event| async move {
                match event {
                    Ok(Event::Message(message)) => {
                        if message.data == "[DONE]" {
                            return Some(Ok(StreamEvent {
                                event_type: "done".to_string(),
                                content: None,
                                tool_call: None,
                                done: true,
                            }));
                        }

                        let parsed: serde_json::Value = serde_json::from_str(&message.data).ok()?;
                        let delta = &parsed["choices"][0]["delta"];

                        let content = delta["content"].as_str().map(|s| s.to_string());

                        let tool_call = if let Some(calls) = delta["tool_calls"].as_array() {
                            calls.first().and_then(|call| {
                                let id = call["id"].as_str()?.to_string();
                                let name = call["function"]["name"].as_str()?.to_string();
                                let arguments = call["function"]["arguments"]
                                    .as_str()
                                    .and_then(|s| serde_json::from_str(s).ok())
                                    .unwrap_or(serde_json::json!({}));

                                Some(ToolCall {
                                    id,
                                    name,
                                    arguments,
                                })
                            })
                        } else {
                            None
                        };

                        Some(Ok(StreamEvent {
                            event_type: "token".to_string(),
                            content,
                            tool_call,
                            done: false,
                        }))
                    }
                    _ => None,
                }
            })
            .boxed();

        Ok(stream)
    }

    async fn model_context_window(&self) -> Option<usize> {
        if let Some(value) = *self.context_window.read().await {
            return Some(value);
        }

        if let Some(ref catalog) = self.catalog {
            let model = self.current_model().await;
            let provider_type = &self.config.provider_type;
            if let Some(window) = catalog.lookup_context_window(provider_type, &model) {
                let mut cached = self.context_window.write().await;
                *cached = Some(window);
                return Some(window);
            }
        }

        if *self.context_window_attempted.read().await {
            return None;
        }

        {
            let mut attempted = self.context_window_attempted.write().await;
            *attempted = true;
        }

        let fetched = self.fetch_openrouter_context_window().await;
        if let Some(value) = fetched {
            let mut cached = self.context_window.write().await;
            *cached = Some(value);
            return Some(value);
        }

        None
    }

    async fn current_model(&self) -> String {
        if let Some(model) = self.model_override.read().await.clone() {
            return model;
        }
        self.config.model.clone()
    }

    async fn set_model(&self, model: String) {
        let mut override_model = self.model_override.write().await;
        *override_model = Some(model);

        let mut cached = self.context_window.write().await;
        *cached = None;

        let mut attempted = self.context_window_attempted.write().await;
        *attempted = false;
    }

    fn provider_type(&self) -> &str {
        &self.config.provider_type
    }
}
