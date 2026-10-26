//! Platform backends for [`crate::sandbox`].
//!
//! Each backend implements three things:
//!
//! * [`ChildHandle`] — process handle + wait + kill-tree for [`SandboxedChild`]
//! * [`spawn`] — process creation under the sandbox policy
//! * [`backend_report`] — honest status (`enforced` + degradation notes)

use super::child::{SandboxedChild, SpawnSpec};
use super::Sandbox;
use std::io;
use std::sync::Mutex;

#[cfg(windows)]
mod windows;
#[cfg(windows)]
mod wfp;
#[cfg(not(windows))]
mod unix;

#[cfg(windows)]
pub(crate) use windows::ChildHandle;
#[cfg(not(windows))]
pub(crate) use unix::ChildHandle;

/// Per-process-lifetime backend preparation (Windows: AppContainer profile +
/// WFP session + ACL grants; Unix: nothing global). Cheap to create; real work
/// happens on first spawn.
pub(crate) struct PlatformState {
    pub(crate) notes: Mutex<Vec<String>>,
}

impl PlatformState {
    pub(crate) fn new() -> Self {
        Self {
            notes: Mutex::new(Vec::new()),
        }
    }

    pub(crate) fn add_note(&self, note: String) {
        if let Ok(mut notes) = self.notes.lock() {
            if !notes.contains(&note) {
                notes.push(note);
            }
        }
    }

    pub(crate) fn notes(&self) -> Vec<String> {
        self.notes.lock().map(|n| n.clone()).unwrap_or_default()
    }
}

pub(crate) fn spawn(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
    if sandbox.confines_spawns() {
        spawn_confined(sandbox, spec)
    } else {
        spawn_plain(spec)
    }
}

pub(crate) fn backend_report(sandbox: &Sandbox) -> (&'static str, bool, Vec<String>) {
    let mut notes = sandbox.platform.notes();
    if !sandbox.confines_spawns() {
        if sandbox.is_unrestricted() {
            notes.push("sandbox mode is danger-full-access: no OS confinement".to_string());
        } else {
            notes.push("OS enforcement disabled in settings: spawned processes are not confined".to_string());
        }
        return (backend_name(), false, notes);
    }
    let mut enforced = true;
    if sandbox.network() == crate::config::SandboxNetworkPolicy::Deny {
        enforced = network_report(sandbox, &mut notes);
    }
    (backend_name(), enforced, notes)
}

#[cfg(windows)]
fn backend_name() -> &'static str {
    "windows-restricted-token"
}

#[cfg(all(not(windows), target_os = "linux"))]
fn backend_name() -> &'static str {
    "landlock-seccomp"
}

#[cfg(all(not(windows), target_os = "macos"))]
fn backend_name() -> &'static str {
    "seatbelt"
}

#[cfg(all(not(windows), not(any(target_os = "linux", target_os = "macos"))))]
fn backend_name() -> &'static str {
    "none"
}

#[cfg(windows)]
fn spawn_confined(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
    windows::spawn_confined(sandbox, spec)
}

#[cfg(not(windows))]
fn spawn_confined(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
    unix::spawn_confined(sandbox, spec)
}

#[cfg(windows)]
fn spawn_plain(spec: SpawnSpec) -> io::Result<SandboxedChild> {
    windows::spawn_plain(spec)
}

#[cfg(not(windows))]
fn spawn_plain(spec: SpawnSpec) -> io::Result<SandboxedChild> {
    unix::spawn_plain(spec)
}

#[cfg(windows)]
fn network_report(sandbox: &Sandbox, notes: &mut Vec<String>) -> bool {
    windows::network_report(sandbox, notes)
}

#[cfg(not(windows))]
fn network_report(sandbox: &Sandbox, notes: &mut Vec<String>) -> bool {
    unix::network_report(sandbox, notes)
}
