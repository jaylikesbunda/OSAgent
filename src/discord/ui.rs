//! Rendering helpers shared by the command, panel, and chat surfaces.

use serenity::builder::CreateEmbed;
use serenity::http::Http;
use serenity::model::colour::Colour;
use serenity::model::id::ChannelId;
use tracing::error;

pub(super) const COLOR_PRIMARY: Colour = Colour::from_rgb(124, 129, 141);
pub(super) const COLOR_SUCCESS: Colour = Colour::from_rgb(87, 242, 135);
pub(super) const COLOR_ERROR: Colour = Colour::from_rgb(237, 66, 69);
pub(super) const COLOR_WARNING: Colour = Colour::from_rgb(254, 231, 92);
pub(super) const COLOR_INFO: Colour = Colour::from_rgb(150, 155, 167);

/// Discord caps messages at 2000 characters; leave headroom for the subtext footer.
pub(super) const MESSAGE_LIMIT: usize = 1880;

/// Select menus accept at most 25 options.
pub(super) const SELECT_LIMIT: usize = 25;

pub(super) fn embed(title: &str, description: impl Into<String>, colour: Colour) -> CreateEmbed {
    CreateEmbed::new()
        .title(title)
        .description(description.into())
        .colour(colour)
}

pub(super) fn truncate_chars(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let mut out: String = text.chars().take(max.saturating_sub(1)).collect();
    out.push('…');
    out
}

/// Format a duration for a status line: `840ms`, `12.4s`, `3m 05s`.
pub(super) fn humanize_duration(ms: u64) -> String {
    if ms < 1000 {
        format!("{ms}ms")
    } else if ms < 60_000 {
        format!("{:.1}s", ms as f64 / 1000.0)
    } else {
        format!("{}m {:02}s", ms / 60_000, (ms % 60_000) / 1000)
    }
}

fn push_line(buf: &mut String, line: &str) {
    if !buf.is_empty() && !buf.ends_with('\n') {
        buf.push('\n');
    }
    buf.push_str(line);
}

/// Split a response into Discord-sized messages without severing code fences.
///
/// When a chunk boundary lands inside a fenced block the block is closed at the
/// end of the chunk and reopened (with the same info string) at the start of the
/// next one, so every message renders correctly on its own.
pub(super) fn split_message(text: &str, limit: usize) -> Vec<String> {
    let text = text.trim();
    if text.is_empty() {
        return Vec::new();
    }

    // Room for the fence that may need to be appended/reopened on a split.
    let budget = limit.saturating_sub(8).max(16);
    let mut out: Vec<String> = Vec::new();
    let mut cur = String::new();
    let mut fence: Option<String> = None;

    for line in text.lines() {
        let trimmed = line.trim_start();
        let toggles_fence = trimmed.starts_with("```");

        // A single line can exceed the budget on its own (minified output, long
        // base64 blobs); break it on character boundaries first.
        let mut segments: Vec<String> = Vec::new();
        if line.chars().count() > budget {
            let mut buf = String::new();
            for ch in line.chars() {
                if buf.chars().count() >= budget {
                    segments.push(std::mem::take(&mut buf));
                }
                buf.push(ch);
            }
            if !buf.is_empty() {
                segments.push(buf);
            }
        } else {
            segments.push(line.to_string());
        }

        for segment in segments {
            let projected = cur.chars().count() + segment.chars().count() + 1;
            if !cur.is_empty() && projected > budget {
                let mut chunk = std::mem::take(&mut cur);
                if fence.is_some() {
                    chunk.push_str("\n```");
                }
                out.push(chunk);
                if let Some(info) = &fence {
                    cur.push_str("```");
                    cur.push_str(info);
                    cur.push('\n');
                }
            }
            push_line(&mut cur, &segment);
        }

        if toggles_fence {
            fence = match fence {
                Some(_) => None,
                None => Some(trimmed.trim_start_matches('`').trim().to_string()),
            };
        }
    }

    if !cur.trim().is_empty() {
        if fence.is_some() {
            cur.push_str("\n```");
        }
        out.push(cur);
    }

    out
}

/// Send a response as one or more messages, attaching `footer` as subtext on the last one.
pub(super) async fn send_chunks(
    http: &Http,
    channel_id: ChannelId,
    text: &str,
    footer: Option<&str>,
) {
    let chunks = split_message(text, MESSAGE_LIMIT);
    if chunks.is_empty() {
        return;
    }
    let last = chunks.len() - 1;

    for (index, chunk) in chunks.iter().enumerate() {
        let body = match footer {
            Some(footer) if index == last => {
                format!("{chunk}\n-# {}", truncate_chars(footer, 90))
            }
            _ => chunk.clone(),
        };

        if let Err(e) = channel_id.say(http, body).await {
            error!("Discord: failed to send response chunk: {e}");
            break;
        } else {
            tracing::info!(
                "Discord: sent response chunk {}/{} to channel {}",
                index + 1,
                chunks.len(),
                channel_id.get()
            );
        }
    }
}

/// Turn a raw provider/agent error into a title and an actionable description.
pub(super) fn describe_error(raw: &str) -> (String, String) {
    let lower = raw.to_lowercase();

    let (title, hint) = if lower.contains("401")
        || lower.contains("403")
        || lower.contains("unauthorized")
        || lower.contains("invalid api key")
        || lower.contains("authentication")
    {
        (
            "Provider Rejected the API Key",
            "Check the API key for the active provider in the web UI (Settings → Providers), then retry. `/provider list` shows which providers are connected.",
        )
    } else if (lower.contains("model") && lower.contains("not found"))
        || lower.contains("does not exist")
        || lower.contains("unknown model")
    {
        (
            "Model Unavailable",
            "The active model is not available on this provider. Pick another with `/model set`, or switch providers with `/provider use`.",
        )
    } else if lower.contains("429") || lower.contains("rate limit") {
        (
            "Rate Limited",
            "The provider is throttling this key. Wait a moment and try again.",
        )
    } else if lower.contains("context") && (lower.contains("length") || lower.contains("window")) {
        (
            "Context Window Exceeded",
            "This conversation no longer fits in the model's context window. Start a fresh session with `/session new`, or switch to a larger-context model with `/model set`.",
        )
    } else if lower.contains("timed out") || lower.contains("timeout") {
        (
            "Provider Timed Out",
            "The provider did not respond in time. Retry, or try a faster model with `/model set`.",
        )
    } else if lower.contains("connection")
        || lower.contains("dns")
        || lower.contains("unreachable")
        || lower.contains("tcp connect")
    {
        (
            "Provider Unreachable",
            "Could not connect to the provider endpoint. Check its base URL and your network.",
        )
    } else {
        ("Request Failed", "")
    };

    let mut description = String::new();
    if !hint.is_empty() {
        description.push_str(hint);
        description.push_str("\n\n");
    }
    description.push_str("```\n");
    description.push_str(&truncate_chars(raw.trim(), 1200));
    description.push_str("\n```");

    (title.to_string(), description)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn short_text_stays_in_one_chunk() {
        let chunks = split_message("hello world", 100);
        assert_eq!(chunks, vec!["hello world".to_string()]);
    }

    #[test]
    fn empty_input_produces_no_chunks() {
        assert!(split_message("   \n  ", 100).is_empty());
    }

    #[test]
    fn code_fences_are_reopened_across_chunks() {
        let body: String = (0..40).map(|i| format!("let line_{i} = {i};\n")).collect();
        let text = format!("intro text\n```rust\n{body}```\ntrailing");

        let chunks = split_message(&text, 200);
        assert!(chunks.len() > 1, "expected the sample to split");

        for chunk in &chunks {
            let fences = chunk.matches("```").count();
            assert_eq!(fences % 2, 0, "unbalanced fences in chunk:\n{chunk}");
        }
        assert!(chunks.iter().any(|c| c.contains("```rust")));
    }

    #[test]
    fn oversized_single_line_is_broken_up() {
        let line = "x".repeat(500);
        let chunks = split_message(&line, 100);
        assert!(chunks.len() > 1);
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 100);
        }
    }

    #[test]
    fn multibyte_text_never_panics_and_stays_within_limit() {
        let text = "日本語のテキスト".repeat(80);
        let chunks = split_message(&text, 120);
        assert!(!chunks.is_empty());
        for chunk in &chunks {
            assert!(chunk.chars().count() <= 120);
        }
        let rejoined: String = chunks.concat();
        assert!(rejoined.contains("日本語"));
    }

    #[test]
    fn auth_errors_get_an_actionable_hint() {
        let (title, description) = describe_error("HTTP 401 Unauthorized: invalid api key");
        assert_eq!(title, "Provider Rejected the API Key");
        assert!(description.contains("/provider list"));
    }

    #[test]
    fn unknown_errors_still_include_the_raw_text() {
        let (title, description) = describe_error("something exploded");
        assert_eq!(title, "Request Failed");
        assert!(description.contains("something exploded"));
    }

    #[test]
    fn truncate_is_character_safe() {
        let out = truncate_chars("日本語のテキスト", 4);
        assert_eq!(out.chars().count(), 4);
    }
}
