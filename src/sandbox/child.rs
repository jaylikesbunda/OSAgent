//! Cross-platform sandboxed child processes.
//!
//! [`SandboxedChild`] is deliberately *not* `tokio::process::Child`: the
//! Windows sandbox spawns with `CreateProcessAsUserW` (restricted token,
//! AppContainer capabilities, Job Object), which tokio cannot express. The
//! surface covers what the tools need — pipe handles, blocking *and* async
//! wait, and a kill-tree guard on drop.

use std::ffi::{OsStr, OsString};
use std::io;
use std::path::PathBuf;
use std::process::{ExitStatus, Output};
use std::time::Duration;

/// How a standard handle is wired for the child.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum StdioKind {
    /// `NUL` / `/dev/null`.
    #[default]
    Null,
    /// The parent's handle (interactive-ish tools).
    Inherit,
    /// A pipe owned by the parent, readable/writable through the child's
    /// handle fields.
    Pipe,
}

/// Everything needed to spawn one process under a [`crate::sandbox::Sandbox`].
#[derive(Debug, Clone)]
pub struct SpawnSpec {
    pub program: OsString,
    pub args: Vec<OsString>,
    /// Windows only: an exact `lpCommandLine` when the caller depends on raw
    /// quoting (the bash tool passes `cmd /C <command>` verbatim). When set it
    /// replaces `program`/`args` for the actual `CreateProcess` call.
    pub windows_raw_cmdline: Option<String>,
    pub cwd: PathBuf,
    pub envs: Vec<(OsString, OsString)>,
    pub clear_env: bool,
    pub stdin: StdioKind,
    pub stdout: StdioKind,
    pub stderr: StdioKind,
    /// Lead a fresh process group (Unix) so kill-tree can signal the whole
    /// group. The Windows sandbox always uses its Job Object for this.
    pub new_process_group: bool,
}

impl SpawnSpec {
    pub fn new(
        program: impl Into<OsString>,
        args: impl IntoIterator<Item = impl Into<OsString>>,
    ) -> Self {
        Self {
            program: program.into(),
            args: args.into_iter().map(Into::into).collect(),
            windows_raw_cmdline: None,
            cwd: std::env::current_dir().unwrap_or_else(|_| PathBuf::from(".")),
            envs: Vec::new(),
            clear_env: false,
            stdin: StdioKind::Null,
            stdout: StdioKind::Pipe,
            stderr: StdioKind::Pipe,
            new_process_group: true,
        }
    }

    pub fn cwd(mut self, cwd: impl Into<PathBuf>) -> Self {
        self.cwd = cwd.into();
        self
    }

    pub fn env(mut self, key: impl Into<OsString>, value: impl Into<OsString>) -> Self {
        self.envs.push((key.into(), value.into()));
        self
    }

    pub fn stdio(mut self, stdin: StdioKind, stdout: StdioKind, stderr: StdioKind) -> Self {
        self.stdin = stdin;
        self.stdout = stdout;
        self.stderr = stderr;
        self
    }

    pub fn windows_raw_cmdline(mut self, raw: impl Into<String>) -> Self {
        self.windows_raw_cmdline = Some(raw.into());
        self
    }
}

/// A process spawned under the sandbox. Killing it kills the whole tree: the
/// Job Object on Windows, the process group on Unix. Dropping a child that is
/// still running terminates it (the tool layer wants no orphans; long-running
/// services use the `process` tool, which owns its child explicitly).
pub struct SandboxedChild {
    pub(crate) inner: super::platform::ChildHandle,
    stdin: Option<std::fs::File>,
    stdout: Option<std::fs::File>,
    stderr: Option<std::fs::File>,
}

impl SandboxedChild {
    pub(crate) fn new(
        inner: super::platform::ChildHandle,
        stdin: Option<std::fs::File>,
        stdout: Option<std::fs::File>,
        stderr: Option<std::fs::File>,
    ) -> Self {
        Self {
            inner,
            stdin,
            stdout,
            stderr,
        }
    }

    pub fn id(&self) -> u32 {
        self.inner.id()
    }

    pub fn take_stdin(&mut self) -> Option<std::fs::File> {
        self.stdin.take()
    }

    pub fn take_stdout(&mut self) -> Option<std::fs::File> {
        self.stdout.take()
    }

    pub fn take_stderr(&mut self) -> Option<std::fs::File> {
        self.stderr.take()
    }

    pub fn is_finished(&self) -> bool {
        self.inner.is_finished()
    }

    /// Blocking wait. Used by the `process` tool's reader threads.
    pub fn wait_blocking(&mut self) -> io::Result<ExitStatus> {
        self.inner.wait_blocking()
    }

    /// Async wait. Polls the handle with a short backoff so the runtime never
    /// blocks; process waits here are tool-scoped and short.
    pub async fn wait(&mut self) -> io::Result<ExitStatus> {
        let mut delay = std::time::Duration::from_millis(10);
        loop {
            if self.inner.is_finished() {
                break;
            }
            if let Some(status) = self.inner.try_wait()? {
                return Ok(status);
            }
            tokio::time::sleep(delay).await;
            if delay < std::time::Duration::from_millis(100) {
                delay *= 2;
            }
        }
        self.inner.wait_blocking()
    }

    /// Drain stdout/stderr concurrently, then wait for exit.
    pub async fn wait_with_output(mut self) -> io::Result<Output> {
        use tokio::io::AsyncReadExt;

        let read_all = |file: Option<std::fs::File>| async move {
            let mut buffer = Vec::new();
            if let Some(file) = file {
                let mut async_file = tokio::fs::File::from_std(file);
                let _ = async_file.read_to_end(&mut buffer).await;
            }
            buffer
        };

        let stdout_task = tokio::spawn(read_all(self.stdout.take()));
        let stderr_task = tokio::spawn(read_all(self.stderr.take()));

        let status = self.wait().await?;

        let stdout = stdout_task.await.unwrap_or_default();
        let stderr = stderr_task.await.unwrap_or_default();

        Ok(Output {
            status,
            stdout,
            stderr,
        })
    }

    /// Terminate the whole process tree.
    pub fn kill_tree(&mut self) {
        self.inner.kill_tree();
    }

    /// Wait up to `timeout`, then kill the tree and return `None`. Keeps
    /// whatever the pipes already delivered (callers read streams themselves).
    pub async fn wait_timeout(&mut self, timeout: Duration) -> io::Result<Option<ExitStatus>> {
        match tokio::time::timeout(timeout, self.wait()).await {
            Ok(result) => result.map(Some),
            Err(_) => {
                self.kill_tree();
                let _ = tokio::time::timeout(Duration::from_secs(5), self.wait()).await;
                Ok(None)
            }
        }
    }
}

impl Drop for SandboxedChild {
    fn drop(&mut self) {
        if !self.inner.is_finished() {
            self.inner.kill_tree();
        }
    }
}

/// Quote one argument per the MSVCRT rules (Windows command line building).
/// Also used by the sandbox launcher to reconstruct `lpCommandLine`.
#[cfg(windows)]
pub fn quote_windows_arg(arg: &OsStr) -> String {
    use std::os::windows::ffi::OsStrExt;

    const SPACE: u16 = 0x20;
    const TAB: u16 = 0x09;
    const NEWLINE: u16 = 0x0A;
    const VTAB: u16 = 0x0B;
    const QUOTE: u16 = 0x22;
    const BACKSLASH: u16 = 0x5C;

    let text: Vec<u16> = arg.encode_wide().collect();
    if !text.is_empty()
        && !text
            .iter()
            .any(|unit| matches!(*unit, SPACE | TAB | NEWLINE | VTAB | QUOTE))
    {
        return String::from_utf16_lossy(&text);
    }

    let mut out = String::from("\"");
    let mut backslashes = 0u32;
    for unit in text {
        match unit {
            BACKSLASH => backslashes += 1,
            QUOTE => {
                out.push_str(&"\\".repeat((backslashes * 2 + 1) as usize));
                out.push('"');
                backslashes = 0;
            }
            other => {
                out.push_str(&"\\".repeat(backslashes as usize));
                backslashes = 0;
                out.push(char::from_u32(other as u32).unwrap_or('\u{FFFD}'));
            }
        }
    }
    out.push_str(&"\\".repeat((backslashes * 2) as usize));
    out.push('"');
    out
}

#[cfg(not(windows))]
pub fn quote_windows_arg(arg: &OsStr) -> String {
    arg.to_string_lossy().into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quoting_round_trips_simple_args() {
        #[cfg(windows)]
        {
            assert_eq!(quote_windows_arg(OsStr::new("hello")), "hello");
            assert_eq!(quote_windows_arg(OsStr::new("a b")), "\"a b\"");
            assert_eq!(
                quote_windows_arg(OsStr::new("say \"hi\"")),
                "\"say \\\"hi\\\"\""
            );
            // No whitespace/quotes means no quoting needed, even with a
            // trailing backslash: the backslash rule only applies inside
            // quoted spans.
            assert_eq!(quote_windows_arg(OsStr::new("trail\\")), "trail\\");
            assert_eq!(quote_windows_arg(OsStr::new("a b\\")), "\"a b\\\\\"");
        }
        #[cfg(not(windows))]
        {
            // Non-Windows builds pass args through unchanged: argv arrays
            // need no quoting.
            assert_eq!(quote_windows_arg(OsStr::new("a b")), "a b");
            assert_eq!(quote_windows_arg(OsStr::new("say \"hi\"")), "say \"hi\"");
        }
    }
}
