use crate::agent::instruction::format_system_reminder;
use crate::config::Config;
use crate::error::{OSAgentError, Result};
use crate::tools::file_cache::FileReadCache;
use crate::tools::fuzzy_edit::{apply_replacement, fuzzy_find};
use crate::tools::guard::{ensure_relative_path_not_backups, path_touches_backups};
use crate::tools::output::path_touches_tool_outputs;
use crate::tools::registry::{Tool, ToolAttachment, ToolOutcome, ToolResult};
use async_trait::async_trait;
use base64::Engine;
use chrono::Utc;
use serde_json::{json, Value};
use std::fs;
use std::path::PathBuf;
use std::sync::Arc;

fn workspace_is_read_only(config: &Config) -> bool {
    if let Some(workspace) = config.get_workspace_by_path(&config.agent.workspace) {
        if let Some((_, wp)) = config.get_workspace_for_path(&config.agent.workspace) {
            return !wp.permission.allows_writes();
        }
        return !workspace.permission.allows_writes();
    }

    false
}

fn path_is_in_workspace(path: &str, config: &Config) -> bool {
    config.is_path_in_workspace(path)
}

fn ensure_workspace(workspaces: &[PathBuf]) -> Result<()> {
    if workspaces.is_empty() {
        return Err(OSAgentError::ToolExecution(
            "No workspace configured. Set a workspace path in settings.".to_string(),
        ));
    }
    Ok(())
}

/// Outcome of the streaming disk read: the requested window plus an
/// optional full text for cache insertion (only for small files), or a
/// classification that avoids ever loading the whole file
/// (binary/image/PDF detection from a header).
#[derive(Debug)]
enum WindowRead {
    Found {
        text: Option<String>,
        total_lines: usize,
        lines: Vec<String>,
    },
    Empty,
    PastEnd {
        total_lines: usize,
    },
    BinaryImage {
        mime: String,
    },
    Pdf,
    Binary,
    NonUtf8,
}

/// Full text is retained for the content cache only up to this size;
/// anything bigger streams the window and skips insertion.
const CACHE_TEXT_MAX_BYTES: usize = 1024 * 1024;

/// Read only the requested line window from disk. Streams bytes, splits
/// on newlines incrementally, and stops collecting once the last wanted
/// line is complete; the remainder is newline-counted with memchr over
/// raw bytes (no UTF-8 validation, no per-line allocation). Full text is
/// retained only for small files so the content cache can serve repeats.
fn read_window_from_disk(path: &std::path::Path, start_line: usize, limit: usize) -> Result<WindowRead> {
    use std::io::{BufRead, Read};

    let file = std::fs::File::open(path)
        .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;
    let mut reader = std::io::BufReader::with_capacity(64 * 1024, file);

    // Header probe for binary/image/PDF, chained back so no bytes are lost.
    let mut head = vec![0u8; 4096];
    let head_len = reader
        .read(&mut head)
        .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;
    head.truncate(head_len);
    if head_len == 0 {
        return Ok(WindowRead::Empty);
    }
    if memchr::memchr(0, &head).is_some() {
        if let Some(mime) = detect_image_mime(&head) {
            return Ok(WindowRead::BinaryImage {
                mime: mime.to_string(),
            });
        }
        if head.starts_with(b"%PDF-") {
            return Ok(WindowRead::Pdf);
        }
        return Ok(WindowRead::Binary);
    }
    let mut reader = head.chain(reader);

    let end_line = start_line.saturating_add(limit).saturating_sub(1).max(start_line);
    let mut lines: Vec<String> = Vec::new();
    let mut total_lines = 0usize;
    let mut pending: Vec<u8> = Vec::new();
    let mut retain_text: Option<Vec<u8>> = Some(Vec::new());

    loop {
        let chunk = reader
            .fill_buf()
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;
        if chunk.is_empty() {
            break;
        }
        if let Some(buf) = retain_text.as_mut() {
            buf.extend_from_slice(chunk);
            if buf.len() > CACHE_TEXT_MAX_BYTES {
                retain_text = None;
            }
        }
        let consumed = chunk.len();
        pending.extend_from_slice(chunk);
        reader.consume(consumed);

        while let Some(pos) = memchr::memchr(b'\n', &pending) {
            let mut raw: Vec<u8> = pending.drain(..=pos).collect();
            if raw.last() == Some(&b'\n') {
                raw.pop();
            }
            if raw.last() == Some(&b'\r') {
                raw.pop();
            }
            total_lines += 1;
            if total_lines >= start_line && total_lines <= end_line {
                match String::from_utf8(raw) {
                    Ok(line) => lines.push(line),
                    Err(_) => return Ok(WindowRead::NonUtf8),
                }
            }
        }

        if total_lines >= end_line {
            // Window complete: count the rest with memchr over raw bytes.
            let mut extra_newlines = 0usize;
            let mut last_was_newline = pending.is_empty();
            loop {
                let chunk = reader
                    .fill_buf()
                    .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;
                if chunk.is_empty() {
                    break;
                }
                if let Some(buf) = retain_text.as_mut() {
                    buf.extend_from_slice(chunk);
                    if buf.len() > CACHE_TEXT_MAX_BYTES {
                        retain_text = None;
                    }
                }
                extra_newlines += memchr::memchr_iter(b'\n', chunk).count();
                last_was_newline = *chunk.last().unwrap() == b'\n';
                let consumed = chunk.len();
                reader.consume(consumed);
            }
            total_lines += extra_newlines;
            if !pending.is_empty() {
                match String::from_utf8(std::mem::take(&mut pending)) {
                    Ok(_) => total_lines += 1,
                    Err(_) => return Ok(WindowRead::NonUtf8),
                }
            } else if !last_was_newline && total_lines == 0 {
                // Unreachable: empty files return early.
            }
            let text = match retain_text {
                Some(buf) => match String::from_utf8(buf) {
                    Ok(text) => Some(text),
                    Err(_) => return Ok(WindowRead::NonUtf8),
                },
                None => None,
            };
            return Ok(WindowRead::Found {
                text,
                total_lines,
                lines,
            });
        }
    }

    // Reached EOF while still inside (or before) the window.
    if !pending.is_empty() {
        let mut raw = std::mem::take(&mut pending);
        if raw.last() == Some(&b'\r') {
            raw.pop();
        }
        total_lines += 1;
        if total_lines >= start_line && total_lines <= end_line {
            match String::from_utf8(raw) {
                Ok(line) => lines.push(line),
                Err(_) => return Ok(WindowRead::NonUtf8),
            }
        }
    }
    if total_lines == 0 {
        return Ok(WindowRead::Empty);
    }
    if start_line > total_lines {
        return Ok(WindowRead::PastEnd { total_lines });
    }
    let text = match retain_text {
        Some(buf) => match String::from_utf8(buf) {
            Ok(text) => Some(text),
            Err(_) => return Ok(WindowRead::NonUtf8),
        },
        None => None,
    };
    Ok(WindowRead::Found {
        text,
        total_lines,
        lines,
    })
}

fn line_starts_of(lines: &[String], text: &Option<String>) -> Vec<usize> {
    if let Some(text) = text {
        let mut starts = vec![0usize];
        for (idx, byte) in text.bytes().enumerate() {
            if byte == b'\n' && idx + 1 < text.len() {
                starts.push(idx + 1);
            }
        }
        return starts;
    }
    // No full text (large file): synthesize an index for the window alone.
    let mut starts = Vec::with_capacity(lines.len());
    let mut offset = 0usize;
    for line in lines {
        starts.push(offset);
        offset += line.len() + 1;
    }
    starts
}

/// Render one window of lines with numbers + paging footer. Used both for
/// cache-served and freshly-streamed reads so output stays identical.
fn render_lines(
    lines: &[String],
    start_line: usize,
    total_lines: usize,
    limit: usize,
    requested_path: &str,
) -> ToolResult {
    let mut output = String::new();
    for (index, line) in lines.iter().enumerate() {
        if index > 0 {
            output.push('\n');
        }
        if line.chars().count() > ReadFileTool::MAX_LINE_CHARS {
            let mut clipped: String = line.chars().take(ReadFileTool::MAX_LINE_CHARS).collect();
            clipped.push_str("...[line truncated]");
            output.push_str(&format!("{}: {}", start_line + index, clipped));
        } else {
            output.push_str(&format!("{}: {}", start_line + index, line));
        }
    }

    let end_line = start_line + lines.len().saturating_sub(1);
    let truncated = end_line < total_lines;
    if truncated {
        output.push_str(&format!(
            "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
            start_line,
            end_line,
            total_lines,
            end_line + 1
        ));
    } else {
        output.push_str(&format!(
            "\n\n[Showing lines {}-{} of {}]",
            start_line, end_line, total_lines
        ));
    }

    ToolResult {
        output,
        outcome: ToolOutcome::Success,
        title: Some(requested_path.to_string()),
        metadata: json!({
            "kind": "file",
            "path": requested_path,
            "offset": start_line,
            "limit": limit,
            "total_lines": total_lines,
            "truncated": truncated
        }),
        attachments: Vec::new(),
    }
}

pub struct ReadFileTool {
    workspaces: Vec<PathBuf>,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl ReadFileTool {
    const DEFAULT_LIMIT: usize = 200;
    const MAX_LIMIT: usize = 2000;
    const MAX_LINE_CHARS: usize = 2000;

    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        let active_workspace = config.get_active_workspace();
        let workspaces: Vec<PathBuf> = active_workspace
            .paths
            .iter()
            .map(|wp| {
                let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                path.canonicalize().unwrap_or(path)
            })
            .collect();

        Self {
            workspaces,
            config,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        let full_path = self.workspaces[0].join(path);
        let full_path = full_path.canonicalize().unwrap_or(full_path);

        if !full_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "Path not found: {}",
                path
            )));
        }

        if path_touches_tool_outputs(&full_path) && !path.starts_with(".osa_tool_outputs") {
            return Err(OSAgentError::ToolExecution(
                "Tool output files must be read by explicit .osa_tool_outputs path".to_string(),
            ));
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    fn normalize_read_target<'a>(&self, args: &'a Value) -> Result<&'a str> {
        args["filePath"]
            .as_str()
            .or_else(|| args["path"].as_str())
            .ok_or_else(|| {
                OSAgentError::ToolExecution(
                    "Missing 'filePath' parameter (or compatibility alias 'path')".to_string(),
                )
            })
    }

    fn normalize_paging(&self, args: &Value, is_dir: bool) -> Result<(usize, usize)> {
        if let Some(offset) = args["offset"].as_u64() {
            let limit = args["limit"].as_u64().unwrap_or(Self::DEFAULT_LIMIT as u64) as usize;
            let limit = limit.clamp(1, Self::MAX_LIMIT);
            return Ok((offset.max(1) as usize, limit));
        }

        if !is_dir {
            let start_line = args["start_line"].as_u64().unwrap_or(1).max(1) as usize;
            if let Some(end_line) = args["end_line"].as_u64() {
                let end_line = end_line.max(start_line as u64) as usize;
                let limit = end_line.saturating_sub(start_line).saturating_add(1);
                return Ok((start_line, limit.clamp(1, Self::MAX_LIMIT)));
            }
            return Ok((start_line, Self::DEFAULT_LIMIT));
        }

        Ok((1, Self::DEFAULT_LIMIT))
    }

    fn format_directory_entry(&self, absolute: &PathBuf, base: &PathBuf) -> String {
        let relative = absolute.strip_prefix(base).unwrap_or(absolute.as_path());
        if relative.as_os_str().is_empty() {
            return ".".to_string();
        }
        let mut display = relative.display().to_string();
        if absolute.is_dir() {
            display.push('/');
        }
        display
    }

    fn read_directory(
        &self,
        dir_path: &PathBuf,
        offset: usize,
        limit: usize,
        requested_path: &str,
    ) -> Result<ToolResult> {
        let entries = fs::read_dir(dir_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read directory: {}", e)))?;

        let mut formatted: Vec<String> = Vec::new();
        for entry in entries {
            let entry = entry.map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to read directory entry: {}", e))
            })?;
            let path = entry.path();
            let relative = path
                .strip_prefix(&self.workspaces[0])
                .unwrap_or(path.as_path());
            if path_touches_backups(relative) {
                continue;
            }
            if path_touches_tool_outputs(relative)
                && !requested_path.starts_with(".osa_tool_outputs")
            {
                continue;
            }
            formatted.push(self.format_directory_entry(&path, &self.workspaces[0]));
        }

        formatted.sort();

        if formatted.is_empty() {
            return Ok(ToolResult {
                output: "(empty directory)".to_string(),
                outcome: ToolOutcome::Success,
                title: Some(requested_path.to_string()),
                metadata: json!({
                    "kind": "directory",
                    "path": requested_path,
                    "offset": offset,
                    "limit": limit,
                    "count": 0,
                    "truncated": false
                }),
                attachments: Vec::new(),
            });
        }

        let start_index = offset.saturating_sub(1);
        if start_index >= formatted.len() {
            return Err(OSAgentError::ToolExecution(format!(
                "offset {} is past end of directory listing ({} entries)",
                offset,
                formatted.len()
            )));
        }
        let end_index = (start_index + limit).min(formatted.len());
        let slice = &formatted[start_index..end_index];
        let mut output = slice.join("\n");

        let truncated = end_index < formatted.len();
        if truncated {
            output.push_str(&format!(
                "\n\n[Results truncated at {} entries. Use offset={} to continue.]",
                limit,
                end_index + 1
            ));
        }

        Ok(ToolResult {
            output,
            outcome: ToolOutcome::Success,
            title: Some(requested_path.to_string()),
            metadata: json!({
                "kind": "directory",
                "path": requested_path,
                "offset": offset,
                "limit": limit,
                "count": formatted.len(),
                "truncated": truncated
            }),
            attachments: Vec::new(),
        })
    }

    async fn read_file_text(
        &self,
        file_path: &PathBuf,
        offset: usize,
        limit: usize,
        requested_path: &str,
    ) -> Result<ToolResult> {
        // Fast path: cached content slices the window with zero disk I/O.
        // Freshness is verified inside get_content via mtime+size.
        let canonical_hint = file_path.clone();
        if let Some(entry) = self.cache.get_content(&canonical_hint) {
            if let (Some(text), Some(starts)) = (entry.content, entry.line_starts) {
                return Ok(Self::render_window(
                    &text,
                    &starts,
                    entry.line_count,
                    offset,
                    limit,
                    requested_path,
                    &self.workspaces,
                    &self.config,
                    file_path,
                ));
            }
        }

        let fp = file_path.clone();
        let start_line = offset.max(1);
        // Stream only the needed window: stop reading once the last
        // requested line is complete. Large files cost O(window), not O(file).
        let window = tokio::task::spawn_blocking(move || {
            read_window_from_disk(&fp, start_line, limit)
        })
        .await
        .map_err(|e| OSAgentError::ToolExecution(format!("spawn_blocking error: {}", e)))??;

        match window {
            WindowRead::Found {
                text,
                total_lines,
                lines,
            } => {
                let start_line = offset.max(1);
                // Small files carry full text: insert once so repeats are
                // zero-I/O. Large files skip the cache (window already in
                // hand, no second copy).
                if let Some(text) = text {
                    let canonical = file_path
                        .canonicalize()
                        .unwrap_or_else(|_| file_path.clone());
                    self.cache.insert(&canonical, text);
                    if let Some(entry) = self.cache.get_content(&canonical) {
                        if let (Some(text), Some(starts)) = (entry.content, entry.line_starts) {
                            return Ok(Self::render_window(
                                &text,
                                &starts,
                                entry.line_count,
                                offset,
                                limit,
                                requested_path,
                                &self.workspaces,
                                &self.config,
                                file_path,
                            ));
                        }
                    }
                }
                let mut result = render_lines(&lines, start_line, total_lines, limit, requested_path);
                Self::append_instructions(&mut result, &self.workspaces, &self.config, file_path);
                Ok(result)
            }
            WindowRead::Empty => Ok(ToolResult {
                output: "(empty file)".to_string(),
                outcome: ToolOutcome::Success,
                title: Some(requested_path.to_string()),
                metadata: json!({
                    "kind": "file",
                    "path": requested_path,
                    "offset": offset,
                    "limit": limit,
                    "total_lines": 0,
                    "truncated": false
                }),
                attachments: Vec::new(),
            }),
            WindowRead::PastEnd { total_lines } => Err(OSAgentError::ToolExecution(format!(
                "offset {} is past end of file ({} lines)",
                start_line, total_lines
            ))),
            WindowRead::BinaryImage { mime } => {
                let bytes = std::fs::read(file_path).map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to read file: {}", e))
                })?;
                self.read_image(&bytes, &mime, requested_path).await
            }
            WindowRead::Pdf => {
                let bytes = std::fs::read(file_path).map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to read file: {}", e))
                })?;
                self.read_pdf(&bytes, offset, limit, requested_path).await
            }
            WindowRead::Binary => Err(OSAgentError::ToolExecution(
                "File appears to be binary and cannot be displayed as text".to_string(),
            )),
            WindowRead::NonUtf8 => Err(OSAgentError::ToolExecution(
                "File contains non-UTF8 data and cannot be displayed as text".to_string(),
            )),
        }
    }

    /// Render a window from full text + line index. Shared by the cache
    /// fast path and the streaming disk path so both stay identical.
    #[allow(clippy::too_many_arguments)]
    fn render_window(
        text: &str,
        starts: &[usize],
        total_lines: usize,
        offset: usize,
        limit: usize,
        requested_path: &str,
        workspaces: &[PathBuf],
        config: &Config,
        file_path: &PathBuf,
    ) -> ToolResult {
        let start_line = offset.max(1);
        if total_lines == 0 || start_line > total_lines {
            if total_lines == 0 {
                return ToolResult {
                    output: "(empty file)".to_string(),
                    outcome: ToolOutcome::Success,
                    title: Some(requested_path.to_string()),
                    metadata: json!({
                        "kind": "file",
                        "path": requested_path,
                        "offset": offset,
                        "limit": limit,
                        "total_lines": 0,
                        "truncated": false
                    }),
                    attachments: Vec::new(),
                };
            }
            return ToolResult {
                output: format!(
                    "offset {} is past end of file ({} lines)",
                    start_line, total_lines
                ),
                outcome: ToolOutcome::Failure,
                title: Some(requested_path.to_string()),
                metadata: json!({
                    "kind": "file",
                    "path": requested_path,
                    "offset": start_line,
                    "limit": limit,
                    "total_lines": total_lines,
                    "truncated": false
                }),
                attachments: Vec::new(),
            };
        }

        let end_line = (start_line + limit - 1).min(total_lines);
        let mut window: Vec<String> = Vec::new();
        for line_no in start_line..=end_line {
            let start = starts.get(line_no - 1).copied().unwrap_or(text.len());
            let end = starts.get(line_no).copied().unwrap_or(text.len());
            let mut line = text[start..end.min(text.len())]
                .strip_suffix('\n')
                .unwrap_or(&text[start..end.min(text.len())]);
            line = line.strip_suffix('\r').unwrap_or(line);
            window.push(line.to_string());
        }

        let mut result = render_lines(&window, start_line, total_lines, limit, requested_path);
        Self::append_instructions(&mut result, workspaces, config, file_path);
        result
    }

    fn append_instructions(
        result: &mut ToolResult,
        workspaces: &[PathBuf],
        config: &Config,
        file_path: &PathBuf,
    ) {
        Self::append_instructions_inner(&mut result.output, workspaces, config, file_path);
    }

    fn append_instructions_inner(
        output: &mut String,
        workspaces: &[PathBuf],
        config: &Config,
        file_path: &PathBuf,
    ) {
        let instruction_root = workspaces
            .iter()
            .filter(|workspace| file_path.starts_with(workspace))
            .max_by_key(|workspace| workspace.components().count())
            .unwrap_or(&workspaces[0]);
        if let Some(reminder) =
            format_system_reminder(&crate::agent::instruction::cached_nearby_blocks(
                instruction_root,
                file_path,
            ))
        {
            *output = format!("{}\n\n{}", reminder, output);
        }
        let _ = config;
    }

    async fn read_image(
        &self,
        bytes: &[u8],
        mime: &str,
        requested_path: &str,
    ) -> Result<ToolResult> {
        const MAX_IMAGE_BYTES: usize = 10 * 1024 * 1024;
        if bytes.len() > MAX_IMAGE_BYTES {
            return Err(OSAgentError::ToolExecution(format!(
                "Image file is {} bytes (max supported: {} bytes). Use a smaller image.",
                bytes.len(),
                MAX_IMAGE_BYTES
            )));
        }
        let data_url = format!(
            "data:{};base64,{}",
            mime,
            base64::engine::general_purpose::STANDARD.encode(bytes)
        );
        Ok(ToolResult {
            output: format!(
                "Image {} ({} bytes, {}) has been attached to the conversation. Describe what you see in it.",
                requested_path,
                bytes.len(),
                mime
            ),
            outcome: ToolOutcome::Success,
            title: Some(requested_path.to_string()),
            metadata: json!({
                "kind": "image",
                "path": requested_path,
                "mime": mime,
                "bytes": bytes.len()
            }),
            attachments: vec![ToolAttachment {
                filename: requested_path.to_string(),
                mime: mime.to_string(),
                data_url,
            }],
        })
    }

    async fn read_pdf(
        &self,
        bytes: &[u8],
        offset: usize,
        limit: usize,
        requested_path: &str,
    ) -> Result<ToolResult> {
        let bytes = bytes.to_vec();
        let text = tokio::task::spawn_blocking(move || {
            pdf_extract::extract_text_from_mem(&bytes)
                .map_err(|e| format!("Failed to extract text from PDF: {}", e))
        })
        .await
        .map_err(|e| OSAgentError::ToolExecution(format!("spawn_blocking error: {}", e)))?
        .map_err(OSAgentError::ToolExecution)?;
        let text = text.trim().to_string();
        let total_lines = text.lines().count();

        let (output, start_line, end_line, truncated) = if total_lines == 0 {
            (
                "(PDF contains no extractable text; it may be a scanned or image-only document)"
                    .to_string(),
                0usize,
                0usize,
                false,
            )
        } else {
            let start_line = offset.max(1);
            if start_line > total_lines {
                return Err(OSAgentError::ToolExecution(format!(
                    "offset {} is past end of PDF text ({} lines)",
                    start_line, total_lines
                )));
            }
            let end_line = (start_line + limit - 1).min(total_lines);
            let truncated = end_line < total_lines;
            let mut output = text
                .lines()
                .skip(start_line - 1)
                .take(end_line - start_line + 1)
                .enumerate()
                .map(|(index, line)| format!("{}: {}", start_line + index, line))
                .collect::<Vec<_>>()
                .join("\n");
            if truncated {
                output.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}. Use offset={} to continue.]",
                    start_line,
                    end_line,
                    total_lines,
                    end_line + 1
                ));
            } else {
                output.push_str(&format!(
                    "\n\n[Showing lines {}-{} of {}]",
                    start_line, end_line, total_lines
                ));
            }
            (output, start_line, end_line, truncated)
        };

        Ok(ToolResult {
            output,
            outcome: ToolOutcome::Success,
            title: Some(requested_path.to_string()),
            metadata: json!({
                "kind": "pdf",
                "path": requested_path,
                "offset": start_line,
                "limit": limit,
                "total_lines": total_lines,
                "truncated": truncated
            }),
            attachments: Vec::new(),
        })
    }
}

fn detect_image_mime(bytes: &[u8]) -> Option<&'static str> {
    if bytes.len() >= 8 && bytes.starts_with(&[0x89, b'P', b'N', b'G', 0x0D, 0x0A, 0x1A, 0x0A]) {
        return Some("image/png");
    }
    if bytes.len() >= 3 && bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Some("image/jpeg");
    }
    if bytes.len() >= 6 && (bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a")) {
        return Some("image/gif");
    }
    if bytes.len() >= 12 && bytes.starts_with(b"RIFF") && &bytes[8..12] == b"WEBP" {
        return Some("image/webp");
    }
    None
}

#[async_trait]
impl Tool for ReadFileTool {
    fn name(&self) -> &str {
        "read_file"
    }

    fn description(&self) -> &str {
        "Read a file or directory from the local filesystem. If the path does not exist, an error is returned.\n\nUsage:\n- Paths are relative to the workspace root.\n- By default, returns up to 200 lines from the start of the file.\n- The offset parameter is the line number to start from (1-indexed).\n- To read later sections, call this tool again with a larger offset.\n- Use the grep tool to find specific content in large files or files with long lines.\n- If you are unsure of the correct file path, use the glob tool to look up filenames by pattern.\n- Contents are returned with each line prefixed by its line number.\n- For directories, entries are returned one per line with a trailing / for subdirectories.\n- Any line longer than 2000 characters is truncated.\n- Call this tool in parallel when you know there are multiple files you want to read.\n- Avoid tiny repeated slices (30 line chunks). If you need more context, read a larger window.\n- Image files (png/jpeg/gif/webp) are attached to the conversation as images so you can see them.\n- PDF files are read as extracted text with the same offset/limit pagination."
    }

    fn when_to_use(&self) -> &str {
        "Use when you have an exact path and need file content or directory listings. Always read before editing."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for broad content discovery across many files; use glob or grep first. Do not use for file modifications."
    }

    fn examples(&self) -> Vec<crate::tools::registry::ToolExample> {
        vec![
            crate::tools::registry::ToolExample {
                description: "Read a whole file".to_string(),
                input: json!({
                    "filePath": "src/main.rs"
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Read a focused page of lines".to_string(),
                input: json!({
                    "filePath": "src/main.rs",
                    "offset": 40,
                    "limit": 50
                }),
            },
            crate::tools::registry::ToolExample {
                description: "Read directory entries".to_string(),
                input: json!({
                    "filePath": "src",
                    "offset": 1,
                    "limit": 200
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Compatibility alias for filePath"
                },
                "filePath": {
                    "type": "string",
                    "description": "Relative path to the file or directory within workspace"
                },
                "offset": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "1-based line/entry offset (default: 1)"
                },
                "limit": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Maximum lines/entries to return (default: 200, max: 2000). Reads over ~80 lines are previewed head-only in context, so prefer limit<=80 and page with offset for large files."
                },
                "start_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Compatibility alias for offset (file reads only)"
                },
                "end_line": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Compatibility alias used with start_line (file reads only)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let result = self.execute_result(args).await?;
        Ok(result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let path = self.normalize_read_target(&args)?;
        let target_path = self.validate_path(path)?;
        let (offset, limit) = self.normalize_paging(&args, target_path.is_dir())?;

        if target_path.is_dir() {
            return self.read_directory(&target_path, offset, limit, path);
        }

        self.read_file_text(&target_path, offset, limit, path).await
    }
}

pub struct WriteFileTool {
    workspaces: Vec<PathBuf>,
    backup_dir: PathBuf,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl WriteFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspaces: Vec<PathBuf> = config
                .get_active_workspace()
                .paths
                .iter()
                .map(|wp| {
                    let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                    path.canonicalize().unwrap_or(path)
                })
                .collect();
            return Self {
                workspaces,
                backup_dir: PathBuf::new(),
                config,
                cache,
            };
        }

        let active_workspace = config.get_active_workspace();
        let mut workspaces: Vec<PathBuf> = Vec::new();
        let mut backup_dir = PathBuf::new();

        for (i, wp) in active_workspace.paths.iter().enumerate() {
            let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
            let canonical = path.canonicalize().unwrap_or(path.clone());

            if i == 0 {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                backup_dir = canonical.join(".osagent_backups");
                if !backup_dir.exists() {
                    let _ = fs::create_dir_all(&backup_dir);
                }
            }
            workspaces.push(canonical);
        }

        Self {
            workspaces,
            backup_dir,
            config,
            cache,
        }
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<Option<PathBuf>> {
        if !file_path.exists() {
            return Ok(None);
        }

        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let backup_name = format!("{}_{}.bak", file_name, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(Some(backup_path))
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        let full_path = self.workspaces[0].join(path);
        let full_path = full_path.canonicalize().unwrap_or(full_path);

        if let Some(parent) = full_path.parent() {
            if !parent.exists() {
                fs::create_dir_all(parent).map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to create directory: {}", e))
                })?;
            }
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for WriteFileTool {
    fn name(&self) -> &str {
        "write_file"
    }

    fn description(&self) -> &str {
        "Writes a file to the local filesystem.\n\nUsage:\n- This tool will overwrite the existing file if there is one at the provided path.\n- If this is an existing file, you MUST use read_file first to read the file's contents. This tool will fail if you did not read the file first.\n- ALWAYS prefer editing existing files in the codebase. NEVER write new files unless explicitly required.\n- NEVER proactively create documentation files (*.md) or README files. Only create documentation files if explicitly requested by the user.\n- Creates an automatic backup in .osagent_backups before overwriting."
    }

    fn when_to_use(&self) -> &str {
        "Use for creating new files or when a full file rewrite is more appropriate than targeted edits."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for partial changes to existing files; use edit_file or apply_patch instead."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within workspace"
                },
                "content": {
                    "type": "string",
                    "description": "Content to write to the file"
                }
            },
            "required": ["path", "content"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; write operations are disabled".to_string(),
            ));
        }

        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let content = args["content"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'content' parameter".to_string())
        })?;

        let file_path = self.validate_path(path)?;

        let backup = self.create_backup(&file_path)?;

        let fp = file_path.clone();
        let content_owned = content.to_string();
        tokio::task::spawn_blocking(move || {
            std::fs::write(&fp, content_owned)
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))
        })
        .await
        .map_err(|e| OSAgentError::ToolExecution(format!("spawn_blocking error: {}", e)))??;

        if let Ok(canonical) = file_path.canonicalize() {
            self.cache.invalidate(&canonical);
        }

        let backup_msg = if let Some(backup_path) = backup {
            format!(" (backup created at {:?})", backup_path)
        } else {
            String::new()
        };

        Ok(format!("Successfully wrote to {}{}", path, backup_msg))
    }
}

pub struct EditFileTool {
    workspaces: Vec<PathBuf>,
    backup_dir: PathBuf,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl EditFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspaces: Vec<PathBuf> = config
                .get_active_workspace()
                .paths
                .iter()
                .map(|wp| {
                    let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                    path.canonicalize().unwrap_or(path)
                })
                .collect();
            return Self {
                workspaces,
                backup_dir: PathBuf::new(),
                config,
                cache,
            };
        }

        let active_workspace = config.get_active_workspace();
        let mut workspaces: Vec<PathBuf> = Vec::new();
        let mut backup_dir = PathBuf::new();

        for (i, wp) in active_workspace.paths.iter().enumerate() {
            let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
            let canonical = path.canonicalize().unwrap_or(path.clone());

            if i == 0 {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                backup_dir = canonical.join(".osagent_backups");
                if !backup_dir.exists() {
                    let _ = fs::create_dir_all(&backup_dir);
                }
            }
            workspaces.push(canonical);
        }

        Self {
            workspaces,
            backup_dir,
            config,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        let full_path = self.workspaces[0].join(path);
        let full_path = full_path.canonicalize().unwrap_or(full_path);

        if !full_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "File not found: {}",
                path
            )));
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<PathBuf> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let backup_name = format!("{}_{}.bak", file_name, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(backup_path)
    }
}

#[async_trait]
impl Tool for EditFileTool {
    fn name(&self) -> &str {
        "edit_file"
    }

    fn description(&self) -> &str {
        "Performs exact string replacements in files with fuzzy matching fallbacks.\n\nUsage:\n- You MUST use read_file at least once before editing a file. The tool will error if you attempt an edit without reading the file first.\n- When editing text from read_file output, copy the exact text you want to replace, preserving indentation (tabs/spaces).\n- The tool will FAIL if old_text is not found in the file. Read the file first and copy the exact text.\n- The tool will FAIL if old_text is found multiple times. Provide more surrounding context to make the match unique, or use replace_all to change every instance.\n- ALWAYS prefer editing existing files. NEVER write new files unless explicitly required.\n- If the exact text is not found, the tool falls back to fuzzy matching (whitespace, indentation, and small context differences).\n- Creates an automatic backup in .osagent_backups before modifying."
    }

    fn when_to_use(&self) -> &str {
        "Use for targeted inline changes to existing files. For multi-hunk changes across a file, prefer apply_patch."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for creating new files (use write_file), full file rewrites (use write_file), or multi-hunk changes (use apply_patch)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file within workspace"
                },
                "old_text": {
                    "type": "string",
                    "description": "Text to find and replace"
                },
                "new_text": {
                    "type": "string",
                    "description": "Text to replace with"
                },
                "replace_all": {
                    "type": "boolean",
                    "description": "Replace all occurrences (default: false)"
                }
            },
            "required": ["path", "old_text", "new_text"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; edit operations are disabled".to_string(),
            ));
        }

        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let old_text = args["old_text"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'old_text' parameter".to_string())
        })?;

        let new_text = args["new_text"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'new_text' parameter".to_string())
        })?;

        let replace_all = args["replace_all"].as_bool().unwrap_or(false);

        if old_text.is_empty() {
            return Err(OSAgentError::ToolExecution(
                "'old_text' cannot be empty".to_string(),
            ));
        }

        let file_path = self.validate_path(path)?;

        if let Ok(canonical) = file_path.canonicalize() {
            if let Some(entry) = self.cache.check(&canonical) {
                if let Ok(meta) = fs::metadata(&canonical) {
                    let current_mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_secs())
                        .unwrap_or(0);
                    if current_mtime != entry.mtime_secs {
                        return Err(OSAgentError::ToolExecution(
                            "File has been modified since last read. Re-read the file first with read_file, then retry the edit.".to_string(),
                        ));
                    }
                }
            }
        }

        let _backup_path = self.create_backup(&file_path)?;

        let content = fs::read_to_string(&file_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to read file: {}", e)))?;

        if replace_all {
            let match_count = content.match_indices(old_text).count();
            if match_count == 0 {
                return Err(OSAgentError::ToolExecution(
                    "Text not found in file (exact match for replace_all)".to_string(),
                ));
            }
            let new_content = content.replace(old_text, new_text);
            fs::write(&file_path, new_content)
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;
            if let Ok(canonical) = file_path.canonicalize() {
                self.cache.invalidate(&canonical);
            }
            return Ok(format!(
                "Successfully edited {} ({} replacement{})",
                path,
                match_count,
                if match_count == 1 { "" } else { "s" }
            ));
        }

        let exact_count = content.match_indices(old_text).count();
        if exact_count == 1 {
            let new_content = content.replacen(old_text, new_text, 1);
            fs::write(&file_path, new_content)
                .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;
            if let Ok(canonical) = file_path.canonicalize() {
                self.cache.invalidate(&canonical);
            }
            return Ok(format!(
                "Successfully edited {} (1 replacement, exact match)",
                path
            ));
        }

        if exact_count > 1 {
            return Err(OSAgentError::ToolExecution(format!(
                "Text matched {} times; refine 'old_text', set replace_all=true, or use apply_patch",
                exact_count
            )));
        }

        let match_result = fuzzy_find(&content, old_text).ok_or_else(|| {
            OSAgentError::ToolExecution(
                "Text not found in file (tried exact, line-trimmed, whitespace-normalized, indentation-flexible, escape-normalized, trimmed-boundary, block-anchor, and context-aware matching)".to_string(),
            )
        })?;

        let new_content = apply_replacement(&content, &match_result, old_text, new_text);

        fs::write(&file_path, new_content)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to write file: {}", e)))?;

        if let Ok(canonical) = file_path.canonicalize() {
            self.cache.invalidate(&canonical);
        }

        Ok(format!(
            "Successfully edited {} (1 replacement via {} matching, confidence: {:.0}%)",
            path,
            match_result.strategy,
            match_result.confidence * 100.0
        ))
    }
}

pub struct ListFilesTool {
    workspaces: Vec<PathBuf>,
    config: Config,
}

impl ListFilesTool {
    pub fn new(config: Config) -> Self {
        let active_workspace = config.get_active_workspace();
        let workspaces: Vec<PathBuf> = active_workspace
            .paths
            .iter()
            .map(|wp| {
                let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                path.canonicalize().unwrap_or(path)
            })
            .collect();

        Self { workspaces, config }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        let full_path = if path.is_empty() || path == "." {
            self.workspaces[0].clone()
        } else {
            let joined = self.workspaces[0].join(path);
            joined.canonicalize().unwrap_or(joined)
        };

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }
}

#[async_trait]
impl Tool for ListFilesTool {
    fn name(&self) -> &str {
        "list_files"
    }

    fn description(&self) -> &str {
        "List files and directories in the workspace.\n\nUsage:\n- Returns entries sorted alphabetically.\n- Directories are shown with a trailing /.\n- Skips common noise directories like node_modules, target, .git by default.\n- Use recursive:true to list all files under a directory tree.\n- Use glob for pattern-based file discovery instead."
    }

    fn when_to_use(&self) -> &str {
        "Use for quick directory inspection when you need to understand the local file layout."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use when you need content search (use grep) or pattern-based file finding (use glob) or when you already know the exact file path (use read_file)."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to directory (default: workspace root)"
                },
                "recursive": {
                    "type": "boolean",
                    "description": "List recursively (default: false)"
                }
            }
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let path = args["path"].as_str().unwrap_or("");
        let recursive = args["recursive"].as_bool().unwrap_or(false);

        let dir_path = self.validate_path(path)?;

        if !dir_path.exists() {
            return Err(OSAgentError::ToolExecution(
                "Directory not found".to_string(),
            ));
        }

        if !dir_path.is_dir() {
            return Err(OSAgentError::ToolExecution(
                "Path is not a directory".to_string(),
            ));
        }

        let mut results = Vec::new();

        fn list_dir(
            dir: &PathBuf,
            base: &PathBuf,
            results: &mut Vec<String>,
            recursive: bool,
        ) -> Result<()> {
            let entries = fs::read_dir(dir).map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to read directory: {}", e))
            })?;

            for entry in entries {
                let entry = entry.map_err(|e| {
                    OSAgentError::ToolExecution(format!("Failed to read entry: {}", e))
                })?;
                let path = entry.path();
                let relative = path.strip_prefix(base).unwrap_or(&path);

                if path_touches_backups(relative) {
                    continue;
                }
                if path_touches_tool_outputs(relative) {
                    continue;
                }

                let type_str = if path.is_dir() { "DIR" } else { "FILE" };
                results.push(format!("[{}] {}", type_str, relative.display()));

                if recursive && path.is_dir() {
                    list_dir(&path, base, results, recursive)?;
                }
            }

            Ok(())
        }

        list_dir(&dir_path, &self.workspaces[0], &mut results, recursive)?;

        if results.is_empty() {
            Ok("Empty directory".to_string())
        } else {
            Ok(results.join("\n"))
        }
    }
}

pub struct DeleteFileTool {
    workspaces: Vec<PathBuf>,
    backup_dir: PathBuf,
    config: Config,
    cache: Arc<FileReadCache>,
}

impl DeleteFileTool {
    pub fn new(config: Config, cache: Arc<FileReadCache>) -> Self {
        if workspace_is_read_only(&config) {
            let workspaces: Vec<PathBuf> = config
                .get_active_workspace()
                .paths
                .iter()
                .map(|wp| {
                    let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                    path.canonicalize().unwrap_or(path)
                })
                .collect();
            return Self {
                workspaces,
                backup_dir: PathBuf::new(),
                config,
                cache,
            };
        }

        let active_workspace = config.get_active_workspace();
        let mut workspaces: Vec<PathBuf> = Vec::new();
        let mut backup_dir = PathBuf::new();

        for (i, wp) in active_workspace.paths.iter().enumerate() {
            let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
            let canonical = path.canonicalize().unwrap_or(path.clone());

            if i == 0 {
                if !path.exists() {
                    let _ = fs::create_dir_all(&path);
                }
                backup_dir = canonical.join(".osagent_backups");
                if !backup_dir.exists() {
                    let _ = fs::create_dir_all(&backup_dir);
                }
            }
            workspaces.push(canonical);
        }

        Self {
            workspaces,
            backup_dir,
            config,
            cache,
        }
    }

    fn validate_path(&self, path: &str) -> Result<PathBuf> {
        ensure_relative_path_not_backups(path)?;
        ensure_workspace(&self.workspaces)?;

        let full_path = self.workspaces[0].join(path);
        let full_path = full_path.canonicalize().unwrap_or(full_path);

        if !full_path.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "File not found: {}",
                path
            )));
        }

        if self.workspaces.iter().any(|ws| full_path.starts_with(ws))
            || path_is_in_workspace(&full_path.to_string_lossy(), &self.config)
        {
            Ok(full_path)
        } else {
            Err(OSAgentError::ToolExecution(
                "Path is outside workspace".to_string(),
            ))
        }
    }

    fn create_backup(&self, file_path: &PathBuf) -> Result<PathBuf> {
        let timestamp = Utc::now().format("%Y%m%d_%H%M%S");
        let file_name = file_path
            .file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("unknown");

        let backup_name = format!("{}_{}_DELETED.bak", file_name, timestamp);
        let backup_path = self.backup_dir.join(backup_name);

        fs::copy(file_path, &backup_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to create backup: {}", e)))?;

        Ok(backup_path)
    }
}

#[async_trait]
impl Tool for DeleteFileTool {
    fn name(&self) -> &str {
        "delete_file"
    }

    fn description(&self) -> &str {
        "Delete a file from the workspace. Irreversible - creates an automatic backup before deletion.\n\nUsage:\n- Only use when a file truly needs removal and the user requested or clearly implied that change.\n- Creates a timestamped backup in .osagent_backups before deleting.\n- The path must be relative to the workspace root.\n- Cannot delete directories; only individual files."
    }

    fn when_to_use(&self) -> &str {
        "Use only when a file must be removed and the user explicitly requested deletion."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for routine edits (use edit_file), content replacement (use write_file), or when keeping the file with modifications is safer."
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "path": {
                    "type": "string",
                    "description": "Relative path to the file to delete"
                }
            },
            "required": ["path"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        if self.backup_dir.as_os_str().is_empty() {
            return Err(OSAgentError::ToolExecution(
                "Workspace is read-only; delete operations are disabled".to_string(),
            ));
        }

        let path = args["path"]
            .as_str()
            .ok_or_else(|| OSAgentError::ToolExecution("Missing 'path' parameter".to_string()))?;

        let file_path = self.validate_path(path)?;

        let canonical = file_path.canonicalize().ok();

        let backup_path = self.create_backup(&file_path)?;

        fs::remove_file(&file_path)
            .map_err(|e| OSAgentError::ToolExecution(format!("Failed to delete file: {}", e)))?;

        if let Some(canonical) = canonical {
            self.cache.invalidate(&canonical);
        }

        Ok(format!(
            "Successfully deleted {} (backup at {:?})",
            path, backup_path
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;
    use tempfile::tempdir;

    fn config_for_workspace(path: &str) -> Config {
        let mut config = Config::default();
        config.agent.workspace = path.to_string();
        config.agent.workspaces.clear();
        config.agent.active_workspace = None;
        config.ensure_workspace_defaults();
        config
    }

    #[tokio::test]
    async fn read_file_supports_offset_and_limit() {
        let dir = tempdir().expect("tempdir");
        let file_path = dir.path().join("sample.txt");
        std::fs::write(&file_path, "a\nb\nc\nd\n").expect("write file");

        let config = config_for_workspace(&dir.path().to_string_lossy());
        let tool = ReadFileTool::new(config, Arc::new(FileReadCache::with_default_capacity()));

        let result = Tool::execute_result(
            &tool,
            json!({
                "filePath": "sample.txt",
                "offset": 2,
                "limit": 2
            }),
        )
        .await
        .expect("read result");

        assert!(result.output.contains("2: b"));
        assert!(result.output.contains("3: c"));
        assert!(result.output.contains("Use offset=4 to continue"));
        assert_eq!(result.metadata["kind"], "file");
    }

    #[tokio::test]
    async fn read_file_can_read_directory_entries() {
        let dir = tempdir().expect("tempdir");
        std::fs::create_dir_all(dir.path().join("nested")).expect("create nested dir");
        std::fs::write(dir.path().join("root.txt"), "hello").expect("write file");

        let config = config_for_workspace(&dir.path().to_string_lossy());
        let tool = ReadFileTool::new(config, Arc::new(FileReadCache::with_default_capacity()));

        let result = Tool::execute_result(
            &tool,
            json!({
                "filePath": ".",
                "offset": 1,
                "limit": 10
            }),
        )
        .await
        .expect("directory read result");

        assert!(result.output.contains("nested/"));
        assert!(result.output.contains("root.txt"));
        assert_eq!(result.metadata["kind"], "directory");
    }

    #[tokio::test]
    async fn read_file_repeat_reads_hit_cache() {
        let dir = tempdir().expect("tempdir");
        std::fs::write(dir.path().join("cached.txt"), "one\ntwo\nthree\n").expect("write file");

        let config = config_for_workspace(&dir.path().to_string_lossy());
        let cache = Arc::new(FileReadCache::with_default_capacity());
        let tool = ReadFileTool::new(config, cache.clone());

        let first = Tool::execute_result(
            &tool,
            json!({ "filePath": "cached.txt", "offset": 1, "limit": 2 }),
        )
        .await
        .expect("first read");
        assert!(first.output.contains("1: one"));
        assert!(cache.bytes_used() > 0);

        // Second read of a different window must still hit the cache.
        let (entries_before, hits_before, _) = cache.stats();
        assert!(entries_before > 0);
        let second = Tool::execute_result(
            &tool,
            json!({ "filePath": "cached.txt", "offset": 2, "limit": 2 }),
        )
        .await
        .expect("second read");
        assert!(second.output.contains("2: two"));
        assert!(second.output.contains("3: three"));
        let (_, hits_after, _) = cache.stats();
        assert!(hits_after > hits_before);
    }

    #[tokio::test]
    async fn read_file_window_matches_full_read() {
        let dir = tempdir().expect("tempdir");
        let body: String = (1..=500).map(|i| format!("line {i}")).collect::<Vec<_>>().join("\n");
        std::fs::write(dir.path().join("big.txt"), &body).expect("write file");

        let config = config_for_workspace(&dir.path().to_string_lossy());
        let tool = ReadFileTool::new(config, Arc::new(FileReadCache::with_default_capacity()));

        let window = Tool::execute_result(
            &tool,
            json!({ "filePath": "big.txt", "offset": 200, "limit": 50 }),
        )
        .await
        .expect("window read");
        assert!(window.output.contains("200: line 200"));
        assert!(window.output.contains("249: line 249"));
        assert!(!window.output.contains("199: line 199"));
        assert_eq!(window.metadata["total_lines"], 500);
    }

    #[test]
    fn streaming_window_handles_crlf_and_no_trailing_newline() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("crlf.txt");
        std::fs::write(&path, "a\r\nb\r\nc").expect("write file");
        let window = read_window_from_disk(&path, 1, 10).expect("window");
        match window {
            WindowRead::Found { lines, total_lines, .. } => {
                assert_eq!(total_lines, 3);
                assert_eq!(lines, vec!["a".to_string(), "b".to_string(), "c".to_string()]);
            }
            other => panic!("unexpected window: {other:?}"),
        }
    }
}
