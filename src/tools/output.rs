use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use chrono::Utc;
use uuid::Uuid;

pub const TOOL_OUTPUT_DIR_NAME: &str = ".osa_tool_outputs";
const MAX_INLINE_LINES: usize = 200;
const MAX_INLINE_CHARS: usize = 12_000;
const RETENTION_AGE: Duration = Duration::from_secs(7 * 24 * 60 * 60);

fn sanitize_source(source: &str) -> String {
    let sanitized: String = source
        .chars()
        .map(|ch| {
            if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                ch
            } else {
                '_'
            }
        })
        .collect();

    sanitized.trim_matches('_').to_string()
}

fn build_output_path(workspace: &Path, source: &str) -> PathBuf {
    let prefix = sanitize_source(source);
    let prefix = if prefix.is_empty() {
        "tool_output".to_string()
    } else {
        prefix
    };

    workspace.join(TOOL_OUTPUT_DIR_NAME).join(format!(
        "{}_{}_{}.log",
        prefix,
        Utc::now().format("%Y%m%d_%H%M%S"),
        Uuid::new_v4().simple()
    ))
}

pub fn path_touches_tool_outputs(path: &Path) -> bool {
    path.components().any(|component| {
        component
            .as_os_str()
            .to_string_lossy()
            .eq_ignore_ascii_case(TOOL_OUTPUT_DIR_NAME)
    })
}

fn cleanup_old_outputs(workspace: &Path) {
    let output_dir = workspace.join(TOOL_OUTPUT_DIR_NAME);
    let Ok(entries) = fs::read_dir(&output_dir) else {
        return;
    };

    for entry in entries.flatten() {
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        let Ok(modified) = metadata.modified() else {
            continue;
        };
        let Ok(age) = modified.elapsed() else {
            continue;
        };
        if age > RETENTION_AGE {
            let _ = fs::remove_file(entry.path());
        }
    }
}

pub fn maybe_store_large_output(
    workspace: &Path,
    writable: bool,
    source: &str,
    output: &str,
) -> String {
    let normalized = output.replace('\r', "");
    let total_lines = normalized.lines().count();
    let total_chars = normalized.chars().count();
    let exceeds_limits = total_lines > MAX_INLINE_LINES || total_chars > MAX_INLINE_CHARS;

    if !exceeds_limits {
        return output.to_string();
    }

    let mut preview = normalized
        .lines()
        .take(MAX_INLINE_LINES)
        .collect::<Vec<_>>()
        .join("\n");

    if preview.chars().count() > MAX_INLINE_CHARS {
        preview = preview.chars().take(MAX_INLINE_CHARS).collect::<String>();
        preview.push_str("\n...[truncated]");
    } else if total_lines > MAX_INLINE_LINES {
        preview.push_str(&format!(
            "\n...[truncated {} more lines]",
            total_lines - MAX_INLINE_LINES
        ));
    }

    if writable {
        cleanup_old_outputs(workspace);

        let output_path = build_output_path(workspace, source);
        if let Some(parent) = output_path.parent() {
            let _ = fs::create_dir_all(parent);
        }

        if fs::write(&output_path, output).is_ok() {
            let relative = output_path
                .strip_prefix(workspace)
                .unwrap_or(&output_path)
                .display()
                .to_string();
            return format!(
                "{}\n\n[output truncated: {} chars across {} lines]\nFull output saved to {}\nUse read_file with start_line/end_line to inspect specific sections. Cached tool outputs are retained for about 7 days.",
                preview, total_chars, total_lines, relative
            );
        }
    }

    format!(
        "{}\n\n[output truncated: {} chars across {} lines]\nFull output could not be cached; rerun a narrower command or request a specific section.",
        preview, total_chars, total_lines
    )
}
