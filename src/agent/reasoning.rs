use serde::Serialize;

use super::model_catalog::ModelReasoningMetadata;

#[derive(Debug, Clone, Serialize)]
pub struct ThinkingOptionsState {
    pub provider_id: String,
    pub model: String,
    pub options: Vec<String>,
    pub selected: String,
}

pub fn options_for(
    provider_id: &str,
    model: &str,
    meta: Option<&ModelReasoningMetadata>,
) -> Vec<&'static str> {
    let provider = provider_id.to_ascii_lowercase();
    let id = model.to_ascii_lowercase();
    let family = model_family(&id, meta);
    let release_date = meta.map(|value| value.release_date.as_str()).unwrap_or("");

    match provider.as_str() {
        "openai" => openai_options(&id, &family, release_date),
        "github-copilot" | "github-copilot-enterprise" => copilot_options(&id),
        "openrouter" => openrouter_options(&id, &family),
        "anthropic" => anthropic_options(&id),
        "google" | "google-vertex" => google_options(&id),
        "groq" => {
            if supports_reasoning(&id, meta) {
                vec!["none", "low", "medium", "high"]
            } else {
                Vec::new()
            }
        }
        "xai" => xai_options(&id),
        _ => Vec::new(),
    }
}

pub fn can_disable(provider_id: &str, model: &str, meta: Option<&ModelReasoningMetadata>) -> bool {
    let provider = provider_id.to_ascii_lowercase();
    let id = model.to_ascii_lowercase();

    match provider.as_str() {
        "openai" => !is_strict_codex(&id),
        "github-copilot" | "github-copilot-enterprise" => !is_strict_codex(&id),
        _ => !options_for(provider_id, model, meta).is_empty(),
    }
}

pub fn ui_options_for(
    provider_id: &str,
    model: &str,
    meta: Option<&ModelReasoningMetadata>,
) -> Vec<String> {
    let mut options = vec!["auto".to_string()];
    if can_disable(provider_id, model, meta) {
        options.push("off".to_string());
    }
    options.extend(
        options_for(provider_id, model, meta)
            .into_iter()
            .map(str::to_string),
    );
    options
}

pub fn normalize_selection(
    selection: &str,
    provider_id: &str,
    model: &str,
    meta: Option<&ModelReasoningMetadata>,
) -> Option<String> {
    let value = selection.trim().to_ascii_lowercase();
    if value.is_empty() || value == "auto" || value == "default" {
        return None;
    }

    let supported = options_for(provider_id, model, meta);
    if value == "off" || value == "none" || value == "disabled" {
        if can_disable(provider_id, model, meta) {
            return Some("none".to_string());
        }
        return supported.first().map(|value| (*value).to_string());
    }

    if supported.iter().any(|candidate| *candidate == value) {
        return Some(value);
    }

    supported.first().map(|value| (*value).to_string())
}

pub fn state_for(
    provider_id: &str,
    model: &str,
    meta: Option<&ModelReasoningMetadata>,
    selected: &str,
) -> ThinkingOptionsState {
    let normalized = normalize_selection(selected, provider_id, model, meta)
        .unwrap_or_else(|| "auto".to_string());
    ThinkingOptionsState {
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        options: ui_options_for(provider_id, model, meta),
        selected: if normalized == "none" {
            "off".to_string()
        } else {
            normalized
        },
    }
}

fn is_strict_codex(id: &str) -> bool {
    id.contains("codex")
}

fn supports_reasoning(id: &str, meta: Option<&ModelReasoningMetadata>) -> bool {
    if let Some(meta) = meta {
        if meta.reasoning {
            return true;
        }
    }
    id.contains("codex")
        || id.contains("reason")
        || id.starts_with("o1")
        || id.starts_with("o3")
        || id.contains("gemini")
        || id.contains("claude")
        || id.contains("grok-3-mini")
}

fn model_family(id: &str, meta: Option<&ModelReasoningMetadata>) -> String {
    if let Some(meta) = meta {
        if !meta.family.is_empty() {
            return meta.family.to_ascii_lowercase();
        }
    }

    if id.contains("claude") {
        return "claude".to_string();
    }
    if id.contains("gemini") {
        return "gemini".to_string();
    }
    if id.contains("grok") {
        return "grok".to_string();
    }
    if id.contains("gpt") || id.contains("codex") {
        return "gpt".to_string();
    }
    if id.starts_with("o1") || id.starts_with("o3") || id.starts_with("o4") {
        return "o".to_string();
    }
    String::new()
}

fn openai_options(id: &str, family: &str, release_date: &str) -> Vec<&'static str> {
    if id == "gpt-5-pro" || !supports_reasoning(id, None) && family != "gpt" && family != "o" {
        return Vec::new();
    }

    if id.contains("codex") {
        if id.contains("5.2") || id.contains("5.3") {
            return vec!["low", "medium", "high", "xhigh"];
        }
        return vec!["low", "medium", "high"];
    }

    let mut efforts = vec!["low", "medium", "high"];
    if id.contains("gpt-5") || id == "gpt-5" {
        efforts.insert(0, "minimal");
    }
    if !release_date.is_empty() && release_date >= "2025-11-13" {
        efforts.insert(0, "none");
    }
    if !release_date.is_empty() && release_date >= "2025-12-04" {
        efforts.push("xhigh");
    }
    efforts
}

fn copilot_options(id: &str) -> Vec<&'static str> {
    if id.contains("gemini") {
        return Vec::new();
    }
    if id.contains("claude") {
        return vec!["high"];
    }
    if id.contains("codex") {
        if id.contains("5.1-codex-max") || id.contains("5.2") || id.contains("5.3") {
            return vec!["low", "medium", "high", "xhigh"];
        }
        return vec!["low", "medium", "high"];
    }
    Vec::new()
}

fn openrouter_options(id: &str, family: &str) -> Vec<&'static str> {
    if ["deepseek", "minimax", "glm", "mistral", "kimi", "k2p5"]
        .iter()
        .any(|term| id.contains(term))
    {
        return Vec::new();
    }
    if id.contains("grok-3-mini") {
        return vec!["low", "high"];
    }
    if id.contains("grok") {
        return Vec::new();
    }
    if family == "gpt" || id.contains("gemini-3") || family == "claude" {
        return vec!["none", "minimal", "low", "medium", "high", "xhigh"];
    }
    Vec::new()
}

fn anthropic_options(id: &str) -> Vec<&'static str> {
    if ["opus-4-6", "opus-4.6", "sonnet-4-6", "sonnet-4.6"]
        .iter()
        .any(|term| id.contains(term))
    {
        return vec!["low", "medium", "high", "max"];
    }
    vec!["high", "max"]
}

fn google_options(id: &str) -> Vec<&'static str> {
    if id.contains("2.5") {
        return vec!["high", "max"];
    }
    if id.contains("3.1") {
        return vec!["low", "medium", "high"];
    }
    if id.contains("gemini") {
        return vec!["low", "high"];
    }
    Vec::new()
}

fn xai_options(id: &str) -> Vec<&'static str> {
    if id.contains("grok-3-mini") {
        return vec!["low", "high"];
    }
    Vec::new()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn openai_release_date_expands_efforts() {
        let meta = ModelReasoningMetadata {
            provider_id: "openai".to_string(),
            model_id: "gpt-5".to_string(),
            family: "gpt".to_string(),
            reasoning: true,
            release_date: "2025-12-05".to_string(),
            output_limit: 0,
        };

        let options = options_for("openai", "gpt-5", Some(&meta));
        assert_eq!(
            options,
            vec!["none", "minimal", "low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn codex_off_normalizes_to_supported_effort() {
        let meta = ModelReasoningMetadata {
            provider_id: "openai".to_string(),
            model_id: "gpt-5.3-codex".to_string(),
            family: "gpt".to_string(),
            reasoning: true,
            release_date: "2025-10-01".to_string(),
            output_limit: 0,
        };

        let selected = normalize_selection("off", "openai", "gpt-5.3-codex", Some(&meta));
        assert_eq!(selected.as_deref(), Some("low"));
    }
}
