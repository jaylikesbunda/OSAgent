//! Token pricing and cost computation.
//!
//! Prices resolve in three tiers:
//!
//! 1. `[usage.price_overrides]` in config (keyed `provider/model` or `model`)
//! 2. the bundled models.dev catalog (`ModelInfo` cost fields)
//! 3. a small curated fallback table for common model families
//!
//! All prices are USD per 1M tokens. A model without a price is simply not
//! charged to budgets (`0`), never guessed — an unpriced model must not
//! silently burn spend or block a budget.

use crate::agent::provider::TokenUsage;
use crate::config::{Config, TokenPrice};

/// Where a price came from — surfaced in `/usage` so the user can tell real
/// billing data from a curated estimate.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum PriceSource {
    Override,
    Catalog,
    Fallback,
}

#[derive(Debug, Clone, Copy, serde::Serialize)]
pub struct ResolvedPrice {
    pub price: TokenPrice,
    pub source: PriceSource,
}

/// Compute the USD cost of one provider request.
///
/// Providers split cache tokens differently: OpenAI-style `prompt_tokens`
/// *includes* cached tokens, Anthropic-style `input_tokens` excludes them.
/// When `input` covers the cache counts they are carved out of it; otherwise
/// `input` is already uncached and is charged as-is — so neither style
/// double-charges cached tokens at full price.
pub fn request_cost(usage: &TokenUsage, price: &TokenPrice) -> f64 {
    let cached_read = usage.cached_read.unwrap_or(0);
    let cached_write = usage.cached_write.unwrap_or(0);
    let (uncached_input, charge_cached_read, charge_cached_write) =
        if usage.input >= cached_read.saturating_add(cached_write) {
            (
                usage.input - cached_read - cached_write,
                cached_read,
                cached_write,
            )
        } else {
            (usage.input, cached_read, cached_write)
        };
    let output = usage.output as f64;

    (uncached_input as f64 * price.input
        + charge_cached_read as f64 * price.cache_read
        + charge_cached_write as f64 * price.cache_write
        + output * price.output)
        / 1_000_000.0
}

/// Curated fallback prices (USD per 1M tokens) for common families, used only
/// when neither the config nor the catalog carries a price. Keyed by model-id
/// prefix, longest prefix wins.
const FALLBACK_PRICES: &[(&str, TokenPrice)] = &[
    (
        "claude-opus-4",
        TokenPrice {
            input: 15.0,
            output: 75.0,
            cache_read: 1.5,
            cache_write: 18.75,
        },
    ),
    (
        "claude-sonnet-4",
        TokenPrice {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        },
    ),
    (
        "claude-3-5-sonnet",
        TokenPrice {
            input: 3.0,
            output: 15.0,
            cache_read: 0.3,
            cache_write: 3.75,
        },
    ),
    (
        "claude-3-5-haiku",
        TokenPrice {
            input: 0.8,
            output: 4.0,
            cache_read: 0.08,
            cache_write: 1.0,
        },
    ),
    (
        "gpt-4.1-mini",
        TokenPrice {
            input: 0.4,
            output: 1.6,
            cache_read: 0.1,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4.1-nano",
        TokenPrice {
            input: 0.1,
            output: 0.4,
            cache_read: 0.025,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4.1",
        TokenPrice {
            input: 2.0,
            output: 8.0,
            cache_read: 0.5,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4o-mini",
        TokenPrice {
            input: 0.15,
            output: 0.6,
            cache_read: 0.075,
            cache_write: 0.0,
        },
    ),
    (
        "gpt-4o",
        TokenPrice {
            input: 2.5,
            output: 10.0,
            cache_read: 1.25,
            cache_write: 0.0,
        },
    ),
    (
        "o4-mini",
        TokenPrice {
            input: 1.1,
            output: 4.4,
            cache_read: 0.275,
            cache_write: 0.0,
        },
    ),
    (
        "o3-mini",
        TokenPrice {
            input: 1.1,
            output: 4.4,
            cache_read: 0.55,
            cache_write: 0.0,
        },
    ),
    (
        "o3",
        TokenPrice {
            input: 10.0,
            output: 40.0,
            cache_read: 2.5,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-pro",
        TokenPrice {
            input: 1.25,
            output: 10.0,
            cache_read: 0.31,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-flash-lite",
        TokenPrice {
            input: 0.1,
            output: 0.4,
            cache_read: 0.025,
            cache_write: 0.0,
        },
    ),
    (
        "gemini-2.5-flash",
        TokenPrice {
            input: 0.3,
            output: 2.5,
            cache_read: 0.075,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-reasoner",
        TokenPrice {
            input: 0.55,
            output: 2.19,
            cache_read: 0.14,
            cache_write: 0.0,
        },
    ),
    (
        "deepseek-chat",
        TokenPrice {
            input: 0.27,
            output: 1.1,
            cache_read: 0.07,
            cache_write: 0.0,
        },
    ),
];

fn fallback_price(model_id: &str) -> Option<TokenPrice> {
    let lowered = model_id.to_ascii_lowercase();
    FALLBACK_PRICES
        .iter()
        .filter(|(prefix, _)| lowered.starts_with(prefix))
        .max_by_key(|(prefix, _)| prefix.len())
        .map(|(_, price)| *price)
}

/// Resolve a price for `provider_id:model_id` against config overrides, the
/// catalog, and the fallback table.
pub fn resolve_price(
    config: &Config,
    catalog_prices: Option<TokenPrice>,
    provider_id: &str,
    model_id: &str,
) -> Option<ResolvedPrice> {
    let overrides = &config.usage.price_overrides;
    if let Some(price) = overrides
        .get(&format!("{}/{}", provider_id, model_id))
        .or_else(|| overrides.get(model_id))
    {
        return Some(ResolvedPrice {
            price: *price,
            source: PriceSource::Override,
        });
    }
    if let Some(price) = catalog_prices {
        // A catalog entry of all zeros means "not published", not "free".
        if price.input > 0.0 || price.output > 0.0 {
            return Some(ResolvedPrice {
                price,
                source: PriceSource::Catalog,
            });
        }
    }
    fallback_price(model_id).map(|price| ResolvedPrice {
        price,
        source: PriceSource::Fallback,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn usage(input: usize, output: usize, cached_read: usize, cached_write: usize) -> TokenUsage {
        TokenUsage {
            input,
            output,
            total: input + output,
            cached_read: Some(cached_read),
            cached_write: Some(cached_write),
            reasoning: None,
        }
    }

    #[test]
    fn cost_splits_cached_and_uncached_input() {
        let price = TokenPrice {
            input: 10.0,
            output: 20.0,
            cache_read: 1.0,
            cache_write: 5.0,
        };
        // OpenAI-style: input includes the cache counts. 1M total input:
        // 400k uncached + 500k cache-read + 100k cache-write, 200k output.
        let cost = request_cost(&usage(1_000_000, 200_000, 500_000, 100_000), &price);
        // 400k*10 + 500k*1 + 100k*5 + 200k*20 (all / 1M)
        assert!((cost - (4.0 + 0.5 + 0.5 + 4.0)).abs() < 1e-9, "{}", cost);

        // Anthropic-style: input is already uncached and the cache counts are
        // separate; they must be charged at cache rates without being carved
        // out of (or added on top of) the uncached count twice.
        let cost = request_cost(&usage(400_000, 200_000, 500_000, 100_000), &price);
        assert!((cost - (4.0 + 0.5 + 0.5 + 4.0)).abs() < 1e-9, "{}", cost);
    }

    #[test]
    fn zero_price_costs_nothing() {
        let price = TokenPrice::default();
        assert_eq!(
            request_cost(&usage(1_000_000, 1_000_000, 0, 0), &price),
            0.0
        );
    }

    #[test]
    fn fallback_matches_longest_prefix() {
        let price = fallback_price("claude-sonnet-4-5-20250929").unwrap();
        assert_eq!(price.input, 3.0);
        let price = fallback_price("gpt-4.1-mini-2025-04-14").unwrap();
        assert_eq!(price.input, 0.4);
        assert!(fallback_price("totally-unknown-model").is_none());
    }

    #[test]
    fn override_beats_catalog() {
        let config = Config::default();
        let mut config = config;
        config.usage.price_overrides.insert(
            "my-model".to_string(),
            TokenPrice {
                input: 1.0,
                output: 2.0,
                cache_read: 0.0,
                cache_write: 0.0,
            },
        );
        let resolved = resolve_price(
            &config,
            Some(TokenPrice {
                input: 9.0,
                output: 9.0,
                cache_read: 0.0,
                cache_write: 0.0,
            }),
            "prov",
            "my-model",
        )
        .unwrap();
        assert_eq!(resolved.source, PriceSource::Override);
        assert_eq!(resolved.price.input, 1.0);
    }

    #[test]
    fn zeroed_catalog_price_falls_through() {
        let config = Config::default();
        let resolved = resolve_price(
            &config,
            Some(TokenPrice::default()),
            "prov",
            "claude-sonnet-4",
        )
        .unwrap();
        assert_eq!(resolved.source, PriceSource::Fallback);
    }
}
