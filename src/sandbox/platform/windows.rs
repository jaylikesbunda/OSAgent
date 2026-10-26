//! Windows sandbox backend.
//!
//! "Restricted token + ACL + WFP firewall", composed the way Windows security
//! primitives expect:
//!
//! * **Restricted token** — `CreateRestrictedToken` with `WRITE_RESTRICTED`
//!   semantics: the restricting SID set (the OSA sandbox AppContainer package
//!   SID) is consulted for *write* access checks only. Reads behave like the
//!   user's own (Codex's "read anywhere" default), writes require a matching
//!   grant — which only the sandbox hands out.
//! * **ACL** — the package SID gets an inheritable Modify ACE on the writable
//!   roots (workspace roots, temp, extra roots). That is the only place the
//!   restricted write check can pass. In `read-only` mode no grants are made,
//!   so every write fails at the OS level.
//! * **Job Object** — every spawned tree lands in a kill-on-close job, so the
//!   tool layer can guarantee no orphaned descendants.
//! * **WFP firewall** — with `network = "deny"` the process additionally runs
//!   inside the AppContainer and Windows Filtering Platform filters keyed to
//!   its package SID block outbound connections (dynamic session: filters
//!   vanish when OSA exits). Adding filters needs admin rights; without them
//!   the spawn fails closed instead of pretending.
//!
//! When a primitive cannot be applied (except the strict `network = "deny"`),
//! the spawn proceeds with an explicit degradation note.

use super::super::child::{quote_windows_arg, SandboxedChild, SpawnSpec, StdioKind};
use super::super::Sandbox;
use crate::config::{SandboxMode, SandboxNetworkPolicy};
use std::ffi::c_void;
use std::io;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle, RawHandle};
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::atomic::{AtomicI64, Ordering};

use windows_sys::Win32::Foundation::{
    CloseHandle, SetHandleInformation, GENERIC_READ, GENERIC_WRITE, HANDLE, INVALID_HANDLE_VALUE,
    WAIT_OBJECT_0,
};
use windows_sys::Win32::Security::Authorization::{
    SetEntriesInAclW, SetNamedSecurityInfoW, EXPLICIT_ACCESS_W, GRANT_ACCESS, SE_FILE_OBJECT,
    TRUSTEE_IS_SID, TRUSTEE_W,
};
use windows_sys::Win32::Security::Isolation::DeriveAppContainerSidFromAppContainerName;
use windows_sys::Win32::Security::{
    CreateRestrictedToken, FreeSid, IsValidSid, DACL_SECURITY_INFORMATION, PSID,
    SECURITY_CAPABILITIES, SID_AND_ATTRIBUTES,
};
use windows_sys::Win32::Storage::FileSystem::{
    CreateFileW, DELETE, FILE_ATTRIBUTE_NORMAL, FILE_GENERIC_EXECUTE, FILE_GENERIC_READ,
    FILE_GENERIC_WRITE, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE, FILE_TRAVERSE,
    OPEN_EXISTING,
};
use windows_sys::Win32::System::JobObjects::{
    AssignProcessToJobObject, CreateJobObjectW, JobObjectExtendedLimitInformation,
    SetInformationJobObject, TerminateJobObject, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
    JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE,
};
use windows_sys::Win32::System::Pipes::CreatePipe;
use windows_sys::Win32::System::Threading::{
    CreateProcessAsUserW, CreateProcessWithTokenW, DeleteProcThreadAttributeList,
    GetCurrentProcess, GetExitCodeProcess, InitializeProcThreadAttributeList, OpenProcessToken,
    UpdateProcThreadAttribute, WaitForSingleObject, CREATE_NO_WINDOW, EXTENDED_STARTUPINFO_PRESENT,
    PROCESS_CREATION_FLAGS, PROCESS_INFORMATION, PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES,
    STARTF_USESTDHANDLES, STARTUPINFOEXW, STARTUPINFOW,
};

const HANDLE_FLAG_INHERIT: u32 = 0x0000_0001;
const DISABLE_MAX_PRIVILEGE: u32 = 0x1;
const WRITE_RESTRICTED: u32 = 0x8;
const CREATE_UNICODE_ENVIRONMENT: u32 = 0x0000_0400;
const CONTAINER_INHERIT_ACE: u32 = 0x02;
const OBJECT_INHERIT_ACE: u32 = 0x01;

const PACKAGE_NAME: &str = "OSA.Sandbox";

const TOKEN_QUERY: u32 = 0x0008;
const TOKEN_DUPLICATE: u32 = 0x0002;
const TOKEN_ASSIGN_PRIMARY: u32 = 0x0001;
const TOKEN_ADJUST_DEFAULT: u32 = 0x0080;
const TOKEN_ADJUST_SESSIONID: u32 = 0x0100;

// ---------------------------------------------------------------------------
// Child handle
// ---------------------------------------------------------------------------

struct HandleGuard(HANDLE);
impl Drop for HandleGuard {
    fn drop(&mut self) {
        if !self.0.is_null() && self.0 != INVALID_HANDLE_VALUE {
            unsafe {
                CloseHandle(self.0);
            }
        }
    }
}

enum Inner {
    /// Plain (unconfined) spawn.
    Plain(std::process::Child),
    /// Sandboxed spawn: raw process handle + Job Object.
    Sandboxed {
        process: HandleGuard,
        job: Option<HandleGuard>,
    },
}

pub(crate) struct ChildHandle {
    inner: Inner,
    pid: u32,
    exit_code: AtomicI64,
}

// SAFETY: the process/job handles are owned exclusively by this value and are
// plain kernel handles, safe to move across threads.
unsafe impl Send for ChildHandle {}

impl ChildHandle {
    pub(crate) fn id(&self) -> u32 {
        self.pid
    }

    fn process_handle(&self) -> HANDLE {
        match &self.inner {
            Inner::Plain(child) => child.as_raw_handle() as HANDLE,
            Inner::Sandboxed { process, .. } => process.0,
        }
    }

    pub(crate) fn is_finished(&self) -> bool {
        if self.exit_code.load(Ordering::SeqCst) >= 0 {
            return true;
        }
        let handle = self.process_handle();
        if handle.is_null() || handle == INVALID_HANDLE_VALUE {
            return true;
        }
        unsafe { WaitForSingleObject(handle, 0) == WAIT_OBJECT_0 }
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        if self.is_finished() {
            self.wait_blocking().map(Some)
        } else {
            Ok(None)
        }
    }

    pub(crate) fn wait_blocking(&mut self) -> io::Result<std::process::ExitStatus> {
        use std::os::windows::process::ExitStatusExt;

        let cached = self.exit_code.load(Ordering::SeqCst);
        if cached >= 0 {
            return Ok(std::process::ExitStatus::from_raw(cached as u32));
        }
        match &mut self.inner {
            Inner::Plain(child) => {
                let status = child.wait()?;
                self.exit_code
                    .store(status.code().unwrap_or(-1) as i64, Ordering::SeqCst);
                Ok(status)
            }
            Inner::Sandboxed { .. } => {
                let handle = self.process_handle();
                let wait = unsafe { WaitForSingleObject(handle, u32::MAX) };
                if wait != WAIT_OBJECT_0 {
                    return Err(io::Error::last_os_error());
                }
                let mut code: u32 = 0;
                let ok = unsafe { GetExitCodeProcess(handle, &mut code) };
                if ok == 0 {
                    return Err(io::Error::last_os_error());
                }
                self.exit_code.store(code as i64, Ordering::SeqCst);
                Ok(std::process::ExitStatus::from_raw(code))
            }
        }
    }

    pub(crate) fn kill_tree(&mut self) {
        match &mut self.inner {
            Inner::Plain(child) => {
                let pid = child.id();
                if pid != 0 {
                    taskkill_tree(pid);
                }
                let _ = child.try_wait();
            }
            Inner::Sandboxed { process, job } => {
                if let Some(job) = job {
                    if !job.0.is_null() && job.0 != INVALID_HANDLE_VALUE {
                        unsafe {
                            TerminateJobObject(job.0, 1);
                        }
                    }
                }
                taskkill_tree(self.pid);
                let _ = process;
            }
        }
    }
}

fn taskkill_tree(pid: u32) {
    let _ = Command::new("taskkill")
        .args(["/F", "/T", "/PID", &pid.to_string()])
        .creation_flags(CREATE_NO_WINDOW)
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
}

// ---------------------------------------------------------------------------
// Spawn entry points
// ---------------------------------------------------------------------------

pub(crate) fn spawn_plain(spec: SpawnSpec) -> io::Result<SandboxedChild> {
    let mut cmd = match &spec.windows_raw_cmdline {
        Some(raw) => {
            // Preserve the caller's raw tail verbatim (bash: `cmd /C <cmd>`).
            let (program, rest) = split_raw_cmdline(raw);
            let mut cmd = Command::new(program);
            if !rest.is_empty() {
                cmd.raw_arg(rest);
            }
            cmd
        }
        None => {
            let mut cmd = Command::new(&spec.program);
            cmd.args(&spec.args);
            cmd
        }
    };
    cmd.current_dir(&spec.cwd);
    if spec.clear_env {
        cmd.env_clear();
    }
    for (key, value) in &spec.envs {
        cmd.env(key, value);
    }
    cmd.stdin(stdio_of(spec.stdin))
        .stdout(stdio_of(spec.stdout))
        .stderr(stdio_of(spec.stderr));
    cmd.creation_flags(CREATE_NO_WINDOW);

    let mut child = cmd.spawn()?;
    let pid = child.id();
    let stdin = child.stdin.take().map(file_of_child_stdio);
    let stdout = child.stdout.take().map(file_of_child_stdio);
    let stderr = child.stderr.take().map(file_of_child_stdio);
    Ok(SandboxedChild::new(
        ChildHandle {
            inner: Inner::Plain(child),
            pid,
            exit_code: AtomicI64::new(-1),
        },
        stdin,
        stdout,
        stderr,
    ))
}

/// "cmd /C foo" -> ("cmd", "/C foo"); quoted program names are supported.
fn split_raw_cmdline(raw: &str) -> (String, String) {
    let trimmed = raw.trim_start();
    if let Some(rest) = trimmed.strip_prefix('"') {
        if let Some(end) = rest.find('"') {
            return (
                rest[..end].to_string(),
                rest[end + 1..].trim_start().to_string(),
            );
        }
    }
    match trimmed.find(char::is_whitespace) {
        Some(index) => (
            trimmed[..index].to_string(),
            trimmed[index..].trim_start().to_string(),
        ),
        None => (trimmed.to_string(), String::new()),
    }
}

fn stdio_of(kind: StdioKind) -> Stdio {
    match kind {
        StdioKind::Null => Stdio::null(),
        StdioKind::Inherit => Stdio::inherit(),
        StdioKind::Pipe => Stdio::piped(),
    }
}

fn file_of_child_stdio<S: IntoRawHandle>(stream: S) -> std::fs::File {
    unsafe { std::fs::File::from_raw_handle(stream.into_raw_handle()) }
}

pub(crate) fn spawn_confined(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
    let deny_network = sandbox.network() == SandboxNetworkPolicy::Deny;

    // 1. Token: privilege-stripped restricted token at Low integrity.
    let token = create_sandbox_token()?;

    // 2. ACL labels. Read-only mode intentionally labels nothing: with no
    //    Low label anywhere, the mandatory NO_WRITE_UP policy denies every
    //    write at the OS level while reads stay open.
    if sandbox.mode() == SandboxMode::WorkspaceWrite {
        for root in sandbox.roots() {
            // Writable roots are shared scratch (temp, ~/.osagent) or full
            // project trees (workspaces, extra roots). Scratch only needs its
            // own directory labeled — new children inherit the label — while
            // project trees must propagate to pre-existing files so edits
            // work. Propagation walks the tree; keep it to the roots that
            // actually need it.
            let propagate = root_is_project_root(root);
            if let Err(error) = label_tree(root, propagate) {
                sandbox.platform.add_note(format!(
                    "integrity label failed on {}: {} (writes there will be denied by the sandbox)",
                    root.display(),
                    error
                ));
            }
        }
    }

    // 3. Network deny: AppContainer + WFP keyed to the package SID. The
    //    appcontainer check gates *all* access (not just writes), so the roots
    //    and the user profile need their own package-SID grants.
    let package_sid = if deny_network {
        let package_sid = derive_package_sid()?;
        for root in sandbox.roots() {
            let _ = grant_tree(&package_sid, root, AccessKind::Modify);
        }
        // Reads outside the granted roots break under an AppContainer unless
        // the profile is explicitly opened (git config, caches, dotfiles).
        if let Some(profile) = dirs_next::home_dir() {
            if let Err(error) = grant_tree(&package_sid, &profile, AccessKind::Read) {
                sandbox.platform.add_note(format!(
                    "profile read grant failed: {} (reads outside granted roots may fail)",
                    error
                ));
            }
        }
        // 4. WFP: block outbound connections for the package SID (needs admin).
        if let Err(error) = block_network_for_package(&package_sid) {
            return Err(io::Error::new(
                io::ErrorKind::PermissionDenied,
                format!(
                    "sandbox network=deny requires the WFP firewall (run elevated or set sandbox.network = \"allow\"): {}",
                    error
                ),
            ));
        }
        Some(package_sid)
    } else {
        None
    };

    // 5. Spawn. Sandbox children write temp files to the OSA scratch dir
    //    (small, labeled above) instead of the shared %TEMP% tree.
    let mut spec = spec;
    if let Some(home) = dirs_next::home_dir() {
        let scratch = home.join(".osagent").join("tmp");
        let _ = std::fs::create_dir_all(&scratch);
        let value = std::ffi::OsString::from(scratch.to_string_lossy().into_owned());
        spec.envs
            .push((std::ffi::OsString::from("TEMP"), value.clone()));
        spec.envs.push((std::ffi::OsString::from("TMP"), value));
    }
    spawn_with_token(
        &spec,
        &token,
        package_sid.as_deref().unwrap_or(&[]),
        deny_network,
    )
}

/// Scratch roots (under `~/.osagent`) only get their own directory labeled —
/// everything else is a project tree and gets full propagation so
/// pre-existing files are writable.
fn root_is_project_root(root: &Path) -> bool {
    if let Some(home) = dirs_next::home_dir() {
        if root.starts_with(home.join(".osagent")) {
            return false;
        }
    }
    true
}

// ---------------------------------------------------------------------------
// AppContainer package SID
// ---------------------------------------------------------------------------

/// Derives the deterministic AppContainer package SID for the OSA sandbox
/// identity. Works without registering a profile.
fn derive_package_sid() -> io::Result<Vec<u8>> {
    let name: Vec<u16> = std::ffi::OsStr::new(PACKAGE_NAME)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut sid: PSID = std::ptr::null_mut();
    let result = unsafe { DeriveAppContainerSidFromAppContainerName(name.as_ptr(), &mut sid) };
    if result != 0 || sid.is_null() {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let length = unsafe { windows_sys::Win32::Security::GetLengthSid(sid) } as usize;
    let mut bytes = vec![0u8; length];
    unsafe {
        std::ptr::copy_nonoverlapping(sid as *const u8, bytes.as_mut_ptr(), length);
        FreeSid(sid);
    }
    Ok(bytes)
}

// ---------------------------------------------------------------------------
// Sandboxed token: restricted + Low integrity
// ---------------------------------------------------------------------------

/// Builds the token every sandboxed process runs under:
///
/// * `CreateRestrictedToken` with `DISABLE_MAX_PRIVILEGE` — a restricted token
///   with dangerous privileges (SeDebug, SeBackup, SeTakeOwnership, ...)
///   stripped.
/// * Integrity level **Low** — the mandatory integrity policy (`NO_WRITE_UP`)
///   then denies writes to any object not explicitly labeled Low, while reads
///   stay unrestricted. Combined with the Low labels the sandbox places on the
///   writable roots (see [`label_tree`]), this is the "restricted token + ACL"
///   pair: writes are possible exactly where the sandbox's ACL says so.
///
/// (`WRITE_RESTRICTED` was evaluated and rejected: with it, Windows subsystem
/// initialization fails with STATUS_DLL_INIT_FAILED — window station/shared
/// section writes are needed before the first user instruction.)
fn create_sandbox_token() -> io::Result<OwnedHandle> {
    use windows_sys::Win32::Security::{
        AddMandatoryAce, CreateWellKnownSid, SetTokenInformation, TokenIntegrityLevel,
        WinLowLabelSid, TOKEN_MANDATORY_LABEL,
    };

    let mut primary: HANDLE = std::ptr::null_mut();
    let access = TOKEN_QUERY
        | TOKEN_DUPLICATE
        | TOKEN_ASSIGN_PRIMARY
        | TOKEN_ADJUST_DEFAULT
        | TOKEN_ADJUST_SESSIONID;
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), access, &mut primary) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let primary_guard = HandleGuard(primary);

    let mut restricted: HANDLE = std::ptr::null_mut();
    let ok = unsafe {
        CreateRestrictedToken(
            primary_guard.0,
            DISABLE_MAX_PRIVILEGE,
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            0,
            std::ptr::null(),
            &mut restricted,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let token = unsafe { OwnedHandle::from_raw_handle(restricted as RawHandle) };

    // Drop the integrity level to Low.
    let mut low_sid = [0u8; 64];
    let mut low_sid_len = low_sid.len() as u32;
    let ok = unsafe {
        CreateWellKnownSid(
            WinLowLabelSid,
            std::ptr::null_mut(),
            low_sid.as_mut_ptr() as PSID,
            &mut low_sid_len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let label = TOKEN_MANDATORY_LABEL {
        Label: SID_AND_ATTRIBUTES {
            Sid: low_sid.as_ptr() as PSID,
            Attributes: SE_GROUP_INTEGRITY,
        },
    };
    let ok = unsafe {
        SetTokenInformation(
            token.as_raw_handle() as HANDLE,
            TokenIntegrityLevel,
            &label as *const _ as *mut c_void,
            std::mem::size_of::<TOKEN_MANDATORY_LABEL>() as u32
                + low_sid_len
                + std::mem::size_of::<usize>() as u32,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok(token)
}

const SE_GROUP_INTEGRITY: u32 = 0x0000_0020;

// ---------------------------------------------------------------------------
// ACL grants
// ---------------------------------------------------------------------------

// ---------------------------------------------------------------------------
// ACL grants + mandatory labels
// ---------------------------------------------------------------------------

enum AccessKind {
    Read,
    Modify,
    Full,
}

fn access_mask(kind: &AccessKind) -> u32 {
    match kind {
        AccessKind::Read => FILE_GENERIC_READ | FILE_GENERIC_EXECUTE | FILE_TRAVERSE,
        AccessKind::Modify => {
            FILE_GENERIC_READ | FILE_GENERIC_WRITE | FILE_GENERIC_EXECUTE | DELETE
        }
        // FILE_ALL_ACCESS: everything, including WRITE_DAC/WRITE_OWNER.
        AccessKind::Full => 0x001F_01FF,
    }
}

/// The user's own SID (from the process token).
fn user_sid_bytes() -> io::Result<Vec<u8>> {
    use windows_sys::Win32::Security::{GetTokenInformation, TokenUser};

    let mut primary: HANDLE = std::ptr::null_mut();
    let ok = unsafe { OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut primary) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let guard = HandleGuard(primary);

    let mut needed: u32 = 0;
    unsafe {
        GetTokenInformation(guard.0, TokenUser, std::ptr::null_mut(), 0, &mut needed);
    }
    let mut buffer: Vec<u64> = vec![0; (needed as usize).div_ceil(std::mem::size_of::<u64>())];
    let ok = unsafe {
        GetTokenInformation(
            guard.0,
            TokenUser,
            buffer.as_mut_ptr() as *mut c_void,
            needed,
            &mut needed,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    unsafe {
        let sid_and_attrs = buffer.as_ptr() as *const SID_AND_ATTRIBUTES;
        let sid = (*sid_and_attrs).Sid;
        let length = windows_sys::Win32::Security::GetLengthSid(sid) as usize;
        let mut bytes = vec![0u8; length];
        std::ptr::copy_nonoverlapping(sid as *const u8, bytes.as_mut_ptr(), length);
        Ok(bytes)
    }
}

/// Frees security descriptors returned by `GetNamedSecurityInfoW`.
struct LocalFreeGuard(*mut c_void);
impl Drop for LocalFreeGuard {
    fn drop(&mut self) {
        if !self.0.is_null() {
            unsafe {
                windows_sys::Win32::Foundation::LocalFree(self.0);
            }
        }
    }
}

/// Sets an inheritable DACL grant on `root`, merged with the existing DACL.
/// `propagate` selects `SetNamedSecurityInfoW` (which walks existing children)
/// vs `SetSecurityInfo` on a handle (which deliberately does not).
fn set_dacl_grant(root: &Path, sid: &[u8], kind: &AccessKind, propagate: bool) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::{GetNamedSecurityInfoW, SetSecurityInfo};

    if unsafe { IsValidSid(sid.as_ptr() as PSID) } == 0 {
        return Err(io::Error::new(io::ErrorKind::InvalidInput, "invalid SID"));
    }

    let path_wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    // Fetch the existing DACL so the grant merges instead of replacing
    // (replacing would strip SYSTEM/owner entries).
    let mut old_dacl: *mut windows_sys::Win32::Security::ACL = std::ptr::null_mut();
    let mut descriptor: *mut c_void = std::ptr::null_mut();
    let result = unsafe {
        GetNamedSecurityInfoW(
            path_wide.as_ptr(),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            &mut old_dacl,
            std::ptr::null_mut(),
            &mut descriptor,
        )
    };
    let _descriptor_guard = LocalFreeGuard(descriptor);
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }

    let access = EXPLICIT_ACCESS_W {
        grfAccessPermissions: access_mask(kind),
        grfAccessMode: GRANT_ACCESS,
        grfInheritance: CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
        Trustee: TRUSTEE_W {
            pMultipleTrustee: std::ptr::null_mut(),
            MultipleTrusteeOperation: 0,
            TrusteeForm: TRUSTEE_IS_SID,
            TrusteeType: 0,
            ptstrName: sid.as_ptr() as *mut u16,
        },
    };

    let mut new_dacl: *mut windows_sys::Win32::Security::ACL = std::ptr::null_mut();
    let result = unsafe { SetEntriesInAclW(1, &access, old_dacl, &mut new_dacl) };
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    let _new_guard = LocalFreeGuard(new_dacl as *mut c_void);

    if propagate {
        let result = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_ptr(),
                SE_FILE_OBJECT,
                DACL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                new_dacl,
                std::ptr::null_mut(),
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        return Ok(());
    }

    let handle = open_for_security(
        root,
        0x0004_0000 /* WRITE_DAC */ | 0x0002_0000, /* READ_CONTROL */
    )
    .map_err(|e| io::Error::new(e.kind(), format!("dacl open: {e}")))?;
    let result = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            new_dacl,
            std::ptr::null_mut(),
        )
    };
    unsafe {
        CloseHandle(handle);
    }
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}

fn open_for_security(root: &Path, access: u32) -> io::Result<HANDLE> {
    const FILE_FLAG_BACKUP_SEMANTICS: u32 = 0x0200_0000;
    let path_wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let handle = unsafe {
        CreateFileW(
            path_wide.as_ptr(),
            access,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            std::ptr::null(),
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            std::ptr::null_mut(),
        )
    };
    if handle == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    Ok(handle)
}

/// Cached tree-wide DACL grant (network-deny AppContainer path).
fn grant_tree(sid: &[u8], root: &Path, kind: AccessKind) -> io::Result<()> {
    use std::sync::{LazyLock, Mutex};
    static GRANTED: LazyLock<Mutex<std::collections::HashSet<(String, u8)>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

    if !root.exists() {
        return Ok(());
    }
    let key = (
        root.to_string_lossy().to_string(),
        match kind {
            AccessKind::Read => 0u8,
            AccessKind::Modify => 1,
            AccessKind::Full => 2,
        },
    );
    if let Ok(mut granted) = GRANTED.lock() {
        if granted.contains(&key) {
            return Ok(());
        }
        set_dacl_grant(root, sid, &kind, true)?;
        granted.insert(key);
        return Ok(());
    }
    set_dacl_grant(root, sid, &kind, true)
}

/// Places a Low mandatory-integrity label on `root` so the sandbox's
/// Low-integrity token can write there — the "ACL" half of the sandbox. New
/// children inherit the label at creation; `propagate` additionally writes it
/// onto pre-existing children.
///
/// Two steps because many directories (notably children of `%TEMP%`) carry a
/// logon-SID-only DACL with no `WRITE_OWNER` for anyone: the object owner
/// still has implicit `WRITE_DAC`, so the user SID gets a Full grant first,
/// which unlocks the label write.
fn label_tree(root: &Path, propagate: bool) -> io::Result<()> {
    use std::sync::{LazyLock, Mutex};
    static LABELED: LazyLock<Mutex<std::collections::HashSet<String>>> =
        LazyLock::new(|| Mutex::new(std::collections::HashSet::new()));

    if !root.exists() {
        return Ok(());
    }
    let key = root.to_string_lossy().to_string();
    if let Ok(mut labeled) = LABELED.lock() {
        if labeled.contains(&key) {
            return Ok(());
        }
        let result = label_tree_inner(root, propagate);
        if result.is_ok() {
            labeled.insert(key);
        }
        return result;
    }
    label_tree_inner(root, propagate)
}

fn label_tree_inner(root: &Path, propagate: bool) -> io::Result<()> {
    let user = user_sid_bytes().map_err(|e| io::Error::new(e.kind(), format!("user_sid: {e}")))?;
    set_dacl_grant(root, &user, &AccessKind::Full, propagate)
        .map_err(|e| io::Error::new(e.kind(), format!("dacl_grant: {e}")))?;
    // The label write has been observed to fail once right after a DACL
    // change and succeed immediately after; retry before giving up.
    match set_mandatory_label(root, propagate) {
        Ok(()) => Ok(()),
        Err(first) => set_mandatory_label(root, propagate).map_err(|second| {
            io::Error::new(second.kind(), format!("label: {first} (then {second})"))
        }),
    }
}

fn set_mandatory_label(root: &Path, propagate: bool) -> io::Result<()> {
    // `icacls` is the documented user tool for mandatory labels and the only
    // mechanism verified to actually land the label on real hosts (the raw
    // `Set*SecurityInfo` LABEL calls have been observed to return success
    // while silently dropping the ACE). The result is verified by reading the
    // listing back — icacls exit codes alone are not trustworthy either.
    let mut command = std::process::Command::new("icacls");
    command.arg(root);
    if propagate {
        command.args(["/setintegritylevel", "(OI)(CI)Low", "/T", "/C", "/Q"]);
    } else {
        command.args(["/setintegritylevel", "(OI)(CI)Low", "/Q"]);
    }
    let _ = command
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();

    if label_visible(root) {
        return Ok(());
    }

    // Fallback: the raw API (may work where icacls is unavailable).
    if native_set_mandatory_label(root, propagate).is_ok() && label_visible(root) {
        return Ok(());
    }

    Err(io::Error::new(
        io::ErrorKind::PermissionDenied,
        "could not set the Low mandatory-integrity label (tried icacls and SetSecurityInfo)",
    ))
}

/// Reads the object's security descriptor listing and checks for a mandatory
/// label entry. `icacls <path>` prints `Mandatory Label\Low Mandatory Level:...`
/// when the label is present (the Get-Acl SDDL view hides ML ACEs entirely).
fn label_visible(root: &Path) -> bool {
    std::process::Command::new("icacls")
        .arg(root)
        .output()
        .map(|output| String::from_utf8_lossy(&output.stdout).contains("Mandatory Label"))
        .unwrap_or(false)
}

fn native_set_mandatory_label(root: &Path, propagate: bool) -> io::Result<()> {
    use windows_sys::Win32::Security::Authorization::SetSecurityInfo;
    use windows_sys::Win32::Security::{
        AddMandatoryAce, CreateWellKnownSid, InitializeAcl, WinLowLabelSid, ACL_REVISION,
    };

    const LABEL_SECURITY_INFORMATION: u32 = 0x0000_0100;
    const SYSTEM_MANDATORY_LABEL_NO_WRITE_UP: u32 = 0x1;
    const WRITE_OWNER: u32 = 0x0008_0000;

    let mut sacl_buf = [0u8; 128];
    let sacl = sacl_buf.as_mut_ptr() as *mut windows_sys::Win32::Security::ACL;
    let ok = unsafe { InitializeAcl(sacl, sacl_buf.len() as u32, ACL_REVISION) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let mut low_sid = [0u8; 64];
    let mut low_sid_len = low_sid.len() as u32;
    let ok = unsafe {
        CreateWellKnownSid(
            WinLowLabelSid,
            std::ptr::null_mut(),
            low_sid.as_mut_ptr() as PSID,
            &mut low_sid_len,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    let ok = unsafe {
        AddMandatoryAce(
            sacl,
            ACL_REVISION,
            CONTAINER_INHERIT_ACE | OBJECT_INHERIT_ACE,
            SYSTEM_MANDATORY_LABEL_NO_WRITE_UP,
            low_sid.as_ptr() as PSID,
        )
    };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }

    let path_wide: Vec<u16> = root
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    if propagate {
        let result = unsafe {
            SetNamedSecurityInfoW(
                path_wide.as_ptr(),
                SE_FILE_OBJECT,
                LABEL_SECURITY_INFORMATION,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
                sacl,
            )
        };
        if result != 0 {
            return Err(io::Error::from_raw_os_error(result as i32));
        }
        return Ok(());
    }

    let handle = open_for_security(root, WRITE_OWNER)?;
    let result = unsafe {
        SetSecurityInfo(
            handle,
            SE_FILE_OBJECT,
            LABEL_SECURITY_INFORMATION,
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            sacl,
        )
    };
    unsafe {
        CloseHandle(handle);
    }
    if result != 0 {
        return Err(io::Error::from_raw_os_error(result as i32));
    }
    Ok(())
}
fn block_network_for_package(sid: &[u8]) -> io::Result<()> {
    super::wfp::block_package_network(sid)
}

fn windows_str(text: &str) -> *mut u16 {
    let mut wide: Vec<u16> = std::ffi::OsStr::new(text)
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let ptr = wide.as_mut_ptr();
    std::mem::forget(wide);
    ptr
}

pub(crate) fn network_report(_sandbox: &Sandbox, notes: &mut Vec<String>) -> bool {
    match _sandbox.network() {
        SandboxNetworkPolicy::Allow => true,
        SandboxNetworkPolicy::Deny => {
            notes.push(
                "network=deny enforced by WFP filters keyed to the sandbox package SID (adding filters requires admin rights)"
                    .to_string(),
            );
            true
        }
    }
}

// ---------------------------------------------------------------------------
// Process creation
// ---------------------------------------------------------------------------

fn spawn_with_token(
    spec: &SpawnSpec,
    token: &OwnedHandle,
    package_sid: &[u8],
    deny_network: bool,
) -> io::Result<SandboxedChild> {
    // Pipes: (parent_end, child_end).
    let mut stdin_pair = match spec.stdin {
        StdioKind::Pipe => Some(create_pipe()?),
        _ => None,
    };
    let mut stdout_pair = match spec.stdout {
        StdioKind::Pipe => Some(create_pipe()?),
        _ => None,
    };
    let mut stderr_pair = match spec.stderr {
        StdioKind::Pipe => Some(create_pipe()?),
        _ => None,
    };

    // Child ends must be inheritable; parent ends must not.
    for (parent, child) in [&stdin_pair, &stdout_pair, &stderr_pair]
        .into_iter()
        .flatten()
    {
        unsafe {
            SetHandleInformation(
                child.as_raw_handle() as HANDLE,
                HANDLE_FLAG_INHERIT,
                HANDLE_FLAG_INHERIT,
            );
            SetHandleInformation(parent.as_raw_handle() as HANDLE, HANDLE_FLAG_INHERIT, 0);
        }
    }

    // Standard handles handed to the child.
    let nul_handle = || -> HANDLE {
        let name: Vec<u16> = std::ffi::OsStr::new("NUL")
            .encode_wide()
            .chain(std::iter::once(0))
            .collect();
        unsafe {
            CreateFileW(
                name.as_ptr(),
                GENERIC_READ | GENERIC_WRITE,
                FILE_SHARE_READ | FILE_SHARE_WRITE,
                std::ptr::null(),
                OPEN_EXISTING,
                FILE_ATTRIBUTE_NORMAL,
                std::ptr::null_mut(),
            )
        }
    };

    let child_stdin = match spec.stdin {
        StdioKind::Pipe => stdin_pair.as_ref().unwrap().1.as_raw_handle() as HANDLE,
        StdioKind::Inherit => {
            let handle = std::io::stdin().as_raw_handle() as HANDLE;
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            }
            handle
        }
        StdioKind::Null => nul_handle(),
    };
    let child_stdout = match spec.stdout {
        StdioKind::Pipe => stdout_pair.as_ref().unwrap().1.as_raw_handle() as HANDLE,
        StdioKind::Inherit => {
            let handle = std::io::stdout().as_raw_handle() as HANDLE;
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            }
            handle
        }
        StdioKind::Null => nul_handle(),
    };
    let child_stderr = match spec.stderr {
        StdioKind::Pipe => stderr_pair.as_ref().unwrap().1.as_raw_handle() as HANDLE,
        StdioKind::Inherit => {
            let handle = std::io::stderr().as_raw_handle() as HANDLE;
            unsafe {
                SetHandleInformation(handle, HANDLE_FLAG_INHERIT, HANDLE_FLAG_INHERIT);
            }
            handle
        }
        StdioKind::Null => nul_handle(),
    };

    // Command line: raw form preserves bash's `cmd /C <command>` byte-for-byte.
    let mut cmdline: Vec<u16> = match &spec.windows_raw_cmdline {
        Some(raw) => std::ffi::OsStr::new(raw)
            .encode_wide()
            .chain(std::iter::once(0))
            .collect(),
        None => {
            let built = std::iter::once(quote_windows_arg(&spec.program))
                .chain(spec.args.iter().map(|arg| quote_windows_arg(arg)))
                .collect::<Vec<_>>()
                .join(" ");
            std::ffi::OsStr::new(&built)
                .encode_wide()
                .chain(std::iter::once(0))
                .collect()
        }
    };

    let env_block = build_env_block(&spec.envs, spec.clear_env);
    let cwd_wide: Vec<u16> = spec
        .cwd
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();

    let mut flags: PROCESS_CREATION_FLAGS = CREATE_NO_WINDOW | CREATE_UNICODE_ENVIRONMENT;

    // STARTUPINFOEX + SECURITY_CAPABILITIES tags the process with the
    // AppContainer package SID — the WFP key for network=deny.
    let mut startup_ex: STARTUPINFOEXW = unsafe { std::mem::zeroed() };
    // cb must describe the structure actually passed: the extended size only
    // with EXTENDED_STARTUPINFO_PRESENT, else CreateProcess fails with
    // ERROR_INVALID_PARAMETER.
    startup_ex.StartupInfo.cb = if deny_network {
        std::mem::size_of::<STARTUPINFOEXW>()
    } else {
        std::mem::size_of::<STARTUPINFOW>()
    } as u32;
    startup_ex.StartupInfo.dwFlags = STARTF_USESTDHANDLES;
    startup_ex.StartupInfo.hStdInput = child_stdin;
    startup_ex.StartupInfo.hStdOutput = child_stdout;
    startup_ex.StartupInfo.hStdError = child_stderr;

    let mut capabilities: SECURITY_CAPABILITIES = unsafe { std::mem::zeroed() };
    let mut attr_list: *mut c_void = std::ptr::null_mut();
    // Allocated when the attribute list is needed; must outlive the spawn.
    let mut attr_buffer: Option<Vec<u8>> = None;

    if deny_network {
        capabilities.AppContainerSid = package_sid.as_ptr() as PSID;

        let mut size: usize = 0;
        unsafe {
            InitializeProcThreadAttributeList(std::ptr::null_mut(), 1, 0, &mut size);
        }
        let buffer = vec![0u8; size];
        attr_list = buffer.as_ptr() as *mut c_void;
        let ok = unsafe { InitializeProcThreadAttributeList(attr_list, 1, 0, &mut size) };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        attr_buffer = Some(buffer);
        let ok = unsafe {
            UpdateProcThreadAttribute(
                attr_list,
                0,
                PROC_THREAD_ATTRIBUTE_SECURITY_CAPABILITIES as usize,
                &mut capabilities as *mut _ as *mut c_void,
                std::mem::size_of::<SECURITY_CAPABILITIES>(),
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        if ok == 0 {
            return Err(io::Error::last_os_error());
        }
        startup_ex.lpAttributeList = attr_list;
        flags |= EXTENDED_STARTUPINFO_PRESENT;
    }

    let mut process_info: PROCESS_INFORMATION = unsafe { std::mem::zeroed() };
    let startup_ptr = &mut startup_ex.StartupInfo as *mut _;

    let created = unsafe {
        CreateProcessAsUserW(
            token.as_raw_handle() as HANDLE,
            std::ptr::null(),
            cmdline.as_mut_ptr(),
            std::ptr::null_mut(),
            std::ptr::null_mut(),
            1,
            flags,
            env_block.as_ptr() as *const c_void,
            cwd_wide.as_ptr(),
            startup_ptr,
            &mut process_info,
        )
    };

    if created == 0 {
        // Same-user restricted tokens do not normally need privileges, but
        // policy can still refuse CreateProcessAsUser; the token-aware
        // CreateProcessWithTokenW is the documented fallback.
        let status = unsafe {
            CreateProcessWithTokenW(
                token.as_raw_handle() as HANDLE,
                0,
                std::ptr::null(),
                cmdline.as_mut_ptr(),
                flags,
                env_block.as_ptr() as *const c_void,
                cwd_wide.as_ptr(),
                startup_ptr,
                &mut process_info,
            )
        };
        if status == 0 {
            return Err(io::Error::last_os_error());
        }
    }

    if !attr_list.is_null() {
        unsafe {
            DeleteProcThreadAttributeList(attr_list);
        }
    }
    // Explicitly release the attribute-list backing store (it only existed to
    // keep the allocation alive across the spawn calls).
    drop(attr_buffer);

    // Parent-side pipe ends become the child's stdio surface.
    let stdin_file = stdin_pair
        .take()
        .map(|(parent, _child)| file_of_handle(parent));
    let stdout_file = stdout_pair
        .take()
        .map(|(parent, _child)| file_of_handle(parent));
    let stderr_file = stderr_pair
        .take()
        .map(|(parent, _child)| file_of_handle(parent));

    // Job Object: kill-on-close so a dropped/timeout tool run never leaves
    // orphans.
    let job = create_job(process_info.hProcess)?;
    unsafe {
        CloseHandle(process_info.hThread);
    }

    Ok(SandboxedChild::new(
        ChildHandle {
            inner: Inner::Sandboxed {
                process: HandleGuard(process_info.hProcess),
                job: Some(job),
            },
            pid: process_info.dwProcessId,
            exit_code: AtomicI64::new(-1),
        },
        stdin_file,
        stdout_file,
        stderr_file,
    ))
}

/// Test-only diagnostic: which step of the mandatory-label dance fails on
/// this host, with concrete error codes.
fn file_of_handle(handle: OwnedHandle) -> std::fs::File {
    unsafe { std::fs::File::from_raw_handle(handle.into_raw_handle()) }
}

fn create_pipe() -> io::Result<(OwnedHandle, OwnedHandle)> {
    let mut read: HANDLE = std::ptr::null_mut();
    let mut write: HANDLE = std::ptr::null_mut();
    let ok = unsafe { CreatePipe(&mut read, &mut write, std::ptr::null(), 0) };
    if ok == 0 {
        return Err(io::Error::last_os_error());
    }
    Ok((
        unsafe { OwnedHandle::from_raw_handle(read as RawHandle) },
        unsafe { OwnedHandle::from_raw_handle(write as RawHandle) },
    ))
}

fn create_job(process: HANDLE) -> io::Result<HandleGuard> {
    let job = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
    if job.is_null() || job == INVALID_HANDLE_VALUE {
        return Err(io::Error::last_os_error());
    }
    let mut limits: JOBOBJECT_EXTENDED_LIMIT_INFORMATION = unsafe { std::mem::zeroed() };
    limits.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
    let ok = unsafe {
        SetInformationJobObject(
            job,
            JobObjectExtendedLimitInformation,
            &limits as *const _ as *const c_void,
            std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32,
        )
    };
    if ok == 0 {
        unsafe {
            CloseHandle(job);
        }
        return Err(io::Error::last_os_error());
    }
    let ok = unsafe { AssignProcessToJobObject(job, process) };
    if ok == 0 {
        // A parent job with breakaway restrictions can refuse; degrade to
        // taskkill-tree rather than failing the spawn.
        unsafe {
            CloseHandle(job);
        }
        return Ok(HandleGuard(INVALID_HANDLE_VALUE));
    }
    Ok(HandleGuard(job))
}

fn build_env_block(
    overrides: &[(std::ffi::OsString, std::ffi::OsString)],
    clear: bool,
) -> Vec<u16> {
    use std::os::windows::ffi::OsStringExt;

    let mut entries: Vec<(String, std::ffi::OsString)> = Vec::new();
    if !clear {
        for (key, value) in std::env::vars_os() {
            entries.push((key.to_string_lossy().to_uppercase(), value));
        }
    }
    for (key, value) in overrides {
        let upper = key.to_string_lossy().to_uppercase();
        entries.retain(|(existing, _)| *existing != upper);
        entries.push((upper, value.clone()));
    }

    let mut block: Vec<u16> = Vec::new();
    for (key, value) in &entries {
        let line = std::ffi::OsString::from(format!("{}=", key));
        block.extend(line.encode_wide());
        block.extend(value.encode_wide());
        block.push(0);
    }
    block.push(0);
    block
}
