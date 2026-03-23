use crate::config::ProviderConfig;
use crate::error::{OSAgentError, Result};
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

pub struct OpenAICompatibleProvider {
    client: reqwest::Client,
    config: ProviderConfig,
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

        let api_key = &self.config.api_key;
        let api_key_preview = if api_key.len() > 10 {
            format!("{}...{}", &api_key[..7], &api_key[api_key.len() - 4..])
        } else if api_key.is_empty() {
            "(empty)".to_string()
        } else {
            "(too short)".to_string()
        };

        info!(
            "Sending request to {} with model {}, API key: {}",
            self.config.base_url, self.config.model, api_key_preview
        );

        let response = {
            let mut req = self
                .client
                .post(format!("{}/chat/completions", self.config.base_url))
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

        let tool_calls = if let Some(calls) = message["tool_calls"].as_array() {
            Some(
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
                    .collect(),
            )
        } else {
            None
        };

        let finish_reason = choice["finish_reason"]
            .as_str()
            .unwrap_or("stop")
            .to_string();

        let usage = if let Some(usage_obj) = response_json.get("usage") {
            Some(TokenUsage {
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
            })
        } else {
            None
        };

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
        let request_body = serde_json::json!({
            "model": self.config.model,
            "messages": self.build_messages(messages),
            "tools": tools,
            "tool_choice": "auto",
            "stream": true,
        });

        let event_source = self
            .client
            .post(format!("{}/chat/completions", self.config.base_url))
            .header("Authorization", format!("Bearer {}", self.config.api_key))
            .header("Content-Type", "application/json")
            .json(&request_body)
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
