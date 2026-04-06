use crate::config::{AgentConfig, ProviderConfig};
use crate::error::{OSAgentError, Result};
use crate::oauth::{extract_account_id, OAuthStorage, OAuthTokenEntry};
use crate::storage::models::{Message, ToolCall};
use async_trait::async_trait;
use futures::StreamExt;
use reqwest_eventsource::{Event, RequestBuilderExt};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::RwLock;
use tracing::{error, info, warn};

use super::model_catalog::ModelCatalog;
use super::provider_adapter::{create_provider_adapter, ProviderAdapter, RequestMode};
use super::provider_auth::{get_extra_headers, resolve_provider_config};
use super::reasoning;

const MAX_RETRIES: u32 = 4;
const BASE_RETRY_DELAY_SECS: u64 = 2;
const MAX_TOOL_SCHEMA_TOKENS: usize = 8_000;
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
    #[serde(default)]
    pub thinking: Option<String>,
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
pub struct StreamToolCallDelta {
    pub index: usize,
    #[serde(default)]
    pub id: Option<String>,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub arguments: Option<String>,
}

#[allow(dead_code)]
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StreamEvent {
    #[serde(rename = "type")]
    pub event_type: String,
    pub content: Option<String>,
    #[serde(default)]
    pub thinking: Option<String>,
    #[serde(default)]
    pub tool_call_deltas: Vec<StreamToolCallDelta>,
    #[serde(default)]
    pub finish_reason: Option<String>,
    #[serde(default)]
    pub usage: Option<TokenUsage>,
    pub done: bool,
}

struct ResolvedRequestAuth {
    request_url: String,
    api_key: String,
    extra_headers: Vec<(String, String)>,
    request_mode: RequestMode,
}

pub struct OpenAICompatibleProvider {
    client: reqwest::Client,
    config: ProviderConfig,
    adapter: Arc<dyn ProviderAdapter>,
    agent_settings: Arc<RwLock<AgentConfig>>,
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
        Self::with_catalog_oauth_and_agent_settings(
            config,
            catalog,
            oauth_storage,
            Arc::new(RwLock::new(AgentConfig::default())),
        )
    }

    pub fn with_catalog_oauth_and_agent_settings(
        config: ProviderConfig,
        catalog: Option<Arc<ModelCatalog>>,
        oauth_storage: Option<OAuthStorage>,
        agent_settings: Arc<RwLock<AgentConfig>>,
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
            adapter: create_provider_adapter(&config.provider_type),
            config,
            agent_settings,
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

    fn build_responses_input(&self, messages: &[Message]) -> Vec<serde_json::Value> {
        let mut input = Vec::new();

        for msg in messages {
            match msg.role.as_str() {
                "system" | "user" | "assistant" => {
                    if !msg.content.is_empty() {
                        input.push(serde_json::json!({
                            "role": msg.role,
                            "content": msg.content,
                        }));
                    }

                    if msg.role == "assistant" {
                        if let Some(tool_calls) = &msg.tool_calls {
                            for tool_call in tool_calls {
                                input.push(serde_json::json!({
                                    "type": "function_call",
                                    "call_id": tool_call.id,
                                    "name": tool_call.name,
                                    "arguments": serde_json::to_string(&tool_call.arguments).unwrap_or_default(),
                                }));
                            }
                        }
                    }
                }
                "tool" => {
                    if let Some(call_id) = &msg.tool_call_id {
                        input.push(serde_json::json!({
                            "type": "function_call_output",
                            "call_id": call_id,
                            "output": msg.content,
                        }));
                    }
                }
                _ => {
                    if !msg.content.is_empty() {
                        input.push(serde_json::json!({
                            "role": msg.role,
                            "content": msg.content,
                        }));
                    }
                }
            }
        }

        input
    }

    fn responses_instructions(messages: &[Message]) -> Option<String> {
        let joined = messages
            .iter()
            .filter(|msg| msg.role == "system")
            .map(|msg| msg.content.trim())
            .filter(|content| !content.is_empty())
            .collect::<Vec<_>>()
            .join("\n\n");

        if joined.is_empty() {
            None
        } else {
            Some(joined)
        }
    }

    fn non_system_messages(messages: &[Message]) -> Vec<Message> {
        messages
            .iter()
            .filter(|msg| msg.role != "system")
            .cloned()
            .collect()
    }

    async fn generation_settings(&self) -> AgentConfig {
        self.agent_settings.read().await.clone()
    }

    fn thinking_budget_for_level(level: &str) -> usize {
        match level {
            "none" => 0,
            "minimal" | "low" => 4_000,
            "medium" => 16_000,
            "high" => 32_000,
            "max" | "xhigh" => 48_000,
            _ => 16_000,
        }
    }

    fn apply_generation_controls(
        request_body: &mut serde_json::Value,
        mode: RequestMode,
        provider_type: &str,
        model: &str,
        settings: &AgentConfig,
        meta: Option<&crate::agent::model_catalog::ModelReasoningMetadata>,
    ) {
        if settings.max_tokens > 0 {
            match mode {
                RequestMode::Responses => {
                    request_body["max_output_tokens"] = serde_json::json!(settings.max_tokens);
                }
                _ => {
                    request_body["max_tokens"] = serde_json::json!(settings.max_tokens);
                }
            }
        }

        if settings.temperature.is_finite() {
            match mode {
                RequestMode::Responses => {
                    if provider_type != "openai" {
                        request_body["temperature"] = serde_json::json!(settings.temperature);
                    }
                }
                _ => {
                    request_body["temperature"] = serde_json::json!(settings.temperature);
                }
            }
        }

        let selected =
            reasoning::normalize_selection(&settings.thinking_level, provider_type, model, meta);
        let Some(thinking_level) = selected.as_deref() else {
            return;
        };
        match provider_type {
            "openai" if mode == RequestMode::Responses => {
                request_body["reasoning"] = serde_json::json!({
                    "effort": thinking_level
                });
            }
            "anthropic" => {
                if thinking_level == "none" {
                    request_body["thinking"] = serde_json::json!({
                        "type": "disabled"
                    });
                } else {
                    request_body["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": Self::thinking_budget_for_level(thinking_level)
                    });
                }
            }
            "google" | "google-vertex" => {
                if model.to_ascii_lowercase().contains("2.5") {
                    request_body["thinkingConfig"] = serde_json::json!({
                        "includeThoughts": thinking_level != "none",
                        "thinkingBudget": Self::thinking_budget_for_level(thinking_level)
                    });
                } else if thinking_level == "none" {
                    request_body["thinkingConfig"] = serde_json::json!({
                        "includeThoughts": false
                    });
                } else {
                    request_body["thinkingConfig"] = serde_json::json!({
                        "includeThoughts": true,
                        "thinkingLevel": thinking_level
                    });
                }
            }
            "groq" => {
                request_body["reasoning_effort"] = serde_json::json!(thinking_level);
            }
            "openrouter" => {
                request_body["reasoning"] = if thinking_level == "none" {
                    serde_json::json!({ "enabled": false })
                } else {
                    serde_json::json!({ "effort": thinking_level })
                };
            }
            "xai" => {
                request_body["reasoningEffort"] = serde_json::json!(thinking_level);
            }
            _ => {}
        }
    }

    fn transform_tools_for_request(
        &self,
        tools: &[ToolDefinition],
        model: &str,
    ) -> Vec<ToolDefinition> {
        tools
            .iter()
            .map(|tool| ToolDefinition {
                tool_type: tool.tool_type.clone(),
                function: ToolFunction {
                    name: tool.function.name.clone(),
                    description: tool.function.description.clone(),
                    parameters: self
                        .adapter
                        .transform_schema(tool.function.parameters.clone(), model),
                },
            })
            .collect::<Vec<_>>()
    }

    fn parse_response_content(response_json: &serde_json::Value) -> Option<String> {
        if let Some(output_text) = response_json.get("output_text").and_then(|v| v.as_str()) {
            if !output_text.is_empty() {
                return Some(output_text.to_string());
            }
        }

        let mut chunks = Vec::new();
        if let Some(output_items) = response_json.get("output").and_then(|v| v.as_array()) {
            for item in output_items {
                if item.get("type").and_then(|v| v.as_str()) == Some("message") {
                    if let Some(content_items) = item.get("content").and_then(|v| v.as_array()) {
                        for content in content_items {
                            if let Some(text) = content.get("text").and_then(|v| v.as_str()) {
                                if !text.is_empty() {
                                    chunks.push(text.to_string());
                                }
                            }
                        }
                    }
                }
            }
        }

        if chunks.is_empty() {
            None
        } else {
            Some(chunks.join("\n"))
        }
    }

    fn join_non_empty(parts: Vec<String>) -> Option<String> {
        let joined = parts
            .into_iter()
            .filter(|part| !part.trim().is_empty())
            .collect::<Vec<_>>()
            .join("\n");

        if joined.trim().is_empty() {
            None
        } else {
            Some(joined)
        }
    }

    fn collect_block_texts(
        content: &serde_json::Value,
        block_types: &[&str],
        fields: &[&str],
    ) -> Vec<String> {
        content
            .as_array()
            .into_iter()
            .flatten()
            .filter(|item| {
                item.get("type")
                    .and_then(|v| v.as_str())
                    .map(|value| block_types.contains(&value))
                    .unwrap_or(false)
            })
            .flat_map(|item| {
                fields.iter().filter_map(|field| {
                    item.get(*field)
                        .and_then(|v| v.as_str())
                        .map(|value| value.to_string())
                })
            })
            .collect()
    }

    fn parse_chat_message_content(message: &serde_json::Value) -> Option<String> {
        if let Some(content) = message.get("content") {
            if content.is_string() {
                return content.as_str().map(|value| value.to_string());
            }

            let text_blocks =
                Self::collect_block_texts(content, &["text", "output_text"], &["text"]);
            if let Some(text) = Self::join_non_empty(text_blocks) {
                return Some(text);
            }
        }

        None
    }

    fn parse_chat_message_thinking(message: &serde_json::Value) -> Option<String> {
        if let Some(content) = message.get("content") {
            let thinking_blocks = Self::collect_block_texts(
                content,
                &[
                    "thinking",
                    "reasoning",
                    "reasoning_content",
                    "reasoning_summary",
                ],
                &["thinking", "text", "summary"],
            );
            if let Some(thinking) = Self::join_non_empty(thinking_blocks) {
                return Some(thinking);
            }
        }

        message
            .get("reasoning_content")
            .and_then(|v| v.as_str())
            .or_else(|| message.get("thinking").and_then(|v| v.as_str()))
            .or_else(|| message.get("reasoning").and_then(|v| v.as_str()))
            .map(|value| value.to_string())
    }

    fn parse_response_thinking(response_json: &serde_json::Value) -> Option<String> {
        let mut parts = Vec::new();

        if let Some(reasoning) = response_json.get("reasoning") {
            if let Some(text) = reasoning.get("text").and_then(|v| v.as_str()) {
                parts.push(text.to_string());
            }

            if let Some(summary) = reasoning.get("summary").and_then(|v| v.as_array()) {
                parts.extend(summary.iter().filter_map(|item| {
                    item.get("text")
                        .and_then(|v| v.as_str())
                        .or_else(|| item.as_str())
                        .map(|value| value.to_string())
                }));
            }
        }

        if let Some(output_items) = response_json.get("output").and_then(|v| v.as_array()) {
            for item in output_items {
                match item.get("type").and_then(|v| v.as_str()) {
                    Some("reasoning") | Some("reasoning_summary") => {
                        if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                            parts.push(text.to_string());
                        }
                        if let Some(summary) = item.get("summary").and_then(|v| v.as_array()) {
                            parts.extend(summary.iter().filter_map(|entry| {
                                entry
                                    .get("text")
                                    .and_then(|v| v.as_str())
                                    .or_else(|| entry.as_str())
                                    .map(|value| value.to_string())
                            }));
                        }
                    }
                    Some("message") => {
                        if let Some(content_items) = item.get("content") {
                            parts.extend(Self::collect_block_texts(
                                content_items,
                                &["reasoning", "reasoning_summary", "thinking"],
                                &["text", "thinking", "summary"],
                            ));
                        }
                    }
                    _ => {}
                }
            }
        }

        if let Some(text) = response_json
            .get("reasoning_content")
            .and_then(|v| v.as_str())
        {
            parts.push(text.to_string());
        }
        if let Some(text) = response_json.get("thinking").and_then(|v| v.as_str()) {
            parts.push(text.to_string());
        }

        Self::join_non_empty(parts)
    }

    fn parse_stream_content(value: &serde_json::Value) -> Option<String> {
        if let Some(text) = value.as_str() {
            return Some(text.to_string());
        }

        Self::join_non_empty(Self::collect_block_texts(
            value,
            &["text", "output_text"],
            &["text"],
        ))
    }

    fn parse_stream_thinking(value: &serde_json::Value) -> Option<String> {
        if let Some(text) = value.as_str() {
            return Some(text.to_string());
        }

        if let Some(items) = value.as_array() {
            let mut parts = Vec::new();
            for item in items {
                if let Some(text) = item.get("text").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                    continue;
                }
                if let Some(text) = item.get("content").and_then(|v| v.as_str()) {
                    parts.push(text.to_string());
                    continue;
                }
                if let Some(text) = item.as_str() {
                    parts.push(text.to_string());
                }
            }
            if let Some(joined) = Self::join_non_empty(parts) {
                return Some(joined);
            }
        }

        Self::join_non_empty(Self::collect_block_texts(
            value,
            &[
                "thinking",
                "reasoning",
                "reasoning_content",
                "reasoning_summary",
            ],
            &["thinking", "text", "summary"],
        ))
    }

    fn parse_response_tool_calls(response_json: &serde_json::Value) -> Option<Vec<ToolCall>> {
        let mut calls = Vec::new();
        if let Some(output_items) = response_json.get("output").and_then(|v| v.as_array()) {
            for item in output_items {
                if item.get("type").and_then(|v| v.as_str()) != Some("function_call") {
                    continue;
                }

                let Some(id) = item.get("call_id").and_then(|v| v.as_str()) else {
                    continue;
                };
                let Some(name) = item.get("name").and_then(|v| v.as_str()) else {
                    continue;
                };

                let arguments = item
                    .get("arguments")
                    .and_then(|v| v.as_str())
                    .and_then(|s| serde_json::from_str(s).ok())
                    .unwrap_or(serde_json::json!({}));

                calls.push(ToolCall {
                    id: id.to_string(),
                    name: name.to_string(),
                    arguments,
                });
            }
        }

        if calls.is_empty() {
            None
        } else {
            Some(calls)
        }
    }

    fn parse_response_usage(response_json: &serde_json::Value) -> Option<TokenUsage> {
        let usage = response_json.get("usage")?;

        let input = usage
            .get("input_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| usage.get("prompt_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0) as usize;
        let output = usage
            .get("output_tokens")
            .and_then(|v| v.as_u64())
            .or_else(|| usage.get("completion_tokens").and_then(|v| v.as_u64()))
            .unwrap_or(0) as usize;
        let total = usage
            .get("total_tokens")
            .and_then(|v| v.as_u64())
            .unwrap_or((input + output) as u64) as usize;

        Some(TokenUsage {
            input,
            output,
            total,
            cached_read: usage
                .get("input_tokens_details")
                .and_then(|v| v.get("cached_tokens"))
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
            cached_write: None,
            reasoning: usage
                .get("output_tokens_details")
                .and_then(|v| v.get("reasoning_tokens"))
                .and_then(|v| v.as_u64())
                .map(|v| v as usize),
        })
    }

    fn parse_chat_completions_response(response_json: &serde_json::Value) -> ProviderResponse {
        let choice = &response_json["choices"][0];
        let message = &choice["message"];

        let content = Self::parse_chat_message_content(message);
        let thinking = Self::parse_chat_message_thinking(message);

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

        ProviderResponse {
            content,
            thinking,
            tool_calls,
            finish_reason,
            retry_count: 0,
            context_compressed: false,
            usage,
        }
    }

    fn parse_responses_response(response_json: &serde_json::Value) -> ProviderResponse {
        ProviderResponse {
            content: Self::parse_response_content(response_json),
            thinking: Self::parse_response_thinking(response_json),
            tool_calls: Self::parse_response_tool_calls(response_json),
            finish_reason: response_json
                .get("status")
                .and_then(|v| v.as_str())
                .unwrap_or("completed")
                .to_string(),
            retry_count: 0,
            context_compressed: false,
            usage: Self::parse_response_usage(response_json),
        }
    }

    async fn send_responses_request(
        &self,
        request_auth: &ResolvedRequestAuth,
        request_body: &serde_json::Value,
        config: &ProviderConfig,
    ) -> Result<ProviderResponse> {
        let mut req = self
            .client
            .post(&request_auth.request_url)
            .header("Content-Type", "application/json")
            .timeout(Duration::from_secs(3600))
            .json(request_body);

        if !request_auth.api_key.is_empty() {
            req = req.header(
                "Authorization",
                format!("Bearer {}", request_auth.api_key.as_str()),
            );
        } else {
            warn!("API key is empty - request will likely fail with 401");
        }

        if config.base_url.contains("openrouter.ai") {
            req = req
                .header("HTTP-Referer", "https://osagent.local")
                .header("X-Title", "OSA");
        }

        for (key, value) in &request_auth.extra_headers {
            req = req.header(key, value);
        }

        let response = req
            .send()
            .await
            .map_err(|e| OSAgentError::Provider(format!("Responses request failed: {}", e)))?;

        let status = response.status();
        let body = response
            .text()
            .await
            .map_err(|e| OSAgentError::Provider(format!("Failed to read responses body: {}", e)))?;

        if !status.is_success() {
            let detail = serde_json::from_str::<serde_json::Value>(&body)
                .ok()
                .and_then(|json| {
                    json.get("detail")
                        .and_then(|v| v.as_str())
                        .map(|s| s.to_string())
                        .or_else(|| {
                            json.get("error")
                                .and_then(|v| v.get("message"))
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                        .or_else(|| {
                            json.get("message")
                                .and_then(|v| v.as_str())
                                .map(|s| s.to_string())
                        })
                })
                .unwrap_or_else(|| body.clone());

            return Err(OSAgentError::Provider(format!(
                "API request failed ({}): {}",
                status, detail
            )));
        }

        let mut content = String::new();
        let mut thinking = String::new();
        let mut calls: HashMap<String, (String, String)> = HashMap::new();
        let mut final_response: Option<serde_json::Value> = None;
        let mut final_status: Option<String> = None;
        let mut usage: Option<TokenUsage> = None;

        for chunk in body.split("\n\n") {
            let trimmed = chunk.trim();
            if trimmed.is_empty() {
                continue;
            }

            let mut event_name = String::new();
            let mut data_lines = Vec::new();

            for line in trimmed.lines() {
                if let Some(rest) = line.strip_prefix("event:") {
                    event_name = rest.trim().to_string();
                } else if let Some(rest) = line.strip_prefix("data:") {
                    data_lines.push(rest.trim_start().to_string());
                }
            }

            let data = data_lines.join("\n");
            if data == "[DONE]" || data.is_empty() {
                continue;
            }

            let parsed: serde_json::Value = serde_json::from_str(&data).map_err(|e| {
                OSAgentError::Parse(format!(
                    "Failed to parse responses stream JSON: {} | Body: {}",
                    e, data
                ))
            })?;

            if event_name.is_empty() {
                event_name = parsed
                    .get("type")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
            }

            match event_name.as_str() {
                "error" => {
                    let msg = parsed
                        .get("error")
                        .and_then(|v| v.get("message"))
                        .and_then(|v| v.as_str())
                        .or_else(|| parsed.get("message").and_then(|v| v.as_str()))
                        .unwrap_or("Unknown responses stream error");
                    return Err(OSAgentError::Provider(msg.to_string()));
                }
                "response.output_text.delta" => {
                    if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
                        content.push_str(delta);
                    }
                }
                "response.reasoning_summary_text.delta"
                | "response.reasoning_text.delta"
                | "response.reasoning.delta" => {
                    if let Some(delta) = parsed
                        .get("delta")
                        .and_then(|v| v.as_str())
                        .or_else(|| parsed.get("text").and_then(|v| v.as_str()))
                    {
                        thinking.push_str(delta);
                    }
                }
                "response.function_call_arguments.delta" => {
                    if let Some(item_id) = parsed.get("item_id").and_then(|v| v.as_str()) {
                        let entry = calls
                            .entry(item_id.to_string())
                            .or_insert_with(|| (String::new(), String::new()));
                        if let Some(delta) = parsed.get("delta").and_then(|v| v.as_str()) {
                            entry.1.push_str(delta);
                        }
                    }
                }
                "response.output_item.added" | "response.output_item.done" => {
                    if let Some(item) = parsed.get("item") {
                        if item.get("type").and_then(|v| v.as_str()) == Some("function_call") {
                            let id = item
                                .get("call_id")
                                .or_else(|| item.get("id"))
                                .and_then(|v| v.as_str())
                                .unwrap_or("")
                                .to_string();
                            if !id.is_empty() {
                                let entry = calls
                                    .entry(id)
                                    .or_insert_with(|| (String::new(), String::new()));
                                if let Some(name) = item.get("name").and_then(|v| v.as_str()) {
                                    entry.0 = name.to_string();
                                }
                                if let Some(arguments) =
                                    item.get("arguments").and_then(|v| v.as_str())
                                {
                                    entry.1 = arguments.to_string();
                                }
                            }
                        }
                    }
                }
                "response.completed" => {
                    if let Some(response) = parsed.get("response") {
                        usage = Self::parse_response_usage(response);
                        final_status = response
                            .get("status")
                            .and_then(|v| v.as_str())
                            .map(|s| s.to_string());
                        final_response = Some(response.clone());
                    }
                }
                _ => {}
            }
        }

        if let Some(response_json) = final_response {
            return Ok(Self::parse_responses_response(&response_json));
        }

        let tool_calls = if calls.is_empty() {
            None
        } else {
            Some(
                calls
                    .into_iter()
                    .map(|(id, (name, arguments))| ToolCall {
                        id,
                        name,
                        arguments: serde_json::from_str(&arguments)
                            .unwrap_or(serde_json::json!({})),
                    })
                    .collect(),
            )
        };

        Ok(ProviderResponse {
            content: if content.is_empty() {
                None
            } else {
                Some(content)
            },
            thinking: if thinking.is_empty() {
                None
            } else {
                Some(thinking)
            },
            tool_calls,
            finish_reason: final_status.unwrap_or_else(|| "completed".to_string()),
            retry_count: 0,
            context_compressed: false,
            usage,
        })
    }
}

impl OpenAICompatibleProvider {
    fn resolved_config(&self) -> ProviderConfig {
        resolve_provider_config(self.config.clone())
    }

    fn oauth_enabled(&self) -> bool {
        self.config.auth_type.as_deref() == Some("oauth")
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
                status, body
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

    async fn resolve_request_auth(&self, model: &str) -> Result<ResolvedRequestAuth> {
        let config = self.resolved_config();
        let oauth_entry = if self.oauth_enabled() {
            self.resolve_oauth_entry().await?
        } else {
            None
        };

        let request_mode = self
            .adapter
            .request_mode(&config, oauth_entry.as_ref(), model);

        let request_url = self.adapter.resolve_endpoint(&config, oauth_entry.as_ref());
        let mut api_key = config.api_key.clone();
        let mut extra_headers = Vec::new();

        if let Some(headers) = &config.custom_headers {
            for (key, value) in headers {
                extra_headers.push((key.clone(), value.clone()));
            }
        }

        if let Some(entry) = oauth_entry.as_ref() {
            api_key = entry.access_token.clone();
        }

        extra_headers.extend(get_extra_headers(&config.provider_type, &config));
        extra_headers.extend(self.adapter.extra_headers(&config, oauth_entry.as_ref()));

        Ok(ResolvedRequestAuth {
            request_url,
            api_key,
            extra_headers,
            request_mode,
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
        let config = self.resolved_config();
        if !config.base_url.contains("openrouter.ai") {
            return None;
        }

        let mut req = self
            .client
            .get(format!("{}/models", config.base_url))
            .header("Content-Type", "application/json")
            .timeout(Duration::from_secs(30));

        if !config.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", config.api_key));
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

    fn extract_ollama_context_window(payload: &serde_json::Value) -> Option<usize> {
        if let Some(model_info) = payload
            .get("model_info")
            .and_then(|value| value.as_object())
        {
            for (key, value) in model_info {
                if (key.ends_with(".context_length") || key.ends_with(".context_window"))
                    && value.as_u64().unwrap_or(0) > 0
                {
                    return value.as_u64().map(|raw| raw as usize);
                }
            }

            for key in ["context_length", "context_window", "num_ctx"] {
                if let Some(raw) = model_info.get(key).and_then(|value| value.as_u64()) {
                    if raw > 0 {
                        return Some(raw as usize);
                    }
                }
            }
        }

        if let Some(parameters) = payload.get("parameters").and_then(|value| value.as_str()) {
            for line in parameters.lines() {
                let normalized = line.trim().replace('\t', " ");
                let mut parts = normalized.split_whitespace();
                let key = parts.next().unwrap_or_default().to_ascii_lowercase();
                if key == "num_ctx" || key == "context_length" {
                    if let Some(raw) = parts.next().and_then(|value| value.parse::<usize>().ok()) {
                        if raw > 0 {
                            return Some(raw);
                        }
                    }
                }
            }
        }

        None
    }

    fn normalize_ollama_base_url(base_url: &str) -> String {
        let mut normalized = base_url.trim().trim_end_matches('/').to_string();
        if normalized.is_empty() {
            return "http://localhost:11434".to_string();
        }

        let lower = normalized.to_ascii_lowercase();
        if lower.ends_with("/v1/models") {
            normalized.truncate(normalized.len().saturating_sub(10));
        } else if lower.ends_with("/api/tags") {
            normalized.truncate(normalized.len().saturating_sub(9));
        } else if lower.ends_with("/v1") {
            normalized.truncate(normalized.len().saturating_sub(3));
        } else if lower.ends_with("/api") {
            normalized.truncate(normalized.len().saturating_sub(4));
        }

        normalized = normalized.trim_end_matches('/').to_string();
        if normalized.is_empty() {
            "http://localhost:11434".to_string()
        } else {
            normalized
        }
    }

    async fn fetch_ollama_context_window(&self) -> Option<usize> {
        let config = self.resolved_config();
        if config.provider_type != "ollama" {
            return None;
        }

        let model = self.current_model().await;
        if model.trim().is_empty() {
            return None;
        }

        let base_url = Self::normalize_ollama_base_url(&config.base_url);
        let mut req = self
            .client
            .post(format!("{}/api/show", base_url.trim_end_matches('/')))
            .header("Content-Type", "application/json")
            .json(&serde_json::json!({ "name": model }))
            .timeout(Duration::from_secs(15));

        if !config.api_key.is_empty() {
            req = req.header("Authorization", format!("Bearer {}", config.api_key));
        }

        let response = req.send().await.ok()?;
        if !response.status().is_success() {
            return None;
        }

        let payload: serde_json::Value = response.json().await.ok()?;
        Self::extract_ollama_context_window(&payload)
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

    async fn complete_stream_with_retry(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent>>> {
        let mut last_error: Option<OSAgentError> = None;
        let mut current_messages = messages.to_vec();

        for attempt in 1..=MAX_RETRIES {
            match self.do_complete_stream(&current_messages, tools).await {
                Ok(stream) => return Ok(stream),
                Err(e) => {
                    if e.is_context_limit() && attempt < MAX_RETRIES {
                        let (compressed_messages, changed) =
                            Self::compress_for_context_limit(&current_messages, tools);
                        if changed {
                            warn!(
                                "Provider stream exceeded context (attempt {}/{}). Compressing messages and retrying.",
                                attempt,
                                MAX_RETRIES
                            );
                            current_messages = compressed_messages;
                            last_error = Some(e);
                            continue;
                        }
                    }

                    if e.is_retryable() && attempt < MAX_RETRIES {
                        let delay = Self::retry_delay_for_attempt(attempt, &e);
                        warn!(
                            "Provider stream failed (attempt {}/{}): {}. Retrying in {}s...",
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

    async fn do_complete_stream(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<futures::stream::BoxStream<'static, Result<StreamEvent>>> {
        let model = self.current_model().await;
        let mut config = self.resolved_config();
        config.model = model.clone();
        let request_auth = self.resolve_request_auth(&model).await?;
        let mode = request_auth.request_mode;
        let generation_settings = self.generation_settings().await;
        let reasoning_meta = self.catalog.as_ref().and_then(|catalog| {
            catalog.lookup_reasoning_metadata(&config.provider_type, &config.model)
        });
        if mode != RequestMode::ChatCompletions {
            return Err(OSAgentError::Provider(
                "Streaming currently supports only chat/completions mode".to_string(),
            ));
        }

        let transformed_messages = self.adapter.transform_messages(messages, &config);
        let transformed_tools = self.transform_tools_for_request(tools, &config.model);
        let include_tools = Self::should_send_tools(&transformed_messages, tools)
            && !Self::should_trim_tools(&transformed_messages, tools);
        let provider_options = self
            .adapter
            .default_options(&config.provider_type, &config.model);
        let mut request_body = serde_json::json!({
            "model": model,
            "messages": self.build_messages(&transformed_messages),
            "stream": true,
        });
        if let Some(options) = provider_options.as_object() {
            for (key, value) in options {
                request_body[key] = value.clone();
            }
        }
        Self::apply_generation_controls(
            &mut request_body,
            mode,
            &config.provider_type,
            &config.model,
            &generation_settings,
            reasoning_meta.as_ref(),
        );
        if include_tools {
            request_body["tools"] = serde_json::to_value(&transformed_tools).unwrap();
            request_body["tool_choice"] = serde_json::json!("auto");
        } else if !tools.is_empty() {
            info!(
                "Skipping tool schema in streaming provider request to reduce context size (estimated {} tokens)",
                Self::estimated_tools_tokens(tools)
            );
        }

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
            .filter_map(|event| async move {
                match event {
                    Ok(Event::Open) => None,
                    Ok(Event::Message(message)) => {
                        if message.data == "[DONE]" {
                            return Some(Ok(StreamEvent {
                                event_type: "done".to_string(),
                                content: None,
                                thinking: None,
                                tool_call_deltas: Vec::new(),
                                finish_reason: None,
                                usage: None,
                                done: true,
                            }));
                        }

                        let parsed: serde_json::Value = serde_json::from_str(&message.data).ok()?;
                        let choice = &parsed["choices"][0];
                        let delta = &choice["delta"];
                        let final_message = &choice["message"];

                        let content = Self::parse_stream_content(&delta["content"])
                            .or_else(|| Self::parse_chat_message_content(final_message));
                        let thinking = Self::parse_stream_thinking(&delta["reasoning_content"])
                            .or_else(|| Self::parse_stream_thinking(&delta["reasoning_details"]))
                            .or_else(|| Self::parse_stream_thinking(&delta["thinking"]))
                            .or_else(|| Self::parse_stream_thinking(&delta["reasoning"]))
                            .or_else(|| Self::parse_chat_message_thinking(final_message));
                        let tool_call_deltas = delta["tool_calls"]
                            .as_array()
                            .map(|calls| {
                                calls
                                    .iter()
                                    .map(|call| StreamToolCallDelta {
                                        index: call["index"].as_u64().unwrap_or(0) as usize,
                                        id: call["id"].as_str().map(|s| s.to_string()),
                                        name: call["function"]["name"]
                                            .as_str()
                                            .map(|s| s.to_string()),
                                        arguments: call["function"]["arguments"]
                                            .as_str()
                                            .map(|s| s.to_string()),
                                    })
                                    .collect::<Vec<_>>()
                            })
                            .unwrap_or_default();
                        let finish_reason = choice["finish_reason"].as_str().map(|s| s.to_string());
                        let usage = Self::parse_response_usage(&parsed);

                        if content.is_none()
                            && thinking.is_none()
                            && tool_call_deltas.is_empty()
                            && finish_reason.is_none()
                            && usage.is_none()
                        {
                            return None;
                        }

                        Some(Ok(StreamEvent {
                            event_type: "token".to_string(),
                            content,
                            thinking,
                            tool_call_deltas,
                            finish_reason,
                            usage,
                            done: false,
                        }))
                    }
                    Err(err) => Some(Err(OSAgentError::Provider(format!(
                        "stream error: {}",
                        err
                    )))),
                }
            })
            .boxed();

        Ok(stream)
    }

    async fn do_complete(
        &self,
        messages: &[Message],
        tools: &[ToolDefinition],
    ) -> Result<ProviderResponse> {
        let model = self.current_model().await;
        let mut config = self.resolved_config();
        config.model = model.clone();
        let transformed_messages = self.adapter.transform_messages(messages, &config);
        let request_auth = self.resolve_request_auth(&model).await?;
        let mode = request_auth.request_mode;
        let generation_settings = self.generation_settings().await;
        let reasoning_meta = self.catalog.as_ref().and_then(|catalog| {
            catalog.lookup_reasoning_metadata(&config.provider_type, &config.model)
        });
        let transformed_tools = self.transform_tools_for_request(tools, &config.model);
        let non_system_messages = Self::non_system_messages(&transformed_messages);

        let mut request_body = match mode {
            RequestMode::ChatCompletions | RequestMode::Custom => serde_json::json!({
                "model": model,
                "messages": self.build_messages(&transformed_messages),
            }),
            RequestMode::Responses => serde_json::json!({
                "model": model,
                "input": self.build_responses_input(&non_system_messages),
            }),
        };
        if mode == RequestMode::Responses {
            if let Some(instructions) = Self::responses_instructions(&transformed_messages) {
                request_body["instructions"] = serde_json::json!(instructions);
            }
        }

        let provider_options = self
            .adapter
            .default_options(&config.provider_type, &config.model);
        if let Some(options) = provider_options.as_object() {
            for (key, value) in options {
                request_body[key] = value.clone();
            }
        }
        Self::apply_generation_controls(
            &mut request_body,
            mode,
            &config.provider_type,
            &config.model,
            &generation_settings,
            reasoning_meta.as_ref(),
        );
        if mode == RequestMode::Responses
            && request_auth
                .request_url
                .contains("chatgpt.com/backend-api/codex")
        {
            if let Some(obj) = request_body.as_object_mut() {
                obj.remove("max_output_tokens");
            }
        }

        let include_tools = Self::should_send_tools(&transformed_messages, tools)
            && !Self::should_trim_tools(&transformed_messages, tools);
        if include_tools {
            let tool_payload = match mode {
                RequestMode::Responses => serde_json::to_value(
                    transformed_tools
                        .iter()
                        .map(|tool| {
                            serde_json::json!({
                                "type": "function",
                                "name": tool.function.name,
                                "description": tool.function.description,
                                "parameters": tool.function.parameters,
                            })
                        })
                        .collect::<Vec<_>>(),
                )
                .unwrap(),
                _ => serde_json::to_value(&transformed_tools).unwrap(),
            };

            request_body["tools"] = tool_payload;
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

        if mode == RequestMode::Responses {
            request_body["stream"] = serde_json::json!(true);
            let parsed = self
                .send_responses_request(&request_auth, &request_body, &config)
                .await?;

            info!(
                "Parsed response - content: {:?}, finish_reason: {}",
                parsed.content.as_ref().map(|c| {
                    if c.chars().count() > 100 {
                        format!("{}...", c.chars().take(100).collect::<String>())
                    } else {
                        c.clone()
                    }
                }),
                parsed.finish_reason
            );

            return Ok(parsed);
        }

        let api_key = &request_auth.api_key;
        let api_key_preview = if api_key.len() > 10 {
            format!("{}...{}", &api_key[..7], &api_key[api_key.len() - 4..])
        } else if api_key.is_empty() {
            "(empty)".to_string()
        } else {
            "(too short)".to_string()
        };

        info!(
            "Sending request to {} with provider {}, model {}, mode {:?}, API key: {}",
            request_auth.request_url,
            self.adapter.provider_type(),
            config.model,
            mode,
            api_key_preview
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

            if config.base_url.contains("openrouter.ai") {
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

        let parsed = Self::parse_chat_completions_response(&response_json);

        info!(
            "Parsed response - content: {:?}, finish_reason: {}",
            parsed.content.as_ref().map(|c| {
                if c.chars().count() > 100 {
                    format!("{}...", c.chars().take(100).collect::<String>())
                } else {
                    c.clone()
                }
            }),
            parsed.finish_reason
        );

        Ok(parsed)
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
        self.complete_stream_with_retry(messages, tools).await
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

        let fetched = if self.config.provider_type == "ollama" {
            self.fetch_ollama_context_window().await
        } else {
            self.fetch_openrouter_context_window().await
        };
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_chat_completions_response() {
        let payload = serde_json::json!({
            "choices": [{
                "finish_reason": "tool_calls",
                "message": {
                    "content": "",
                    "tool_calls": [{
                        "id": "call_1",
                        "function": {
                            "name": "grep",
                            "arguments": "{\"pattern\":\"foo\"}"
                        }
                    }]
                }
            }],
            "usage": {
                "prompt_tokens": 10,
                "completion_tokens": 5,
                "total_tokens": 15
            }
        });

        let parsed = OpenAICompatibleProvider::parse_chat_completions_response(&payload);
        assert_eq!(parsed.finish_reason, "tool_calls");
        assert_eq!(parsed.tool_calls.as_ref().map(|x| x.len()), Some(1));
        assert_eq!(parsed.usage.as_ref().map(|x| x.total), Some(15));
        assert!(parsed.thinking.is_none());
    }

    #[test]
    fn parses_chat_completions_thinking_blocks() {
        let payload = serde_json::json!({
            "choices": [{
                "finish_reason": "stop",
                "message": {
                    "content": [
                        {
                            "type": "thinking",
                            "thinking": "Inspecting the workspace"
                        },
                        {
                            "type": "text",
                            "text": "Done."
                        }
                    ]
                }
            }]
        });

        let parsed = OpenAICompatibleProvider::parse_chat_completions_response(&payload);
        assert_eq!(parsed.content.as_deref(), Some("Done."));
        assert_eq!(parsed.thinking.as_deref(), Some("Inspecting the workspace"));
    }

    #[test]
    fn parses_responses_api_response() {
        let payload = serde_json::json!({
            "status": "completed",
            "output_text": "done",
            "output": [
                {
                    "type": "reasoning",
                    "summary": [
                        { "text": "Planning tool usage" }
                    ]
                },
                {
                    "type": "function_call",
                    "call_id": "call_a",
                    "name": "glob",
                    "arguments": "{\"pattern\":\"**/*.rs\"}"
                }
            ],
            "usage": {
                "input_tokens": 21,
                "output_tokens": 9,
                "total_tokens": 30,
                "output_tokens_details": {
                    "reasoning_tokens": 3
                }
            }
        });

        let parsed = OpenAICompatibleProvider::parse_responses_response(&payload);
        assert_eq!(parsed.finish_reason, "completed");
        assert_eq!(parsed.content.as_deref(), Some("done"));
        assert_eq!(parsed.thinking.as_deref(), Some("Planning tool usage"));
        assert_eq!(parsed.tool_calls.as_ref().map(|x| x.len()), Some(1));
        assert_eq!(parsed.usage.as_ref().map(|x| x.reasoning), Some(Some(3)));
    }

    #[test]
    fn builds_responses_input_with_function_call_output() {
        let provider = OpenAICompatibleProvider::new(ProviderConfig::default()).unwrap();
        let mut assistant = Message::assistant("".to_string(), None);
        assistant.tool_calls = Some(vec![ToolCall {
            id: "call_123".to_string(),
            name: "bash".to_string(),
            arguments: serde_json::json!({ "command": "ls" }),
        }]);

        let tool = Message::tool_result("call_123".to_string(), "ok".to_string());
        let input =
            provider.build_responses_input(&[Message::user("hi".to_string()), assistant, tool]);

        assert!(input.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_123")
        }));
        assert!(input.iter().any(|item| {
            item.get("type").and_then(|v| v.as_str()) == Some("function_call_output")
                && item.get("call_id").and_then(|v| v.as_str()) == Some("call_123")
        }));
    }

    #[test]
    fn extracts_responses_instructions_from_system_messages() {
        let messages = vec![
            Message::system("System A".to_string()),
            Message::user("Hi".to_string()),
            Message::system("System B".to_string()),
        ];

        let instructions = OpenAICompatibleProvider::responses_instructions(&messages);
        assert_eq!(instructions.as_deref(), Some("System A\n\nSystem B"));

        let filtered = OpenAICompatibleProvider::non_system_messages(&messages);
        assert_eq!(filtered.len(), 1);
        assert_eq!(filtered[0].role, "user");
    }

    #[test]
    fn applies_generation_controls_for_openai_responses() {
        let mut request = serde_json::json!({});
        let settings = AgentConfig {
            max_tokens: 2048,
            temperature: 0.3,
            thinking_level: "high".to_string(),
            ..AgentConfig::default()
        };

        OpenAICompatibleProvider::apply_generation_controls(
            &mut request,
            RequestMode::Responses,
            "openai",
            "gpt-5.3-codex",
            &settings,
            None,
        );

        assert_eq!(request["max_output_tokens"], serde_json::json!(2048));
        assert_eq!(request["reasoning"]["effort"], serde_json::json!("high"));
        assert!(request.get("temperature").is_none());
    }

    #[test]
    fn applies_generation_controls_for_anthropic() {
        let mut request = serde_json::json!({});
        let settings = AgentConfig {
            max_tokens: 1024,
            temperature: 0.2,
            thinking_level: "off".to_string(),
            ..AgentConfig::default()
        };

        OpenAICompatibleProvider::apply_generation_controls(
            &mut request,
            RequestMode::ChatCompletions,
            "anthropic",
            "claude-sonnet",
            &settings,
            None,
        );

        assert_eq!(request["max_tokens"], serde_json::json!(1024));
        assert_eq!(request["temperature"].as_f64(), Some(0.20000000298023224));
        assert_eq!(request["thinking"]["type"], serde_json::json!("disabled"));
    }
}
