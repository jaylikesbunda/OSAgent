use crate::tools::web_search::types::SearchResult;
use reqwest::Url;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

fn query_terms(query: &str) -> Vec<String> {
    const STOP_WORDS: &[&str] = &[
        "about",
        "and",
        "are",
        "documentation",
        "for",
        "from",
        "github",
        "how",
        "into",
        "the",
        "this",
        "today",
        "tomorrow",
        "weather",
        "what",
        "when",
        "where",
        "which",
        "with",
    ];
    let words = query
        .split(|ch: char| !ch.is_alphanumeric())
        .map(str::to_ascii_lowercase)
        .filter(|word| word.len() >= 3)
        .collect::<Vec<_>>();
    let meaningful = words
        .iter()
        .filter(|word| !STOP_WORDS.contains(&word.as_str()))
        .cloned()
        .collect::<HashSet<_>>()
        .into_iter()
        .collect::<Vec<_>>();
    if meaningful.is_empty() {
        words
            .into_iter()
            .collect::<HashSet<_>>()
            .into_iter()
            .collect()
    } else {
        meaningful
    }
}

fn relevance_score(result: &SearchResult, terms: &[String]) -> Option<i32> {
    if terms.is_empty() {
        return Some(0);
    }
    let title = result.title.to_ascii_lowercase();
    let url = result.url.to_ascii_lowercase();
    let snippet = result.snippet.to_ascii_lowercase();
    let score = terms.iter().fold(0, |score, term| {
        score
            + if title.contains(term) || url.contains(term) {
                35
            } else if snippet.contains(term) {
                10
            } else {
                0
            }
    });
    (score > 0).then_some(score)
}

fn domain_for(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_ascii_lowercase()))
}

fn source_key(source: &str) -> String {
    source
        .split(':')
        .next()
        .unwrap_or(source)
        .to_ascii_lowercase()
}

#[derive(Clone)]
struct AggregateResult {
    result: SearchResult,
    sources: HashSet<String>,
    best_position: usize,
}

pub fn rank_results(
    mut results: Vec<SearchResult>,
    max_results: usize,
    query: &str,
) -> Vec<SearchResult> {
    let terms = query_terms(query);
    let prefers_github = query
        .split(|ch: char| !ch.is_alphanumeric())
        .any(|word| word.eq_ignore_ascii_case("github"));
    let mut grouped: HashMap<String, AggregateResult> = HashMap::new();

    for result in results.drain(..) {
        let key = result.url.clone();
        let source = source_key(&result.source);
        grouped
            .entry(key)
            .and_modify(|aggregate| {
                aggregate.sources.insert(source.clone());
                if result.position < aggregate.best_position {
                    aggregate.best_position = result.position;
                }
                if aggregate.result.snippet.is_empty() && !result.snippet.is_empty() {
                    aggregate.result.snippet = result.snippet.clone();
                }
                if aggregate.result.title.len() < result.title.len() {
                    aggregate.result.title = result.title.clone();
                }
                aggregate.result.position = aggregate.best_position;
                let mut sources = aggregate.sources.iter().cloned().collect::<Vec<_>>();
                sources.sort();
                aggregate.result.source = sources.join(",");
            })
            .or_insert_with(|| AggregateResult {
                result: SearchResult {
                    source: source.clone(),
                    ..result.clone()
                },
                sources: HashSet::from([source]),
                best_position: result.position,
            });
    }

    let mut preliminary = grouped
        .into_values()
        .filter_map(|aggregate| {
            let relevance = relevance_score(&aggregate.result, &terms)?;
            let mut score = 1_000 - (aggregate.best_position as i32 * 10);
            score += relevance;
            score += aggregate.sources.len() as i32 * 35;
            if prefers_github
                && domain_for(&aggregate.result.url)
                    .is_some_and(|domain| domain == "github.com" || domain.ends_with(".github.com"))
            {
                score += 100;
            }
            if aggregate.result.url.starts_with("https://") {
                score += 5;
            }
            if !aggregate.result.snippet.is_empty() {
                score += 10;
            }
            if aggregate.result.title.len() < 10 {
                score -= 12;
            }

            Some((score, aggregate.result))
        })
        .collect::<Vec<_>>();

    preliminary.sort_by_key(|(score, result)| (Reverse(*score), result.position));

    let mut domain_counts = HashMap::new();
    let mut rescored = preliminary
        .into_iter()
        .map(|(score, result)| {
            let repeated_domain_penalty = domain_for(&result.url)
                .map(|domain| {
                    let count = domain_counts.entry(domain).or_insert(0usize);
                    let penalty = *count as i32 * 12;
                    *count += 1;
                    penalty
                })
                .unwrap_or_default();
            (score - repeated_domain_penalty, result)
        })
        .collect::<Vec<_>>();

    rescored.sort_by_key(|(score, result)| (Reverse(*score), result.position));
    rescored
        .into_iter()
        .take(max_results)
        .map(|(_, result)| result)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::rank_results;
    use crate::tools::web_search::types::SearchResult;

    #[test]
    fn ranking_prefers_diverse_domains_with_snippets() {
        let results = vec![
            SearchResult {
                title: "Example Docs".to_string(),
                url: "https://example.com/docs".to_string(),
                snippet: "Useful docs".to_string(),
                source: "ddg_lite".to_string(),
                position: 1,
            },
            SearchResult {
                title: "Example Blog".to_string(),
                url: "https://example.com/blog".to_string(),
                snippet: "".to_string(),
                source: "ddg_lite".to_string(),
                position: 2,
            },
            SearchResult {
                title: "Rust Async Guide".to_string(),
                url: "https://rust-lang.org/learn/async".to_string(),
                snippet: "Overview".to_string(),
                source: "ddg_lite".to_string(),
                position: 3,
            },
        ];

        let ranked = rank_results(results, 3, "example rust");
        assert_eq!(ranked[0].url, "https://example.com/docs");
        assert_eq!(ranked[1].url, "https://rust-lang.org/learn/async");
    }

    #[test]
    fn ranking_boosts_cross_backend_agreement() {
        let results = vec![
            SearchResult {
                title: "Example Docs".to_string(),
                url: "https://example.com/docs".to_string(),
                snippet: "Useful docs".to_string(),
                source: "ddg_lite".to_string(),
                position: 2,
            },
            SearchResult {
                title: "Example Docs - Rust".to_string(),
                url: "https://example.com/docs".to_string(),
                snippet: "Detailed docs".to_string(),
                source: "startpage".to_string(),
                position: 3,
            },
            SearchResult {
                title: "Other".to_string(),
                url: "https://other.example/post".to_string(),
                snippet: "One source only".to_string(),
                source: "brave".to_string(),
                position: 1,
            },
        ];

        let ranked = rank_results(results, 3, "example other");
        assert_eq!(ranked[0].url, "https://example.com/docs");
        assert_eq!(ranked[0].source, "ddg_lite,startpage");
    }

    #[test]
    fn rejects_unrelated_fallback_results() {
        let results = vec![SearchResult {
            title: "Wikipedia: Cat".to_string(),
            url: "https://en.wikipedia.org/wiki/Cat".to_string(),
            snippet: "A small domestic animal".to_string(),
            source: "wikipedia-api".to_string(),
            position: 1,
        }];
        assert!(rank_results(results, 5, "rust tokio timeout").is_empty());
    }

    #[test]
    fn generic_weather_hit_does_not_satisfy_city_query() {
        let results = vec![SearchResult {
            title: "Canberra Area Weather".to_string(),
            url: "https://example.com/canberra-weather".to_string(),
            snippet: "Tomorrow's forecast".to_string(),
            source: "bing".to_string(),
            position: 1,
        }];
        assert!(rank_results(results, 5, "weather Perth tomorrow").is_empty());
    }

    #[test]
    fn named_site_intent_boosts_matching_domain() {
        let results = vec![
            SearchResult {
                title: "OSAgent documentation".to_string(),
                url: "https://example.com/osagent".to_string(),
                snippet: String::new(),
                source: "bing".to_string(),
                position: 2,
            },
            SearchResult {
                title: "OSAgent repository".to_string(),
                url: "https://github.com/team/OSAgent".to_string(),
                snippet: String::new(),
                source: "bing".to_string(),
                position: 9,
            },
        ];
        let ranked = rank_results(results, 2, "OSAgent github");
        assert_eq!(ranked[0].url, "https://github.com/team/OSAgent");
    }
}
