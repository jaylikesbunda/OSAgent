/// Tool result truncation: prevents oversized tool results from consuming context.
///
/// Ported from OpenClaw's `tool-result-truncation.ts`.
/// Uses a head+tail strategy when the tail contains important content
/// (errors, JSON closing, summaries), otherwise preserves the beginning.

/// Maximum share of the context window a single tool result should occupy.
const MAX_TOOL_RESULT_CONTEXT_SHARE: f32 = 0.3;

/// Hard character limit for a single tool result text block (~100K tokens).
pub const HARD_MAX_TOOL_RESULT_CHARS: usize = 400_000;

/// Minimum characters to keep when truncating.
const MIN_KEEP_CHARS: usize = 2_000;

const TRUNCATION_SUFFIX: &str =
    "\n\n⚠️ [Content truncated — original was too large for the model's context window. \
     The content above is a partial view. If you need more, request specific sections or use \
     offset/limit parameters to read smaller chunks.]";

const MIDDLE_OMISSION_MARKER: &str =
    "\n\n⚠️ [... middle content omitted — showing head and tail ...]\n\n";

/// Options for truncation behavior.
#[derive(Debug, Clone, Default)]
pub struct TruncationOptions {
    pub suffix: Option<String>,
    pub min_keep_chars: Option<usize>,
}

/// Detect whether text likely contains error/diagnostic content near the end,
/// which should be preserved during truncation.
fn has_important_tail(text: &str) -> bool {
    let tail_start = text.len().saturating_sub(2000);
    let tail = &text[tail_start..];
    let tail_lower = tail.to_lowercase();

    // Error-like patterns
    tail_lower.contains("error")
        || tail_lower.contains("exception")
        || tail_lower.contains("failed")
        || tail_lower.contains("fatal")
        || tail_lower.contains("traceback")
        || tail_lower.contains("panic")
        || tail_lower.contains("stack trace")
        || tail_lower.contains("errno")
        || tail_lower.contains("exit code")
        // JSON closing
        || tail.trim().ends_with('}')
        // Summary/result lines
        || tail_lower.contains("total")
        || tail_lower.contains("summary")
        || tail_lower.contains("result")
        || tail_lower.contains("complete")
        || tail_lower.contains("finished")
        || tail_lower.contains("done")
}

/// Find a clean cut point at or before `budget`, preferring newline boundaries.
fn find_clean_cut(text: &str, budget: usize) -> usize {
    if budget >= text.len() {
        return text.len();
    }
    let budget = budget.min(text.len());

    // Look for a newline near the budget point
    if let Some(newline_pos) = text[..budget].rfind('\n') {
        if newline_pos > (budget as f32 * 0.8) as usize {
            return newline_pos;
        }
    }
    budget
}

/// Find a clean tail start at or after `start`, preferring newline boundaries.
fn find_clean_tail_start(text: &str, start: usize) -> usize {
    if start >= text.len() {
        return text.len();
    }
    if let Some(newline_pos) = text[start..].find('\n') {
        let abs_pos = start + newline_pos + 1;
        if abs_pos < start + ((text.len() - start) as f32 * 0.2) as usize + newline_pos {
            return abs_pos;
        }
    }
    start
}

/// Truncate a single text string to fit within `max_chars`.
///
/// Uses a head+tail strategy when the tail contains important content,
/// otherwise preserves the beginning.
pub fn truncate_tool_result_text(
    text: &str,
    max_chars: usize,
    options: &TruncationOptions,
) -> String {
    let suffix = options.suffix.as_deref().unwrap_or(TRUNCATION_SUFFIX);
    let min_keep = options.min_keep_chars.unwrap_or(MIN_KEEP_CHARS);

    if text.len() <= max_chars {
        return text.to_string();
    }

    let budget = max_chars.saturating_sub(suffix.len()).max(min_keep);

    // Head+tail strategy when tail looks important
    if has_important_tail(text) && budget > min_keep * 2 {
        let tail_budget = (budget / 3).min(4_000);
        let head_budget = budget
            .saturating_sub(tail_budget)
            .saturating_sub(MIDDLE_OMISSION_MARKER.len());

        if head_budget > min_keep {
            let head_cut = find_clean_cut(text, head_budget);
            let tail_start = find_clean_tail_start(text, text.len() - tail_budget);

            let mut result = String::with_capacity(max_chars);
            result.push_str(&text[..head_cut]);
            result.push_str(MIDDLE_OMISSION_MARKER);
            result.push_str(&text[tail_start..]);
            result.push_str(suffix);
            return result;
        }
    }

    // Default: keep the beginning
    let cut = find_clean_cut(text, budget);
    let mut result = String::with_capacity(cut + suffix.len());
    result.push_str(&text[..cut]);
    result.push_str(suffix);
    result
}

/// Calculate the maximum allowed characters for a single tool result
/// based on the model's context window tokens.
pub fn calculate_max_tool_result_chars(context_window_tokens: usize) -> usize {
    let max_tokens = (context_window_tokens as f32 * MAX_TOOL_RESULT_CONTEXT_SHARE) as usize;
    // ~4 chars per token heuristic
    let max_chars = max_tokens * 4;
    max_chars.min(HARD_MAX_TOOL_RESULT_CHARS)
}

/// Check if a tool result text exceeds the size limit.
pub fn is_tool_result_oversized(text: &str, context_window_tokens: usize) -> bool {
    let max_chars = calculate_max_tool_result_chars(context_window_tokens);
    text.len() > max_chars
}

/// Truncate tool result output if it exceeds the limit.
/// Returns the original string if under the limit, or a truncated version.
pub fn maybe_truncate_tool_result(
    output: &str,
    context_window_tokens: usize,
    options: &TruncationOptions,
) -> String {
    let max_chars = calculate_max_tool_result_chars(context_window_tokens);
    truncate_tool_result_text(output, max_chars, options)
}

/// Summarize a tool output for context storage, applying size limits.
/// Used in the main agent loop to keep session messages manageable.
pub fn summarize_tool_output_for_context(
    _tool_name: &str,
    output: &str,
    context_window_tokens: Option<usize>,
) -> String {
    // Apply context-based truncation first if we know the window size
    let processed = if let Some(tokens) = context_window_tokens {
        let max_chars = calculate_max_tool_result_chars(tokens);
        if output.len() > max_chars {
            truncate_tool_result_text(output, max_chars, &TruncationOptions::default())
        } else {
            output.to_string()
        }
    } else {
        output.to_string()
    };

    // Secondary safety cap for session storage
    const SESSION_STORAGE_LIMIT: usize = 50_000;
    if processed.len() > SESSION_STORAGE_LIMIT {
        truncate_tool_result_text(
            &processed,
            SESSION_STORAGE_LIMIT,
            &TruncationOptions {
                suffix: Some(
                    "\n\n[Truncated for session storage — full output exceeded limits]".to_string(),
                ),
                min_keep_chars: Some(5_000),
            },
        )
    } else {
        processed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn no_truncation_when_under_limit() {
        let text = "hello world";
        let result = truncate_tool_result_text(text, 1000, &TruncationOptions::default());
        assert_eq!(result, text);
    }

    #[test]
    fn truncates_with_suffix() {
        let text = "a".repeat(10_000);
        let result = truncate_tool_result_text(&text, 1000, &TruncationOptions::default());
        assert!(result.len() <= 1000 + 200, // suffix adds some chars
        assert!(result.contains("Content truncated"));
    }

    #[test]
    fn head_tail_preserves_errors() {
        let mut head = String::new();
        let tail = "\nError: compilation failed at line 42\nSummary: 3 errors found";
        for _ in 0..99 {
            head.push_str(&head);
        }
        let text = format!("{}{}", head, tail);
        let result = truncate_tool_result_text(&text, 500, &TruncationOptions::default());
        // Should preserve Error: and Summary in the tail
        assert!(result.contains("Error:"));
        assert!(result.contains("middle content omitted"));
        // Should be able to fit in 500 chars
        let suffix_len = result.len() - 500;
        assert!(suffix_len <= 300, "Suffix should fit");
        assert!(result.contains("Content truncated"));
    }
}

    #[test]
    fn head_tail_preserves_errors() {
        let head = "Building project...\n".repeat(100);
        let tail = "\nError: compilation failed at line 42\nSummary: 3 errors found";
        let text = format!("{head}{tail}");
        let result = truncate_tool_result_text(&text, 500, &TruncationOptions::default());
        assert!(result.contains("Error:"));
        assert!(result.contains("middle content omitted"));
    }

    #[test]
    fn calculates_max_chars_from_context_window() {
        let max = calculate_max_tool_result_chars(128_000);
        // 30% of 128K = 38400 tokens * 4 = 153600 chars, under hard cap
        assert_eq!(max, 153_600);
    }

    #[test]
    fn respects_hard_cap() {
        let max = calculate_max_tool_result_chars(2_000_000);
        assert_eq!(max, HARD_MAX_TOOL_RESULT_CHARS);
    }

    #[test]
    fn detects_oversized() {
        let text = "x".repeat(500_000);
        assert!(is_tool_result_oversized(&text, 128_000));
        assert!(!is_tool_result_oversized("small", 128_000));
    }

    #[test]
    fn important_tail_detection() {
        assert!(has_important_tail("some output\nError: something failed"));
        assert!(has_important_tail("json output: {\"key\": \"val\"}"));
        assert!(has_important_tail("processing...\nSummary: done"));
        assert!(!has_important_tail("just normal output here"));
    }

    #[test]
    fn clean_cut_at_newline() {
        let text = "line1\nline2\nline3\nline4";
        let cut = find_clean_cut(text, 14);
        assert_eq!(cut, 11); // at the second newline
    }
}
