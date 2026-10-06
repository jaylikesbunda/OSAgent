use crate::config::{BashMode, BashToolConfig, Config};
use crate::error::{OSAgentError, Result};
use crate::tools::guard::{command_touches_backups, ensure_relative_path_not_backups};
use crate::tools::output::maybe_store_large_output_result;
use crate::tools::registry::{Tool, ToolExample, ToolOutcome, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use std::path::PathBuf;
use std::time::Duration;

/// Owns only this tool's shell tree; dropping the execution future kills it.
struct CommandTreeGuard(Option<u32>);

impl Drop for CommandTreeGuard {
    fn drop(&mut self) {
        let Some(pid) = self.0 else { return };
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            let _ = std::process::Command::new("taskkill")
                .args(["/F", "/T", "/PID", &pid.to_string()])
                .creation_flags(0x08000000)
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .status();
        }
        #[cfg(unix)]
        unsafe {
            // The child leads a dedicated process group; include descendants.
            libc::kill(-(pid as i32), libc::SIGKILL);
        }
    }
}

#[cfg(test)]
async fn run_shell_command(
    command: &str,
    workspace: &std::path::Path,
) -> std::io::Result<std::process::Output> {
    #[cfg(windows)]
    let mut builder = {
        use std::os::windows::process::CommandExt;
        let mut builder = tokio::process::Command::new("cmd");
        builder
            .as_std_mut()
            .raw_arg(format!("/C {}", command))
            .creation_flags(0x08000000);
        builder
    };
    #[cfg(not(windows))]
    let mut builder = {
        let mut builder = tokio::process::Command::new("sh");
        builder.args(["-lc", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            builder.as_std_mut().process_group(0);
        }
        builder
    };
    let child = builder
        .current_dir(workspace)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;
    let mut guard = CommandTreeGuard(child.id());
    let output = child.wait_with_output().await?;
    guard.0 = None;
    Ok(output)
}

/// Result of a bounded shell run that keeps whatever the process wrote before
/// it was terminated. The previous timeout path returned a bare
/// `OSAgentError::Timeout` and discarded partial stdout/stderr, so a timed-out
/// build looked like it produced nothing and the agent had no error text to
/// act on.
struct ShellCapture {
    stdout: String,
    stderr: String,
    exit_code: Option<i32>,
    timed_out: bool,
}

async fn drain_pipe<R>(mut reader: R, buffer: std::sync::Arc<std::sync::Mutex<String>>)
where
    R: tokio::io::AsyncRead + Unpin,
{
    use tokio::io::AsyncReadExt;
    let mut chunk = vec![0u8; 8192];
    loop {
        match reader.read(&mut chunk).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let text = String::from_utf8_lossy(&chunk[..read]).into_owned();
                if let Ok(mut guard) = buffer.lock() {
                    guard.push_str(&text);
                }
            }
        }
    }
}

async fn run_shell_command_capture(
    command: &str,
    workspace: &std::path::Path,
    timeout: Duration,
) -> std::io::Result<ShellCapture> {
    #[cfg(windows)]
    let mut builder = {
        use std::os::windows::process::CommandExt;
        let mut builder = tokio::process::Command::new("cmd");
        builder
            .as_std_mut()
            .raw_arg(format!("/C {}", command))
            .creation_flags(0x08000000);
        builder
    };
    #[cfg(not(windows))]
    let mut builder = {
        let mut builder = tokio::process::Command::new("sh");
        builder.args(["-lc", command]);
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            builder.as_std_mut().process_group(0);
        }
        builder
    };

    let mut child = builder
        .current_dir(workspace)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()?;

    let stdout_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));
    let stderr_buffer = std::sync::Arc::new(std::sync::Mutex::new(String::new()));

    let stdout_task = child.stdout.take().map(|stream| {
        let buffer = stdout_buffer.clone();
        tokio::spawn(async move { drain_pipe(stream, buffer).await })
    });
    let stderr_task = child.stderr.take().map(|stream| {
        let buffer = stderr_buffer.clone();
        tokio::spawn(async move { drain_pipe(stream, buffer).await })
    });

    let mut guard = CommandTreeGuard(child.id());
    let (exit_code, timed_out) = match tokio::time::timeout(timeout, child.wait()).await {
        Ok(Ok(status)) => {
            guard.0 = None;
            (status.code(), false)
        }
        Ok(Err(error)) => return Err(error),
        Err(_) => {
            // Dropping the guard `taskkill`s the whole tree; keep whatever the
            // pipes already delivered.
            drop(guard);
            (None, true)
        }
    };

    let join_readers = async {
        if let Some(task) = stdout_task {
            let _ = task.await;
        }
        if let Some(task) = stderr_task {
            let _ = task.await;
        }
    };
    // Bound the wait: if a killed child refuses to release its pipes we still
    // return the captured output instead of hanging the turn.
    let _ = tokio::time::timeout(Duration::from_millis(1000), join_readers).await;

    let stdout = stdout_buffer
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();
    let stderr = stderr_buffer
        .lock()
        .map(|guard| guard.clone())
        .unwrap_or_default();

    Ok(ShellCapture {
        stdout,
        stderr,
        exit_code,
        timed_out,
    })
}

#[cfg(test)]
mod cancellation_tests {
    use super::*;

    #[tokio::test]
    async fn dropping_command_future_kills_child_before_it_writes() {
        let workspace = tempfile::tempdir().unwrap();
        let ready = workspace.path().join("ready");
        let late = workspace.path().join("late");
        #[cfg(windows)]
        let command = format!(
            "powershell -NoProfile -Command \"Set-Content -LiteralPath '{}' -Value ready; Start-Sleep -Milliseconds 1200; Set-Content -LiteralPath '{}' -Value late\"",
            ready.display().to_string().replace('\'', "''"), late.display().to_string().replace('\'', "''")
        );
        #[cfg(not(windows))]
        let command = format!(
            "printf ready > '{}'; sleep 1.2; printf late > '{}'",
            ready.display(),
            late.display()
        );
        let cwd = workspace.path().to_path_buf();
        let task = tokio::spawn(async move { run_shell_command(&command, &cwd).await });
        let ready_wait = tokio::time::timeout(Duration::from_secs(10), async {
            while !ready.exists() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await;
        task.abort();
        let outcome = task.await;
        assert!(
            ready_wait.is_ok(),
            "Child failed to reach the controlled cancellation point"
        );
        assert!(outcome.unwrap_err().is_cancelled());
        tokio::time::sleep(Duration::from_millis(1400)).await;
        assert!(
            !late.exists(),
            "The shell descendant kept running after cancellation"
        );
    }
}

pub struct BashTool {
    config: BashToolConfig,
    workspaces: Vec<PathBuf>,
    writable: bool,
    description: String,
}

impl BashTool {
    fn default_workspace(&self) -> Result<PathBuf> {
        self.workspaces.first().cloned().ok_or_else(|| {
            OSAgentError::ToolExecution(
                "No workspace configured. Set a workspace path in settings.".to_string(),
            )
        })
    }

    pub fn new(config: Config) -> Self {
        let writable = config.is_workspace_writable_for_path(&config.agent.workspace);
        let workspaces: Vec<PathBuf> = config
            .get_active_workspace()
            .paths
            .iter()
            .map(|wp| {
                let path = PathBuf::from(shellexpand::tilde(&wp.path).to_string());
                if !path.exists() {
                    let _ = std::fs::create_dir_all(&path);
                }
                path.canonicalize().unwrap_or(path)
            })
            .collect();

        let description = if cfg!(windows) {
            "Execute a shell command with optional timeout and working directory.\n\nUsage:\n- Runs via `cmd /C` on this Windows host — use Windows/cmd syntax (dir, findstr, del, cmd built-ins, PowerShell via `powershell -Command \"...\"`), NOT POSIX/Unix syntax. Commands like `ls`, `/mnt/c/...`, `2>/dev/null`, or `cat` will fail with 'not recognized'.\n- Use for builds, tests, linting, git operations (staging, diff, log), package management, and any CLI commands.\n- Commands are workspace-scoped by default. Use workdir to run in a subdirectory.\n- Synchronous with a timeout (max 300s). For long-running servers, watch-mode commands, or background jobs, use the `process` tool (start/poll/log/kill) instead of blocking bash.\n- Direct deletes (rm, del) are blocked - use delete_file instead.\n- Do NOT use for simple file reads (use read_file) or content searches (use grep/glob).\n- Do NOT use for file edits (use edit_file or apply_patch).\n- When making multiple independent bash calls, send them in a single message to run in parallel."
        } else {
            "Execute a shell command with optional timeout and working directory.\n\nUsage:\n- Runs via `sh -lc` — use POSIX/Unix shell syntax.\n- Use for builds, tests, linting, git operations (staging, diff, log), package management, and any CLI commands.\n- Commands are workspace-scoped by default. Use workdir to run in a subdirectory.\n- Synchronous with a timeout (max 300s). For long-running servers, watch-mode commands, or background jobs, use the `process` tool (start/poll/log/kill) instead of blocking bash.\n- Direct deletes (rm, del) are blocked - use delete_file instead.\n- Do NOT use for simple file reads (use read_file) or content searches (use grep/glob).\n- Do NOT use for file edits (use edit_file or apply_patch).\n- When making multiple independent bash calls, send them in a single message to run in parallel."
        }
        .to_string();

        Self {
            config: config.tools.bash,
            workspaces,
            writable,
            description,
        }
    }

    /// Words in `command`, lowercased, with single- and double-quoted spans
    /// removed. Returns whether the quote state was balanced: on unbalanced
    /// quotes the caller must fall back to scanning everything (fail closed),
    /// otherwise a stray quote could hide a real mutation.
    fn words_outside_quotes(command: &str) -> (Vec<String>, bool) {
        let lowered = command.to_lowercase();
        let chars: Vec<char> = lowered.chars().collect();
        let mut words = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut escaped = false;

        let mut flush = |current: &mut String, words: &mut Vec<String>| {
            if !current.is_empty() {
                words.push(std::mem::take(current));
            }
        };

        for ch in chars {
            if escaped {
                escaped = false;
                if !in_single && !in_double {
                    current.push(ch);
                }
                continue;
            }
            if ch == '\\' && !in_single {
                escaped = true;
                continue;
            }
            match ch {
                '\'' if !in_double => {
                    in_single = !in_single;
                }
                '"' if !in_single => {
                    in_double = !in_double;
                }
                _ => {
                    if in_single || in_double {
                        continue;
                    }
                    if ch.is_ascii_alphanumeric() || ch == '-' || ch == '_' {
                        current.push(ch);
                    } else {
                        flush(&mut current, &mut words);
                    }
                }
            }
        }
        flush(&mut current, &mut words);

        (words, !in_single && !in_double)
    }

    /// Split into whole words so a command word is only matched as a word.
    /// A raw substring test produces false positives on ordinary arguments
    /// (e.g. "Format-Table" contains "rm", "different" contains "ren").
    /// Quoted spans are excluded (see `words_outside_quotes`): prose inside
    /// string literals such as `'(add+del files): '` must not trip the `del`
    /// token. Falls back to scanning everything when quotes are unbalanced.
    fn validation_words(command: &str) -> Vec<String> {
        let (words, balanced) = Self::words_outside_quotes(command);
        if balanced {
            return words;
        }
        command
            .to_lowercase()
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .filter(|word| !word.is_empty())
            .map(|word| word.to_string())
            .collect()
    }

    /// Script bodies passed to `powershell -Command ...` / `pwsh -c ...`,
    /// unwrapped one level so nested code is validated with the same rules.
    /// Without this, quote-aware scanning would hide real nested mutations
    /// such as `powershell -Command "Remove-Item foo"`. Depth-limited to
    /// terminate on adversarial nesting.
    fn nested_powershell_scripts(command: &str, depth: usize) -> Vec<String> {
        if depth > 4 {
            return Vec::new();
        }
        let mut scripts = Vec::new();
        for segment in Self::split_segments(command) {
            let Some(head) = Self::first_token(&segment).map(|h| h.to_lowercase()) else {
                continue;
            };
            if !matches!(
                head.as_str(),
                "powershell" | "pwsh" | "powershell.exe" | "pwsh.exe"
            ) {
                continue;
            }
            if let Some(script) = Self::powershell_command_arg(&segment) {
                scripts.push(script.clone());
                scripts.extend(Self::nested_powershell_scripts(&script, depth + 1));
            }
        }
        scripts
    }

    /// The argument following `-Command` / `-c` in a PowerShell invocation,
    /// with one layer of surrounding quotes removed.
    fn powershell_command_arg(segment: &str) -> Option<String> {
        let tokens = Self::split_arg_tokens(segment);
        let mut iter = tokens.iter().peekable();
        while let Some(token) = iter.next() {
            let flag = token.to_lowercase();
            if flag == "-command" || flag == "-c" {
                let arg = iter.next()?.clone();
                return Some(Self::strip_one_quote_layer(&arg));
            }
        }
        None
    }

    /// Quote-aware argv split: whitespace separates tokens except inside
    /// single/double quotes (quotes are retained on the token).
    fn split_arg_tokens(segment: &str) -> Vec<String> {
        let chars: Vec<char> = segment.chars().collect();
        let mut tokens = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let mut has_content = false;

        for ch in chars {
            match ch {
                '\'' if !in_double => {
                    in_single = !in_single;
                    current.push(ch);
                    has_content = true;
                }
                '"' if !in_single => {
                    in_double = !in_double;
                    current.push(ch);
                    has_content = true;
                }
                c if c.is_whitespace() && !in_single && !in_double => {
                    if has_content {
                        tokens.push(std::mem::take(&mut current));
                        has_content = false;
                    }
                }
                _ => {
                    current.push(ch);
                    has_content = true;
                }
            }
        }
        if has_content {
            tokens.push(current);
        }
        tokens
    }

    fn strip_one_quote_layer(arg: &str) -> String {
        let trimmed = arg.trim();
        if trimmed.len() >= 2 {
            let bytes = trimmed.as_bytes();
            let (first, last) = (bytes[0], bytes[trimmed.len() - 1]);
            if (first == b'\'' && last == b'\'') || (first == b'"' && last == b'"') {
                return trimmed[1..trimmed.len() - 1].to_string();
            }
        }
        trimmed.to_string()
    }

    fn validate_non_mutating_command(command: &str) -> Result<()> {
        // Nested PowerShell scripts are code, not prose: validate them with
        // the same rules before checking the outer command line.
        for script in Self::nested_powershell_scripts(command, 0) {
            Self::validate_non_mutating_command(&script)?;
        }

        let mutating_tokens = [
            "mkdir",
            "rmdir",
            "del",
            "rm",
            "copy",
            "cp",
            "move",
            "mv",
            "rename",
            "ren",
            "touch",
            "git add",
            "git apply",
            "git commit",
            "git checkout",
            "git clean",
            "git restore",
            "npm install",
            "npm update",
            "pnpm install",
            "yarn install",
            "cargo add",
            "cargo fix",
            "set-content",
            "add-content",
            "out-file",
            "new-item",
            "remove-item",
            "copy-item",
            "move-item",
            ">",
            ">>",
        ];

        let words = Self::validation_words(command);

        let matched = mutating_tokens.iter().find(|token| {
            if token.contains('>') {
                // Redirection operators are punctuation, not words. Ignore
                // operators inside quoted arguments (for example Python code
                // passed to `python -c`) and comparison operators such as
                // `>=`.
                Self::contains_unquoted_output_redirection(command)
            } else if let Some((head, tail)) = token.split_once(' ') {
                // Multi-word forms like "git add" must appear adjacently.
                words
                    .windows(2)
                    .any(|pair| pair[0] == head && pair[1] == tail)
            } else {
                words.iter().any(|word| word == *token)
            }
        });

        if let Some(token) = matched {
            let detail = if token.contains('>') {
                "output redirection".to_string()
            } else {
                format!("'{}'", token)
            };
            return Err(OSAgentError::ToolExecution(format!(
                "Bash read-only mode is limited to non-mutating commands (matched {})",
                detail
            )));
        }

        Ok(())
    }

    fn contains_unquoted_output_redirection(command: &str) -> bool {
        let chars: Vec<char> = command.chars().collect();
        let mut in_single_quote = false;
        let mut in_double_quote = false;
        let mut escaped = false;

        for (index, ch) in chars.iter().enumerate() {
            if escaped {
                escaped = false;
                continue;
            }

            if *ch == '\\' && !in_single_quote {
                escaped = true;
                continue;
            }

            match ch {
                '\'' if !in_double_quote => {
                    in_single_quote = !in_single_quote;
                }
                '"' if !in_single_quote => {
                    in_double_quote = !in_double_quote;
                }
                '>' if !in_single_quote
                    && !in_double_quote
                    && chars.get(index + 1) != Some(&'=') =>
                {
                    // `>=` is a comparison operator, not shell output
                    // redirection. `>>`, `>&1`, and ordinary `> file` remain
                    // blocked.
                    return true;
                }
                _ => {}
            }
        }

        false
    }

    fn ensure_read_only_safe(&self, command: &str) -> Result<()> {
        if self.writable {
            return Ok(());
        }

        Self::validate_non_mutating_command(command)
    }

    pub fn validate_explicit_read_only(command: &str) -> Result<()> {
        Self::validate_non_mutating_command(command)
    }

    fn validate_workdir(&self, workdir: Option<&str>) -> Result<PathBuf> {
        let default_ws = self.workspaces.first().ok_or_else(|| {
            OSAgentError::ToolExecution(
                "No workspace configured. Set a workspace path in settings.".to_string(),
            )
        })?;

        let Some(workdir) = workdir.map(str::trim).filter(|value| !value.is_empty()) else {
            return Ok(default_ws.clone());
        };

        ensure_relative_path_not_backups(workdir)?;

        let resolved = default_ws.join(workdir);
        let canonical = resolved.canonicalize().unwrap_or(resolved.clone());

        if !canonical.exists() {
            return Err(OSAgentError::ToolExecution(format!(
                "workdir does not exist: {}",
                workdir
            )));
        }

        if !canonical.is_dir() {
            return Err(OSAgentError::ToolExecution(format!(
                "workdir is not a directory: {}",
                workdir
            )));
        }

        Ok(canonical)
    }

    fn first_token(segment: &str) -> Option<String> {
        let trimmed = segment.trim_start();
        if trimmed.is_empty() {
            return None;
        }

        let mut token = String::new();
        let mut in_single = false;
        let mut in_double = false;

        for ch in trimmed.chars() {
            match ch {
                '\'' if !in_double => in_single = !in_single,
                '"' if !in_single => in_double = !in_double,
                c if c.is_whitespace() && !in_single && !in_double => break,
                _ => token.push(ch),
            }
        }

        let token = token
            .trim_matches(|ch| ch == '\'' || ch == '"')
            .trim()
            .to_string();
        if token.is_empty() {
            None
        } else {
            Some(token)
        }
    }

    /// Split a command line on `&&`, `||`, `|` and `;`, ignoring separators
    /// inside quotes. Used both for head extraction and for finding nested
    /// PowerShell invocations.
    fn split_segments(command: &str) -> Vec<String> {
        let mut segments = Vec::new();
        let mut current = String::new();
        let mut in_single = false;
        let mut in_double = false;
        let chars: Vec<char> = command.chars().collect();
        let mut idx = 0usize;

        while idx < chars.len() {
            let ch = chars[idx];
            match ch {
                '\'' if !in_double => {
                    in_single = !in_single;
                    current.push(ch);
                    idx += 1;
                }
                '"' if !in_single => {
                    in_double = !in_double;
                    current.push(ch);
                    idx += 1;
                }
                '&' if !in_single
                    && !in_double
                    && idx + 1 < chars.len()
                    && chars[idx + 1] == '&' =>
                {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                    idx += 2;
                }
                '|' if !in_single
                    && !in_double
                    && idx + 1 < chars.len()
                    && chars[idx + 1] == '|' =>
                {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                    idx += 2;
                }
                '|' | ';' if !in_single && !in_double => {
                    if !current.trim().is_empty() {
                        segments.push(current.trim().to_string());
                    }
                    current.clear();
                    idx += 1;
                }
                _ => {
                    current.push(ch);
                    idx += 1;
                }
            }
        }

        if !current.trim().is_empty() {
            segments.push(current.trim().to_string());
        }
        segments
    }

    fn extract_command_heads(command: &str) -> Vec<String> {
        Self::split_segments(command)
            .into_iter()
            .filter_map(|segment| Self::first_token(&segment))
            .collect()
    }

    fn quote_arg(arg: &str) -> String {
        if arg.is_empty() {
            return "\"\"".to_string();
        }

        #[cfg(windows)]
        {
            if arg.contains([' ', '\t', '"', '&', '|', '<', '>']) {
                format!("\"{}\"", arg.replace('"', "\\\""))
            } else {
                arg.to_string()
            }
        }

        #[cfg(not(windows))]
        {
            if arg
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || "-._/:=@".contains(ch))
            {
                arg.to_string()
            } else {
                format!("'{}'", arg.replace('\'', "'\"'\"'"))
            }
        }
    }

    pub fn build_command(command: &str, args_list: &[String]) -> String {
        if args_list.is_empty() {
            return command.to_string();
        }

        let suffix = args_list
            .iter()
            .map(|arg| Self::quote_arg(arg))
            .collect::<Vec<_>>()
            .join(" ");
        format!("{} {}", command, suffix)
    }

    fn is_allowed_command(&self, command_head: &str) -> bool {
        self.config
            .allowed_commands
            .iter()
            .any(|allowed| allowed.eq_ignore_ascii_case(command_head))
    }

    fn is_blocked_command(&self, command_head: &str) -> bool {
        self.config
            .blocked_commands
            .iter()
            .any(|blocked| blocked.eq_ignore_ascii_case(command_head))
    }

    fn contains_blocked_delete(command: &str) -> bool {
        // Nested PowerShell scripts are code: `powershell -Command
        // "Remove-Item foo"` must stay blocked even though the cmdlet sits
        // inside quotes.
        for script in Self::nested_powershell_scripts(command, 0) {
            if Self::contains_blocked_delete(&script) {
                return true;
            }
        }
        // Prose inside string literals (e.g. `'(add+del files): '`) is not a
        // delete invocation.
        Self::validation_words(command).iter().any(|token| {
            matches!(
                token.as_str(),
                "rm" | "del" | "erase" | "rmdir" | "rd" | "remove-item"
            )
        })
    }

    fn contains_destructive_pattern(command: &str) -> bool {
        let lowered = command.to_lowercase();
        let patterns = [
            ("git push --force", "git force push"),
            ("git push -f ", "git force push"),
            ("git push --force-with-lease", "git force push"),
            ("git reset --hard", "git hard reset"),
            ("git clean -f", "git force clean"),
            ("git checkout -- .", "git checkout all changes"),
            ("git restore .", "git restore all changes"),
            ("rm -rf /", "recursive root delete"),
            ("rm -rf /*", "recursive root glob delete"),
            ("rm -rf ~", "recursive home delete"),
            ("rm -rf ~/", "recursive home delete"),
            ("rm -rf $home", "recursive home delete"),
            ("rd /s /q c:", "recursive windows root delete"),
            ("rd /s /q \\", "recursive windows root delete"),
            ("drop table", "SQL drop table"),
            ("truncate table", "SQL truncate table"),
            (":(){:|:&};:", "fork bomb"),
        ];

        for (pattern, label) in &patterns {
            if lowered.contains(pattern) {
                tracing::warn!(
                    "Blocked destructive pattern: {} (matched: {})",
                    label,
                    pattern
                );
                return true;
            }
        }

        let tokens: Vec<&str> = lowered
            .split(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '-' || ch == '_'))
            .filter(|t| !t.is_empty())
            .collect();

        for window in tokens.windows(3) {
            if (window[0] == "delete" || window[0] == "delete_from")
                && window[1] == "from"
                && !window[2..].contains(&"where")
            {
                tracing::warn!("Blocked destructive pattern: DELETE FROM without WHERE");
                return true;
            }
        }

        false
    }

    fn contains_injection(command: &str) -> Result<()> {
        let lowered = command.to_lowercase();

        let shell_builtins = ["eval ", "exec "];
        for builtin in &shell_builtins {
            if lowered.contains(builtin) {
                return Err(OSAgentError::ToolExecution(format!(
                    "Shell builtin '{}' is blocked to prevent injection attacks",
                    builtin.trim()
                )));
            }
        }

        if lowered.contains("$ifs") || lowered.contains("${ifs") {
            return Err(OSAgentError::ToolExecution(
                "$IFS manipulation is blocked".to_string(),
            ));
        }

        if lowered.contains("/proc/") && lowered.contains("/environ") {
            return Err(OSAgentError::ToolExecution(
                "Access to /proc/*/environ is blocked".to_string(),
            ));
        }

        if lowered.contains("$(") || lowered.contains('`') {
            if Self::contains_blocked_delete(command) {
                return Err(OSAgentError::ToolExecution(
                    "Command substitution wrapping delete commands is blocked".to_string(),
                ));
            }
            if Self::contains_destructive_pattern(command) {
                return Err(OSAgentError::ToolExecution(
                    "Command substitution wrapping destructive operations is blocked".to_string(),
                ));
            }
            if lowered.contains("/proc/") {
                return Err(OSAgentError::ToolExecution(
                    "Command substitution accessing /proc is blocked".to_string(),
                ));
            }
        }

        Ok(())
    }

    fn validate_commands(&self, command_heads: &[String]) -> Result<()> {
        match self.config.mode {
            BashMode::Permissive => {
                for head in command_heads {
                    if self.is_blocked_command(head) {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Command '{}' is blocked for system safety",
                            head
                        )));
                    }
                }
            }
            BashMode::Allowlist => {
                for head in command_heads {
                    if !self.is_allowed_command(head) {
                        return Err(OSAgentError::ToolExecution(format!(
                            "Command '{}' is not in the allowed list",
                            head
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[async_trait]
impl Tool for BashTool {
    fn name(&self) -> &str {
        "bash"
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(self.config.timeout_seconds.saturating_mul(1_000).max(1_000))
    }

    fn description(&self) -> &str {
        &self.description
    }

    fn when_to_use(&self) -> &str {
        "Use for running build tools, test suites, linters, git commands, package managers, and any system commands that aren't covered by dedicated tools."
    }

    fn when_not_to_use(&self) -> &str {
        "Do not use for file reads (use read_file), content searches (use grep/glob), file edits (use edit_file), or file creation (use write_file)."
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "Run focused validation".to_string(),
                input: json!({
                    "command": "cargo test",
                    "workdir": "osagent"
                }),
            },
            ToolExample {
                description: "Create project structure".to_string(),
                input: json!({
                    "command": "mkdir -p src/components src/utils"
                }),
            },
            ToolExample {
                description: "Use a subdirectory with a timeout override".to_string(),
                input: json!({
                    "command": "npm run build",
                    "workdir": "frontend",
                    "timeout_seconds": 120
                }),
            },
            ToolExample {
                description: "Run pip install".to_string(),
                input: json!({
                    "command": "pip install -r requirements.txt"
                }),
            },
            ToolExample {
                description: "Run bash in explicit read-only mode".to_string(),
                input: json!({
                    "command": "git status",
                    "read_only": true
                }),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "command": {
                    "type": "string",
                    "description": "The shell command to execute"
                },
                "args": {
                    "type": "array",
                    "items": {"type": "string"},
                    "description": "Optional extra arguments appended to the command"
                },
                "workdir": {
                    "type": "string",
                    "description": "Optional relative directory inside the workspace to run the command from"
                },
                "timeout_seconds": {
                    "type": "integer",
                    "minimum": 1,
                    "description": "Optional timeout override in seconds"
                },
                "read_only": {
                    "type": "boolean",
                    "description": "If true, enforce non-mutating read-only command validation even in writable workspaces"
                }
            },
            "required": ["command"]
        })
    }

    async fn execute(&self, args: Value) -> Result<String> {
        let result = self.execute_result(args).await?;
        Ok(result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        let command = args["command"].as_str().ok_or_else(|| {
            OSAgentError::ToolExecution("Missing 'command' parameter".to_string())
        })?;
        let workdir = args["workdir"].as_str();
        let timeout_seconds = args["timeout_seconds"]
            .as_u64()
            .unwrap_or(self.config.timeout_seconds)
            .clamp(1, 300);
        let explicit_read_only = args["read_only"].as_bool().unwrap_or(false);

        let args_list: Vec<String> = args["args"]
            .as_array()
            .map(|arr| {
                arr.iter()
                    .filter_map(|value| value.as_str().map(|value| value.to_string()))
                    .collect()
            })
            .unwrap_or_default();

        let full_command = Self::build_command(command, &args_list);
        self.ensure_read_only_safe(&full_command)?;
        if explicit_read_only {
            Self::validate_explicit_read_only(&full_command)?;
        }

        if command_touches_backups(&full_command) {
            return Err(OSAgentError::ToolExecution(
                "Access to backup files and .osagent_backups is blocked".to_string(),
            ));
        }

        if Self::contains_blocked_delete(&full_command) {
            return Err(OSAgentError::ToolExecution(
                "Direct shell deletes are blocked. Use delete_file or apply_patch so OSA can create managed backups first."
                    .to_string(),
            ));
        }

        if Self::contains_destructive_pattern(&full_command) {
            return Err(OSAgentError::ToolExecution(
                "Command contains a destructive pattern (force push, hard reset, root delete, etc.) that is blocked for safety."
                    .to_string(),
            ));
        }

        Self::contains_injection(&full_command)?;

        let command_heads = Self::extract_command_heads(&full_command);
        if command_heads.is_empty() {
            return Err(OSAgentError::ToolExecution("Empty command".to_string()));
        }

        self.validate_commands(&command_heads)?;

        let workspace = self.validate_workdir(workdir)?;
        let timeout_duration = Duration::from_secs(timeout_seconds);
        let capture = run_shell_command_capture(&full_command, &workspace, timeout_duration)
            .await
            .map_err(|e| {
                OSAgentError::ToolExecution(format!("Failed to spawn command: {}", e))
            })?;

        let (merged_output, exit_code) = if capture.timed_out {
            (
                format!(
                    "Command timed out after {}s and was terminated.\nStdout:\n{}\nStderr:\n{}",
                    timeout_seconds, capture.stdout, capture.stderr
                ),
                capture.exit_code.unwrap_or(-1),
            )
        } else if let Some(code) = capture.exit_code.filter(|code| *code != 0) {
            (
                format!(
                    "Exit code: {}\nStdout:\n{}\nStderr:\n{}",
                    code, capture.stdout, capture.stderr
                ),
                code,
            )
        } else if capture.stderr.is_empty() {
            (capture.stdout, 0)
        } else {
            (format!("{}\n{}", capture.stdout, capture.stderr), 0)
        };

        let summarized = maybe_store_large_output_result(
            &self.default_workspace()?,
            self.writable,
            "bash",
            &merged_output,
        );

        Ok(ToolResult {
            output: summarized.display_output,
            outcome: if capture.timed_out || exit_code != 0 {
                ToolOutcome::Failure
            } else {
                ToolOutcome::Success
            },
            title: Some(full_command),
            metadata: json!({
                "exit_code": exit_code,
                "timed_out": capture.timed_out,
                "truncated": summarized.truncated,
                "output_path": summarized.output_path,
                "original_chars": summarized.original_chars,
                "original_lines": summarized.original_lines,
            }),
            attachments: Vec::new(),
        })
    }
}

#[cfg(test)]
mod readonly_validation_tests {
    use super::BashTool;

    #[test]
    fn allows_commands_that_merely_contain_mutating_substrings() {
        for cmd in [
            r"powershell -Command Get-ChildItem 'C:\Users\deki\Documents' | Format-Table Name",
            "git diff --stat",
            "grep -r different .",
            "cargo tree",
            r"dir C:\Users\deki\Documents",
        ] {
            assert!(
                BashTool::validate_explicit_read_only(cmd).is_ok(),
                "should be allowed: {cmd}"
            );
        }
    }

    #[test]
    fn still_blocks_actually_mutating_commands() {
        for cmd in [
            "rm -rf build",
            "mkdir newdir",
            "git add .",
            "npm install",
            "echo hi > out.txt",
            "Remove-Item foo",
        ] {
            assert!(
                BashTool::validate_explicit_read_only(cmd).is_err(),
                "should be blocked: {cmd}"
            );
        }
    }

    #[test]
    fn allows_prose_inside_quoted_strings() {
        // Regression: a PowerShell format string containing "(add+del files)"
        // tokenized to the word `del` and was rejected in read-only mode, and
        // then would have tripped the shell-delete guard too.
        let cmd = r#"powershell -NoProfile -Command "$since='2026-08-17'; foreach($a in 'main','components','webui','build.py','docs'){ $c = git log --since=$since --pretty=oneline -- $a | Measure-Object | ForEach-Object Count; '{0,-14} {1,4} commits' -f $a, $c }; ''; 'loc churn since Aug 17:'; git log --since=$since --numstat --format='' -- main components | ForEach-Object { $_ } | Measure-Object -Line | ForEach-Object { 'lines touched (add+del files): ' + $_.Lines }""#;
        assert!(
            BashTool::validate_explicit_read_only(cmd).is_ok(),
            "quoted prose must not trip read-only validation"
        );
        assert!(
            !BashTool::contains_blocked_delete(cmd),
            "quoted prose must not trip the shell-delete guard"
        );
    }

    #[test]
    fn still_blocks_nested_powershell_deletes() {
        // Quote-aware scanning must not hide real code nested inside
        // `powershell -Command "..."`.
        for cmd in [
            r#"powershell -NoProfile -Command "Remove-Item foo""#,
            r#"powershell -Command 'del foo'"#,
            r#"pwsh -c "rm -rf build""#,
        ] {
            assert!(
                BashTool::validate_explicit_read_only(cmd).is_err(),
                "nested delete should be blocked: {cmd}"
            );
            assert!(
                BashTool::contains_blocked_delete(cmd),
                "nested delete should trip the shell-delete guard: {cmd}"
            );
        }
    }

    #[test]
    fn unbalanced_quotes_fail_closed() {
        // A stray quote must not hide a mutation from the scanner.
        assert!(BashTool::validate_explicit_read_only("echo \"hi; rm -rf /").is_err());
    }

    #[test]
    fn allows_comparisons_and_redirection_characters_inside_quoted_code() {
        for cmd in [
            r#"python -c "print(1 >= 0)""#,
            r#"python -c "print('left > right')""#,
        ] {
            assert!(
                BashTool::validate_explicit_read_only(cmd).is_ok(),
                "quoted code should not be treated as shell redirection: {cmd}"
            );
        }
    }

    #[test]
    fn still_blocks_unquoted_redirection_and_comparison_safe_checks() {
        for cmd in [
            "echo hi > out.txt",
            "echo hi >> out.txt",
            "cmd /c echo hi > out.txt",
        ] {
            assert!(
                BashTool::validate_explicit_read_only(cmd).is_err(),
                "unquoted redirection should be blocked: {cmd}"
            );
        }
        assert!(BashTool::validate_explicit_read_only("echo 1 >= 0").is_ok());
    }
}
