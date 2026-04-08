use crate::config::ProviderConfig;
use crate::storage::models::Message;

pub struct ProviderTransforms;

impl ProviderTransforms {
    pub fn transform_messages(
        messages: &[Message],
        provider_type: &str,
        model: &str,
    ) -> Vec<Message> {
        let mut result: Vec<Message> = messages.to_vec();

        match provider_type {
            "anthropic" => {
                Self::filter_empty_content_in_place(&mut result);
                Self::normalize_claude_tool_call_ids_in_place(&mut result);
            }
            "openai" => {
                Self::normalize_openai_tool_call_ids_in_place(&mut result);
            }
            "google" | "google-vertex" => {
                Self::filter_empty_content_in_place(&mut result);
                Self::normalize_google_tool_call_ids_in_place(&mut result);
            }
            "mistral" => {
                Self::normalize_mistral_tool_call_ids_in_place(&mut result);
                Self::fix_mistral_message_sequence(&mut result);
            }
            "ollama" | "deepseek" | "groq" | "xai" => {
                Self::filter_empty_content_in_place(&mut result);
            }
            _ => {}
        }

        if model.contains("claude") {
            Self::normalize_claude_tool_call_ids_in_place(&mut result);
        }

        if model.contains("mistral") || model.to_lowercase().contains("mistral") {
            Self::normalize_mistral_tool_call_ids_in_place(&mut result);
        }

        result
    }

    fn filter_empty_content_in_place(messages: &mut Vec<Message>) {
        messages.retain(|msg| {
            if msg.role == "tool" {
                return !msg.content.trim().is_empty();
            }
            if msg.role == "user" || msg.role == "system" {
                return !msg.content.trim().is_empty();
            }
            if msg.role == "assistant" {
                if msg.tool_calls.is_some() {
                    return true;
                }
                return !msg.content.trim().is_empty();
            }
            true
        });
    }

    fn normalize_claude_tool_call_ids_in_place(messages: &mut [Message]) {
        for msg in messages.iter_mut() {
            if (msg.role == "assistant" || msg.role == "tool") && msg.tool_calls.is_some() {
                if let Some(ref mut calls) = msg.tool_calls {
                    for call in calls.iter_mut() {
                        call.id = call
                            .id
                            .chars()
                            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                            .collect();
                        if call.id.is_empty() {
                            call.id = format!(
                                "tool_{}",
                                uuid::Uuid::new_v4()
                                    .to_string()
                                    .replace("-", "")
                                    .chars()
                                    .take(8)
                                    .collect::<String>()
                            );
                        }
                    }
                }
            }
        }
    }

    fn normalize_mistral_tool_call_ids_in_place(messages: &mut [Message]) {
        for msg in messages.iter_mut() {
            if (msg.role == "assistant" || msg.role == "tool") && msg.tool_calls.is_some() {
                if let Some(ref mut calls) = msg.tool_calls {
                    for call in calls.iter_mut() {
                        let normalized: String = call
                            .id
                            .chars()
                            .filter(|c| c.is_alphanumeric())
                            .take(9)
                            .collect();
                        call.id = if normalized.len() < 9 {
                            format!("{}{}", normalized, "0".repeat(9 - normalized.len()))
                        } else {
                            normalized
                        };
                    }
                }
            }
        }
    }

    fn normalize_openai_tool_call_ids_in_place(messages: &mut [Message]) {
        for msg in messages.iter_mut() {
            if (msg.role == "assistant" || msg.role == "tool") && msg.tool_calls.is_some() {
                if let Some(ref mut calls) = msg.tool_calls {
                    for call in calls.iter_mut() {
                        call.id = call
                            .id
                            .chars()
                            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                            .collect();
                    }
                }
            }
        }
    }

    fn normalize_google_tool_call_ids_in_place(messages: &mut [Message]) {
        for msg in messages.iter_mut() {
            if (msg.role == "assistant" || msg.role == "tool") && msg.tool_calls.is_some() {
                if let Some(ref mut calls) = msg.tool_calls {
                    for call in calls.iter_mut() {
                        call.id = call
                            .id
                            .chars()
                            .filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-')
                            .collect();
                    }
                }
            }
        }
    }

    pub fn fix_mistral_message_sequence(messages: &mut [Message]) {
        let mut i = 0;
        while i < messages.len().saturating_sub(1) {
            if messages[i].role == "tool" && messages[i + 1].role == "user" {
                messages[i + 1].role = "assistant".to_string();
                if messages[i + 1].content.is_empty() {
                    messages[i + 1].content = "Done.".to_string();
                }
            }
            i += 1;
        }
    }

    pub fn get_provider_headers(
        provider_type: &str,
        base_url: &str,
    ) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();

        if base_url.contains("openrouter.ai") {
            headers.push(("HTTP-Referer", "https://osagent.local".to_string()));
            headers.push(("X-Title", "OSA".to_string()));
        }

        if provider_type == "anthropic" {
            headers.push((
                "anthropic-beta",
                "interleaved-thinking-2025-05-14".to_string(),
            ));
        }

        headers
    }

    pub fn get_provider_specific_options(provider_type: &str, model: &str) -> serde_json::Value {
        let model_lower = model.to_lowercase();

        match provider_type {
            "anthropic" => {
                let mut opts = serde_json::json!({});
                if model_lower.contains("claude-sonnet-4") || model_lower.contains("claude-3.5") {
                    opts["thinking"] = serde_json::json!({
                        "type": "enabled",
                        "budget_tokens": 16000
                    });
                }
                opts
            }
            "google" | "google-vertex" => {
                if model_lower.contains("gemini-2.5") || model_lower.contains("gemini-3") {
                    serde_json::json!({
                        "thinkingConfig": {
                            "includeThoughts": true,
                            "thinkingBudget": 16000
                        }
                    })
                } else {
                    serde_json::json!({})
                }
            }
            "openai" => {
                if model_lower.contains("gpt-4") || model_lower.contains("gpt-5") {
                    serde_json::json!({
                        "store": false
                    })
                } else {
                    serde_json::json!({})
                }
            }
            _ => serde_json::json!({}),
        }
    }
}

pub fn transform_messages(messages: &[Message], config: &ProviderConfig) -> Vec<Message> {
    ProviderTransforms::transform_messages(messages, &config.provider_type, &config.model)
}

pub fn get_provider_headers(provider_type: &str, base_url: &str) -> Vec<(&'static str, String)> {
    ProviderTransforms::get_provider_headers(provider_type, base_url)
}

pub fn get_provider_specific_options(provider_type: &str, model: &str) -> serde_json::Value {
    ProviderTransforms::get_provider_specific_options(provider_type, model)
}
