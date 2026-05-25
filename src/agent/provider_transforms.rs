use crate::config::ProviderConfig;
use crate::storage::models::Message;
use serde_json::{Map, Value};

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
            "deepseek" => {
                Self::filter_empty_content_in_place(&mut result);
                Self::ensure_deepseek_reasoning_in_place(&mut result);
            }
            "ollama" | "groq" | "xai" => {
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
                if !msg.content.trim().is_empty() {
                    return true;
                }
                if !msg.images.is_empty() {
                    return true;
                }
                return false;
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
        _base_url: &str,
    ) -> Vec<(&'static str, String)> {
        let mut headers = Vec::new();

        // OpenRouter headers are now set exclusively in ProviderAuth::openrouter()
        // to avoid duplicates. No longer set here.

        if provider_type == "anthropic" {
            headers.push((
                "anthropic-beta",
                "interleaved-thinking-2025-05-14".to_string(),
            ));
        }

        headers
    }

    pub fn transform_schema(schema: Value, provider_type: &str, model: &str) -> Value {
        let model_lower = model.to_lowercase();

        // Moonshot/Kimi: strip $ref siblings, convert tuple items to single schema
        if provider_type == "moonshotai" || model_lower.contains("kimi") {
            return Self::transform_schema_moonshot(schema);
        }

        // Google/Gemini: integer enum to string, sanitize required, remove properties from non-object types
        if provider_type == "google" || model_lower.contains("gemini") {
            return Self::transform_schema_gemini(schema);
        }

        schema
    }

    fn transform_schema_moonshot(schema: Value) -> Value {
        match schema {
            Value::Object(map) => {
                // Moonshot expands $ref before validation and rejects sibling keywords
                if map.contains_key("$ref") {
                    return serde_json::json!({ "$ref": map["$ref"] });
                }
                let mut result = Map::new();
                for (key, val) in map {
                    result.insert(key, Self::transform_schema_moonshot(val));
                }
                // MFJS does not support tuple-style items arrays
                if let Some(Value::Array(items)) = result.get("items").cloned() {
                    if !items.is_empty() {
                        result.insert(
                            "items".to_string(),
                            Self::transform_schema_moonshot(items[0].clone()),
                        );
                    }
                }
                Value::Object(result)
            }
            Value::Array(arr) => Value::Array(
                arr.into_iter()
                    .map(Self::transform_schema_moonshot)
                    .collect(),
            ),
            _ => schema,
        }
    }

    fn transform_schema_gemini(schema: Value) -> Value {
        fn is_plain_object(v: &Value) -> bool {
            matches!(v, Value::Object(_))
        }

        fn has_schema_intent(v: &Value) -> bool {
            if !is_plain_object(v) {
                return false;
            }
            let obj = v.as_object().unwrap();
            if obj.contains_key("anyOf") || obj.contains_key("oneOf") || obj.contains_key("allOf") {
                return true;
            }
            [
                "type",
                "properties",
                "items",
                "prefixItems",
                "enum",
                "const",
                "$ref",
                "additionalProperties",
                "patternProperties",
                "required",
                "not",
                "if",
                "then",
                "else",
            ]
            .iter()
            .any(|k| obj.contains_key(*k))
        }

        fn sanitize(val: Value) -> Value {
            match val {
                Value::Object(map) => {
                    let mut result = Map::new();
                    for (key, value) in map {
                        let transformed = sanitize(value);
                        if key == "enum" {
                            if let Value::Array(items) = &transformed {
                                let string_vals: Vec<Value> = items
                                    .iter()
                                    .map(|v| match v {
                                        Value::Number(n) => Value::String(n.to_string()),
                                        Value::String(s) => Value::String(s.clone()),
                                        Value::Bool(b) => Value::String(b.to_string()),
                                        other => other.clone(),
                                    })
                                    .collect();
                                result.insert(key, Value::Array(string_vals));
                            } else {
                                result.insert(key, transformed);
                            }
                        } else {
                            result.insert(key, transformed);
                        }
                    }

                    // Convert integer/number type to string when enum is present
                    if result.contains_key("enum") {
                        if let Some(Value::String(t)) = result.get("type") {
                            if t == "integer" || t == "number" {
                                result.insert(
                                    "type".to_string(),
                                    Value::String("string".to_string()),
                                );
                            }
                        }
                    }

                    // Filter required array to only include fields that exist in properties
                    if let (Some(Value::Object(props)), Some(Value::Array(required))) = (
                        result.get("properties").cloned(),
                        result.get("required").cloned(),
                    ) {
                        let filtered: Vec<Value> = required
                            .iter()
                            .filter(|r| r.as_str().is_some_and(|f| props.contains_key(f)))
                            .cloned()
                            .collect();
                        result.insert("required".to_string(), Value::Array(filtered));
                    }

                    // Ensure array items have valid schema
                    if let Some(Value::String(t)) = result.get("type").cloned() {
                        if t == "array"
                            && !result.contains_key("anyOf")
                            && !result.contains_key("oneOf")
                            && !result.contains_key("allOf")
                        {
                            if !result.contains_key("items")
                                || result.get("items") == Some(&Value::Null)
                            {
                                result.insert("items".to_string(), Value::Object(Map::new()));
                            }
                            if let Some(Value::Object(items)) = result.get("items").cloned() {
                                if !has_schema_intent(&Value::Object(items)) {
                                    result.insert(
                                        "items".to_string(),
                                        serde_json::json!({ "type": "string" }),
                                    );
                                }
                            }
                        }

                        // Remove properties/required from non-object types
                        if t != "object"
                            && !result.contains_key("anyOf")
                            && !result.contains_key("oneOf")
                            && !result.contains_key("allOf")
                        {
                            result.remove("properties");
                            result.remove("required");
                        }
                    }

                    Value::Object(result)
                }
                Value::Array(arr) => Value::Array(arr.into_iter().map(sanitize).collect()),
                _ => val,
            }
        }

        sanitize(schema)
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
                let mut opts = serde_json::json!({});
                opts["store"] = serde_json::json!(false);
                if model_lower.contains("gpt-5") && !model_lower.contains("codex") {
                    opts["text_verbosity"] = serde_json::json!("low");
                }
                opts
            }
            "github-copilot" => {
                serde_json::json!({
                    "store": false
                })
            }
            _ => serde_json::json!({}),
        }
    }

    /// Ensure every DeepSeek assistant message has a reasoning/reasoning_content field.
    /// DeepSeek's API may behave unexpectedly if assistant messages lack a reasoning part.
    fn ensure_deepseek_reasoning_in_place(messages: &mut [Message]) {
        for msg in messages.iter_mut() {
            if msg.role == "assistant" && msg.thinking.is_none() {
                msg.thinking = Some(String::new());
            }
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

pub fn transform_schema(
    schema: serde_json::Value,
    provider_type: &str,
    model: &str,
) -> serde_json::Value {
    ProviderTransforms::transform_schema(schema, provider_type, model)
}
