use crate::tools::web_search::types::SearchResult;
use reqwest::Url;
use std::cmp::Reverse;
use std::collections::HashMap;

fn domain_for(url: &str) -> Option<String> {
    Url::parse(url)
        .ok()
        .and_then(|parsed| parsed.host_str().map(|host| host.to_ascii_lowercase()))
}

pub fn rank_results(mut results: Vec<SearchResult>, max_results: usize) -> Vec<SearchResult> {
    let mut domain_counts = HashMap::new();
    let mut scored = results
        .drain(..)
        .map(|result| {
            let repeated_domain_penalty = domain_for(&result.url)
                .map(|domain| {
                    let count = domain_counts.entry(domain).or_insert(0usize);
                    let penalty = *count as i32 * 12;
                    *count += 1;
                    penalty
                })
                .unwrap_or_default();

            let mut score = 1_000 - (result.position as i32 * 10) - repeated_domain_penalty;
            if result.url.starts_with("https://") {
                score += 5;
            }
            if !result.snippet.is_empty() {
                score += 8;
            }
            if result.title.len() < 10 {
                score -= 12;
            }

            (score, result)
        })
        .collect::<Vec<_>>();

    scored.sort_by_key(|(score, result)| (Reverse(*score), result.position));
    scored
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
}
