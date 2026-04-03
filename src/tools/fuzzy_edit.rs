use std::cmp::min;

#[derive(Debug, Clone)]
pub struct MatchResult {
    pub start: usize,
    pub end: usize,
    pub strategy: &'static str,
    pub confidence: f64,
}

pub trait MatchStrategy: Send + Sync {
    fn name(&self) -> &'static str;
    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult>;
}

pub struct ExactMatcher;

impl MatchStrategy for ExactMatcher {
    fn name(&self) -> &'static str {
        "exact"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let start = content.find(old_text)?;
        Some(MatchResult {
            start,
            end: start + old_text.len(),
            strategy: self.name(),
            confidence: 1.0,
        })
    }
}

pub struct LineTrimmedMatcher;

impl MatchStrategy for LineTrimmedMatcher {
    fn name(&self) -> &'static str {
        "line_trimmed"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_text.lines().collect();

        if old_lines.is_empty() || content_lines.len() < old_lines.len() {
            return None;
        }

        let content_trimmed: Vec<&str> = content_lines.iter().map(|l| l.trim()).collect();
        let old_trimmed: Vec<&str> = old_lines.iter().map(|l| l.trim()).collect();

        for window_start in 0..=(content_trimmed.len() - old_trimmed.len()) {
            if content_trimmed[window_start..window_start + old_trimmed.len()] == old_trimmed[..] {
                let byte_start = line_byte_offset(content, window_start)?;
                let byte_end = line_byte_offset(content, window_start + old_lines.len())
                    .unwrap_or(content.len());
                return Some(MatchResult {
                    start: byte_start,
                    end: byte_end,
                    strategy: self.name(),
                    confidence: 0.95,
                });
            }
        }

        None
    }
}

pub struct WhitespaceNormalizedMatcher;

impl MatchStrategy for WhitespaceNormalizedMatcher {
    fn name(&self) -> &'static str {
        "whitespace_normalized"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let norm_content = normalize_whitespace(content);
        let norm_old = normalize_whitespace(old_text);

        let start = norm_content.find(&norm_old)?;
        Some(MatchResult {
            start,
            end: start + norm_old.len(),
            strategy: self.name(),
            confidence: 0.85,
        })
    }
}

pub struct IndentationFlexibleMatcher;

impl MatchStrategy for IndentationFlexibleMatcher {
    fn name(&self) -> &'static str {
        "indentation_flexible"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_text.lines().collect();

        if old_lines.is_empty() || content_lines.len() < old_lines.len() {
            return None;
        }

        for window_start in 0..=(content_lines.len() - old_lines.len()) {
            if lines_match_indent_flexible(
                &content_lines[window_start..window_start + old_lines.len()],
                &old_lines,
            ) {
                let byte_start = line_byte_offset(content, window_start)?;
                let byte_end = line_byte_offset(content, window_start + old_lines.len())
                    .unwrap_or(content.len());
                return Some(MatchResult {
                    start: byte_start,
                    end: byte_end,
                    strategy: self.name(),
                    confidence: 0.95,
                });
            }
        }

        None
    }
}

pub struct BlockAnchorMatcher {
    pub min_similarity: f64,
}

impl MatchStrategy for BlockAnchorMatcher {
    fn name(&self) -> &'static str {
        "block_anchor"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_text.lines().collect();

        if old_lines.is_empty() || content_lines.len() < old_lines.len() {
            return None;
        }

        let old_len = old_lines.len();
        let best = (0..=(content_lines.len() - old_len))
            .map(|window_start| {
                let window = &content_lines[window_start..window_start + old_len];
                let sim = line_block_similarity(window, &old_lines);
                (window_start, sim)
            })
            .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap_or(std::cmp::Ordering::Equal))?;

        if best.1 >= self.min_similarity {
            let byte_start = line_byte_offset(content, best.0)?;
            let byte_end = line_byte_offset(content, best.0 + old_len).unwrap_or(content.len());
            return Some(MatchResult {
                start: byte_start,
                end: byte_end,
                strategy: self.name(),
                confidence: best.1,
            });
        }

        None
    }
}

pub struct TrimmedBoundaryMatcher;

impl MatchStrategy for TrimmedBoundaryMatcher {
    fn name(&self) -> &'static str {
        "trimmed_boundary"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let content_trimmed = content.trim();
        let old_trimmed = old_text.trim();

        let start = content_trimmed.find(old_trimmed)?;
        let byte_start = content.len() - content_trimmed.len() + start;
        let byte_end = byte_start + old_trimmed.len();
        Some(MatchResult {
            start: byte_start,
            end: byte_end,
            strategy: self.name(),
            confidence: 0.75,
        })
    }
}

pub struct ContextAwareMatcher;

impl MatchStrategy for ContextAwareMatcher {
    fn name(&self) -> &'static str {
        "context_aware"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let content_lines: Vec<&str> = content.lines().collect();
        let old_lines: Vec<&str> = old_text.lines().collect();

        if old_lines.len() < 3 || content_lines.len() < old_lines.len() {
            return None;
        }

        let first_old = old_lines.first()?.trim();
        let last_old = old_lines.last()?.trim();

        if first_old.is_empty() || last_old.is_empty() {
            return None;
        }

        for window_start in 0..=(content_lines.len() - old_lines.len()) {
            let window_end = window_start + old_lines.len();
            let first_content = content_lines[window_start].trim();
            let last_content = content_lines[window_end - 1].trim();

            if first_content == first_old && last_content == last_old {
                let matched = &content_lines[window_start..window_end];
                let matching_lines = matched
                    .iter()
                    .zip(old_lines.iter())
                    .filter(|(a, b)| a.trim() == b.trim())
                    .count();
                let similarity = matching_lines as f64 / old_lines.len() as f64;

                if similarity >= 0.6 {
                    let byte_start = line_byte_offset(content, window_start)?;
                    let byte_end = line_byte_offset(content, window_end).unwrap_or(content.len());
                    return Some(MatchResult {
                        start: byte_start,
                        end: byte_end,
                        strategy: self.name(),
                        confidence: similarity,
                    });
                }
            }
        }

        None
    }
}

pub struct EscapeNormalizedMatcher;

impl MatchStrategy for EscapeNormalizedMatcher {
    fn name(&self) -> &'static str {
        "escape_normalized"
    }

    fn find(&self, content: &str, old_text: &str) -> Option<MatchResult> {
        let norm_content = normalize_escapes(content);
        let norm_old = normalize_escapes(old_text);

        if norm_content == norm_old {
            return Some(MatchResult {
                start: 0,
                end: content.len(),
                strategy: self.name(),
                confidence: 0.80,
            });
        }

        let start = norm_content.find(&norm_old)?;
        Some(MatchResult {
            start,
            end: start + norm_old.len(),
            strategy: self.name(),
            confidence: 0.80,
        })
    }
}

pub fn fuzzy_find(content: &str, old_text: &str) -> Option<MatchResult> {
    let strategies: Vec<Box<dyn MatchStrategy>> = vec![
        Box::new(ExactMatcher),
        Box::new(LineTrimmedMatcher),
        Box::new(WhitespaceNormalizedMatcher),
        Box::new(IndentationFlexibleMatcher),
        Box::new(EscapeNormalizedMatcher),
        Box::new(TrimmedBoundaryMatcher),
        Box::new(BlockAnchorMatcher {
            min_similarity: 0.75,
        }),
        Box::new(ContextAwareMatcher),
    ];

    for strategy in &strategies {
        if let Some(result) = strategy.find(content, old_text) {
            return Some(result);
        }
    }

    None
}

pub fn apply_replacement(
    content: &str,
    match_result: &MatchResult,
    old_text: &str,
    new_text: &str,
) -> String {
    if match_result.strategy == "exact" {
        return content.replacen(old_text, new_text, 1);
    }

    let mut result = String::with_capacity(content.len() + new_text.len());
    result.push_str(&content[..match_result.start]);
    result.push_str(new_text);
    result.push_str(&content[match_result.end..]);
    result
}

fn line_byte_offset(content: &str, line_index: usize) -> Option<usize> {
    if line_index == 0 {
        return Some(0);
    }
    content
        .lines()
        .nth(line_index)
        .map(|line| line.as_ptr() as usize - content.as_ptr() as usize)
}

fn normalize_whitespace(text: &str) -> String {
    let mut result = String::with_capacity(text.len());
    let mut prev_was_space = false;
    for ch in text.chars() {
        if ch.is_whitespace() {
            if !prev_was_space {
                result.push(' ');
                prev_was_space = true;
            }
        } else {
            result.push(ch);
            prev_was_space = false;
        }
    }
    result.trim().to_string()
}

fn normalize_escapes(text: &str) -> String {
    text.replace("\\n", "\n")
        .replace("\\t", "\t")
        .replace("\\r", "\r")
        .replace("\\\"", "\"")
        .replace("\\'", "'")
        .replace("\\\\", "\\")
}

fn lines_match_indent_flexible(content_lines: &[&str], old_lines: &[&str]) -> bool {
    if content_lines.len() != old_lines.len() {
        return false;
    }

    let content_indents: Vec<usize> = content_lines.iter().map(|s| leading_spaces(s)).collect();
    let old_indents: Vec<usize> = old_lines.iter().map(|s| leading_spaces(s)).collect();

    let min_content = content_indents.iter().min().copied().unwrap_or(0);
    let min_old = old_indents.iter().min().copied().unwrap_or(0);

    for (i, (c_line, o_line)) in content_lines.iter().zip(old_lines.iter()).enumerate() {
        let c_adjusted = content_indents[i] - min_content;
        let o_adjusted = old_indents[i] - min_old;
        let c_content = c_line.trim_start();
        let o_content = o_line.trim_start();

        if c_content != o_content {
            return false;
        }
        if c_adjusted != o_adjusted {
            return false;
        }
    }

    true
}

fn leading_spaces(s: &str) -> usize {
    s.chars().take_while(|c| c.is_whitespace()).count()
}

fn line_block_similarity(window: &[&str], old: &[&str]) -> f64 {
    if window.len() != old.len() {
        return 0.0;
    }

    let matching = window
        .iter()
        .zip(old.iter())
        .filter(|(a, b)| a.trim() == b.trim())
        .count();

    let line_sim = matching as f64 / old.len() as f64;

    let char_sim = {
        let w_joined: String = window.iter().map(|l| l.trim()).collect::<Vec<_>>().join("");
        let o_joined: String = old.iter().map(|l| l.trim()).collect::<Vec<_>>().join("");
        levenshtein_similarity(&w_joined, &o_joined)
    };

    0.5 * line_sim + 0.5 * char_sim
}

fn levenshtein_similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() && b.is_empty() {
        return 1.0;
    }
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }

    let distance = levenshtein_distance(a, b);
    let max_len = min(a.len(), b.len()).max(1);
    1.0 - (distance as f64 / (max_len as f64 + distance as f64))
}

fn levenshtein_distance(a: &str, b: &str) -> usize {
    let a_chars: Vec<char> = a.chars().collect();
    let b_chars: Vec<char> = b.chars().collect();
    let a_len = a_chars.len();
    let b_len = b_chars.len();

    if a_len == 0 {
        return b_len;
    }
    if b_len == 0 {
        return a_len;
    }

    let mut prev: Vec<usize> = (0..=b_len).collect();
    let mut curr: Vec<usize> = vec![0; b_len + 1];

    for i in 1..=a_len {
        curr[0] = i;
        for j in 1..=b_len {
            let cost = if a_chars[i - 1] == b_chars[j - 1] {
                0
            } else {
                1
            };
            curr[j] = (prev[j] + 1).min(curr[j - 1] + 1).min(prev[j - 1] + cost);
        }
        std::mem::swap(&mut prev, &mut curr);
    }

    prev[b_len]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exact_match() {
        let content = "hello world\nfoo bar\nbaz";
        let old = "foo bar";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "exact");
        assert_eq!(&content[result.start..result.end], old);
    }

    #[test]
    fn line_trimmed_match() {
        let content = "hello world\n  foo bar  \nbaz";
        let old = "foo bar";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "line_trimmed");
    }

    #[test]
    fn whitespace_normalized_match() {
        let content = "hello   world  foo bar";
        let old = "hello world foo    bar";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "whitespace_normalized");
    }

    #[test]
    fn indentation_flexible_match() {
        let content = "fn main() {\n    println!(\"hello\");\n}";
        let old = "fn main() {\nprintln!(\"hello\");\n}";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "indentation_flexible");
    }

    #[test]
    fn no_match_returns_none() {
        let content = "hello world";
        let old = "xyz not found at all";
        assert!(fuzzy_find(content, old).is_none());
    }

    #[test]
    fn block_anchor_match() {
        let content = "line 1\nline two with typo\nline 3";
        let old = "line 1\nline 2 with typo\nline 3";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "block_anchor");
        assert!(result.confidence >= 0.75);
    }

    #[test]
    fn context_aware_match() {
        let content = "fn foo() {\n    let x = 1;\n    let y = 2;\n    x + y\n}";
        let old = "fn foo() {\n    let x = 1;\n    let y = 3;\n    x + y\n}";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "context_aware");
    }

    #[test]
    fn trimmed_boundary_match() {
        let content = "  hello world  ";
        let old = "hello world";
        let result = fuzzy_find(content, old).unwrap();
        assert_eq!(result.strategy, "trimmed_boundary");
    }

    #[test]
    fn apply_replacement_exact() {
        let content = "hello world";
        let old = "world";
        let mr = MatchResult {
            start: 6,
            end: 11,
            strategy: "exact",
            confidence: 1.0,
        };
        let result = apply_replacement(content, &mr, old, "universe");
        assert_eq!(result, "hello universe");
    }

    #[test]
    fn apply_replacement_fuzzy() {
        let content = "hello world";
        let mr = MatchResult {
            start: 6,
            end: 11,
            strategy: "line_trimmed",
            confidence: 0.9,
        };
        let result = apply_replacement(content, &mr, "world", "universe");
        assert_eq!(result, "hello universe");
    }

    #[test]
    fn levenshtein_distance_basic() {
        assert_eq!(levenshtein_distance("kitten", "sitting"), 3);
        assert_eq!(levenshtein_distance("", "abc"), 3);
        assert_eq!(levenshtein_distance("abc", "abc"), 0);
    }
}
