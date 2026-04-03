use std::env;
use std::fs;
use std::path::PathBuf;
use std::process::{self, Command};
use std::time::{Duration, Instant};

const PID_POLL_INTERVAL: Duration = Duration::from_millis(500);
const PID_TIMEOUT: Duration = Duration::from_secs(30);
const MAX_COPY_ATTEMPTS: u32 = 10;
const COPY_BACKOFF_BASE_MS: u64 = 1000;
const COPY_BACKOFF_CAP_MS: u64 = 30_000;

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::os::windows::process::CommandExt;
    use std::process::{self, Command};
    pub const CREATE_NO_WINDOW: u32 = 0x08000000;
    pub const DETACHED_PROCESS: u32 = 0x00000008;

    #[allow(dead_code)]
    pub fn spawn_detached(cmd: &mut process::Command) -> std::io::Result<process::Child> {
        cmd.creation_flags(CREATE_NO_WINDOW | DETACHED_PROCESS)
            .spawn()
    }

    pub fn is_pid_alive(pid: u32) -> bool {
        #[repr(C)]
        struct ProcessEntry32 {
            dw_size: u32,
            cnt_usage: u32,
            th32_process_id: u32,
            th32_default_heap_id: usize,
            th32_module_id: u32,
            cnt_threads: u32,
            th32_parent_process_id: u32,
            pc_pri_class_base: i32,
            dw_flags: u32,
            sz_exe_file: [u8; 260],
        }

        type CreateToolhelp32Snapshot = unsafe extern "system" fn(u32, u32) -> isize;
        type Process32First = unsafe extern "system" fn(isize, *mut ProcessEntry32) -> i32;
        type Process32Next = unsafe extern "system" fn(isize, *mut ProcessEntry32) -> i32;
        type CloseHandle = unsafe extern "system" fn(isize) -> i32;

        const TH32CS_SNAPPROCESS: u32 = 0x00000002;
        const INVALID_HANDLE_VALUE: isize = -1;

        unsafe {
            let kernel32 = Library::new("kernel32.dll");
            let create_snapshot: CreateToolhelp32Snapshot =
                kernel32.get("CreateToolhelp32Snapshot");
            let process_first: Process32First = kernel32.get("Process32First");
            let process_next: Process32Next = kernel32.get("Process32Next");
            let close_handle: CloseHandle = kernel32.get("CloseHandle");

            let snapshot = create_snapshot(TH32CS_SNAPPROCESS, 0);
            if snapshot == INVALID_HANDLE_VALUE {
                return true;
            }

            let mut entry = ProcessEntry32 {
                dw_size: std::mem::size_of::<ProcessEntry32>() as u32,
                ..std::mem::zeroed()
            };

            let mut found = false;
            if process_first(snapshot, &mut entry) == 1 {
                loop {
                    if entry.th32_process_id == pid {
                        found = true;
                        break;
                    }
                    if process_next(snapshot, &mut entry) != 1 {
                        break;
                    }
                }
            }

            close_handle(snapshot);
            found
        }
    }

    pub fn force_kill(pid: u32) {
        let _ = Command::new("taskkill")
            .args(["/F", "/PID", &pid.to_string()])
            .creation_flags(CREATE_NO_WINDOW)
            .output();
    }

    struct Library(*mut c_void);

    impl Library {
        fn new(name: &str) -> Self {
            let wide: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
            let handle = unsafe {
                extern "system" {
                    fn LoadLibraryW(name: *const u16) -> *mut c_void;
                }
                LoadLibraryW(wide.as_ptr())
            };
            Self(handle)
        }

        fn get<T>(&self, name: &str) -> T {
            let name_cstr = std::ffi::CString::new(name).unwrap();
            let ptr = unsafe {
                extern "system" {
                    fn GetProcAddress(module: *mut c_void, name: *const i8) -> *const u8;
                }
                GetProcAddress(self.0, name_cstr.as_ptr())
            };
            assert!(!ptr.is_null(), "Failed to load {}", name);
            unsafe { std::mem::transmute_copy(&ptr) }
        }
    }
}

#[cfg(unix)]
mod platform {
    use std::process;

    pub fn spawn_detached(cmd: &mut process::Command) -> std::io::Result<process::Child> {
        cmd.spawn()
    }

    pub fn is_pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    pub fn force_kill(pid: u32) {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        std::thread::sleep(std::time::Duration::from_millis(500));
    }
}

fn log_file_path() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("updater.log")
}

fn log_msg(msg: &str) {
    let path = log_file_path();
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let timestamp = chrono::Utc::now().format("%Y-%m-%d %H:%M:%S UTC");
    let line = format!("[{}] {}\n", timestamp, msg);
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut f| std::io::Write::write_all(&mut f, line.as_bytes()));
}

fn write_failure(error: &str, old: &str, new: &str) {
    let path = dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("update-failed.json");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let timestamp = chrono::Utc::now().to_rfc3339();
    let json = format!(
        r#"{{
  "error": "{}",
  "old_path": "{}",
  "new_path": "{}",
  "timestamp": "{}"
}}"#,
        error.replace('"', "\\\""),
        old.replace('\\', "\\\\"),
        new.replace('\\', "\\\\"),
        timestamp
    );
    let _ = fs::write(&path, json);
}

fn wait_for_pid(pid: u32) -> bool {
    log_msg(&format!("Waiting for PID {} to exit...", pid));
    let start = Instant::now();

    loop {
        if !platform::is_pid_alive(pid) {
            log_msg(&format!(
                "PID {} has exited (waited {:.1}s)",
                pid,
                start.elapsed().as_secs_f64()
            ));
            std::thread::sleep(Duration::from_secs(1));
            return true;
        }

        if start.elapsed() > PID_TIMEOUT {
            log_msg(&format!(
                "PID {} still alive after {}s, force killing",
                pid,
                PID_TIMEOUT.as_secs()
            ));
            platform::force_kill(pid);
            std::thread::sleep(Duration::from_secs(2));
            return !platform::is_pid_alive(pid);
        }

        std::thread::sleep(PID_POLL_INTERVAL);
    }
}

fn copy_with_retries(src: &str, dst: &str) -> bool {
    log_msg(&format!("Copying {} -> {}", src, dst));

    for attempt in 1..=MAX_COPY_ATTEMPTS {
        match fs::copy(src, dst) {
            Ok(bytes) => {
                let src_size = match fs::metadata(src) {
                    Ok(m) => m.len(),
                    Err(_) => bytes,
                };

                if bytes != src_size {
                    log_msg(&format!(
                        "Copy size mismatch: expected {} bytes, got {} (attempt {}/{})",
                        src_size, bytes, attempt, MAX_COPY_ATTEMPTS
                    ));
                } else {
                    match fs::metadata(dst) {
                        Ok(meta) if meta.len() == src_size => {
                            log_msg(&format!(
                                "Copy verified: {} bytes (attempt {}/{})",
                                bytes, attempt, MAX_COPY_ATTEMPTS
                            ));
                            return true;
                        }
                        Ok(meta) => {
                            log_msg(&format!(
                                "Verification failed: dst size {} != src size {} (attempt {}/{})",
                                meta.len(),
                                src_size,
                                attempt,
                                MAX_COPY_ATTEMPTS
                            ));
                        }
                        Err(e) => {
                            log_msg(&format!(
                                "Verification stat failed: {} (attempt {}/{})",
                                e, attempt, MAX_COPY_ATTEMPTS
                            ));
                        }
                    }
                }
            }
            Err(e) => {
                log_msg(&format!(
                    "Copy failed: {} (attempt {}/{})",
                    e, attempt, MAX_COPY_ATTEMPTS
                ));
            }
        }

        if attempt < MAX_COPY_ATTEMPTS {
            let backoff = std::cmp::min(
                COPY_BACKOFF_BASE_MS * 2u64.pow(attempt - 1),
                COPY_BACKOFF_CAP_MS,
            );
            log_msg(&format!("Retrying in {}ms...", backoff));
            std::thread::sleep(Duration::from_millis(backoff));
        }
    }

    false
}

#[cfg(windows)]
fn copy_windows_runtime_files(new_path: &str, old_path: &str) -> bool {
    let Some(new_dir) = PathBuf::from(new_path).parent().map(|p| p.to_path_buf()) else {
        return true;
    };
    let Some(old_dir) = PathBuf::from(old_path).parent().map(|p| p.to_path_buf()) else {
        return true;
    };

    let entries = match fs::read_dir(&new_dir) {
        Ok(entries) => entries,
        Err(e) => {
            log_msg(&format!("Failed to read staged runtime directory: {}", e));
            return false;
        }
    };

    for entry in entries {
        let entry = match entry {
            Ok(entry) => entry,
            Err(e) => {
                log_msg(&format!("Failed to read staged runtime entry: {}", e));
                return false;
            }
        };
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let is_runtime_dll = path
            .extension()
            .and_then(|ext| ext.to_str())
            .map(|ext| ext.eq_ignore_ascii_case("dll"))
            .unwrap_or(false);
        if !is_runtime_dll {
            continue;
        }

        let Some(name) = path.file_name() else {
            continue;
        };
        let dest = old_dir.join(name);
        if !copy_with_retries(&path.to_string_lossy(), &dest.to_string_lossy()) {
            return false;
        }
    }

    true
}

fn launch_new(launch_path: &str) -> bool {
    log_msg(&format!("Launching new binary: {}", launch_path));
    let result = Command::new(launch_path).spawn();
    match result {
        Ok(_) => {
            log_msg("New binary launched successfully");
            true
        }
        Err(e) => {
            log_msg(&format!("Failed to launch new binary: {}", e));
            false
        }
    }
}

#[cfg(windows)]
fn run_installer_and_wait(installer_path: &str) -> bool {
    use std::os::windows::process::CommandExt;

    log_msg(&format!("Running installer: {}", installer_path));
    let mut cmd = Command::new(installer_path);
    cmd.arg("/S");
    cmd.creation_flags(platform::CREATE_NO_WINDOW);

    match cmd.status() {
        Ok(status) if status.success() => {
            log_msg("Installer completed successfully");
            true
        }
        Ok(status) => {
            log_msg(&format!("Installer failed with status: {}", status));
            false
        }
        Err(e) => {
            log_msg(&format!("Failed to run installer: {}", e));
            false
        }
    }
}

fn cleanup_dir(dir: &str) {
    let path = PathBuf::from(dir);
    if !path.exists() {
        return;
    }
    log_msg(&format!("Cleaning up staged directory: {}", dir));
    match fs::remove_dir_all(&path) {
        Ok(()) => log_msg("Cleanup complete"),
        Err(e) => log_msg(&format!("Cleanup failed (non-fatal): {}", e)),
    }
}

fn print_usage() {
    eprintln!("Usage: osagent-updater --pid <PID> (--old <old_exe> --new <new_exe> | --installer <installer_exe>) --launch <exe_to_launch> [--cleanup <dir>]");
    eprintln!();
    eprintln!("Runs a binary swap or installer after PID exits, then launches exe_to_launch.");
}

fn main() {
    let args: Vec<String> = env::args().collect();

    if args.len() < 7 || args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
        print_usage();
        process::exit(
            if args.contains(&"--help".to_string()) || args.contains(&"-h".to_string()) {
                0
            } else {
                1
            },
        );
    }

    let mut pid = None;
    let mut old_path = None;
    let mut new_path = None;
    let mut installer_path = None;
    let mut launch_path = None;
    let mut cleanup = None;

    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--pid" if i + 1 < args.len() => {
                pid = args[i + 1].parse::<u32>().ok();
                i += 2;
            }
            "--old" if i + 1 < args.len() => {
                old_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--new" if i + 1 < args.len() => {
                new_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--launch" if i + 1 < args.len() => {
                launch_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--installer" if i + 1 < args.len() => {
                installer_path = Some(args[i + 1].clone());
                i += 2;
            }
            "--cleanup" if i + 1 < args.len() => {
                cleanup = Some(args[i + 1].clone());
                i += 2;
            }
            _ => {
                i += 1;
            }
        }
    }

    let pid = match pid {
        Some(p) => p,
        None => {
            eprintln!("Error: --pid is required");
            process::exit(1);
        }
    };
    let launch_path = match launch_path {
        Some(p) => p,
        None => {
            eprintln!("Error: --launch is required");
            process::exit(1);
        }
    };

    log_msg(&format!("=== osagent-updater starting ==="));
    log_msg(&format!("  PID to wait for: {}", pid));
    if let Some(ref old) = old_path {
        log_msg(&format!("  Old binary:      {}", old));
    }
    if let Some(ref new) = new_path {
        log_msg(&format!("  New binary:      {}", new));
    }
    if let Some(ref installer) = installer_path {
        log_msg(&format!("  Installer:       {}", installer));
    }
    log_msg(&format!("  Launch after:    {}", launch_path));
    if let Some(ref dir) = cleanup {
        log_msg(&format!("  Cleanup dir:     {}", dir));
    }

    let swap_mode = old_path.is_some() && new_path.is_some();
    let installer_mode = installer_path.is_some();
    if !swap_mode && !installer_mode {
        eprintln!("Error: either --old/--new or --installer is required");
        process::exit(1);
    }
    if swap_mode && installer_mode {
        eprintln!("Error: --installer cannot be combined with --old/--new");
        process::exit(1);
    }

    if swap_mode {
        let old = old_path.as_ref().unwrap();
        let new = new_path.as_ref().unwrap();
        if !PathBuf::from(new).exists() {
            let err = format!("New binary does not exist: {}", new);
            log_msg(&err);
            write_failure(&err, old, new);
            process::exit(1);
        }
    }

    if let Some(installer) = installer_path.as_ref() {
        if !PathBuf::from(installer).exists() {
            let err = format!("Installer does not exist: {}", installer);
            log_msg(&err);
            write_failure(&err, installer, installer);
            process::exit(1);
        }
    }

    if !wait_for_pid(pid) {
        let err = format!("Process {} did not exit within timeout", pid);
        log_msg(&err);
        let failure_ref = installer_path.as_ref().or(new_path.as_ref()).unwrap();
        let old_ref = old_path.as_ref().unwrap_or(failure_ref);
        write_failure(&err, old_ref, failure_ref);
        process::exit(1);
    }

    if let Some(installer) = installer_path.as_ref() {
        #[cfg(windows)]
        if !run_installer_and_wait(installer) {
            let err = format!("Failed to run installer {}", installer);
            log_msg(&err);
            write_failure(&err, installer, installer);
            process::exit(1);
        }
        #[cfg(not(windows))]
        {
            let err = "Installer mode is only supported on Windows".to_string();
            log_msg(&err);
            write_failure(&err, installer, installer);
            process::exit(1);
        }
    } else {
        let old_path = old_path.as_ref().unwrap();
        let new_path = new_path.as_ref().unwrap();
        if !copy_with_retries(new_path, old_path) {
            let err = format!(
                "Failed to copy {} -> {} after {} attempts",
                new_path, old_path, MAX_COPY_ATTEMPTS
            );
            log_msg(&err);
            write_failure(&err, old_path, new_path);
            process::exit(1);
        }

        #[cfg(windows)]
        if !copy_windows_runtime_files(new_path, old_path) {
            let err = format!("Failed to copy Windows runtime files from {}", new_path);
            log_msg(&err);
            write_failure(&err, old_path, new_path);
            process::exit(1);
        }
    }

    if let Some(ref dir) = cleanup {
        cleanup_dir(dir);
    }

    if !launch_new(&launch_path) {
        let err = format!("Failed to launch new binary: {}", launch_path);
        log_msg(&err);
        let failure_ref = installer_path.as_ref().or(new_path.as_ref()).unwrap();
        let old_ref = old_path.as_ref().unwrap_or(failure_ref);
        write_failure(&err, old_ref, failure_ref);
        process::exit(1);
    }

    log_msg("=== osagent-updater complete ===");
}
