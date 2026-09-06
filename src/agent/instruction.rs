use std::collections::HashSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::process::Command;

const INSTRUCTION_FILES: [&str; 3] = ["AGENTS.md", "CLAUDE.md", "CONTEXT.md"];
const MAX_TOTAL_CHARS: usize = 2_000;
/// Instruction-block cache TTL: AGENTS.md files change rarely; a short TTL
/// avoids a directory-tree `exists()` walk on every read_file call while
/// still picking up edits within a minute.
const NEARBY_CACHE_TTL: Duration = Duration::from_secs(60);
const NEARBY_CACHE_MAX_ENTRIES: usize = 512;

struct NearbyCacheEntry {
    blocks: Vec<String>,
    /// Newest mtime (seconds) observed across the probed dirs when cached;
    /// a newer mtime on lookup forces a rescan even within the TTL.
    newest_mtime: u64,
    cached_at: Instant,
}

fn nearby_cache() -> &'static dashmap::DashMap<String, NearbyCacheEntry> {
    static CACHE: OnceLock<dashmap::DashMap<String, NearbyCacheEntry>> = OnceLock::new();
    CACHE.get_or_init(dashmap::DashMap::new)
}

fn dir_mtime_secs(path: &Path) -> u64 {
    std::fs::metadata(path)
        .and_then(|meta| meta.modified())
        .ok()
        .and_then(|time| time.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

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

pub fn global_instruction_blocks(config_dir: &Path) -> Vec<String> {
    workspace_instruction_blocks(config_dir)
}

async fn git_output(workspace: &Path, args: &[&str]) -> Option<String> {
    let mut command = Command::new("git");
    command
        .arg("-C")
        .arg(workspace)
        .args(args)
        .kill_on_drop(true);
    let output = tokio::time::timeout(Duration::from_secs(2), command.output())
        .await
        .ok()?
        .ok()?;
    if !output.status.success() {
        return None;
    }
    let text = String::from_utf8_lossy(&output.stdout).trim().to_string();
    (!text.is_empty()).then_some(text)
}

pub async fn git_workspace_context(workspace: &Path) -> Option<String> {
    let root = git_output(workspace, &["rev-parse", "--show-toplevel"]).await?;
    let status = git_output(
        workspace,
        &["status", "--short", "--branch", "--untracked-files=normal"],
    )
    .await
    .unwrap_or_else(|| "status unavailable".to_string());
    let mut lines = status.lines().take(21).collect::<Vec<_>>();
    if status.lines().count() > lines.len() {
        lines.push("... additional changes omitted");
    }
    Some(format!(
        "# Git Context\n- Worktree: {}\n- Status:\n{}",
        root,
        lines.join("\n")
    ))
}

fn push_unique_block(path: &Path, seen: &mut HashSet<PathBuf>, found: &mut Vec<String>) {
    if seen.insert(path.to_path_buf()) {
        if let Some(block) = read_instruction_file(path) {
            found.push(block);
        }
    }
}

pub fn nearby_instruction_blocks(workspace: &Path, target: &Path) -> Vec<String> {
    scan_nearby_blocks(workspace, target).0
}

/// Cached variant for hot paths (read_file on every call): same result as
/// [`nearby_instruction_blocks`], served from a 60s TTL cache keyed on the
/// workspace+target directory, invalidated early when any probed dir's
/// mtime advances. RAM-bounded: capped entries, blocks already capped at
/// 2KB total per entry.
pub fn cached_nearby_blocks(workspace: &Path, target: &Path) -> Vec<String> {
    let key = format!("{}|{}", workspace.display(), target.display());
    let cache = nearby_cache();
    if let Some(entry) = cache.get(&key) {
        if entry.cached_at.elapsed() < NEARBY_CACHE_TTL {
            let (_, newest) = scan_nearby_mtimes(workspace, target);
            if newest <= entry.newest_mtime {
                return entry.blocks.clone();
            }
        }
    }
    let (blocks, newest) = scan_nearby_blocks(workspace, target);
    if cache.len() >= NEARBY_CACHE_MAX_ENTRIES {
        let cutoff = Instant::now() - NEARBY_CACHE_TTL;
        cache.retain(|_, entry| entry.cached_at > cutoff);
        if cache.len() >= NEARBY_CACHE_MAX_ENTRIES {
            cache.clear();
        }
    }
    cache.insert(
        key,
        NearbyCacheEntry {
            blocks: blocks.clone(),
            newest_mtime: newest,
            cached_at: Instant::now(),
        },
    );
    blocks
}

/// Walk target→workspace collecting probed dir mtimes (for invalidation).
fn scan_nearby_mtimes(workspace: &Path, target: &Path) -> (Vec<PathBuf>, u64) {
    let mut dirs = Vec::new();
    if !target.starts_with(workspace) {
        return (dirs, 0);
    }
    let mut current = target.parent().map(PathBuf::from);
    while let Some(dir) = current {
        if !dir.starts_with(workspace) {
            break;
        }
        dirs.push(dir.clone());
        if dir == workspace {
            break;
        }
        current = dir.parent().map(PathBuf::from);
    }
    let newest = dirs.iter().map(|dir| dir_mtime_secs(dir)).max().unwrap_or(0);
    (dirs, newest)
}

fn scan_nearby_blocks(workspace: &Path, target: &Path) -> (Vec<String>, u64) {
    let (dirs, newest) = scan_nearby_mtimes(workspace, target);
    if dirs.is_empty() {
        return (Vec::new(), newest);
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
        return (Vec::new(), newest);
    }

    let mut found = Vec::new();
    let mut seen = HashSet::new();

    for dir in dirs.iter().rev() {
        for name in INSTRUCTION_FILES {
            let path = dir.join(name);
            if path.exists() {
                push_unique_block(&path, &mut seen, &mut found);
                break;
            }
        }
    }

    (truncate_blocks(found), newest)
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn global_instructions_use_the_config_directory() {
        let temp = tempfile::tempdir().expect("temp directory");
        fs::write(temp.path().join("AGENTS.md"), "Always run focused tests.")
            .expect("write instructions");

        let blocks = global_instruction_blocks(temp.path());

        assert_eq!(blocks.len(), 1);
        assert!(blocks[0].contains("Always run focused tests."));
    }

    #[test]
    fn nearby_instructions_are_ordered_root_to_nested() {
        let temp = tempfile::tempdir().expect("temp directory");
        let nested = temp.path().join("src").join("feature");
        fs::create_dir_all(&nested).expect("nested directory");
        fs::write(temp.path().join("AGENTS.md"), "Root rule").expect("root instructions");
        fs::write(nested.join("AGENTS.md"), "Feature rule").expect("nested instructions");
        let target = nested.join("mod.rs");
        fs::write(&target, "fn feature() {}").expect("target file");

        let blocks = nearby_instruction_blocks(temp.path(), &target);

        assert_eq!(blocks.len(), 2);
        assert!(blocks[0].contains("Root rule"));
        assert!(blocks[1].contains("Feature rule"));
    }
}
