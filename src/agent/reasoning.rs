use serde::Serialize;

use super::model_catalog::{ModelReasoningMetadata, ReasoningLevels};

#[derive(Debug, Clone, Serialize)]
pub struct ThinkingOptionsState {
    pub provider_id: String,
    pub model: String,
    pub options: Vec<String>,
    pub selected: String,
}

/// Fractions of a model's published budget window backing the generic
/// low/medium/high/max labels. The token ceilings themselves always come from
/// catalog metadata (`reasoning_options`), never from code.
const BUDGET_LADDER: &[(&str, f64)] = &[
    ("low", 0.25),
    ("medium", 0.5),
    ("high", 0.85),
    ("max", 1.0),
];

/// The thinking levels a model supports, straight from its published
/// `reasoning_options`. Models without catalog entry, without published
/// controls, or flagged non-reasoning get no options at all — nothing is
/// guessed from provider or model names.
pub fn options_for(meta: Option<&ModelReasoningMetadata>) -> Vec<String> {
    let Some(meta) = meta.filter(|meta| meta.reasoning) else {
        return Vec::new();
    };
    match &meta.levels {
        Some(ReasoningLevels::Efforts(values)) => values.clone(),
        Some(ReasoningLevels::Budget { min, max }) => {
            // Collapse ladder rungs that clamp onto the same budget so tiny
            // windows don't present several identical choices.
            let mut labels = Vec::new();
            let mut last_budget: Option<i64> = None;
            for (label, fraction) in BUDGET_LADDER {
                let budget = budget_at(*fraction, *min, *max);
                if last_budget != Some(budget) {
                    labels.push(label.to_string());
                    last_budget = Some(budget);
                }
            }
            labels
        }
        Some(ReasoningLevels::Toggle) => vec!["on".to_string()],
        None => Vec::new(),
    }
}

/// Whether an explicit "off" is meaningful. Effort-based models that don't
/// publish a `none` value cannot be switched off — selecting off falls back to
/// their weakest effort instead, mirroring the upstream API contract. Models
/// with no catalog entry get no fabricated switches either: nothing is sent.
pub fn can_disable(meta: Option<&ModelReasoningMetadata>) -> bool {
    let Some(meta) = meta else {
        return false;
    };
    if !meta.reasoning {
        return false;
    }
    match &meta.levels {
        Some(ReasoningLevels::Efforts(values)) => values.iter().any(|value| value == "none"),
        // Budget and toggle families have a real disable switch; with a known
        // reasoning model but no published controls, "off" means the same.
        _ => true,
    }
}

pub fn ui_options_for(meta: Option<&ModelReasoningMetadata>) -> Vec<String> {
    let mut options = vec!["auto".to_string()];
    if meta.is_some_and(|meta| meta.reasoning) {
        options.push("off".to_string());
    }
    options.extend(options_for(meta));
    options
}

pub fn normalize_selection(
    selection: &str,
    meta: Option<&ModelReasoningMetadata>,
) -> Option<String> {
    let value = selection.trim().to_ascii_lowercase();
    if value.is_empty() || value == "auto" || value == "default" {
        return None;
    }

    let supported = options_for(meta);
    if value == "off" || value == "none" || value == "disabled" {
        if can_disable(meta) {
            return Some("none".to_string());
        }
        return supported.first().cloned();
    }

    if supported.iter().any(|candidate| candidate == &value) {
        return Some(value);
    }

    supported.first().cloned()
}

/// Concrete thinking-budget tokens for a UI level against budget-style
/// metadata. `None` when the model doesn't publish a budget window or the
/// level isn't on the ladder.
pub fn budget_for_level(level: &str, meta: &ModelReasoningMetadata) -> Option<i64> {
    let ReasoningLevels::Budget { min, max } = meta.levels.as_ref()? else {
        return None;
    };
    let (_, fraction) = BUDGET_LADDER.iter().find(|(label, _)| *label == level)?;
    Some(budget_at(*fraction, *min, *max))
}

fn budget_at(fraction: f64, min: i64, max: i64) -> i64 {
    ((max as f64 * fraction).round() as i64).clamp(min, max)
}

pub fn state_for(
    provider_id: &str,
    model: &str,
    meta: Option<&ModelReasoningMetadata>,
    selected: &str,
) -> ThinkingOptionsState {
    let normalized = normalize_selection(selected, meta).unwrap_or_else(|| "auto".to_string());
    ThinkingOptionsState {
        provider_id: provider_id.to_string(),
        model: model.to_string(),
        options: ui_options_for(meta),
        selected: if normalized == "none" {
            "off".to_string()
        } else {
            normalized
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model_catalog::ReasoningLevels;

    fn meta(reasoning: bool, levels: Option<ReasoningLevels>) -> ModelReasoningMetadata {
        ModelReasoningMetadata {
            provider_id: "test-provider".to_string(),
            model_id: "test-model".to_string(),
            reasoning,
            output_limit: 64_000,
            levels,
        }
    }

    #[test]
    fn efforts_are_published_verbatim() {
        let meta = meta(
            true,
            Some(ReasoningLevels::Efforts(vec![
                "none".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
                "xhigh".to_string(),
            ])),
        );

        assert_eq!(
            options_for(Some(&meta)),
            vec!["none", "low", "medium", "high", "xhigh"]
        );
    }

    #[test]
    fn budget_windows_synthesize_a_ladder() {
        let meta = meta(
            true,
            Some(ReasoningLevels::Budget {
                min: 1024,
                max: 31_999,
            }),
        );

        assert_eq!(options_for(Some(&meta)), vec!["low", "medium", "high", "max"]);
        assert_eq!(budget_for_level("low", &meta), Some(8_000));
        assert_eq!(budget_for_level("high", &meta), Some(27_199));
        assert_eq!(budget_for_level("max", &meta), Some(31_999));
    }

    #[test]
    fn budgets_clamp_into_the_published_window() {
        let meta = meta(
            true,
            Some(ReasoningLevels::Budget {
                min: 30_000,
                max: 32_000,
            }),
        );

        // low/medium/high all clamp to the floor, so they collapse away.
        assert_eq!(options_for(Some(&meta)), vec!["low", "max"]);
        assert_eq!(budget_for_level("low", &meta), Some(30_000));
        assert_eq!(budget_for_level("max", &meta), Some(32_000));
    }

    #[test]
    fn toggle_exposes_on_only() {
        let meta = meta(true, Some(ReasoningLevels::Toggle));

        assert_eq!(options_for(Some(&meta)), vec!["on"]);
        assert!(can_disable(Some(&meta)));
    }

    #[test]
    fn non_reasoning_models_get_no_options() {
        let meta = meta(
            false,
            Some(ReasoningLevels::Efforts(vec!["low".to_string()])),
        );

        assert!(options_for(Some(&meta)).is_empty());
        assert!(!can_disable(Some(&meta)));
        assert_eq!(ui_options_for(Some(&meta)), vec!["auto"]);
    }

    #[test]
    fn missing_metadata_gets_no_options_and_no_fabricated_switches() {
        assert!(options_for(None).is_empty());
        assert_eq!(ui_options_for(None), vec!["auto"]);
        // Unknown model: "off" degrades to sending nothing at all.
        assert!(!can_disable(None));
        assert_eq!(normalize_selection("off", None), None);
        assert_eq!(normalize_selection("high", None), None);
    }

    #[test]
    fn off_maps_to_none_when_the_model_can_be_disabled() {
        let meta = meta(true, Some(ReasoningLevels::Toggle));
        assert_eq!(normalize_selection("off", Some(&meta)), Some("none".to_string()));
    }

    #[test]
    fn off_falls_back_to_weakest_effort_when_none_is_unpublished() {
        let meta = meta(
            true,
            Some(ReasoningLevels::Efforts(vec![
                "minimal".to_string(),
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
            ])),
        );

        assert_eq!(normalize_selection("off", Some(&meta)), Some("minimal".to_string()));
        assert!(!can_disable(Some(&meta)));
    }

    #[test]
    fn selections_are_validated_against_published_levels() {
        let meta = meta(
            true,
            Some(ReasoningLevels::Efforts(vec![
                "low".to_string(),
                "medium".to_string(),
                "high".to_string(),
            ])),
        );

        assert_eq!(normalize_selection("high", Some(&meta)), Some("high".to_string()));
        assert_eq!(normalize_selection("xhigh", Some(&meta)), Some("low".to_string()));
        assert_eq!(normalize_selection("auto", Some(&meta)), None);
    }

    #[test]
    fn state_reports_off_for_none() {
        let meta = meta(true, Some(ReasoningLevels::Toggle));
        let state = state_for("google", "gemini-flash", Some(&meta), "off");

        assert_eq!(state.selected, "off");
        assert_eq!(state.options, vec!["auto", "off", "on"]);
    }
}
