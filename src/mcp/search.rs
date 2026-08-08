//! Lexical ranking over the deferred tool catalog.
//!
//! Deliberately not embeddings: tool names and descriptions are short,
//! the corpus is small enough to score exhaustively, and a synchronous
//! pure function keeps `tool_search` a single round trip with no model
//! or index to keep warm.

use std::collections::HashMap;

/// Field weights. Name matches dominate because MCP tool names are
/// hand-written and specific ("create_issue"), while descriptions are
/// often boilerplate that matches everything.
const WEIGHT_NAME: f32 = 6.0;
const WEIGHT_SERVER: f32 = 3.0;
const WEIGHT_TITLE: f32 = 2.5;
const WEIGHT_DESCRIPTION: f32 = 1.0;

/// Anything a query token can match against, pre-tokenized at index time.
#[derive(Debug, Clone, Default)]
pub struct SearchDocument {
    pub name_tokens: Vec<String>,
    pub server_tokens: Vec<String>,
    pub title_tokens: Vec<String>,
    pub description_tokens: Vec<String>,
    /// Lowercased "server tool title description" for substring bonuses.
    pub haystack: String,
}

impl SearchDocument {
    pub fn build(server: &str, name: &str, title: Option<&str>, description: Option<&str>) -> Self {
        let title = title.unwrap_or_default();
        let description = description.unwrap_or_default();
        Self {
            name_tokens: tokenize(name),
            server_tokens: tokenize(server),
            title_tokens: tokenize(title),
            description_tokens: tokenize(description),
            haystack: format!("{} {} {} {}", server, name, title, description).to_lowercase(),
        }
    }
}

/// Split on non-alphanumerics and camelCase boundaries, lowercase, and
/// drop stopwords. `listPullRequests` and `list_pull_requests` must
/// produce the same tokens or half a catalog becomes unsearchable.
pub fn tokenize(text: &str) -> Vec<String> {
    let mut tokens = Vec::new();
    let mut current = String::new();
    let mut previous_lower = false;

    for character in text.chars() {
        if character.is_alphanumeric() {
            if character.is_uppercase() && previous_lower && !current.is_empty() {
                tokens.push(std::mem::take(&mut current));
            }
            current.push(character.to_ascii_lowercase());
            previous_lower = character.is_lowercase() || character.is_numeric();
        } else if !current.is_empty() {
            tokens.push(std::mem::take(&mut current));
            previous_lower = false;
        } else {
            previous_lower = false;
        }
    }
    if !current.is_empty() {
        tokens.push(current);
    }

    tokens.retain(|token| !is_stopword(token));
    tokens
}

fn is_stopword(token: &str) -> bool {
    matches!(
        token,
        "a" | "an"
            | "the"
            | "of"
            | "for"
            | "to"
            | "in"
            | "on"
            | "and"
            | "or"
            | "is"
            | "be"
            | "with"
            | "from"
            | "by"
            | "that"
            | "this"
            | "it"
            | "as"
            | "at"
            | "tool"
            | "mcp"
    )
}

/// Inverse document frequency, so a token every tool shares ("get",
/// "list") contributes far less than a distinctive one ("changelog").
fn build_idf(documents: &[SearchDocument]) -> HashMap<String, f32> {
    let total = documents.len().max(1) as f32;
    let mut counts: HashMap<String, usize> = HashMap::new();

    for document in documents {
        let mut seen: Vec<&str> = Vec::new();
        for token in document
            .name_tokens
            .iter()
            .chain(document.server_tokens.iter())
            .chain(document.title_tokens.iter())
            .chain(document.description_tokens.iter())
        {
            if !seen.contains(&token.as_str()) {
                seen.push(token.as_str());
                *counts.entry(token.clone()).or_insert(0) += 1;
            }
        }
    }

    counts
        .into_iter()
        .map(|(token, count)| {
            let idf = ((total - count as f32 + 0.5) / (count as f32 + 0.5) + 1.0).ln();
            (token, idf.max(0.05))
        })
        .collect()
}

/// Score every document against `query`, returning `(index, score)` for
/// non-zero matches, best first.
pub fn rank(documents: &[SearchDocument], query: &str) -> Vec<(usize, f32)> {
    let query_tokens = tokenize(query);
    let normalized_query = query.trim().to_lowercase();
    if query_tokens.is_empty() && normalized_query.is_empty() {
        return Vec::new();
    }

    let idf = build_idf(documents);
    let mut scored: Vec<(usize, f32)> = Vec::new();

    for (index, document) in documents.iter().enumerate() {
        let mut score = 0.0;

        for token in &query_tokens {
            let weight = idf.get(token).copied().unwrap_or(1.0);
            score += WEIGHT_NAME * weight * field_score(&document.name_tokens, token);
            score += WEIGHT_SERVER * weight * field_score(&document.server_tokens, token);
            score += WEIGHT_TITLE * weight * field_score(&document.title_tokens, token);
            score += WEIGHT_DESCRIPTION * weight * field_score(&document.description_tokens, token);
        }

        // Whole-phrase hit: "create issue" beats two scattered tokens.
        if !normalized_query.is_empty() && document.haystack.contains(&normalized_query) {
            score += 4.0;
        }

        if score > 0.0 {
            scored.push((index, score));
        }
    }

    scored.sort_by(|left, right| {
        right
            .1
            .partial_cmp(&left.1)
            .unwrap_or(std::cmp::Ordering::Equal)
            .then(left.0.cmp(&right.0))
    });
    scored
}

/// Exact token match scores full; a prefix match scores partially so
/// "repo" still finds "repository".
fn field_score(tokens: &[String], query_token: &str) -> f32 {
    let mut best = 0.0f32;
    for token in tokens {
        if token == query_token {
            return 1.0;
        }
        if token.starts_with(query_token) && query_token.len() >= 3 {
            best = best.max(0.6);
        } else if query_token.starts_with(token.as_str()) && token.len() >= 4 {
            best = best.max(0.4);
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<SearchDocument> {
        vec![
            SearchDocument::build(
                "linear",
                "create_issue",
                Some("Create issue"),
                Some("Create a new issue in a Linear team"),
            ),
            SearchDocument::build(
                "linear",
                "listProjects",
                None,
                Some("List all projects visible to the user"),
            ),
            SearchDocument::build(
                "weather",
                "get_forecast",
                None,
                Some("Get the weather forecast for a location"),
            ),
        ]
    }

    #[test]
    fn splits_camel_case_and_snake_case_alike() {
        assert_eq!(tokenize("listPullRequests"), vec!["list", "pull", "requests"]);
        assert_eq!(tokenize("list_pull_requests"), vec!["list", "pull", "requests"]);
    }

    #[test]
    fn ranks_name_matches_above_description_matches() {
        let documents = corpus();
        let ranked = rank(&documents, "create issue");
        assert_eq!(ranked[0].0, 0);
    }

    #[test]
    fn finds_camel_case_tool_by_snake_case_query() {
        let documents = corpus();
        let ranked = rank(&documents, "list projects");
        assert_eq!(ranked[0].0, 1);
    }

    #[test]
    fn matches_by_server_name() {
        let documents = corpus();
        let ranked = rank(&documents, "weather");
        assert_eq!(ranked[0].0, 2);
    }

    #[test]
    fn prefix_queries_still_match() {
        let documents = corpus();
        let ranked = rank(&documents, "forecast");
        assert_eq!(ranked[0].0, 2);
    }

    #[test]
    fn unrelated_queries_return_nothing() {
        assert!(rank(&corpus(), "kubernetes").is_empty());
    }

    #[test]
    fn stopword_only_queries_do_not_match_everything() {
        assert!(rank(&corpus(), "the a of").is_empty());
    }
}
