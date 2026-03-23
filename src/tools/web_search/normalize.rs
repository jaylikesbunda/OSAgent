use crate::tools::web_search::types::SearchResult;
use reqwest::Url;
use std::collections::HashSet;

const TRACKING_PARAMS: &[&str] = &[
    "fbclid",
    "gclid",
    "igshid",
    "mc_cid",
    "mc_eid",
    "ref",
    "ref_src",
    "si",
    "source",
    "src",
    "utm_campaign",
    "utm_content",
    "utm_id",
    "utm_medium",
    "utm_name",
    "utm_source",
    "utm_term",
    "ved",
];

pub fn normalize_query(query: &str) -> String {
    clean_text(query)
}

pub fn clean_text(value: &str) -> String {
    value.split_whitespace().collect::<Vec<_>>().join(" ")
}

pub fn decode_duckduckgo_redirect(raw: &str) -> String {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return String::new();
    }

    if let Ok(url) = Url::parse(trimmed) {
        let host = url.host_str().unwrap_or_default().to_ascii_lowercase();
        if host.contains("duckduckgo.com") || host.contains("duck.co") {
            for (key, value) in url.query_pairs() {
                if key == "uddg" && !value.is_empty() {
                    return value.to_string();
                }
            }
        }
    }

    trimmed.to_string()
}

fn should_keep_query_param(key: &str) -> bool {
    let lower = key.to_ascii_lowercase();
    !TRACKING_PARAMS.contains(&lower.as_str()) && !lower.starts_with("utm_")
}

pub fn canonicalize_url(raw: &str) -> Option<String> {
    let decoded = decode_duckduckgo_redirect(raw);
    let mut url = Url::parse(&decoded).ok()?;
    match url.scheme() {
        "http" | "https" => {}
        _ => return None,
    }

    url.set_fragment(None);

    let kept_pairs = url
        .query_pairs()
        .filter(|(key, _)| should_keep_query_param(key))
        .map(|(key, value)| (key.to_string(), value.to_string()))
        .collect::<Vec<_>>();

    url.set_query(None);
    if !kept_pairs.is_empty() {
        let mut pairs = url.query_pairs_mut();
        for (key, value) in kept_pairs {
            pairs.append_pair(&key, &value);
        }
    }

    Some(url.to_string())
}

pub fn normalize_results(results: Vec<SearchResult>) -> Vec<SearchResult> {
    let mut seen = HashSet::new();
    let mut normalized = Vec::new();

    for mut result in results {
        let title = clean_text(&result.title);
        let snippet = clean_text(&result.snippet);
        let Some(url) = canonicalize_url(&result.url) else {
            continue;
        };
        if title.is_empty() || !seen.insert(url.clone()) {
            continue;
        }

        result.title = title;
        result.snippet = snippet;
        result.url = url;
        normalized.push(result);
    }

    normalized
}

#[cfg(test)]
mod tests {
    use super::{canonicalize_url, decode_duckduckgo_redirect, normalize_results};
    use crate::tools::web_search::types::SearchResult;

    #[test]
    fn decodes_duckduckgo_redirects() {
        let decoded = decode_duckduckgo_redirect(
            "https://duckduckgo.com/l/?uddg=https%3A%2F%2Fexample.com%2Fdocs%3Futm_source%3Dddg",
        );
        assert_eq!(decoded, "https://example.com/docs?utm_source=ddg");
    }

    #[test]
    fn canonicalizes_and_strips_tracking_params() {
        let url = canonicalize_url("https://example.com/docs?utm_source=ddg&ref=foo&id=7#intro")
            .expect("expected valid url");
        assert_eq!(url, "https://example.com/docs?id=7");
    }

    #[test]
    fn normalizes_and_deduplicates_results() {
        let results = vec![
            SearchResult {
                title: " Example Docs ".to_string(),
                url: "https://example.com/docs?utm_source=ddg".to_string(),
                snippet: " Learn   more ".to_string(),
                source: "ddg_lite".to_string(),
                position: 1,
            },
            SearchResult {
                title: "Example Docs".to_string(),
                url: "https://example.com/docs".to_string(),
                snippet: "duplicate".to_string(),
                source: "ddg_html".to_string(),
                position: 2,
            },
        ];

        let normalized = normalize_results(results);
        assert_eq!(normalized.len(), 1);
        assert_eq!(normalized[0].title, "Example Docs");
        assert_eq!(normalized[0].snippet, "Learn more");
        assert_eq!(normalized[0].url, "https://example.com/docs");
    }
}
