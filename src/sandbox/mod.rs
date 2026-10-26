//! OS sandboxing for tool execution.
//!
//! The sandbox mode (`read-only | workspace-write | danger-full-access`) is an
//! OS capability axis, separate from the approval policy in
//! [`crate::permission`] — the approval policy decides *who signs off*, the
//! sandbox decides *what is even possible* once a tool runs. Mirrors Codex's
//! split.
//!
//! Enforcement happens in two places:
//!
//! * **In-process tools** (file writes/edits/deletes) check
//!   [`Sandbox::check_write`] / [`Sandbox::check_delete`] — for them the tool
//!   layer *is* the boundary, since nothing is spawned.
//! * **Spawned processes** (`bash`, `process`, `code_*`) go through
//!   [`Sandbox::spawn`], which confines them with the platform sandbox:
//!   restricted token + mandatory-label ACLs + Job Object (+ WFP firewall when
//!   `network = "deny"`) on Windows, Landlock + seccomp on Linux, Seatbelt
//!   (`sandbox-exec`) on macOS.
//!
//! The advisory checks in `src/tools/bash.rs` (command blocklist etc.) remain
//! as defense in depth; the OS sandbox is the real boundary behind them.

use crate::config::{Config, SandboxMode, SandboxNetworkPolicy};
use crate::error::{OSAgentError, Result};
use std::path::{Path, PathBuf};
use std::sync::Arc;

mod child;
mod platform;

pub use child::{SandboxedChild, SpawnSpec, StdioKind};

/// What the sandbox will actually enforce on this host. Surfaced through the
/// system_status tool and the settings UI so the product never claims OS
/// enforcement it did not get (e.g. missing Linux kernel support, WFP without
/// admin rights).
#[derive(Debug, Clone, Default, serde::Serialize, serde::Deserialize)]
pub struct SandboxStatus {
    pub mode: String,
    pub network: String,
    pub os_enforcement: bool,
    /// "windows-restricted-token" | "landlock-seccomp" | "seatbelt" | "none"
    pub backend: String,
    /// True when spawned processes are genuinely confined by the OS.
    pub enforced: bool,
    /// Human-readable degradations ("WFP firewall requires admin rights").
    pub notes: Vec<String>,
}

/// A resolved sandbox policy: mode + writable roots + network policy.
pub struct Sandbox {
    mode: SandboxMode,
    network: SandboxNetworkPolicy,
    os_enforcement: bool,
    /// Canonicalized roots the agent may write (workspace roots, temp dirs,
    /// scratchpad, config `extra_roots`).
    roots: Arc<Vec<PathBuf>>,
    /// Status of the platform backend for this policy. Filled at spawn/first
    /// use by the platform code.
    platform: platform::PlatformState,
}

impl Sandbox {
    /// Build a sandbox from config and the workspace roots the current session
    /// may write. Roots that do not exist yet are kept (parents get the ACL
    /// treatment lazily at spawn time).
    pub fn from_config(config: &Config, workspace_roots: impl IntoIterator<Item = PathBuf>) -> Self {
        let mut roots: Vec<PathBuf> = workspace_roots
            .into_iter()
            .map(|path| {
                let expanded = PathBuf::from(shellexpand::tilde(&path.to_string_lossy()).to_string());
                canonicalize_best_effort(&expanded)
            })
            .collect();

        // Scratch space is always writable in workspace-write mode: builds and
        // interpreters need a temp dir, and Codex makes its scratchpad writable
        // for exactly this reason.
        for temp in temp_roots() {
            let canonical = temp.canonicalize().unwrap_or(temp);
            if !roots.contains(&canonical) {
                roots.push(canonical);
            }
        }
        for extra in &config.sandbox.extra_roots {
            let expanded = PathBuf::from(shellexpand::tilde(extra).to_string());
            let canonical = expanded.canonicalize().unwrap_or(expanded);
            if !roots.contains(&canonical) {
                roots.push(canonical);
            }
        }

        Self {
            mode: config.sandbox.mode,
            network: config.sandbox.network,
            os_enforcement: config.sandbox.os_enforcement,
            roots: Arc::new(roots),
            platform: platform::PlatformState::new(),
        }
    }

    /// A sandbox that allows everything (danger-full-access). Used for
    /// non-agent surfaces (updater, service plumbing) that must not be
    /// confined by the agent's policy.
    pub fn unrestricted() -> Self {
        Self {
            mode: SandboxMode::DangerFullAccess,
            network: SandboxNetworkPolicy::Allow,
            os_enforcement: false,
            roots: Arc::new(Vec::new()),
            platform: platform::PlatformState::new(),
        }
    }

    /// Sandbox for the config's active workspace roots. Convenience for
    /// in-process tools that only need the mutation gates.
    pub fn for_config(config: &Config) -> Self {
        let roots: Vec<PathBuf> = config
            .get_active_workspace()
            .paths
            .iter()
            .map(|wp| PathBuf::from(shellexpand::tilde(&wp.path).to_string()))
            .collect();
        Self::from_config(config, roots)
    }

    pub fn mode(&self) -> SandboxMode {
        self.mode
    }

    pub fn network(&self) -> SandboxNetworkPolicy {
        self.network
    }

    pub fn roots(&self) -> &[PathBuf] {
        &self.roots
    }

    pub fn is_unrestricted(&self) -> bool {
        matches!(self.mode, SandboxMode::DangerFullAccess)
    }

    /// Would an OS-level spawn be confined on this policy?
    pub fn confines_spawns(&self) -> bool {
        !self.is_unrestricted() && self.os_enforcement
    }

    fn is_writable_root(&self, path: &Path) -> bool {
        let path = canonicalize_best_effort(path);
        self.roots.iter().any(|root| path.starts_with(root))
    }

    /// Tool-layer write gate for in-process tools. Returns an error the agent
    /// can act on (it names the mode and what to do instead).
    pub fn check_write(&self, path: &Path) -> Result<()> {
        if self.mode.allows_unrestricted_writes() {
            return Ok(());
        }
        if !self.mode.allows_writes() {
            return Err(OSAgentError::ToolExecution(format!(
                "Blocked by sandbox (read-only): cannot modify {}. Switch the sandbox to workspace-write or danger-full-access in Settings -> Sandbox to allow writes.",
                path.display()
            )));
        }
        if self.is_writable_root(path) {
            return Ok(());
        }
        Err(OSAgentError::ToolExecution(format!(
            "Blocked by sandbox (workspace-write): {} is outside the writable roots. Writes are confined to the workspace{}. Use Settings -> Sandbox to add roots or raise the sandbox mode.",
            path.display(),
            self.roots_suffix()
        )))
    }

    /// Gate for destructive actions (delete). Same rules as writes.
    pub fn check_delete(&self, path: &Path) -> Result<()> {
        self.check_write(path)
    }

    /// Gate for mutations without a concrete path yet (e.g. "run this
    /// mutating tool at all"). `action` names the operation in the error.
    pub fn check_mutation(&self, action: &str) -> Result<()> {
        if self.mode.allows_writes() {
            return Ok(());
        }
        Err(OSAgentError::ToolExecution(format!(
            "Blocked by sandbox (read-only): {} modifies the system. Switch the sandbox to workspace-write or danger-full-access in Settings -> Sandbox to allow it.",
            action
        )))
    }

    fn roots_suffix(&self) -> String {
        if self.roots.is_empty() {
            return String::new();
        }
        let shown: Vec<String> = self
            .roots
            .iter()
            .take(3)
            .map(|root| root.display().to_string())
            .collect();
        format!(" (roots: {})", shown.join(", "))
    }

    /// Status snapshot for this sandbox + host.
    pub fn status(&self) -> SandboxStatus {
        let (backend, enforced, notes) = platform::backend_report(self);
        SandboxStatus {
            mode: self.mode.as_str().to_string(),
            network: match self.network {
                SandboxNetworkPolicy::Allow => "allow".to_string(),
                SandboxNetworkPolicy::Deny => "deny".to_string(),
            },
            os_enforcement: self.os_enforcement,
            backend: backend.to_string(),
            enforced,
            notes,
        }
    }

    /// Spawn a process under this sandbox. The returned child carries a
    /// kill-tree guard: dropping it terminates the whole process tree (the
    /// sandbox's job object on Windows, the process group on Unix).
    pub fn spawn(&self, spec: SpawnSpec) -> Result<SandboxedChild> {
        platform::spawn(self, spec).map_err(OSAgentError::from)
    }
}

/// Scratch roots always writable in workspace-write mode. Deliberately *not*
/// `%TEMP%`: it is a huge shared tree the sandbox must not re-protect, and
/// sandboxed children get `TEMP`/`TMP` redirected to the OSA scratch dir
/// (see the platform spawn code), which is small and always labelable.
fn temp_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    let scratch = PathBuf::from(shellexpand::tilde("~/.osagent/tmp").to_string());
    let _ = std::fs::create_dir_all(&scratch);
    let scratch = scratch.canonicalize().unwrap_or(scratch);
    roots.push(scratch);
    let spill = PathBuf::from(shellexpand::tilde("~/.osagent").to_string());
    if spill.exists() {
        roots.push(spill.canonicalize().unwrap_or(spill));
    }
    roots
}

/// Canonicalize a path that may not exist yet (writes to new files): resolve
/// the deepest existing ancestor and re-attach the remainder. Without this,
/// Windows' `\\?\`-prefixed canonical roots never match plain paths.
pub(crate) fn canonicalize_best_effort(path: &Path) -> PathBuf {
    if let Ok(canonical) = path.canonicalize() {
        return canonical;
    }
    let mut remainder = std::ffi::OsString::new();
    let mut cursor = path;
    while let Some(parent) = cursor.parent() {
        if let Some(name) = cursor.file_name() {
            if remainder.is_empty() {
                remainder.push(name);
            } else {
                let mut combined = name.to_os_string();
                combined.push(std::path::MAIN_SEPARATOR.to_string());
                combined.push(&remainder);
                remainder = combined;
            }
        }
        if let Ok(canonical_parent) = parent.canonicalize() {
            return canonical_parent.join(remainder);
        }
        if parent == cursor {
            break;
        }
        cursor = parent;
    }
    path.to_path_buf()
}

#[cfg(test)]
fn config_with_mode(mode: SandboxMode) -> Config {
    let mut config = Config::default();
    config.sandbox.mode = mode;
    config.sandbox.os_enforcement = true;
    config
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn read_only_blocks_all_writes() {
        let sandbox = Sandbox::from_config(&config_with_mode(SandboxMode::ReadOnly), vec![]);
        assert!(sandbox.check_write(Path::new("anything.txt")).is_err());
        assert!(sandbox.check_delete(Path::new("anything.txt")).is_err());
        assert!(sandbox.check_mutation("writing a file").is_err());
    }

    #[test]
    fn workspace_write_confines_writes_to_roots() {
        let workspace = std::env::temp_dir();
        let sandbox = Sandbox::from_config(
            &config_with_mode(SandboxMode::WorkspaceWrite),
            vec![workspace.clone()],
        );
        assert!(sandbox.check_write(&workspace.join("inside.txt")).is_ok());
        assert!(sandbox
            .check_write(Path::new("C:\\definitely\\outside.txt"))
            .is_err());
    }

    #[test]
    fn danger_mode_allows_everything() {
        let sandbox = Sandbox::from_config(&config_with_mode(SandboxMode::DangerFullAccess), vec![]);
        assert!(sandbox.check_write(Path::new("C:\\anywhere.txt")).is_ok());
        assert!(!sandbox.confines_spawns());
    }

    #[test]
    fn mode_parses_codex_spellings() {
        assert_eq!(
            SandboxMode::parse("workspace-write"),
            Some(SandboxMode::WorkspaceWrite)
        );
        assert_eq!(
            SandboxMode::parse("read_only"),
            Some(SandboxMode::ReadOnly)
        );
        assert_eq!(
            SandboxMode::parse("danger-full-access"),
            Some(SandboxMode::DangerFullAccess)
        );
    }

    #[test]
    fn status_never_lies_about_enforcement() {
        let sandbox = Sandbox::from_config(&config_with_mode(SandboxMode::WorkspaceWrite), vec![]);
        let status = sandbox.status();
        // Whatever the backend turns out to be, `enforced` must agree with
        // `confines_spawns` and name a real backend.
        assert_eq!(status.enforced, sandbox.confines_spawns());
        assert!(!status.backend.is_empty());
    }
}

/// Behavioral proof of the Windows sandbox: these tests spawn real processes
/// and assert what the restricted token + ACL actually permits.
#[cfg(all(test, windows))]
mod windows_behavior_tests {
    use super::*;

    async fn run_to_completion(sandbox: &Sandbox, command: &str) -> std::process::Output {
        let spec = SpawnSpec::new("cmd", vec!["/C", command]).cwd(std::env::temp_dir());
        let mut child = sandbox.spawn(spec).expect("spawn failed");
        child.wait_with_output().await.expect("wait failed")
    }

    #[tokio::test]
    async fn workspace_write_allows_inside_and_blocks_outside() {
        let inside = tempfile::tempdir().expect("tempdir");
        // Pre-existing nested file: proves inheritable ACL grants reach
        // children that existed before the grant (propagation).
        let subdir = inside.path().join("sub");
        std::fs::create_dir_all(&subdir).expect("mkdir sub");
        let existing = subdir.join("existing.txt");
        std::fs::write(&existing, "before").expect("seed file");
        let outside_dir = dirs_next::home_dir()
            .expect("home dir")
            .join("osa_sandbox_outside_probe");
        let _ = std::fs::create_dir_all(&outside_dir);
        let outside = outside_dir.join("probe.txt");
        let _ = std::fs::remove_file(&outside);

        let sandbox = Sandbox::from_config(
            &config_with_mode(SandboxMode::WorkspaceWrite),
            vec![inside.path().to_path_buf()],
        );

        let inside_file = inside.path().join("probe.txt");
        let output = run_to_completion(
            &sandbox,
            &format!("echo hi > {}", inside_file.display()),
        )
        .await;
        assert!(
            inside_file.exists(),
            "write inside workspace should succeed: {:?}",
            output
        );

        // Overwrite the pre-existing nested file through the sandbox.
        let output = run_to_completion(
            &sandbox,
            &format!("echo overwritten > {}", existing.display()),
        )
        .await;
        let contents = std::fs::read_to_string(&existing).unwrap_or_default();
        assert!(
            contents.contains("overwritten"),
            "write to pre-existing nested file should succeed (ACL propagation): {:?} contents={:?}",
            output,
            contents
        );

        let output = run_to_completion(
            &sandbox,
            &format!("echo hi > {}", outside.display()),
        )
        .await;
        assert!(
            !outside.exists(),
            "write outside workspace must be denied by the sandbox (got: {:?})",
            output
        );
    }

    #[tokio::test]
    async fn reads_outside_the_workspace_still_work() {
        let sandbox = Sandbox::from_config(&config_with_mode(SandboxMode::WorkspaceWrite), vec![]);
        let output = run_to_completion(&sandbox, "type %WINDIR%\\win.ini").await;
        assert!(
            output.status.success(),
            "reads must not be restricted: {:?}",
            output
        );
    }

    #[tokio::test]
    async fn read_only_mode_blocks_writes_even_inside_roots() {
        let inside = tempfile::tempdir().expect("tempdir");
        let sandbox = Sandbox::from_config(
            &config_with_mode(SandboxMode::ReadOnly),
            vec![inside.path().to_path_buf()],
        );
        let inside_file = inside.path().join("probe.txt");
        let _ = run_to_completion(&sandbox, &format!("echo hi > {}", inside_file.display())).await;
        assert!(
            !inside_file.exists(),
            "read-only mode must deny writes everywhere, including workspace roots"
        );
    }

    #[tokio::test]
    async fn drop_kills_the_process_tree() {
        let sandbox = Sandbox::from_config(&config_with_mode(SandboxMode::WorkspaceWrite), vec![]);
        let spec = SpawnSpec::new("cmd", vec!["/C", "ping -n 30 127.0.0.1 > NUL"])
            .cwd(std::env::temp_dir());
        let mut child = sandbox.spawn(spec).expect("spawn failed");
        let pid = child.id();
        assert!(!child.is_finished(), "long-running child should be alive");
        child.kill_tree();
        let _ = child.wait().await;
        assert!(child.is_finished(), "child must be dead after kill_tree");
        // The tree guard must have taken the job/pid down, not just reaped it.
        let probe = run_to_completion(
            &sandbox,
            &format!("tasklist /FI \"PID eq {}\" | find \"{}\"", pid, pid),
        )
        .await;
        assert!(
            !probe.status.success() || !String::from_utf8_lossy(&probe.stdout).contains(&pid.to_string()),
            "process {} survived kill_tree",
            pid
        );
    }
}
