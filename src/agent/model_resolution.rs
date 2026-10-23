//! Resolving a loosely phrased model request to a concrete provider + model.
//!
//! A user (or the main agent) can ask for a model by family ("sonnet"), by id
//! ("claude-sonnet-4-5") or by explicit pair ("anthropic:claude-sonnet-4-5").
//! Only one of those is unambiguous, so the loose forms are matched against
//! the connected model catalog.
//!
//! When several providers offer a match the tie is broken by usage history:
//! `model_usage` remembers the provider each model id was last run on, and a
//! candidate counts as preferred only when the recorded provider is its own.
//! That is what makes "use sonnet" pick the provider the user last used sonnet
//! through. If nothing breaks the tie the caller is expected to ask the user
//! and retry with an explicit pair.

use crate::agent::model_catalog::{CatalogState, ModelInfo};
use std::collections::HashMap;

/// A concrete `(provider, model)` pair.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedModel {
    pub provider_id: String,
    pub model_id: String,
}

impl ResolvedModel {
    /// The `provider_id:model_id` form understood by the subagent overrides.
    pub fn as_spec(&self) -> String {
        format!("{}:{}", self.provider_id, self.model_id)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ModelResolution {
    Resolved(ResolvedModel),
    /// Several providers offer a match and the usage history does not pick
    /// one. The caller asks the user which provider to use.
    Ambiguous {
        query: String,
        candidates: Vec<ResolvedModel>,
    },
    NotFound {
        query: String,
    },
}

/// Providers are keyed by model id only: the last provider a model id was run
/// on is the preference for that exact id.
pub type ModelUsage = HashMap<String, (String, i64)>;

/// Resolve `query` against the connected catalog.
pub fn resolve_model_request(
    query: &str,
    catalog: &CatalogState,
    usage: &ModelUsage,
) -> ModelResolution {
    let query = query.trim();
    if query.is_empty() {
        return ModelResolution::NotFound {
            query: String::new(),
        };
    }

    // "provider:model" is explicit. Provider ids never contain ':' but model
    // ids can ("llama3:8b"), so the prefix only counts as a provider when it
    // names a provider we know about.
    if let Some((prefix, rest)) = query.split_once(':') {
        let prefix = prefix.trim();
        let rest = rest.trim();
        if !prefix.is_empty() && !rest.is_empty() {
            if let Some(provider) = catalog
                .providers
                .iter()
                .find(|provider| provider.id.eq_ignore_ascii_case(prefix))
            {
                if !provider.connected {
                    return ModelResolution::NotFound {
                        query: query.to_string(),
                    };
                }
                return ModelResolution::Resolved(ResolvedModel {
                    provider_id: provider.id.clone(),
                    model_id: rest.to_string(),
                });
            }
        }
    }

    let connected: std::collections::HashSet<&str> = catalog
        .providers
        .iter()
        .filter(|provider| provider.connected)
        .map(|provider| provider.id.as_str())
        .collect();
    let needle = query.to_lowercase();

    // Match tiers: exact id, exact name, then either contains the query.
    let mut best_tier = 0u8;
    let mut candidates: Vec<&ModelInfo> = Vec::new();
    for model in &catalog.all_models {
        if !connected.contains(model.provider_id.as_str()) {
            continue;
        }
        let id = model.id.to_lowercase();
        let name = model.name.to_lowercase();
        let tier = if id == needle {
            3
        } else if name == needle {
            2
        } else if id.contains(&needle) || name.contains(&needle) {
            1
        } else {
            continue;
        };
        if tier > best_tier {
            best_tier = tier;
            candidates.clear();
        }
        if tier == best_tier {
            candidates.push(model);
        }
    }

    let candidates = dedupe(candidates);
    if candidates.is_empty() {
        return ModelResolution::NotFound {
            query: query.to_string(),
        };
    }

    // "The provider I last used that model on" wins, but only for a candidate
    // whose own provider is the recorded one.
    let mut preferred: Vec<(i64, &ModelInfo)> = candidates
        .iter()
        .filter_map(|model| {
            let (provider_id, used_at) = usage.get(&model.id)?;
            (provider_id == &model.provider_id).then_some((*used_at, *model))
        })
        .collect();
    if preferred.len() == 1 {
        return ModelResolution::Resolved(resolved(preferred[0].1));
    }
    if preferred.len() > 1 {
        preferred.sort_by(|a, b| b.0.cmp(&a.0));
        // Strictly newer history wins; a same-second switch stays ambiguous so
        // the user gets the choice instead of a coin flip.
        if preferred[0].0 > preferred[1].0 {
            return ModelResolution::Resolved(resolved(preferred[0].1));
        }
    }

    let providers: std::collections::HashSet<&str> = candidates
        .iter()
        .map(|model| model.provider_id.as_str())
        .collect();
    if providers.len() == 1 {
        // One provider, several matching models ("sonnet" against a provider
        // that ships several Sonnet versions). The newest match wins.
        return ModelResolution::Resolved(resolved(best_of(&candidates)));
    }

    // Several providers still match. Offer one candidate per provider (its
    // newest match) so asking the user stays a short, meaningful choice.
    let mut out: Vec<ResolvedModel> = providers
        .iter()
        .filter_map(|provider_id| {
            let on_provider: Vec<&ModelInfo> = candidates
                .iter()
                .copied()
                .filter(|model| model.provider_id == *provider_id)
                .collect();
            (!on_provider.is_empty()).then(|| resolved(best_of(&on_provider)))
        })
        .collect();
    out.sort_by(|a, b| {
        a.provider_id
            .cmp(&b.provider_id)
            .then_with(|| a.model_id.cmp(&b.model_id))
    });
    out.truncate(12);
    ModelResolution::Ambiguous {
        query: query.to_string(),
        candidates: out,
    }
}

/// The preferred model among matches: newest release, then the shortest id
/// (a dated snapshot is longer than its base id), then alphabetical.
fn best_of<'a>(models: &[&'a ModelInfo]) -> &'a ModelInfo {
    let mut best = models[0];
    for model in &models[1..] {
        let model_is_better = model
            .release_date
            .cmp(&best.release_date)
            .then_with(|| best.id.len().cmp(&model.id.len()))
            .then_with(|| best.id.cmp(&model.id))
            .is_gt();
        if model_is_better {
            best = model;
        }
    }
    best
}

fn resolved(model: &ModelInfo) -> ResolvedModel {
    ResolvedModel {
        provider_id: model.provider_id.clone(),
        model_id: model.id.clone(),
    }
}

fn dedupe(models: Vec<&ModelInfo>) -> Vec<&ModelInfo> {
    let mut seen = std::collections::HashSet::new();
    models
        .into_iter()
        .filter(|model| seen.insert((model.provider_id.clone(), model.id.clone())))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::agent::model_catalog::{ModelInfo, ProviderInfo};

    fn model(provider_id: &str, provider_name: &str, id: &str, name: &str) -> ModelInfo {
        ModelInfo {
            id: id.to_string(),
            name: name.to_string(),
            provider_id: provider_id.to_string(),
            provider_name: provider_name.to_string(),
            context_window: 200_000,
            input_limit: None,
            output_limit: 8_192,
            supports_tools: true,
            supports_vision: false,
            category: "popular".to_string(),
            available: true,
            input_cost_per_1m: 0.0,
            output_cost_per_1m: 0.0,
            release_date: None,
        }
    }

    fn provider(id: &str, name: &str, connected: bool) -> ProviderInfo {
        ProviderInfo {
            id: id.to_string(),
            name: name.to_string(),
            base_url: String::new(),
            description: String::new(),
            connected,
            api_key_source: String::new(),
            oauth_supported: false,
            api_key_url: None,
            models: Vec::new(),
        }
    }

    fn catalog() -> CatalogState {
        CatalogState {
            providers: vec![
                provider("anthropic", "Anthropic", true),
                provider("openrouter", "OpenRouter", true),
                provider("google", "Google AI", true),
                provider("unconfigured", "Unconfigured", false),
            ],
            all_models: vec![
                model("anthropic", "Anthropic", "claude-sonnet-4-5", "Claude Sonnet 4.5"),
                model("anthropic", "Anthropic", "claude-opus-4-5", "Claude Opus 4.5"),
                model(
                    "openrouter",
                    "OpenRouter",
                    "anthropic/claude-sonnet-4-5",
                    "Anthropic: Claude Sonnet 4.5",
                ),
                model("google", "Google AI", "gemini-2-pro", "Gemini 2 Pro"),
                model(
                    "unconfigured",
                    "Unconfigured",
                    "mystery-sonnet",
                    "Mystery Sonnet",
                ),
            ],
        }
    }

    fn usage(entries: &[(&str, &str, i64)]) -> ModelUsage {
        entries
            .iter()
            .map(|(model_id, provider_id, used_at)| {
                (model_id.to_string(), (provider_id.to_string(), *used_at))
            })
            .collect()
    }

    #[test]
    fn explicit_provider_model_is_used_as_is() {
        let resolution = resolve_model_request(
            "openrouter:anthropic/claude-sonnet-4-5",
            &catalog(),
            &usage(&[]),
        );
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "openrouter".to_string(),
                model_id: "anthropic/claude-sonnet-4-5".to_string(),
            })
        );
    }

    #[test]
    fn a_model_id_containing_a_colon_is_not_read_as_a_provider() {
        let mut catalog = catalog();
        catalog.providers.push(provider("ollama", "Ollama (Local)", true));
        catalog
            .all_models
            .push(model("ollama", "Ollama (Local)", "llama3:8b", "Llama 3 8B"));

        let resolution = resolve_model_request("llama3:8b", &catalog, &usage(&[]));
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "ollama".to_string(),
                model_id: "llama3:8b".to_string(),
            })
        );
    }

    #[test]
    fn an_unconfigured_provider_cannot_be_selected_explicitly() {
        let resolution = resolve_model_request("unconfigured:mystery-sonnet", &catalog(), &usage(&[]));
        assert_eq!(
            resolution,
            ModelResolution::NotFound {
                query: "unconfigured:mystery-sonnet".to_string()
            }
        );
    }

    #[test]
    fn an_exact_model_id_resolves_across_providers() {
        let resolution =
            resolve_model_request("claude-sonnet-4-5", &catalog(), &usage(&[]));
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "anthropic".to_string(),
                model_id: "claude-sonnet-4-5".to_string(),
            })
        );
    }

    #[test]
    fn an_ambiguous_family_name_lists_the_candidates() {
        let resolution = resolve_model_request("sonnet", &catalog(), &usage(&[]));
        let ModelResolution::Ambiguous { candidates, .. } = resolution else {
            panic!("expected ambiguity, got {resolution:?}");
        };
        assert!(candidates.contains(&ResolvedModel {
            provider_id: "anthropic".to_string(),
            model_id: "claude-sonnet-4-5".to_string(),
        }));
        assert!(candidates.contains(&ResolvedModel {
            provider_id: "openrouter".to_string(),
            model_id: "anthropic/claude-sonnet-4-5".to_string(),
        }));
    }

    #[test]
    fn the_last_provider_used_for_the_model_wins() {
        let resolution = resolve_model_request(
            "sonnet",
            &catalog(),
            &usage(&[("anthropic/claude-sonnet-4-5", "openrouter", 100)]),
        );
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "openrouter".to_string(),
                model_id: "anthropic/claude-sonnet-4-5".to_string(),
            })
        );
    }

    #[test]
    fn the_most_recent_provider_wins_when_the_model_was_used_twice() {
        // Used anthropic first, then the OpenRouter variant later; the newer
        // history entry is the provider the user last ran the model on.
        let resolution = resolve_model_request(
            "sonnet",
            &catalog(),
            &usage(&[
                ("claude-sonnet-4-5", "anthropic", 100),
                ("anthropic/claude-sonnet-4-5", "openrouter", 200),
            ]),
        );
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "openrouter".to_string(),
                model_id: "anthropic/claude-sonnet-4-5".to_string(),
            })
        );
    }

    #[test]
    fn a_single_provider_picks_the_newest_matching_model() {
        let catalog = CatalogState {
            providers: vec![provider("anthropic", "Anthropic", true)],
            all_models: vec![
                ModelInfo {
                    release_date: Some("2024-10-22".to_string()),
                    ..model(
                        "anthropic",
                        "Anthropic",
                        "claude-3-5-sonnet-20241022",
                        "Claude 3.5 Sonnet",
                    )
                },
                ModelInfo {
                    release_date: Some("2025-09-29".to_string()),
                    ..model("anthropic", "Anthropic", "claude-sonnet-4-5", "Claude Sonnet 4.5")
                },
            ],
        };

        let resolution = resolve_model_request("sonnet", &catalog, &usage(&[]));
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "anthropic".to_string(),
                model_id: "claude-sonnet-4-5".to_string(),
            })
        );
    }

    #[test]
    fn an_ambiguous_family_name_lists_one_model_per_provider() {
        let catalog = CatalogState {
            providers: vec![
                provider("anthropic", "Anthropic", true),
                provider("openrouter", "OpenRouter", true),
            ],
            all_models: vec![
                ModelInfo {
                    release_date: Some("2024-10-22".to_string()),
                    ..model(
                        "anthropic",
                        "Anthropic",
                        "claude-3-5-sonnet-20241022",
                        "Claude 3.5 Sonnet",
                    )
                },
                ModelInfo {
                    release_date: Some("2025-09-29".to_string()),
                    ..model("anthropic", "Anthropic", "claude-sonnet-4-5", "Claude Sonnet 4.5")
                },
                model(
                    "openrouter",
                    "OpenRouter",
                    "anthropic/claude-sonnet-4-5",
                    "Anthropic: Claude Sonnet 4.5",
                ),
            ],
        };

        let resolution = resolve_model_request("sonnet", &catalog, &usage(&[]));
        let ModelResolution::Ambiguous { candidates, .. } = resolution else {
            panic!("expected ambiguity");
        };
        // Only the newest Anthropic match is offered alongside OpenRouter.
        assert!(candidates.contains(&ResolvedModel {
            provider_id: "anthropic".to_string(),
            model_id: "claude-sonnet-4-5".to_string(),
        }));
        assert!(!candidates.contains(&ResolvedModel {
            provider_id: "anthropic".to_string(),
            model_id: "claude-3-5-sonnet-20241022".to_string(),
        }));
        assert!(candidates.contains(&ResolvedModel {
            provider_id: "openrouter".to_string(),
            model_id: "anthropic/claude-sonnet-4-5".to_string(),
        }));
        assert_eq!(candidates.len(), 2);
    }

    #[test]
    fn only_connected_providers_are_candidates() {
        let resolution = resolve_model_request("mystery-sonnet", &catalog(), &usage(&[]));
        assert_eq!(
            resolution,
            ModelResolution::NotFound {
                query: "mystery-sonnet".to_string()
            }
        );
    }

    #[test]
    fn an_unmatched_name_is_not_found() {
        assert_eq!(
            resolve_model_request("nothing-like-this", &catalog(), &usage(&[])),
            ModelResolution::NotFound {
                query: "nothing-like-this".to_string()
            }
        );
    }

    #[test]
    fn an_exact_name_match_beats_a_substring_match() {
        let resolution = resolve_model_request("Gemini 2 Pro", &catalog(), &usage(&[]));
        assert_eq!(
            resolution,
            ModelResolution::Resolved(ResolvedModel {
                provider_id: "google".to_string(),
                model_id: "gemini-2-pro".to_string(),
            })
        );
    }
}
