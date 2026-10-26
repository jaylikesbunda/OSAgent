//! Unix sandbox backend: Landlock + seccomp on Linux, Seatbelt on macOS.
//!
//! Both are applied *per spawned process*, not to the agent itself. Linux
//! restrictions land in `pre_exec` (after fork, before exec) so they are
//! inherited through `execve` and can never race the child's first
//! instruction. macOS wraps the command in `sandbox-exec` with a generated
//! SBPL profile.
//!
//! Enforcement failures degrade with an explicit note (the tool-layer checks
//! remain) — except `network = "deny"`, which is a strict request and fails
//! closed.

use super::super::child::{SandboxedChild, SpawnSpec, StdioKind};
use super::super::Sandbox;
use crate::config::SandboxNetworkPolicy;
use std::io;
use std::process::Stdio;

pub(crate) struct ChildHandle {
    child: std::sync::Mutex<std::process::Child>,
    pgid: Option<i32>,
}

impl ChildHandle {
    pub(crate) fn id(&self) -> u32 {
        self.child.lock().map(|c| c.id()).unwrap_or(0)
    }

    pub(crate) fn is_finished(&self) -> bool {
        self.child
            .lock()
            .map(|mut c| c.try_wait().map(|s| s.is_some()).unwrap_or(true))
            .unwrap_or(true)
    }

    pub(crate) fn try_wait(&mut self) -> io::Result<Option<std::process::ExitStatus>> {
        match self.child.lock() {
            Ok(mut child) => child.try_wait(),
            Err(_) => Ok(None),
        }
    }

    pub(crate) fn wait_blocking(&mut self) -> io::Result<std::process::ExitStatus> {
        match self.child.lock() {
            Ok(mut child) => child.wait(),
            Err(_) => Err(io::Error::new(io::ErrorKind::Other, "child lock poisoned")),
        }
    }

    pub(crate) fn kill_tree(&mut self) {
        #[cfg(unix)]
        unsafe {
            if let Some(pgid) = self.pgid {
                libc::kill(-pgid, libc::SIGKILL);
            }
        }
        if let Ok(mut child) = self.child.lock() {
            let _ = child.kill();
        }
    }
}

fn stdio_of(kind: StdioKind) -> Stdio {
    match kind {
        StdioKind::Null => Stdio::null(),
        StdioKind::Inherit => Stdio::inherit(),
        StdioKind::Pipe => Stdio::piped(),
    }
}

fn finish_spawn(mut child: std::process::Child, new_process_group: bool) -> SandboxedChild {
    use std::os::unix::io::{FromRawFd, IntoRawFd};

    let pgid = if new_process_group {
        Some(child.id() as i32)
    } else {
        None
    };
    let file_of = |stream: std::process::ChildStdout| unsafe {
        std::fs::File::from_raw_fd(stream.into_raw_fd())
    };
    let file_of_in = |stream: std::process::ChildStdin| unsafe {
        std::fs::File::from_raw_fd(stream.into_raw_fd())
    };
    let file_of_err = |stream: std::process::ChildStderr| unsafe {
        std::fs::File::from_raw_fd(stream.into_raw_fd())
    };
    let stdin = child.stdin.take().map(file_of_in);
    let stdout = child.stdout.take().map(file_of);
    let stderr = child.stderr.take().map(file_of_err);
    SandboxedChild::new(
        ChildHandle {
            child: std::sync::Mutex::new(child),
            pgid,
        },
        stdin,
        stdout,
        stderr,
    )
}

fn build_command(spec: &SpawnSpec) -> std::process::Command {
    use std::os::unix::process::CommandExt;

    let mut cmd = std::process::Command::new(&spec.program);
    cmd.args(&spec.args);
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
    if spec.new_process_group {
        cmd.process_group(0);
    }
    cmd
}

pub(crate) fn spawn_plain(spec: SpawnSpec) -> io::Result<SandboxedChild> {
    let new_process_group = spec.new_process_group;
    let child = build_command(&spec).spawn()?;
    Ok(finish_spawn(child, new_process_group))
}

pub(crate) fn spawn_confined(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
    #[cfg(target_os = "linux")]
    {
        linux::spawn_confined(sandbox, spec)
    }
    #[cfg(target_os = "macos")]
    {
        macos::spawn_confined(sandbox, spec)
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    {
        let _ = sandbox;
        // No backend for this Unix: run plain and let `status()` say so.
        spawn_plain(spec)
    }
}

pub(crate) fn network_report(sandbox: &Sandbox, notes: &mut Vec<String>) -> bool {
    match sandbox.network() {
        SandboxNetworkPolicy::Allow => true,
        SandboxNetworkPolicy::Deny => {
            #[cfg(target_os = "linux")]
            {
                notes.push(
                    "network=deny enforced by seccomp (socket filter) per spawned process"
                        .to_string(),
                );
                true
            }
            #[cfg(target_os = "macos")]
            {
                notes.push(
                    "network=deny enforced by the Seatbelt profile per spawned process".to_string(),
                );
                true
            }
            #[cfg(not(any(target_os = "linux", target_os = "macos")))]
            {
                notes.push("network=deny is not supported on this platform".to_string());
                false
            }
        }
    }
}

#[cfg(target_os = "linux")]
mod linux {
    use super::*;
    use crate::config::SandboxMode;

    // Landlock syscall numbers (asm-generic, stable across x86_64/aarch64).
    const SYS_LANDLOCK_CREATE_RULESET: libc::c_long = 444;
    const SYS_LANDLOCK_ADD_RULE: libc::c_long = 445;
    const SYS_LANDLOCK_RESTRICT_SELF: libc::c_long = 446;

    const LANDLOCK_CREATE_RULESET_VERSION: u32 = 1;

    // Access rights, ABI 1..3.
    const ACCESS_EXECUTE: u64 = 1 << 0;
    const ACCESS_WRITE_FILE: u64 = 1 << 1;
    const ACCESS_READ_FILE: u64 = 1 << 2;
    const ACCESS_READ_DIR: u64 = 1 << 3;
    const ACCESS_REMOVE_DIR: u64 = 1 << 4;
    const ACCESS_REMOVE_FILE: u64 = 1 << 5;
    const ACCESS_MAKE_CHAR: u64 = 1 << 6;
    const ACCESS_MAKE_DIR: u64 = 1 << 7;
    const ACCESS_MAKE_FIFO: u64 = 1 << 8;
    const ACCESS_MAKE_REG: u64 = 1 << 9;
    const ACCESS_MAKE_SOCK: u64 = 1 << 10;
    const ACCESS_MAKE_BLOCK: u64 = 1 << 11;
    const ACCESS_MAKE_SYM: u64 = 1 << 12;
    const ACCESS_REFER: u64 = 1 << 13;
    const ACCESS_TRUNCATE: u64 = 1 << 14;

    #[repr(C)]
    struct RulesetAttr {
        handled_access_fs: u64,
    }

    #[repr(C)]
    struct PathBeneathAttr {
        allowed_access: u64,
        parent_fd: i32,
    }

    fn landlock_abi_version() -> Option<i64> {
        let ret = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                std::ptr::null::<libc::c_void>(),
                0usize,
                LANDLOCK_CREATE_RULESET_VERSION,
            )
        };
        if ret < 0 {
            None
        } else {
            Some(ret)
        }
    }

    fn handled_rights(abi: i64) -> u64 {
        let mut rights = ACCESS_EXECUTE
            | ACCESS_WRITE_FILE
            | ACCESS_READ_FILE
            | ACCESS_READ_DIR
            | ACCESS_REMOVE_DIR
            | ACCESS_REMOVE_FILE
            | ACCESS_MAKE_CHAR
            | ACCESS_MAKE_DIR
            | ACCESS_MAKE_FIFO
            | ACCESS_MAKE_REG
            | ACCESS_MAKE_SOCK
            | ACCESS_MAKE_BLOCK
            | ACCESS_MAKE_SYM;
        if abi >= 2 {
            rights |= ACCESS_REFER;
        }
        if abi >= 3 {
            rights |= ACCESS_TRUNCATE;
        }
        rights
    }

    fn all_write_rights(abi: i64) -> u64 {
        let mut rights = ACCESS_WRITE_FILE
            | ACCESS_REMOVE_DIR
            | ACCESS_REMOVE_FILE
            | ACCESS_MAKE_CHAR
            | ACCESS_MAKE_DIR
            | ACCESS_MAKE_FIFO
            | ACCESS_MAKE_REG
            | ACCESS_MAKE_SOCK
            | ACCESS_MAKE_BLOCK
            | ACCESS_MAKE_SYM;
        if abi >= 2 {
            rights |= ACCESS_REFER;
        }
        if abi >= 3 {
            rights |= ACCESS_TRUNCATE;
        }
        rights
    }

    /// Builds the Landlock ruleset for the policy. Returns the ruleset fd and
    /// any degradation notes. `None` fd = kernel support missing.
    fn build_ruleset(sandbox: &Sandbox) -> (Option<i32>, Vec<String>) {
        let mut notes = Vec::new();
        let Some(abi) = landlock_abi_version() else {
            notes.push(
                "landlock unavailable on this kernel: filesystem confinement degraded to tool-layer checks"
                    .to_string(),
            );
            return (None, notes);
        };

        let handled = handled_rights(abi);
        let attr = RulesetAttr {
            handled_access_fs: handled,
        };
        let fd = unsafe {
            libc::syscall(
                SYS_LANDLOCK_CREATE_RULESET,
                &attr as *const RulesetAttr,
                std::mem::size_of::<RulesetAttr>(),
                0u32,
            )
        };
        if fd < 0 {
            notes.push(format!(
                "landlock_create_ruleset failed (errno {}): filesystem confinement degraded",
                io::Error::last_os_error().raw_os_error().unwrap_or(0)
            ));
            return (None, notes);
        }
        let ruleset_fd = fd as i32;

        fn add_rule(
            ruleset_fd: i32,
            path: &std::path::Path,
            access: u64,
            notes: &mut Vec<String>,
        ) -> bool {
            use std::os::unix::io::AsRawFd;
            let file = match std::fs::File::open(path) {
                Ok(file) => file,
                Err(error) => {
                    notes.push(format!(
                        "landlock: cannot anchor rule on {}: {}",
                        path.display(),
                        error
                    ));
                    return false;
                }
            };
            let attr = PathBeneathAttr {
                allowed_access: access,
                parent_fd: file.as_raw_fd(),
            };
            let ret = unsafe {
                libc::syscall(
                    SYS_LANDLOCK_ADD_RULE,
                    ruleset_fd,
                    1i32, // LANDLOCK_RULE_PATH_BENEATH
                    &attr as *const PathBeneathAttr,
                    0u32,
                )
            };
            ret == 0
        }

        // Reads everywhere (Codex semantics), writes never by default.
        let read_rights = ACCESS_EXECUTE | ACCESS_READ_FILE | ACCESS_READ_DIR;
        add_rule(
            ruleset_fd,
            std::path::Path::new("/"),
            read_rights,
            &mut notes,
        );

        if sandbox.mode() == SandboxMode::WorkspaceWrite {
            // Scratch spaces and devices must stay usable for builds,
            // interpreters, and stdout redirection.
            for extra in ["/tmp", "/var/tmp", "/dev", "/run/user"] {
                let path = std::path::Path::new(extra);
                if path.exists() {
                    add_rule(ruleset_fd, path, handled, &mut notes);
                }
            }
            for root in sandbox.roots() {
                if root.exists() {
                    add_rule(
                        ruleset_fd,
                        root,
                        handled | all_write_rights(abi),
                        &mut notes,
                    );
                } else {
                    notes.push(format!(
                        "landlock: writable root {} does not exist yet; creating it is denied in-sandbox",
                        root.display()
                    ));
                }
            }
        }

        (Some(ruleset_fd), notes)
    }

    const SECCOMP_SET_MODE_FILTER: libc::c_long = 1;
    const SECCOMP_RET_ERRNO: u32 = 0x0005_0000;
    const SECCOMP_RET_ALLOW: u32 = 0x7FFF_0000;
    const AUDIT_ARCH_X86_64: u32 = 0xC000_003E;
    const AUDIT_ARCH_AARCH64: u32 = 0xC000_00B7;

    const BPF_LD: u16 = 0x00;
    const BPF_W: u16 = 0x00;
    const BPF_ABS: u16 = 0x20;
    const BPF_JMP: u16 = 0x05;
    const BPF_JEQ: u16 = 0x10;
    const BPF_K: u16 = 0x00;
    const BPF_RET: u16 = 0x06;

    #[repr(C)]
    #[derive(Clone, Copy)]
    struct SockFilter {
        code: u16,
        jt: u8,
        jf: u8,
        k: u32,
    }

    #[repr(C)]
    struct SockFprog {
        len: u16,
        filter: *const SockFilter,
    }

    /// Syscalls denied outright in every sandboxed process.
    fn denied_syscalls() -> Vec<i64> {
        #[cfg(target_arch = "x86_64")]
        let names: &[(libc::c_long, &str)] = &[
            (libc::SYS_mount, "mount"),
            (libc::SYS_umount2, "umount2"),
            (libc::SYS_pivot_root, "pivot_root"),
            (libc::SYS_chroot, "chroot"),
            (libc::SYS_setns, "setns"),
            (libc::SYS_unshare, "unshare"),
            (libc::SYS_ptrace, "ptrace"),
            (libc::SYS_bpf, "bpf"),
            (libc::SYS_perf_event_open, "perf_event_open"),
            (libc::SYS_kexec_load, "kexec_load"),
            (libc::SYS_init_module, "init_module"),
            (libc::SYS_finit_module, "finit_module"),
            (libc::SYS_delete_module, "delete_module"),
            (libc::SYS_swapon, "swapon"),
            (libc::SYS_swapoff, "swapoff"),
            (libc::SYS_reboot, "reboot"),
            (libc::SYS_clock_settime, "clock_settime"),
            (libc::SYS_settimeofday, "settimeofday"),
            (libc::SYS_acct, "acct"),
            (libc::SYS_quotactl, "quotactl"),
            (libc::SYS_syslog, "syslog"),
            (libc::SYS_keyctl, "keyctl"),
            (libc::SYS_add_key, "add_key"),
            (libc::SYS_request_key, "request_key"),
            (libc::SYS_userfaultfd, "userfaultfd"),
        ];
        #[cfg(target_arch = "aarch64")]
        let names: &[(libc::c_long, &str)] = &[
            (libc::SYS_mount, "mount"),
            (libc::SYS_umount2, "umount2"),
            (libc::SYS_pivot_root, "pivot_root"),
            (libc::SYS_chroot, "chroot"),
            (libc::SYS_setns, "setns"),
            (libc::SYS_unshare, "unshare"),
            (libc::SYS_ptrace, "ptrace"),
            (libc::SYS_bpf, "bpf"),
            (libc::SYS_perf_event_open, "perf_event_open"),
            (libc::SYS_init_module, "init_module"),
            (libc::SYS_finit_module, "finit_module"),
            (libc::SYS_delete_module, "delete_module"),
            (libc::SYS_reboot, "reboot"),
            (libc::SYS_clock_settime, "clock_settime"),
            (libc::SYS_settimeofday, "settimeofday"),
            (libc::SYS_acct, "acct"),
            (libc::SYS_syslog, "syslog"),
            (libc::SYS_keyctl, "keyctl"),
            (libc::SYS_add_key, "add_key"),
            (libc::SYS_request_key, "request_key"),
            (libc::SYS_userfaultfd, "userfaultfd"),
        ];
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        let names: &[(libc::c_long, &str)] = &[];
        names.iter().map(|(number, _)| *number as i64).collect()
    }

    /// Installs the seccomp deny-list. `deny_network` additionally blocks
    /// `socket(AF_INET/AF_INET6)` (Unix-domain sockets keep working).
    /// Builds the seccomp filter program (pure construction, unit-testable
    /// with a tiny BPF interpreter below).
    fn build_seccomp_filter(deny_network: bool) -> Vec<SockFilter> {
        let mut filter: Vec<SockFilter> = Vec::new();

        let ld_arch = SockFilter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 4, // offsetof(seccomp_data, arch)
        };
        filter.push(ld_arch);

        #[cfg(target_arch = "x86_64")]
        let expected_arch = AUDIT_ARCH_X86_64;
        #[cfg(target_arch = "aarch64")]
        let expected_arch = AUDIT_ARCH_AARCH64;
        #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
        let expected_arch = 0;

        // Wrong arch -> deny everything. On a match the jump skips the deny
        // below and falls into normal filtering; on a mismatch it lands
        // straight on the deny. (Both offsets were inverted in the first
        // version — which denied *every* syscall on the matching arch.)
        filter.push(SockFilter {
            code: BPF_JMP | BPF_JEQ | BPF_K,
            jt: 1,
            jf: 0,
            k: expected_arch,
        });
        filter.push(SockFilter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ERRNO | (libc::EPERM as u32),
        });

        // Load syscall number.
        filter.push(SockFilter {
            code: BPF_LD | BPF_W | BPF_ABS,
            jt: 0,
            jf: 0,
            k: 0, // offsetof(seccomp_data, nr)
        });

        let deny_at = filter.len();

        if deny_network {
            // socket(AF_INET/AF_INET6) -> EPERM. Load args[0] on match.
            #[cfg(target_arch = "x86_64")]
            let socket_nr = libc::SYS_socket;
            #[cfg(target_arch = "aarch64")]
            let socket_nr = libc::SYS_socket;
            #[cfg(not(any(target_arch = "x86_64", target_arch = "aarch64")))]
            let socket_nr = -1;

            // socket(AF_INET/AF_INET6) -> EPERM. On a socket syscall, fall
            // through to the family check; anything else skips the whole
            // 5-instruction block.
            filter.push(SockFilter {
                code: BPF_JMP | BPF_JEQ | BPF_K,
                jt: 0,
                jf: 5,
                k: socket_nr as u32,
            });
            filter.push(SockFilter {
                code: BPF_LD | BPF_W | BPF_ABS,
                jt: 0,
                jf: 0,
                k: 16, // offsetof(seccomp_data, args[0])
            });
            filter.push(SockFilter {
                code: BPF_JMP | BPF_JEQ | BPF_K,
                jt: 0,
                jf: 1,
                k: libc::AF_INET as u32,
            });
            filter.push(SockFilter {
                code: BPF_RET | BPF_K,
                jt: 0,
                jf: 0,
                k: SECCOMP_RET_ERRNO | (libc::EPERM as u32),
            });
            // AF_INET6 lives in the low 32 bits too; deny it as well.
            filter.push(SockFilter {
                code: BPF_JMP | BPF_JEQ | BPF_K,
                jt: 0,
                jf: 1,
                k: libc::AF_INET6 as u32,
            });
            filter.push(SockFilter {
                code: BPF_RET | BPF_K,
                jt: 0,
                jf: 0,
                k: SECCOMP_RET_ERRNO | (libc::EPERM as u32),
            });
        }

        for nr in denied_syscalls() {
            // Match -> fall into the deny below; mismatch -> skip it.
            filter.push(SockFilter {
                code: BPF_JMP | BPF_JEQ | BPF_K,
                jt: 0,
                jf: 1,
                k: nr as u32,
            });
            filter.push(SockFilter {
                code: BPF_RET | BPF_K,
                jt: 0,
                jf: 0,
                k: SECCOMP_RET_ERRNO | (libc::EPERM as u32),
            });
        }
        let _ = deny_at;

        filter.push(SockFilter {
            code: BPF_RET | BPF_K,
            jt: 0,
            jf: 0,
            k: SECCOMP_RET_ALLOW,
        });

        filter
    }

    fn install_seccomp(deny_network: bool) -> Result<(), String> {
        let filter = build_seccomp_filter(deny_network);
        let prog = SockFprog {
            len: filter.len() as u16,
            filter: filter.as_ptr(),
        };

        let nnp = unsafe { libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) };
        if nnp != 0 {
            return Err(format!(
                "prctl(PR_SET_NO_NEW_PRIVS) failed: {}",
                io::Error::last_os_error()
            ));
        }

        let ret = unsafe {
            libc::syscall(
                libc::SYS_seccomp,
                SECCOMP_SET_MODE_FILTER,
                0u32,
                &prog as *const SockFprog,
            )
        };
        if ret != 0 {
            return Err(format!(
                "seccomp(SECCOMP_SET_MODE_FILTER) failed: {}",
                io::Error::last_os_error()
            ));
        }
        Ok(())
    }

    /// Minimal BPF interpreter over synthetic `seccomp_data`, used to pin the
    /// filter's semantics: the jump offsets are easy to invert silently and
    /// no test spawns through the Linux sandbox in CI.
    #[cfg(test)]
    mod seccomp_filter_tests {
        use super::*;

        const ALLOW: u32 = SECCOMP_RET_ALLOW;
        const DENY: u32 = SECCOMP_RET_ERRNO | (libc::EPERM as u32);

        /// Runs the filter on a fake seccomp_data (nr at 0, arch at 4,
        /// args[0] low word at 16) and returns the verdict.
        fn run(filter: &[SockFilter], arch: u32, nr: u32, arg0: u32) -> u32 {
            let words = |offset: u32| -> u32 {
                match offset {
                    0 => nr,
                    4 => arch,
                    16 => arg0,
                    20 => 0,
                    _ => 0,
                }
            };
            let mut acc: u32 = 0;
            let mut pc: usize = 0;
            loop {
                let insn = filter[pc];
                match insn.code {
                    code if code == (BPF_LD | BPF_W | BPF_ABS) => {
                        acc = words(insn.k);
                        pc += 1;
                    }
                    code if code == (BPF_JMP | BPF_JEQ | BPF_K) => {
                        pc += 1 + if acc == insn.k {
                            insn.jt as usize
                        } else {
                            insn.jf as usize
                        };
                    }
                    code if code == (BPF_RET | BPF_K) => return insn.k,
                    other => panic!("unexpected BPF opcode {:#x} at {}", other, pc),
                }
            }
        }

        #[test]
        fn ordinary_syscalls_pass_and_dangerous_ones_deny() {
            let filter = build_seccomp_filter(false);
            let arch = if cfg!(target_arch = "x86_64") {
                AUDIT_ARCH_X86_64
            } else {
                AUDIT_ARCH_AARCH64
            };
            assert_eq!(run(&filter, arch, libc::SYS_execve as u32, 0), ALLOW);
            assert_eq!(run(&filter, arch, libc::SYS_read as u32, 0), ALLOW);
            assert_eq!(run(&filter, arch, libc::SYS_mount as u32, 0), DENY);
            assert_eq!(run(&filter, arch, libc::SYS_ptrace as u32, 0), DENY);
            assert_eq!(run(&filter, arch, libc::SYS_reboot as u32, 0), DENY);
        }

        #[test]
        fn wrong_arch_is_denied_outright() {
            let filter = build_seccomp_filter(false);
            // Some other arch (i386): the very first check must deny.
            assert_eq!(run(&filter, 0x4000_0003, libc::SYS_read as u32, 0), DENY);
        }

        #[test]
        fn network_deny_blocks_internet_sockets_only() {
            let filter = build_seccomp_filter(true);
            let arch = if cfg!(target_arch = "x86_64") {
                AUDIT_ARCH_X86_64
            } else {
                AUDIT_ARCH_AARCH64
            };
            // socket(AF_INET) / socket(AF_INET6) are refused...
            assert_eq!(
                run(&filter, arch, libc::SYS_socket as u32, libc::AF_INET as u32),
                DENY
            );
            assert_eq!(
                run(
                    &filter,
                    arch,
                    libc::SYS_socket as u32,
                    libc::AF_INET6 as u32
                ),
                DENY
            );
            // ...while unix sockets and everything else keep working.
            assert_eq!(
                run(&filter, arch, libc::SYS_socket as u32, libc::AF_UNIX as u32),
                ALLOW
            );
            assert_eq!(run(&filter, arch, libc::SYS_connect as u32, 0), ALLOW);
            assert_eq!(run(&filter, arch, libc::SYS_mount as u32, 0), DENY);
        }
    }

    pub(super) fn spawn_confined(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
        use std::os::unix::process::CommandExt;

        let (ruleset_fd, mut notes) = build_ruleset(sandbox);
        let deny_network = sandbox.network() == SandboxNetworkPolicy::Deny;

        if ruleset_fd.is_none() && sandbox.network() == SandboxNetworkPolicy::Deny {
            // Strict request without a strict backend: fail closed.
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "sandbox network=deny requires Landlock/seccomp support on this kernel",
            ));
        }

        let mut cmd = build_command(&spec);

        // SAFETY: the closure only performs raw syscalls on data prepared
        // above (ruleset fd, plain BPF program) and is safe to run between
        // fork and exec.
        unsafe {
            cmd.pre_exec(move || {
                if let Some(fd) = ruleset_fd {
                    // nnp is also required by landlock_restrict_self.
                    if libc::prctl(libc::PR_SET_NO_NEW_PRIVS, 1, 0, 0, 0) != 0 {
                        return Err(io::Error::last_os_error());
                    }
                    let ret = libc::syscall(SYS_LANDLOCK_RESTRICT_SELF, fd, 0u32);
                    if ret != 0 {
                        return Err(io::Error::last_os_error());
                    }
                }
                if deny_network {
                    if let Err(error) = install_seccomp(true) {
                        return Err(io::Error::new(io::ErrorKind::Other, error));
                    }
                } else if ruleset_fd.is_some() {
                    // Landlock alone does not block mount/ptrace-class
                    // syscalls; add the deny-list whenever we confine at all.
                    if let Err(error) = install_seccomp(false) {
                        return Err(io::Error::new(io::ErrorKind::Other, error));
                    }
                }
                Ok(())
            });
        }

        let child = match cmd.spawn() {
            Ok(child) => child,
            Err(error) => {
                for note in notes.drain(..) {
                    sandbox.platform.add_note(note);
                }
                return Err(error);
            }
        };
        for note in notes {
            sandbox.platform.add_note(note);
        }
        let new_process_group = spec.new_process_group;
        Ok(finish_spawn(child, new_process_group))
    }
}

#[cfg(target_os = "macos")]
mod macos {
    use super::*;
    use crate::config::SandboxMode;

    fn seatbelt_profile(sandbox: &Sandbox) -> Result<std::path::PathBuf, String> {
        let mut profile = String::from("(version 1)\n(deny default)\n");
        profile.push_str(
            "(allow process*)\n(allow sysctl-read)\n(allow mach-lookup)\n(allow file-read*)\n(allow file-ioctl)\n",
        );

        let mut writable: Vec<String> = vec![
            "/dev".to_string(),
            "/tmp".to_string(),
            "/private/tmp".to_string(),
            "/private/var/tmp".to_string(),
        ];
        if sandbox.mode() == SandboxMode::WorkspaceWrite {
            for root in sandbox.roots() {
                writable.push(root.display().to_string());
            }
        }

        if sandbox.mode() != SandboxMode::ReadOnly {
            profile.push_str("(allow file-write*\n");
            for path in &writable {
                profile.push_str(&format!("  (subpath \"{}\")\n", path.replace('"', "\\\"")));
            }
            profile.push_str(")\n");
            // Scratch writes that are create/delete rather than in-place.
            profile.push_str("(allow file-write-data\n");
            for path in &writable {
                profile.push_str(&format!("  (subpath \"{}\")\n", path.replace('"', "\\\"")));
            }
            profile.push_str(")\n");
        }

        if sandbox.network() == SandboxNetworkPolicy::Allow {
            profile.push_str("(allow network*)\n");
        } else {
            // DNS-less local sockets only.
            profile.push_str("(allow network-out (local ip \"localhost:*\"))\n");
        }

        let key = format!(
            "osa-sandbox-{:x}.sb",
            std::collections::hash_map::DefaultHasher::new().finish() as u64 ^ profile.len() as u64
        );
        let path = std::env::temp_dir().join(key);
        if !path.exists() {
            std::fs::write(&path, &profile)
                .map_err(|error| format!("cannot write seatbelt profile: {}", error))?;
        }
        Ok(path)
    }

    pub(super) fn spawn_confined(sandbox: &Sandbox, spec: SpawnSpec) -> io::Result<SandboxedChild> {
        if sandbox.network() == SandboxNetworkPolicy::Deny {
            // Strict request; the profile must exist to honor it.
            let profile = seatbelt_profile(sandbox).map_err(|error| {
                io::Error::new(io::ErrorKind::Other, format!("sandbox: {}", error))
            })?;
            return spawn_with_profile(profile, spec);
        }
        match seatbelt_profile(sandbox) {
            Ok(profile) => spawn_with_profile(profile, spec),
            Err(error) => {
                sandbox.platform.add_note(format!(
                    "seatbelt profile unavailable ({}); process ran unsandboxed",
                    error
                ));
                spawn_plain(spec)
            }
        }
    }

    fn spawn_with_profile(
        profile: std::path::PathBuf,
        spec: SpawnSpec,
    ) -> io::Result<SandboxedChild> {
        let mut wrapped = spec.clone();
        let mut args = vec![
            std::ffi::OsString::from("-f"),
            profile.into_os_string(),
            wrapped.program.clone(),
        ];
        args.append(&mut wrapped.args);
        wrapped.program = std::ffi::OsString::from("/usr/bin/sandbox-exec");
        wrapped.args = args;
        let new_process_group = wrapped.new_process_group;
        let child = build_command(&wrapped).spawn()?;
        Ok(finish_spawn(child, new_process_group))
    }
}
