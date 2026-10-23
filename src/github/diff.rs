//! Unified-diff parsing for review positioning.
//!
//! GitHub review comments must reference a line that exists inside the
//! pull request's diff hunks: added/context lines by *new* file number
//! (side RIGHT), removed lines by *old* file number (side LEFT). The
//! reviewer model only sees a rendered diff, so every finding is
//! validated against these hunks before it is posted; invalid ones are
//! folded into the review body instead of failing the whole post.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DiffLineKind {
    Added,
    Removed,
    Context,
}

#[derive(Debug, Clone)]
pub struct DiffLine {
    pub old_line: Option<u32>,
    pub new_line: Option<u32>,
    pub kind: DiffLineKind,
    pub content: String,
}

impl DiffLine {
    /// Line number a comment on this line must use for the given side.
    pub fn number_for_side(&self, side: &str) -> Option<u32> {
        if side.eq_ignore_ascii_case("LEFT") {
            match self.kind {
                DiffLineKind::Added => None,
                _ => self.old_line,
            }
        } else {
            match self.kind {
                DiffLineKind::Removed => None,
                _ => self.new_line,
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct FileDiff {
    pub filename: String,
    pub lines: Vec<DiffLine>,
    /// Files GitHub reports without a patch (binary, too large, or the
    /// diff limit was hit) can only receive general comments.
    pub has_patch: bool,
}

impl FileDiff {
    /// Is `line` commentable on `side` (RIGHT = new file, LEFT = old file)?
    pub fn is_valid_position(&self, line: u64, side: &str) -> bool {
        let line = line as u32;
        self.lines.iter().any(|candidate| {
            candidate
                .number_for_side(side)
                .map(|number| number == line)
                .unwrap_or(false)
        })
    }
}

/// Parse a GitHub file `patch` (the unified diff hunk text for one file).
pub fn parse_patch(filename: &str, patch: &str) -> FileDiff {
    let mut lines = Vec::new();
    // Line counters for the current hunk; None outside a hunk.
    let mut old_line: Option<u32> = None;
    let mut new_line: Option<u32> = None;

    for raw in patch.lines() {
        if raw.starts_with("@@") {
            if let Some((old_start, new_start)) = parse_hunk_header(raw) {
                old_line = Some(old_start);
                new_line = Some(new_start);
            } else {
                old_line = None;
                new_line = None;
            }
            continue;
        }

        if old_line.is_none() || new_line.is_none() {
            // Metadata (`diff --git`, `index`, `---`, `+++`) or anything
            // outside a hunk: not commentable, not rendered.
            continue;
        }

        let (kind, content) = if let Some(rest) = raw.strip_prefix('+') {
            (DiffLineKind::Added, rest)
        } else if let Some(rest) = raw.strip_prefix('-') {
            (DiffLineKind::Removed, rest)
        } else if let Some(rest) = raw.strip_prefix(' ') {
            (DiffLineKind::Context, rest)
        } else if raw.starts_with('\\') {
            // "\ No newline at end of file" — annotation, not a line.
            continue;
        } else if raw.is_empty() {
            // Some emitters produce a bare empty line for context; treat
            // it as such only when a hunk is open (handled here).
            (DiffLineKind::Context, "")
        } else {
            continue;
        };

        let (old_for_line, new_for_line) = match kind {
            DiffLineKind::Added => (None, new_line),
            DiffLineKind::Removed => (old_line, None),
            DiffLineKind::Context => (old_line, new_line),
        };

        lines.push(DiffLine {
            old_line: old_for_line,
            new_line: new_for_line,
            kind,
            content: truncate_line(content),
        });

        match kind {
            DiffLineKind::Added => new_line = new_line.map(|n| n + 1),
            DiffLineKind::Removed => old_line = old_line.map(|n| n + 1),
            DiffLineKind::Context => {
                old_line = old_line.map(|n| n + 1);
                new_line = new_line.map(|n| n + 1);
            }
        }
    }

    FileDiff {
        filename: filename.to_string(),
        lines,
        has_patch: !patch.trim().is_empty(),
    }
}

/// Parse `@@ -old_start[,old_count] +new_start[,new_count] @@`.
fn parse_hunk_header(header: &str) -> Option<(u32, u32)> {
    let header = header.trim_start_matches('@').trim_start_matches(' ');
    let header = header.split("@@").next()?.trim();
    let mut parts = header.split_whitespace();
    let old = parts.next()?.strip_prefix('-')?;
    let new = parts.next()?.strip_prefix('+')?;
    let old_start: u32 = old.split(',').next()?.parse().ok()?;
    let new_start: u32 = new.split(',').next()?.parse().ok()?;
    Some((old_start, new_start))
}

/// Render one file's diff for the review prompt with both gutter numbers.
pub fn render_numbered(diff: &FileDiff) -> String {
    let mut out = String::new();
    for line in &diff.lines {
        let old = line
            .old_line
            .map(|n| n.to_string())
            .unwrap_or_default();
        let new = line
            .new_line
            .map(|n| n.to_string())
            .unwrap_or_default();
        let marker = match line.kind {
            DiffLineKind::Added => "+",
            DiffLineKind::Removed => "-",
            DiffLineKind::Context => " ",
        };
        out.push_str(&format!(
            "{:>5} {:>5} | {}{}\n",
            old, new, marker, line.content
        ));
    }
    out
}

/// Per-line cap keeps minified bundles and giant generated rows from
/// eating the whole review budget with a single line.
fn truncate_line(content: &str) -> String {
    const MAX_LINE: usize = 2_000;
    let mut chars = content.chars();
    let head: String = chars.by_ref().take(MAX_LINE).collect();
    if chars.next().is_some() {
        format!("{} …[line truncated]", head)
    } else {
        head
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const PATCH: &str = "@@ -10,6 +10,8 @@ fn main() {\n let a = 1;\n-let b = 2;\n+let b = 3;\n+let c = 4;\n let d = 5;\n";

    #[test]
    fn parses_hunks_with_correct_line_numbers() {
        let diff = parse_patch("src/main.rs", PATCH);
        assert!(diff.has_patch);
        // context 10/10
        assert_eq!(diff.lines[0].old_line, Some(10));
        assert_eq!(diff.lines[0].new_line, Some(10));
        assert_eq!(diff.lines[0].kind, DiffLineKind::Context);
        // removed old 11, no new
        assert_eq!(diff.lines[1].old_line, Some(11));
        assert_eq!(diff.lines[1].new_line, None);
        assert_eq!(diff.lines[1].kind, DiffLineKind::Removed);
        // added new 11 and 12
        assert_eq!(diff.lines[2].new_line, Some(11));
        assert_eq!(diff.lines[3].new_line, Some(12));
        // trailing context old 12 / new 13
        assert_eq!(diff.lines[4].old_line, Some(12));
        assert_eq!(diff.lines[4].new_line, Some(13));
    }

    #[test]
    fn validates_positions_per_side() {
        let diff = parse_patch("src/main.rs", PATCH);
        // RIGHT: added and context lines, by new-file number.
        assert!(diff.is_valid_position(10, "RIGHT")); // context
        assert!(diff.is_valid_position(11, "RIGHT")); // added
        assert!(diff.is_valid_position(12, "RIGHT")); // added
        assert!(diff.is_valid_position(13, "RIGHT")); // context
        assert!(!diff.is_valid_position(14, "RIGHT"));
        // LEFT: removed and context lines, by old-file number.
        assert!(diff.is_valid_position(10, "LEFT")); // context
        assert!(diff.is_valid_position(11, "LEFT")); // removed
        assert!(diff.is_valid_position(12, "LEFT")); // context
        assert!(!diff.is_valid_position(13, "LEFT")); // only exists on the new side
        assert!(!diff.is_valid_position(99, "RIGHT"));
        assert!(!diff.is_valid_position(0, "RIGHT"));
    }

    #[test]
    fn handles_multiple_hunks() {
        let patch = "@@ -1,2 +1,2 @@\n a\n-b\n+c\n@@ -50,3 +60,4 @@\n x\n+y\n z\n";
        let diff = parse_patch("f.txt", patch);
        let added: Vec<u32> = diff
            .lines
            .iter()
            .filter(|l| l.kind == DiffLineKind::Added)
            .filter_map(|l| l.new_line)
            .collect();
        assert_eq!(added, vec![2, 61]);
    }

    #[test]
    fn ignores_no_newline_marker() {
        let patch = "@@ -1 +1 @@\n-old\n\\ No newline at end of file\n+new\n";
        let diff = parse_patch("f", patch);
        assert_eq!(diff.lines.len(), 2);
    }

    #[test]
    fn renders_numbered_output() {
        let diff = parse_patch("src/main.rs", PATCH);
        let rendered = render_numbered(&diff);
        assert!(rendered.contains("   10    10 |  let a = 1;"));
        assert!(rendered.contains("         11 | +let b = 3;"));
        assert!(rendered.contains("   11       | -let b = 2;"));
    }

    #[test]
    fn empty_patch_marks_binary() {
        let diff = parse_patch("image.png", "");
        assert!(!diff.has_patch);
        assert!(diff.lines.is_empty());
        assert!(!diff.is_valid_position(1, "RIGHT"));
    }
}
