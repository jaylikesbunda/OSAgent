//! Query-term extraction for the codesearch (quick-context) tool.
//!
//! Splits a natural-language or identifier-style query into distinctive,
//! lowercase terms: camelCase/snake_case boundaries are split, comments and
//! noise are ignored, and generic programming keywords are dropped so the
//! remaining terms can be fanned out as grep patterns.

use regex::Regex;
use std::collections::HashSet;

lazy_static::lazy_static! {
    static ref CAMEL_CASE: Regex = Regex::new(r"([a-z])([A-Z])").unwrap();
    static ref SNAKE_CASE: Regex = Regex::new(r"_+").unwrap();
    static ref NON_ALPHANUM: Regex = Regex::new(r"[^a-zA-Z0-9]+").unwrap();

    /// Words too generic to be useful as search patterns on their own.
    static ref STOPWORDS: HashSet<&'static str> = {
        let set: HashSet<&'static str> = [
            // language keywords (never distinctive)
            "fn", "function", "func", "def", "class", "struct", "enum", "interface",
            "let", "const", "var", "val", "mut", "static", "final", "public", "private",
            "return", "yield", "await", "async", "sync", "if", "else", "elif", "for",
            "while", "loop", "match", "switch", "case", "break", "continue", "goto",
            "import", "export", "use", "require", "include", "from", "package", "mod",
            "impl", "trait", "extends", "implements", "where", "type", "alias", "typedef",
            "new", "delete", "this", "self", "super", "parent",
            "true", "false", "null", "nil", "none", "undefined", "void",
            "int", "float", "double", "string", "bool", "boolean", "char", "byte",
            // filler words common in natural-language queries
            "the", "and", "for", "with", "how", "does", "do", "is", "are", "was",
            "were", "what", "when", "where", "which", "who", "why", "that", "this",
            "into", "onto", "from", "code", "file", "files", "find", "show", "me",
            "get", "set", "put", "add", "remove", "make", "create", "using", "used", "uses",
            "can", "should", "would", "will",
        ].iter().cloned().collect();
        set
    };
}

/// Extract distinctive lowercase search terms from a query.
///
/// Terms are deduplicated and returned sorted by length (longest first) so
/// the most distinctive terms lead when results are capped.
pub fn extract_query_terms(query: &str) -> Vec<String> {
    let mut terms: Vec<String> = Vec::new();

    for word in NON_ALPHANUM.split(query) {
        if word.is_empty() {
            continue;
        }

        let split_camel = CAMEL_CASE.replace_all(word, "$1 $2");
        for split_word in split_camel.split_whitespace() {
            let lower = split_word.to_lowercase();

            if lower.len() < 3 || STOPWORDS.contains(lower.as_str()) {
                continue;
            }

            if lower.contains('_') {
                for part in SNAKE_CASE.split(&lower) {
                    if part.len() >= 3 && !STOPWORDS.contains(part) && !terms.contains(&part.to_string()) {
                        terms.push(part.to_string());
                    }
                }
            } else if !terms.contains(&lower) {
                terms.push(lower);
            }
        }
    }

    terms.sort_by_key(|t| std::cmp::Reverse(t.len()));
    terms
}

/// camelCase join of terms, e.g. `["process", "data"]` -> `processData`.
pub fn camel_case_join(terms: &[String]) -> Option<String> {
    let mut out = String::new();
    for (i, term) in terms.iter().take(4).enumerate() {
        if i == 0 {
            out.push_str(term);
        } else {
            let mut chars = term.chars();
            if let Some(first) = chars.next() {
                out.extend(first.to_uppercase());
                out.push_str(chars.as_str());
            }
        }
    }
    if terms.len() >= 2 {
        Some(out)
    } else {
        None
    }
}

/// snake_case join of terms, e.g. `["process", "data"]` -> `process_data`.
pub fn snake_case_join(terms: &[String]) -> Option<String> {
    if terms.len() >= 2 {
        Some(terms.iter().take(4).cloned().collect::<Vec<_>>().join("_"))
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn splits_and_filters_terms() {
        let terms = extract_query_terms("how does the RetryHandler handle retries?");
        assert!(terms.contains(&"retry".to_string()));
        assert!(terms.contains(&"handler".to_string()));
        assert!(terms.contains(&"retries".to_string()));
        assert!(!terms.contains(&"how".to_string()));
        assert!(!terms.contains(&"the".to_string()));
        assert!(!terms.contains(&"does".to_string()));
    }

    #[test]
    fn longest_terms_first() {
        let terms = extract_query_terms("auth middleware token");
        assert_eq!(terms.first().map(|s| s.as_str()), Some("middleware"));
    }

    #[test]
    fn drops_short_fragments() {
        let terms = extract_query_terms("a an the io x509");
        assert_eq!(terms, vec!["x509".to_string()]);
    }

    #[test]
    fn joins_variants() {
        let terms = vec!["process".to_string(), "data".to_string()];
        assert_eq!(camel_case_join(&terms).as_deref(), Some("processData"));
        assert_eq!(snake_case_join(&terms).as_deref(), Some("process_data"));
    }

    #[test]
    fn no_join_for_single_term() {
        let terms = vec!["process".to_string()];
        assert!(camel_case_join(&terms).is_none());
        assert!(snake_case_join(&terms).is_none());
    }
}
