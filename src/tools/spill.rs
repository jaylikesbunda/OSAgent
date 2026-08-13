/// Tool-result spill: oversized plain-text tool results are persisted
/// verbatim to a session-scoped file and replaced in the model's view
/// with a bounded head/tail preview plus a retrieval hint. Ported from
/// DeepSeek Harness's `spill` / `spill-local` / `spill-policy` family.
///
/// The policy never makes context *bigger*: the notice's byte cost is
/// reserved out of the budget before the preview is sized, so a spilled
/// replacement is always at most `max_inline_bytes` and strictly
/// smaller than the original result.
use crate::config::SpillConfig;
use crate::error::Result;
use crate::tools::registry::ToolResult;
use sha2::{Digest, Sha256};
use std::path::PathBuf;
use tracing::warn;

pub const RETRIEVAL_HINT: &str =
    "Use read with offset/limit, or grep this path to search within it.";

#[derive(Debug, Clone)]
pub struct SpillRef {
    pub path: PathBuf,
    pub bytes: usize,
    pub hint: &'static str,
}

/// Session-scoped storage backend. Files land at
/// `<root>/session-<sha256 prefix>/<random>-<sanitized name>`; the
/// unpredictable prefix plus exclusive create defeats symlink planting,
/// and the session hash groups files so a future cleanup can drop them
/// per session.
pub struct SpillStore {
    root: PathBuf,
}

impl SpillStore {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// Keep one safe path segment: alphanumerics and `-`/`_` survive;
    /// everything else (including separators and dots) collapses to
    /// `_`, so neither traversal (`..`) nor extension tricks survive.
    fn sanitize_segment(name: &str) -> String {
        let sanitized: String = name
            .chars()
            .map(|ch| {
                if ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_') {
                    ch
                } else {
                    '_'
                }
            })
            .take(80)
            .collect();
        if sanitized.is_empty() {
            "output".to_string()
        } else {
            sanitized
        }
    }

    /// Persist text verbatim; the write is exclusive (`create_new`) so
    /// a pre-existing path — symlink or not — fails instead of being
    /// redirected. Rejects on a real storage failure.
    pub fn save(&self, session_id: &str, suggested_name: &str, content: &str) -> Result<SpillRef> {
        std::fs::create_dir_all(&self.root)?;

        let digest = Sha256::digest(session_id.as_bytes());
        let digest_prefix: String = digest[..8]
            .iter()
            .map(|byte| format!("{:02x}", byte))
            .collect();
        let session_dir = self.root.join(format!("session-{}", digest_prefix));
        std::fs::create_dir_all(&session_dir)?;

        let random_prefix = uuid::Uuid::new_v4().simple().to_string();
        let file_name = format!(
            "{}-{}",
            &random_prefix[..12],
            Self::sanitize_segment(suggested_name)
        );
        let path = session_dir.join(file_name);

        let bytes = content.as_bytes();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)?;
        std::io::Write::write_all(&mut file, bytes)?;

        Ok(SpillRef {
            path,
            bytes: bytes.len(),
            hint: RETRIEVAL_HINT,
        })
    }
}

fn build_head_tail_preview(content: &str, head_budget: usize, tail_budget: usize) -> String {
    const MIDDLE_MARKER: &str =
        "\n\n[... middle content omitted - full result stored in the spill file ...]\n\n";
    let budget = head_budget + tail_budget;
    let marker_budget = budget.min(MIDDLE_MARKER.len());
    let usable = budget.saturating_sub(marker_budget);
    let head = usable / 2;
    let tail = usable - head;
    if content.chars().count() <= head + tail {
        return content.to_string();
    }
    let head_text: String = content.chars().take(head).collect();
    let tail_text: String = content
        .chars()
        .rev()
        .take(tail)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{}{}{}", head_text, MIDDLE_MARKER, tail_text)
}

/// Post-execution spill policy. Returns `true` when the result was
/// spilled and replaced with a bounded preview.
///
/// Skips: oversized results when disabled, `read_file` (prevents a
/// `read -> spill -> read again` loop), and anything at or under the
/// cap. Failures are best-effort: a rejected save logs a warning and
/// keeps the inline result.
pub fn maybe_spill_tool_result(
    store: &SpillStore,
    session_id: &str,
    tool_name: &str,
    result: &mut ToolResult,
    config: &SpillConfig,
) -> bool {
    if !config.enabled {
        return false;
    }
    if tool_name == "read_file" {
        return false;
    }
    let max_inline_bytes = config.max_inline_bytes;
    if result.output.len() <= max_inline_bytes {
        return false;
    }

    let spill = match store.save(session_id, tool_name, &result.output) {
        Ok(spill) => spill,
        Err(error) => {
            warn!(
                "Spill failed for {} in session {}: {}; keeping inline result",
                tool_name, session_id, error
            );
            return false;
        }
    };

    let notice = format!(
        "\n(Omitted {} bytes. Full formatted result stored at: {}. {})",
        spill.bytes,
        spill.path.display(),
        spill.hint
    );
    let budget = max_inline_bytes.saturating_sub(notice.len());
    let head_budget = budget / 2;
    let tail_budget = budget.saturating_sub(head_budget);

    let preview = build_head_tail_preview(&result.output, head_budget, tail_budget);
    result.output = format!("{}{}", preview, notice);
    if let Some(meta) = result.metadata.as_object_mut() {
        meta.insert("spilled".to_string(), serde_json::json!(true));
        meta.insert(
            "spill_path".to_string(),
            serde_json::json!(spill.path.to_string_lossy()),
        );
        meta.insert("spill_bytes".to_string(), serde_json::json!(spill.bytes));
    }
    true
}

pub fn resolve_spill_root(root: &str) -> Result<PathBuf> {
    let expanded = shellexpand::tilde(root).to_string();
    Ok(PathBuf::from(expanded))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn store() -> (SpillStore, tempfile::TempDir) {
        let temp = tempfile::tempdir().unwrap();
        (SpillStore::new(temp.path().to_path_buf()), temp)
    }

    fn config(max_inline_bytes: usize) -> SpillConfig {
        SpillConfig {
            enabled: true,
            root: String::new(),
            max_inline_bytes,
        }
    }

    #[test]
    fn spill_round_trips_content_verbatim() {
        let (store, _temp) = store();
        let content = "line one\n".repeat(10_000);
        let spill = store.save("session-1", "web_fetch", &content).unwrap();
        let loaded = std::fs::read_to_string(&spill.path).unwrap();
        assert_eq!(loaded, content);
        assert_eq!(spill.bytes, content.len());
    }

    #[test]
    fn spilled_replacement_never_exceeds_cap() {
        let (store, _temp) = store();
        let mut result = ToolResult::new("x".repeat(100_000));
        let spilled =
            maybe_spill_tool_result(&store, "session-1", "bash", &mut result, &config(24_000));
        assert!(spilled);
        assert!(result.output.len() <= 24_000);
        assert!(result.output.contains("Omitted"));
        assert_eq!(result.metadata["spilled"], json!(true));
    }

    #[test]
    fn under_cap_results_pass_through() {
        let (store, _temp) = store();
        let mut result = ToolResult::new("small".to_string());
        let spilled =
            maybe_spill_tool_result(&store, "session-1", "bash", &mut result, &config(100));
        assert!(!spilled);
        assert_eq!(result.output, "small");
    }

    #[test]
    fn read_file_is_never_spilled() {
        let (store, _temp) = store();
        let mut result = ToolResult::new("x".repeat(100_000));
        let spilled =
            maybe_spill_tool_result(&store, "session-1", "read_file", &mut result, &config(100));
        assert!(!spilled);
        assert_eq!(result.output.len(), 100_000);
    }

    #[test]
    fn disabled_policy_passes_through() {
        let (store, _temp) = store();
        let mut result = ToolResult::new("x".repeat(100_000));
        let mut cfg = config(100);
        cfg.enabled = false;
        let spilled = maybe_spill_tool_result(&store, "session-1", "bash", &mut result, &cfg);
        assert!(!spilled);
    }

    #[test]
    fn save_fails_on_existing_path() {
        let (store, _temp) = store();
        let first = store.save("session-1", "bash", "content").unwrap();
        assert!(std::fs::write(&first.path, "clobbered").is_ok());
        let again = store.save("session-1", "bash", "content");
        assert!(again.is_ok());
        assert_ne!(again.unwrap().path, first.path);
    }

    #[test]
    fn sanitize_strips_separators_and_dots() {
        assert_eq!(SpillStore::sanitize_segment("a/b\\c"), "a_b_c");
        assert_eq!(SpillStore::sanitize_segment(".."), "__");
        assert_eq!(SpillStore::sanitize_segment("out.txt"), "out_txt");
    }
}
