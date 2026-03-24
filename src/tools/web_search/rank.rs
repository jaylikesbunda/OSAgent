use crate::tools::web_search::types::SearchResult;
use reqwest::Url;
use std::cmp::Reverse;
use std::collections::{HashMap, HashSet};

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

pub fn rank_results(mut results: Vec<SearchResult>, max_results: usize) -> Vec<SearchResult> {
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
        .map(|aggregate| {
            let mut score = 1_000 - (aggregate.best_position as i32 * 10);
            score += aggregate.sources.len() as i32 * 35;
            if aggregate.result.url.starts_with("https://") {
                score += 5;
            }
            if !aggregate.result.snippet.is_empty() {
                score += 10;
            }
            if aggregate.result.title.len() < 10 {
                score -= 12;
            }

            (score, aggregate.result)
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

        let ranked = rank_results(results, 3);
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

        let ranked = rank_results(results, 3);
        assert_eq!(ranked[0].url, "https://example.com/docs");
        assert_eq!(ranked[0].source, "ddg_lite,startpage");
    }
}
