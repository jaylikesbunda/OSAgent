use serde::{Deserialize, Deserializer, Serialize};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::env;
use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{self, Child, Command};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

const PID_POLL_INTERVAL: Duration = Duration::from_millis(500);
const PID_TIMEOUT: Duration = Duration::from_secs(30);
const ACK_POLL_INTERVAL: Duration = Duration::from_millis(100);
const ACK_TIMEOUT: Duration = Duration::from_secs(60);
#[cfg(unix)]
const MAX_COPY_ATTEMPTS: u32 = 10;
#[cfg(unix)]
const COPY_BACKOFF_BASE_MS: u64 = 1000;
#[cfg(unix)]
const COPY_BACKOFF_CAP_MS: u64 = 30_000;
const TRANSACTION_ENV: &str = "OSAGENT_UPDATE_TRANSACTION";

static UNIQUE_PATH_SEQUENCE: AtomicU64 = AtomicU64::new(0);

#[cfg(windows)]
mod platform {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::os::windows::process::CommandExt;
    use std::path::Path;

    pub const CREATE_NO_WINDOW: u32 = 0x08000000;

    fn wide(path: &Path) -> Vec<u16> {
        path.as_os_str()
            .encode_wide()
            .chain(std::iter::once(0))
            .collect()
    }

    pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
        const MOVEFILE_REPLACE_EXISTING: u32 = 0x00000001;
        const MOVEFILE_WRITE_THROUGH: u32 = 0x00000008;

        unsafe {
            extern "system" {
                fn MoveFileExW(
                    existing_file_name: *const u16,
                    new_file_name: *const u16,
                    flags: u32,
                ) -> i32;
            }

            let source = wide(source);
            let destination = wide(destination);
            let result = MoveFileExW(
                source.as_ptr(),
                destination.as_ptr(),
                MOVEFILE_REPLACE_EXISTING | MOVEFILE_WRITE_THROUGH,
            );
            if result == 0 {
                return Err(std::io::Error::last_os_error());
            }
        }

        Ok(())
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
        let _ = std::process::Command::new("taskkill")
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
    use std::fs;
    use std::path::Path;
    use std::time::Duration;

    pub fn replace_file(source: &Path, destination: &Path) -> std::io::Result<()> {
        fs::rename(source, destination)
    }

    pub fn is_pid_alive(pid: u32) -> bool {
        unsafe { libc::kill(pid as i32, 0) == 0 }
    }

    pub fn force_kill(pid: u32) {
        unsafe {
            libc::kill(pid as i32, libc::SIGKILL);
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum UpdatePhase {
    #[default]
    Pending,
    Applying,
    Committed,
    Failed,
}

impl UpdatePhase {
    fn parse(value: &str) -> Option<Self> {
        match value {
            // The backend calls its armed intent "installing"; older markers
            // may call the same durable state "pending".
            "pending" | "installing" => Some(Self::Pending),
            "applying" => Some(Self::Applying),
            "committed" => Some(Self::Committed),
            "failed" => Some(Self::Failed),
            _ => None,
        }
    }
}

// The producer may omit phase or serialize its Option as null.
fn deserialize_update_phase<'de, D>(deserializer: D) -> Result<UpdatePhase, D::Error>
where
    D: Deserializer<'de>,
{
    let value = Option::<String>::deserialize(deserializer)?;
    value
        .map(|phase| {
            UpdatePhase::parse(&phase)
                .ok_or_else(|| serde::de::Error::custom(format!("unknown update phase: {phase}")))
        })
        .transpose()
        .map(|phase| phase.unwrap_or_default())
}

#[derive(Debug, Clone, Copy, Default, Deserialize, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
enum UpdateKind {
    #[default]
    BinarySwap,
    Installer,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
struct UpdateMarker {
    tag: String,
    #[serde(alias = "launcher_path")]
    staged_path: PathBuf,
    #[serde(default)]
    kind: UpdateKind,
    #[serde(default = "default_marker_armed")]
    armed: bool,
    #[serde(default)]
    created_at: Option<Value>,
    #[serde(default)]
    transaction_id: Option<String>,
    #[serde(default, deserialize_with = "deserialize_update_phase")]
    phase: UpdatePhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    error: Option<Value>,
    #[serde(flatten)]
    extra: BTreeMap<String, Value>,
}

fn default_marker_armed() -> bool {
    true
}

#[derive(Debug)]
enum UpdateMode {
    BinarySwap {
        old_path: PathBuf,
        new_path: PathBuf,
    },
    Installer {
        installer_path: PathBuf,
    },
}

#[derive(Debug)]
struct Cli {
    pid: u32,
    mode: UpdateMode,
    launch_path: PathBuf,
    cleanup: Option<PathBuf>,
    marker_path: PathBuf,
    transaction_id: String,
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
    let line = format!("[{timestamp}] {msg}\n");
    let _ = fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .and_then(|mut file| file.write_all(line.as_bytes()));
}

fn write_failure(error: &str, old: &Path, new: &Path) {
    let path = dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("update-failed.json");
    if let Some(parent) = path.parent() {
        let _ = fs::create_dir_all(parent);
    }
    let payload = json!({
        "error": error,
        "old_path": old,
        "new_path": new,
        "timestamp": chrono::Utc::now().to_rfc3339(),
    });
    let _ = fs::write(
        path,
        serde_json::to_vec_pretty(&payload).unwrap_or_else(|_| b"{}".to_vec()),
    );
}

fn print_usage() {
    eprintln!(
        "Usage: osagent-updater --pid <PID> --marker <pending_update.json> \
         --transaction <id> (--old <old_exe> --new <new_exe> | --installer <installer_exe>) \
         --launch <exe_to_launch> [--cleanup <dir>]"
    );
    eprintln!();
    eprintln!(
        "Runs a transactional binary swap or NSIS install and waits for launcher acknowledgement."
    );
}

fn parse_args(args: &[String]) -> Result<Cli, String> {
    let mut pid = None;
    let mut old_path = None;
    let mut new_path = None;
    let mut installer_path = None;
    let mut launch_path = None;
    let mut cleanup = None;
    let mut marker_path = None;
    let mut transaction_id = None;

    let mut index = 1;
    while index < args.len() {
        let value = |index: &mut usize, name: &str| -> Result<String, String> {
            *index += 1;
            args.get(*index)
                .cloned()
                .ok_or_else(|| format!("{name} requires a value"))
        };

        match args[index].as_str() {
            "--pid" => {
                let value = value(&mut index, "--pid")?;
                pid = Some(
                    value
                        .parse::<u32>()
                        .map_err(|_| format!("invalid --pid value: {value}"))?,
                );
            }
            "--old" => old_path = Some(PathBuf::from(value(&mut index, "--old")?)),
            "--new" => new_path = Some(PathBuf::from(value(&mut index, "--new")?)),
            "--installer" => {
                installer_path = Some(PathBuf::from(value(&mut index, "--installer")?))
            }
            "--launch" => launch_path = Some(PathBuf::from(value(&mut index, "--launch")?)),
            "--cleanup" => cleanup = Some(PathBuf::from(value(&mut index, "--cleanup")?)),
            "--marker" => marker_path = Some(PathBuf::from(value(&mut index, "--marker")?)),
            "--transaction" => {
                transaction_id = Some(value(&mut index, "--transaction")?);
            }
            _ => return Err(format!("unknown argument: {}", args[index])),
        }
        index += 1;
    }

    let mode =
        match (old_path, new_path, installer_path) {
            (Some(old_path), Some(new_path), None) => UpdateMode::BinarySwap { old_path, new_path },
            (None, None, Some(installer_path)) => UpdateMode::Installer { installer_path },
            (None, None, None) => {
                return Err("either --old/--new or --installer is required".to_string())
            }
            _ => return Err(
                "--installer cannot be combined with --old/--new, and --old/--new must be paired"
                    .to_string(),
            ),
        };

    Ok(Cli {
        pid: pid.ok_or("--pid is required")?,
        mode,
        launch_path: launch_path.ok_or("--launch is required")?,
        cleanup,
        marker_path: marker_path.ok_or("--marker is required")?,
        transaction_id: transaction_id.ok_or("--transaction is required")?,
    })
}

fn read_marker(path: &Path) -> Result<UpdateMarker, String> {
    let json = fs::read_to_string(path)
        .map_err(|error| format!("Failed to read update marker {}: {error}", path.display()))?;
    serde_json::from_str(&json)
        .map_err(|error| format!("Failed to parse update marker {}: {error}", path.display()))
}

fn write_marker(path: &Path, marker: &UpdateMarker) -> Result<(), String> {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent)
            .map_err(|error| format!("Failed to create update marker directory: {error}"))?;
    }

    let sequence = UNIQUE_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let temp_path = path.with_extension(format!("tmp.{}.{sequence}", process::id()));
    let bytes = serde_json::to_vec_pretty(marker)
        .map_err(|error| format!("Failed to serialize update marker: {error}"))?;

    let write_result = (|| -> io::Result<()> {
        let mut file = OpenOptions::new()
            .create_new(true)
            .write(true)
            .open(&temp_path)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        platform::replace_file(&temp_path, path)
    })();

    if let Err(error) = write_result {
        let _ = fs::remove_file(&temp_path);
        return Err(format!(
            "Failed to write update marker {}: {error}",
            path.display()
        ));
    }

    #[cfg(unix)]
    if let Some(parent) = path.parent() {
        if let Ok(directory) = File::open(parent) {
            let _ = directory.sync_all();
        }
    }

    Ok(())
}

fn mark_phase(
    marker_path: &Path,
    transaction_id: &str,
    phase: UpdatePhase,
    error: Option<String>,
) -> Result<(), String> {
    let mut marker = read_marker(marker_path)?;
    if marker
        .transaction_id
        .as_deref()
        .is_some_and(|existing| !existing.is_empty() && existing != transaction_id)
    {
        return Err(format!(
            "update marker belongs to transaction {}, not {}",
            marker.transaction_id.as_deref().unwrap_or_default(),
            transaction_id
        ));
    }

    marker.transaction_id = Some(transaction_id.to_string());
    marker.phase = phase;
    marker.error = error.map(Value::String);
    write_marker(marker_path, &marker)
}

fn transaction_path_token(transaction_id: &str) -> String {
    let token: String = transaction_id
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '-' | '_' | '.') {
                character
            } else {
                '_'
            }
        })
        .take(96)
        .collect();
    if token.is_empty() || token == "." || token == ".." {
        "transaction".to_string()
    } else {
        token
    }
}

fn transaction_ack_path(marker_path: &Path, transaction_id: &str) -> Option<PathBuf> {
    let parent = marker_path.parent()?;
    let marker_name = marker_path.file_name()?.to_string_lossy();
    Some(parent.join(format!(
        "{marker_name}.{}.ack",
        transaction_path_token(transaction_id)
    )))
}

fn acknowledgement_matches(path: &Path, transaction_id: &str) -> bool {
    fs::read_to_string(path)
        .map(|contents| contents.trim() == transaction_id)
        .unwrap_or(false)
}

fn remove_file_if_present(path: &Path) {
    match fs::remove_file(path) {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => log_msg(&format!("Failed to remove {}: {error}", path.display())),
    }
}

fn wait_for_pid(pid: u32) -> bool {
    log_msg(&format!("Waiting for PID {pid} to exit..."));
    let start = Instant::now();

    loop {
        if !platform::is_pid_alive(pid) {
            log_msg(&format!(
                "PID {pid} has exited (waited {:.1}s)",
                start.elapsed().as_secs_f64()
            ));
            std::thread::sleep(Duration::from_secs(1));
            return true;
        }

        if start.elapsed() > PID_TIMEOUT {
            log_msg(&format!(
                "PID {pid} still alive after {}s, force killing",
                PID_TIMEOUT.as_secs()
            ));
            platform::force_kill(pid);
            std::thread::sleep(Duration::from_secs(2));
            return !platform::is_pid_alive(pid);
        }

        std::thread::sleep(PID_POLL_INTERVAL);
    }
}

#[cfg(unix)]
fn copy_with_retries(source: &Path, destination: &Path) -> bool {
    log_msg(&format!(
        "Copying {} -> {}",
        source.display(),
        destination.display()
    ));

    for attempt in 1..=MAX_COPY_ATTEMPTS {
        match fs::copy(source, destination) {
            Ok(bytes) => {
                let source_size = match fs::metadata(source) {
                    Ok(metadata) => metadata.len(),
                    Err(_) => bytes,
                };

                if bytes != source_size {
                    log_msg(&format!(
                        "Copy size mismatch: expected {source_size} bytes, got {bytes} (attempt {attempt}/{MAX_COPY_ATTEMPTS})"
                    ));
                } else {
                    match fs::metadata(destination) {
                        Ok(metadata) if metadata.len() == source_size => {
                            log_msg(&format!(
                                "Copy verified: {bytes} bytes (attempt {attempt}/{MAX_COPY_ATTEMPTS})"
                            ));
                            return true;
                        }
                        Ok(metadata) => log_msg(&format!(
                            "Verification failed: destination size {} != source size {source_size} (attempt {attempt}/{MAX_COPY_ATTEMPTS})",
                            metadata.len()
                        )),
                        Err(error) => log_msg(&format!(
                            "Verification stat failed: {error} (attempt {attempt}/{MAX_COPY_ATTEMPTS})"
                        )),
                    }
                }
            }
            Err(error) => log_msg(&format!(
                "Copy failed: {error} (attempt {attempt}/{MAX_COPY_ATTEMPTS})"
            )),
        }

        if attempt < MAX_COPY_ATTEMPTS {
            let backoff = std::cmp::min(
                COPY_BACKOFF_BASE_MS * 2u64.pow(attempt - 1),
                COPY_BACKOFF_CAP_MS,
            );
            log_msg(&format!("Retrying in {backoff}ms..."));
            std::thread::sleep(Duration::from_millis(backoff));
        }
    }

    false
}

#[derive(Debug, PartialEq, Eq)]
struct ReplacementPaths {
    staged_sibling: PathBuf,
    backup: PathBuf,
}

#[cfg_attr(windows, allow(dead_code))]
fn replacement_paths(old_path: &Path, transaction_id: &str) -> Result<ReplacementPaths, String> {
    let parent = old_path
        .parent()
        .ok_or_else(|| format!("old binary has no parent: {}", old_path.display()))?;
    let old_name = old_path
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| format!("old binary has no usable filename: {}", old_path.display()))?;
    let sequence = UNIQUE_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed);
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let suffix = format!(
        "{}.{}.{sequence}.{timestamp}",
        transaction_path_token(transaction_id),
        process::id()
    );

    Ok(ReplacementPaths {
        staged_sibling: parent.join(format!(".{old_name}.new.{suffix}")),
        backup: parent.join(format!(".{old_name}.backup.{suffix}")),
    })
}

fn file_digest(path: &Path) -> io::Result<[u8; 32]> {
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buffer = [0_u8; 64 * 1024];
    loop {
        let read = file.read(&mut buffer)?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
    }
    Ok(hasher.finalize().into())
}

#[cfg_attr(windows, allow(dead_code))]
fn replacement_file_healthy(source: &Path, destination: &Path) -> bool {
    let source_size = match fs::metadata(source) {
        Ok(metadata) => metadata.len(),
        Err(_) => return false,
    };
    let destination_size = match fs::metadata(destination) {
        Ok(metadata) => metadata.len(),
        Err(_) => return false,
    };
    if source_size == 0 || source_size != destination_size {
        return false;
    }
    match (file_digest(source), file_digest(destination)) {
        (Ok(source), Ok(destination)) if source == destination => {}
        _ => return false,
    }

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        match fs::metadata(destination) {
            Ok(metadata) if metadata.permissions().mode() & 0o111 != 0 => {}
            _ => return false,
        }
    }

    true
}

#[cfg(unix)]
fn prepare_unix_replacement(
    new_path: &Path,
    old_path: &Path,
    transaction_id: &str,
) -> Result<ReplacementPaths, String> {
    use std::os::unix::fs::PermissionsExt;

    if !new_path.is_file() {
        return Err(format!("new binary does not exist: {}", new_path.display()));
    }
    if !old_path.is_file() {
        return Err(format!("old binary does not exist: {}", old_path.display()));
    }

    let paths = replacement_paths(old_path, transaction_id)?;
    if paths.staged_sibling.exists() || paths.backup.exists() {
        return Err("unique replacement paths already exist".to_string());
    }

    if !copy_with_retries(new_path, &paths.staged_sibling) {
        return Err(format!(
            "failed to stage new binary at {}",
            paths.staged_sibling.display()
        ));
    }
    let mut permissions = fs::metadata(&paths.staged_sibling)
        .map_err(|error| format!("Failed to inspect staged sibling: {error}"))?
        .permissions();
    permissions.set_mode(permissions.mode() | 0o111);
    fs::set_permissions(&paths.staged_sibling, permissions)
        .map_err(|error| format!("Failed to make staged sibling executable: {error}"))?;

    if !copy_with_retries(old_path, &paths.backup) {
        return Err(format!(
            "failed to back up old binary at {}",
            paths.backup.display()
        ));
    }
    if !replacement_file_healthy(old_path, &paths.backup) {
        return Err("old binary backup failed verification".to_string());
    }

    fs::rename(&paths.staged_sibling, old_path).map_err(|error| {
        format!(
            "Failed to atomically replace {} with staged sibling: {error}",
            old_path.display()
        )
    })?;

    log_msg(&format!(
        "Atomically replaced {} (backup: {})",
        old_path.display(),
        paths.backup.display()
    ));
    Ok(paths)
}

#[cfg(unix)]
fn restore_unix_binary(replacement: &ReplacementPaths, old_path: &Path) -> Result<(), String> {
    fs::rename(&replacement.backup, old_path).map_err(|error| {
        format!(
            "Failed to restore old binary from {}: {error}",
            replacement.backup.display()
        )
    })?;
    log_msg(&format!("Restored old binary at {}", old_path.display()));
    Ok(())
}

#[cfg(unix)]
fn rollback_and_relaunch_old(
    cli: &Cli,
    replacement: &ReplacementPaths,
    old_path: &Path,
    reason: &str,
) -> Result<(), String> {
    restore_unix_binary(replacement, old_path)?;
    relaunch_old(&cli.launch_path).map_err(|error| format!("{reason}; {error}"))?;
    Err(reason.to_string())
}

#[cfg(target_os = "macos")]
fn enclosing_app_bundle(path: &Path) -> Option<PathBuf> {
    path.ancestors()
        .find(|ancestor| {
            ancestor
                .extension()
                .and_then(|extension| extension.to_str())
                .map(|extension| extension.eq_ignore_ascii_case("app"))
                .unwrap_or(false)
        })
        .map(Path::to_path_buf)
}

/// Overwriting the main executable of a signed .app invalidates its signature.
#[cfg(target_os = "macos")]
fn resign_app_bundle(binary_path: &Path) -> bool {
    let Some(bundle) = enclosing_app_bundle(binary_path) else {
        log_msg("Swapped binary is not inside an .app bundle; no re-signing needed");
        return true;
    };

    let bundle_string = bundle.to_string_lossy().to_string();
    let _ = Command::new("/usr/bin/xattr")
        .args(["-d", "-r", "com.apple.quarantine", &bundle_string])
        .output();
    match Command::new("/usr/bin/codesign")
        .args(["--force", "--deep", "--sign", "-", &bundle_string])
        .output()
    {
        Ok(output) if output.status.success() => {
            log_msg("App bundle re-signed successfully");
            true
        }
        Ok(output) => {
            log_msg(&format!(
                "codesign failed ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ));
            false
        }
        Err(error) => {
            log_msg(&format!("Failed to run codesign: {error}"));
            false
        }
    }
}

#[cfg(windows)]
fn run_installer_and_wait(installer_path: &Path) -> Result<(), String> {
    use std::os::windows::process::CommandExt;

    log_msg(&format!(
        "Running NSIS installer: {}",
        installer_path.display()
    ));
    let status = Command::new(installer_path)
        .arg("/S")
        .creation_flags(platform::CREATE_NO_WINDOW)
        .status()
        .map_err(|error| format!("Failed to run installer: {error}"))?;
    if !status.success() {
        return Err(format!("Installer failed with status: {status}"));
    }

    log_msg("NSIS installer completed successfully");
    Ok(())
}

fn launch_new(launch_path: &Path, transaction_id: &str) -> Result<Child, String> {
    log_msg(&format!("Launching new binary: {}", launch_path.display()));
    Command::new(launch_path)
        .env(TRANSACTION_ENV, transaction_id)
        .spawn()
        .map_err(|error| format!("Failed to launch new binary: {error}"))
}

#[cfg_attr(windows, allow(dead_code))]
fn relaunch_old(launch_path: &Path) -> Result<(), String> {
    log_msg(&format!(
        "Relaunching old binary: {}",
        launch_path.display()
    ));
    Command::new(launch_path)
        .env_remove(TRANSACTION_ENV)
        .spawn()
        .map(|_| ())
        .map_err(|error| format!("Old binary was restored but could not be relaunched: {error}"))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum LaunchHealth {
    Acknowledged,
    ChildExited,
    TimedOut,
}

fn classify_launch_health(acknowledged: bool, child_exited: bool, timed_out: bool) -> LaunchHealth {
    if acknowledged {
        LaunchHealth::Acknowledged
    } else if child_exited {
        LaunchHealth::ChildExited
    } else if timed_out {
        LaunchHealth::TimedOut
    } else {
        LaunchHealth::ChildExited
    }
}

fn wait_for_ack(marker_path: &Path, transaction_id: &str, child: &mut Child) -> LaunchHealth {
    let ack_path = transaction_ack_path(marker_path, transaction_id);
    let start = Instant::now();
    log_msg("Waiting for the new launcher to acknowledge setup");

    loop {
        let acknowledged = ack_path
            .as_deref()
            .is_some_and(|path| acknowledgement_matches(path, transaction_id));
        if acknowledged {
            log_msg("New launcher acknowledged setup");
            return classify_launch_health(true, false, false);
        }

        match child.try_wait() {
            Ok(Some(status)) => {
                log_msg(&format!(
                    "New launcher exited before acknowledgement: {status}"
                ));
                return classify_launch_health(false, true, false);
            }
            Ok(None) => {}
            Err(error) => {
                log_msg(&format!("Failed to inspect the new launcher: {error}"));
                return classify_launch_health(false, true, false);
            }
        }

        if start.elapsed() >= ACK_TIMEOUT {
            log_msg(&format!(
                "No launcher acknowledgement after {}s",
                ACK_TIMEOUT.as_secs()
            ));
            return classify_launch_health(false, false, true);
        }
        std::thread::sleep(ACK_POLL_INTERVAL);
    }
}

fn cleanup_dir(directory: &Path) {
    if !directory.exists() {
        return;
    }
    log_msg(&format!(
        "Cleaning up staged directory: {}",
        directory.display()
    ));
    match fs::remove_dir_all(directory) {
        Ok(()) => log_msg("Cleanup complete"),
        Err(error) => log_msg(&format!("Cleanup failed (non-fatal): {error}")),
    }
}

#[allow(clippy::needless_return)]
fn validate_mode(cli: &Cli) -> Result<(), String> {
    match &cli.mode {
        UpdateMode::BinarySwap { old_path, new_path } => {
            #[cfg(windows)]
            {
                let _ = (old_path, new_path);
                return Err(
                    "binary swaps are not transactional on Windows; stage an NSIS installer"
                        .to_string(),
                );
            }

            #[cfg(not(windows))]
            {
                if !old_path.is_file() {
                    return Err(format!("old binary does not exist: {}", old_path.display()));
                }
                if !new_path.is_file() {
                    return Err(format!("new binary does not exist: {}", new_path.display()));
                }
                Ok(())
            }
        }
        UpdateMode::Installer { installer_path } => {
            if !installer_path.is_file() {
                return Err(format!(
                    "installer does not exist: {}",
                    installer_path.display()
                ));
            }
            Ok(())
        }
    }
}

fn commit_success(cli: &Cli, unix_replacement: Option<&ReplacementPaths>) -> Result<(), String> {
    mark_phase(
        &cli.marker_path,
        &cli.transaction_id,
        UpdatePhase::Committed,
        None,
    )?;

    if let Some(replacement) = unix_replacement {
        remove_file_if_present(&replacement.backup);
    }
    if let Some(cleanup) = &cli.cleanup {
        cleanup_dir(cleanup);
    }
    if let Some(ack_path) = transaction_ack_path(&cli.marker_path, &cli.transaction_id) {
        remove_file_if_present(&ack_path);
    }
    remove_file_if_present(&cli.marker_path);
    Ok(())
}

#[allow(clippy::question_mark)]
fn run_transaction(cli: &Cli) -> Result<(), String> {
    if let Err(error) = validate_mode(cli) {
        #[cfg(unix)]
        if matches!(&cli.mode, UpdateMode::BinarySwap { .. }) {
            let _ = relaunch_old(&cli.launch_path);
        }
        return Err(error);
    }
    mark_phase(
        &cli.marker_path,
        &cli.transaction_id,
        UpdatePhase::Applying,
        None,
    )?;
    if let Some(ack_path) = transaction_ack_path(&cli.marker_path, &cli.transaction_id) {
        remove_file_if_present(&ack_path);
    }

    if !wait_for_pid(cli.pid) {
        #[cfg(unix)]
        if matches!(&cli.mode, UpdateMode::BinarySwap { .. }) {
            let _ = relaunch_old(&cli.launch_path);
        }
        return Err(format!("process {} did not exit within timeout", cli.pid));
    }

    #[cfg(unix)]
    #[allow(unused_assignments)]
    let mut unix_replacement = None;
    #[cfg(not(unix))]
    let unix_replacement = None;
    match &cli.mode {
        UpdateMode::BinarySwap { old_path, new_path } => {
            #[cfg(windows)]
            let _ = (old_path, new_path);
            #[cfg(unix)]
            {
                let replacement =
                    match prepare_unix_replacement(new_path, old_path, &cli.transaction_id) {
                        Ok(replacement) => replacement,
                        Err(error) => {
                            let _ = relaunch_old(&cli.launch_path);
                            return Err(error);
                        }
                    };
                if !replacement_file_healthy(new_path, old_path) {
                    return rollback_and_relaunch_old(
                        cli,
                        &replacement,
                        old_path,
                        "replacement binary failed verification",
                    );
                }

                #[cfg(target_os = "macos")]
                if !resign_app_bundle(old_path) {
                    return rollback_and_relaunch_old(
                        cli,
                        &replacement,
                        old_path,
                        "failed to re-sign replaced macOS app bundle",
                    );
                }
                unix_replacement = Some(replacement);
            }

            #[cfg(not(unix))]
            return Err("binary swap is unsupported on this platform".to_string());
        }
        UpdateMode::Installer { installer_path } => {
            #[cfg(windows)]
            run_installer_and_wait(installer_path)?;
            #[cfg(not(windows))]
            return Err(format!(
                "installer updates are only supported on Windows: {}",
                installer_path.display()
            ));
        }
    }

    let mut child = match launch_new(&cli.launch_path, &cli.transaction_id) {
        Ok(child) => child,
        Err(error) => {
            #[cfg(unix)]
            if let (Some(replacement), UpdateMode::BinarySwap { old_path, .. }) =
                (unix_replacement.as_ref(), &cli.mode)
            {
                return rollback_and_relaunch_old(
                    cli,
                    replacement,
                    old_path,
                    &format!("failed to launch replacement launcher: {error}"),
                );
            }
            return Err(error);
        }
    };
    let health = wait_for_ack(&cli.marker_path, &cli.transaction_id, &mut child);
    if health == LaunchHealth::Acknowledged {
        match commit_success(cli, unix_replacement.as_ref()) {
            Ok(()) => return Ok(()),
            Err(error) => {
                platform::force_kill(child.id());
                #[cfg(unix)]
                if let (Some(replacement), UpdateMode::BinarySwap { old_path, .. }) =
                    (unix_replacement.as_ref(), &cli.mode)
                {
                    return rollback_and_relaunch_old(
                        cli,
                        replacement,
                        old_path,
                        &format!("failed to commit acknowledged update: {error}"),
                    );
                }
                return Err(error);
            }
        }
    }

    if health == LaunchHealth::TimedOut {
        platform::force_kill(child.id());
    }

    let health_error = match health {
        LaunchHealth::ChildExited => {
            "new launcher exited before acknowledging update setup".to_string()
        }
        LaunchHealth::TimedOut => "new launcher did not acknowledge update setup".to_string(),
        LaunchHealth::Acknowledged => unreachable!(),
    };

    #[cfg(unix)]
    if let (Some(replacement), UpdateMode::BinarySwap { old_path, .. }) =
        (unix_replacement.as_ref(), &cli.mode)
    {
        return rollback_and_relaunch_old(cli, replacement, old_path, &health_error);
    }

    Err(health_error)
}

fn main() {
    let args: Vec<String> = env::args().collect();
    if args.iter().any(|arg| arg == "--help" || arg == "-h") {
        print_usage();
        return;
    }

    let cli = match parse_args(&args) {
        Ok(cli) => cli,
        Err(error) => {
            eprintln!("Error: {error}");
            print_usage();
            process::exit(1);
        }
    };

    log_msg("=== osagent-updater starting ===");
    log_msg(&format!("  PID to wait for: {}", cli.pid));
    log_msg(&format!("  Launch after:    {}", cli.launch_path.display()));
    log_msg(&format!("  Marker:          {}", cli.marker_path.display()));
    log_msg(&format!("  Transaction:     {}", cli.transaction_id));

    let (old_reference, new_reference) = match &cli.mode {
        UpdateMode::BinarySwap { old_path, new_path } => (old_path, new_path),
        UpdateMode::Installer { installer_path } => (installer_path, installer_path),
    };

    if let Err(error) = run_transaction(&cli) {
        if let Err(phase_error) = mark_phase(
            &cli.marker_path,
            &cli.transaction_id,
            UpdatePhase::Failed,
            Some(error.clone()),
        ) {
            log_msg(&format!("Failed to mark transaction failed: {phase_error}"));
        }
        log_msg(&format!("Update transaction failed: {error}"));
        write_failure(&error, old_reference, new_reference);
        eprintln!("Update transaction failed: {error}");
        process::exit(1);
    }

    log_msg("=== osagent-updater complete ===");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pending_marker_with_missing_or_null_phase() {
        for phase in ["", ",\"phase\":null"] {
            let json = format!(r#"{{"tag":"v1","staged_path":"/tmp/new","armed":true{phase}}}"#);
            let marker: UpdateMarker = serde_json::from_str(&json).unwrap();
            assert_eq!(marker.phase, UpdatePhase::Pending);
            assert_eq!(marker.kind, UpdateKind::BinarySwap);
        }
    }

    #[test]
    fn parses_all_known_phases_and_rejects_unknown() {
        for (value, expected) in [
            ("pending", UpdatePhase::Pending),
            ("installing", UpdatePhase::Pending),
            ("applying", UpdatePhase::Applying),
            ("committed", UpdatePhase::Committed),
            ("failed", UpdatePhase::Failed),
        ] {
            let json = format!(r#"{{"tag":"v1","staged_path":"/tmp/new","phase":"{value}"}}"#);
            let marker: UpdateMarker = serde_json::from_str(&json).unwrap();
            assert_eq!(marker.phase, expected);
        }

        let json = r#"{"tag":"v1","staged_path":"/tmp/new","phase":"mystery"}"#;
        assert!(serde_json::from_str::<UpdateMarker>(json).is_err());
    }

    #[test]
    fn replacement_paths_are_unique_siblings() {
        let old = if cfg!(windows) {
            PathBuf::from(r"C:\Program Files\OSAgent\osagent-launcher.exe")
        } else {
            PathBuf::from("/opt/osagent/osagent-launcher")
        };
        let first = replacement_paths(&old, "transaction/one").unwrap();
        let second = replacement_paths(&old, "transaction/one").unwrap();

        assert_ne!(first.staged_sibling, second.staged_sibling);
        assert_ne!(first.backup, second.backup);
        assert_eq!(first.staged_sibling.parent(), old.parent());
        assert_eq!(first.backup.parent(), old.parent());
        assert!(first
            .staged_sibling
            .file_name()
            .unwrap()
            .to_string_lossy()
            .contains("transaction_one"));
    }

    #[test]
    fn launch_health_prioritizes_acknowledgement() {
        assert_eq!(
            classify_launch_health(true, false, false),
            LaunchHealth::Acknowledged
        );
        assert_eq!(
            classify_launch_health(false, true, false),
            LaunchHealth::ChildExited
        );
        assert_eq!(
            classify_launch_health(false, false, true),
            LaunchHealth::TimedOut
        );
    }

    #[test]
    fn replacement_health_rejects_same_size_corruption() {
        let root = env::temp_dir().join(format!(
            "osagent-updater-hash-{}-{}",
            process::id(),
            UNIQUE_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new binary").unwrap();
        fs::write(&destination, b"new binary").unwrap();
        assert!(replacement_file_healthy(&source, &destination));
        fs::write(&destination, b"old binary").unwrap();
        assert!(!replacement_file_healthy(&source, &destination));
        let _ = fs::remove_dir_all(root);
    }

    #[cfg(unix)]
    #[test]
    fn replacement_health_requires_matching_content_and_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let root = env::temp_dir().join(format!(
            "osagent-updater-health-{}-{}",
            process::id(),
            UNIQUE_PATH_SEQUENCE.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&root).unwrap();
        let source = root.join("source");
        let destination = root.join("destination");
        fs::write(&source, b"new binary").unwrap();
        fs::write(&destination, b"new binary").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(replacement_file_healthy(&source, &destination));

        fs::write(&destination, b"old binary").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o755)).unwrap();
        assert!(!replacement_file_healthy(&source, &destination));

        fs::write(&destination, b"new binary").unwrap();
        fs::set_permissions(&destination, fs::Permissions::from_mode(0o644)).unwrap();
        assert!(!replacement_file_healthy(&source, &destination));
        let _ = fs::remove_dir_all(root);
    }
}
