use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};

const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "CONTEXT.md"];
const MAX_TOTAL_CHARS: usize = 8_000;

fn read_instruction_file(path: &Path) -> Option<String> {
    let content = fs::read_to_string(path).ok()?;
    let trimmed = content.trim();
    if trimmed.is_empty() {
        return None;
    }

    Some(format!(
        "Instructions from: {}\n{}",
        path.display(),
        trimmed
    ))
}

fn truncate_blocks(blocks: Vec<String>) -> Vec<String> {
    let mut total = 0usize;
    let mut result = Vec::new();

    for block in blocks {
        let remaining = MAX_TOTAL_CHARS.saturating_sub(total);
        if remaining == 0 {
            break;
        }

        if block.chars().count() <= remaining {
            total += block.chars().count();
            result.push(block);
            continue;
        }

        let mut truncated = block.chars().take(remaining).collect::<String>();
        truncated.push_str("\n...[instruction truncated]");
        result.push(truncated);
        break;
    }

    result
}

pub fn workspace_instruction_blocks(workspace: &Path) -> Vec<String> {
    let mut blocks = Vec::new();

    for name in INSTRUCTION_FILES {
        let path = workspace.join(name);
        if let Some(block) = read_instruction_file(&path) {
            blocks.push(block);
        }
    }

    truncate_blocks(blocks)
}

fn push_unique_block(path: &Path, seen: &mut HashSet<PathBuf>, found: &mut Vec<String>) {
    if seen.insert(path.to_path_buf()) {
        if let Some(block) = read_instruction_file(path) {
            found.push(block);
        }
    }
}

pub fn nearby_instruction_blocks(workspace: &Path, target: &Path) -> Vec<String> {
    if !target.starts_with(workspace) {
        return Vec::new();
    }

    if target
        .file_name()
        .and_then(|name| name.to_str())
        .map(|name| {
            INSTRUCTION_FILES
                .iter()
                .any(|candidate| candidate.eq_ignore_ascii_case(name))
        })
        .unwrap_or(false)
    {
        return Vec::new();
    }

    let mut current = target.parent().map(PathBuf::from);
    let mut found = Vec::new();
    let mut seen = HashSet::new();

    while let Some(dir) = current {
        if !dir.starts_with(workspace) {
            break;
        }

        for name in INSTRUCTION_FILES {
            let path = dir.join(name);
            if path.exists() {
                push_unique_block(&path, &mut seen, &mut found);
                break;
            }
        }

        if dir == workspace {
            break;
        }

        current = dir.parent().map(PathBuf::from);
    }

    found.reverse();
    truncate_blocks(found)
}

pub fn format_system_reminder(blocks: &[String]) -> Option<String> {
    if blocks.is_empty() {
        return None;
    }

    Some(format!(
        "<system-reminder>\n{}\n</system-reminder>",
        blocks.join("\n\n")
    ))
}
