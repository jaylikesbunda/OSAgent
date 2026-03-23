#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader};
use std::path::PathBuf;
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use tauri::{
    CustomMenuItem, Manager, State, SystemTray, SystemTrayEvent, SystemTrayMenu,
    SystemTrayMenuItem, Window, WindowEvent,
};
use tracing::{info, Level};
use tracing_subscriber::EnvFilter;

// --- Helpers ---

/// Strip ANSI/VT escape sequences (CSI, OSC, etc.) from a string.
fn strip_ansi_codes(s: &str) -> String {
    let mut result = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\x1b' {
            match chars.peek() {
                Some(&'[') => {
                    chars.next();
                    // CSI: read until the final byte (an ASCII letter)
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc.is_ascii_alphabetic() {
                            break;
                        }
                    }
                }
                Some(&']') => {
                    chars.next();
                    // OSC: read until BEL or ESC backslash
                    while let Some(&nc) = chars.peek() {
                        chars.next();
                        if nc == '\x07' {
                            break;
                        }
                        if nc == '\x1b' {
                            if chars.peek() == Some(&'\\') {
                                chars.next();
                            }
                            break;
                        }
                    }
                }
                _ => {
                    chars.next(); // skip 2-char ESC sequences
                }
            }
        } else {
            result.push(c);
        }
    }
    result
}

// --- State ---

pub struct AppState {
    osagent_process: Mutex<Option<Child>>,
    osagent_pid: Mutex<Option<u32>>,
    osagent_running: Mutex<bool>,
    build_running: Mutex<bool>,
    osagent_path: PathBuf,
    config_path: PathBuf,
    logs: Mutex<Vec<LogEntry>>,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct LogEntry {
    pub timestamp: String,
    pub level: String,
    pub message: String,
}

#[derive(Clone, Serialize, Deserialize)]
pub struct AgentStatus {
    pub running: bool,
    pub pid: Option<u32>,
    pub osagent_path: String,
    pub config_path: String,
}

// --- Helper Functions ---

fn get_osagent_path() -> PathBuf {
    let exe_path = std::env::current_exe()
        .ok()
        .unwrap_or_else(|| PathBuf::from("."));

    // exe is at: .../osagent/launcher/target/release/osagent-launcher.exe
    // We need:   .../osagent/target/release/osagent.exe
    let candidates: Vec<PathBuf> = vec![
        exe_path
            .parent()
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .and_then(|p| p.parent())
            .map(|p| p.join("target").join("release").join("osagent.exe"))
            .unwrap_or_default(),
        exe_path
            .parent()
            .map(|p| p.join("osagent.exe"))
            .unwrap_or_default(),
        PathBuf::from("osagent/target/release/osagent.exe"),
    ];

    for candidate in &candidates {
        if candidate.exists() {
            return candidate.clone();
        }
    }

    candidates[0].clone()
}

fn get_config_path() -> PathBuf {
    dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("config.toml")
}

fn add_log_to_state(logs: &Mutex<Vec<LogEntry>>, level: &str, message: String) {
    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
    let entry = LogEntry {
        timestamp: timestamp.clone(),
        level: level.to_string(),
        message: message.clone(),
    };

    if let Ok(mut logs) = logs.lock() {
        logs.push(entry);
        if logs.len() > 500 {
            let drain = logs.len() - 500;
            logs.drain(0..drain);
        }
    }
}

fn add_log(state: &AppState, level: &str, message: String) {
    add_log_to_state(&state.logs, level, message);
}

// --- Tauri Commands ---

#[tauri::command]
fn get_status(state: State<AppState>) -> AgentStatus {
    let running = *state.osagent_running.lock().unwrap();
    let pid = *state.osagent_pid.lock().unwrap();
    AgentStatus {
        running,
        pid,
        osagent_path: state.osagent_path.to_string_lossy().to_string(),
        config_path: state.config_path.to_string_lossy().to_string(),
    }
}

#[tauri::command]
fn get_logs(state: State<AppState>) -> Vec<LogEntry> {
    state.logs.lock().unwrap().clone()
}

#[tauri::command]
fn get_build_running(state: State<AppState>) -> bool {
    *state.build_running.lock().unwrap()
}

#[tauri::command]
fn start_osagent(window: Window, state: State<AppState>) -> Result<AgentStatus, String> {
    let running = *state.osagent_running.lock().unwrap();
    if running {
        return Err("OSAgent is already running".into());
    }

    if !state.osagent_path.exists() {
        let msg = format!("osagent.exe not found at {}", state.osagent_path.display());
        add_log(&state, "error", msg.clone());
        return Err(msg);
    }

    if let Some(parent) = state.config_path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }

    add_log(
        &state,
        "info",
        format!("Starting osagent from {}", state.osagent_path.display()),
    );

    let log_dir = dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent");
    let _ = std::fs::create_dir_all(&log_dir);
    let log_file_path = log_dir.join("launcher_output.log");

    let mut cmd = Command::new(&state.osagent_path);
    cmd.arg("start")
        .arg("--config")
        .arg(&state.config_path)
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());

    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
    }

    match cmd.spawn() {
        Ok(mut child) => {
            let pid = child.id();

            let stdout = child.stdout.take();
            let stderr = child.stderr.take();

            *state.osagent_pid.lock().unwrap() = Some(pid);
            *state.osagent_running.lock().unwrap() = true;
            *state.osagent_process.lock().unwrap() = Some(child);

            add_log(&state, "info", format!("OSAgent started with PID: {}", pid));
            add_log(
                &state,
                "info",
                format!("Output log: {}", log_file_path.display()),
            );

            let handle_out = window.app_handle();
            let log_file_clone = log_file_path.clone();
            if let Some(out) = stdout {
                std::thread::spawn(move || {
                    read_output_to_file(out, handle_out, log_file_clone);
                });
            }

            let handle_err = window.app_handle();
            let log_file_clone = log_file_path.clone();
            if let Some(err) = stderr {
                std::thread::spawn(move || {
                    read_output_to_file(err, handle_err, log_file_clone);
                });
            }

            let _ = window.emit(
                "osagent-status-changed",
                AgentStatus {
                    running: true,
                    pid: Some(pid),
                    osagent_path: state.osagent_path.to_string_lossy().to_string(),
                    config_path: state.config_path.to_string_lossy().to_string(),
                },
            );

            update_tray_menu(&window, true);

            Ok(get_status(state))
        }
        Err(e) => {
            let msg = format!("Failed to start OSAgent: {}", e);
            add_log(&state, "error", msg.clone());
            Err(msg)
        }
    }
}

#[tauri::command]
fn stop_osagent(window: Window, state: State<AppState>) -> Result<AgentStatus, String> {
    let running = *state.osagent_running.lock().unwrap();
    if !running {
        return Err("OSAgent is not running".into());
    }

    add_log(&state, "info", "Stopping OSAgent...".into());

    {
        let mut process = state.osagent_process.lock().unwrap();
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }

    *state.osagent_pid.lock().unwrap() = None;
    *state.osagent_running.lock().unwrap() = false;

    add_log(&state, "info", "OSAgent stopped".into());

    let osagent_path = state.osagent_path.to_string_lossy().to_string();
    let config_path = state.config_path.to_string_lossy().to_string();

    let _ = window.emit(
        "osagent-status-changed",
        AgentStatus {
            running: false,
            pid: None,
            osagent_path,
            config_path,
        },
    );

    update_tray_menu(&window, false);

    Ok(AgentStatus {
        running: false,
        pid: None,
        osagent_path: state.osagent_path.to_string_lossy().to_string(),
        config_path: state.config_path.to_string_lossy().to_string(),
    })
}

#[tauri::command]
fn restart_osagent(window: Window, app_handle: tauri::AppHandle) -> Result<AgentStatus, String> {
    let state = app_handle.state::<AppState>();
    let _ = stop_osagent(window.clone(), state);
    std::thread::sleep(std::time::Duration::from_millis(500));
    let state = app_handle.state::<AppState>();
    start_osagent(window, state)
}

#[tauri::command]
fn open_web_ui() {
    let _ = open::that("http://localhost:8765");
}

#[tauri::command]
fn hide_to_tray(window: Window) {
    window.hide().ok();
}

#[tauri::command]
fn build_osagent(window: Window, state: State<AppState>) -> Result<String, String> {
    let osagent_dir = state
        .osagent_path
        .parent()
        .and_then(|p| p.parent())
        .map(|p| p.to_path_buf())
        .ok_or_else(|| "Could not determine osagent directory".to_string())?;

    let log_file_path = dirs_next::home_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(".osagent")
        .join("launcher_output.log");

    let start_msg = format!("Building osagent in {}", osagent_dir.display());
    add_log(&state, "info", start_msg);
    *state.build_running.lock().unwrap() = true;

    let app_handle = window.app_handle();

    std::thread::spawn(move || {
        let mut cmd = std::process::Command::new("cargo");
        cmd.args(["build", "--release"]);
        cmd.current_dir(&osagent_dir);
        cmd.stdout(std::process::Stdio::piped());
        cmd.stderr(std::process::Stdio::piped());
        // Force the progress bar even though stderr is piped (not a TTY).
        // Disable color so we only get the raw progress text without color codes.
        cmd.env("CARGO_TERM_PROGRESS_WHEN", "always");
        cmd.env("CARGO_TERM_PROGRESS_WIDTH", "60");
        cmd.env("CARGO_TERM_COLOR", "never");

        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            cmd.creation_flags(0x08000000); // CREATE_NO_WINDOW
        }

        let mut child = match cmd.spawn() {
            Ok(c) => c,
            Err(e) => {
                let state = app_handle.state::<AppState>();
                add_log(&state, "error", format!("Failed to spawn cargo: {}", e));
                *state.build_running.lock().unwrap() = false;
                return;
            }
        };

        let stdout = child.stdout.take();
        let stderr = child.stderr.take();

        let app_handle_out = app_handle.clone();
        let log_file_out = log_file_path.clone();
        // Save JoinHandles so we can wait for readers to fully drain the pipe
        // before marking the build as done. child.wait() returning does NOT mean
        // the pipe buffers are empty — readers may still have data to read.
        let stdout_handle = if let Some(stdout) = stdout {
            Some(std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stdout);
                for line in reader.lines() {
                    if let Ok(l) = line {
                        let trimmed = l.trim();
                        if !trimmed.is_empty() {
                            let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                            let state = app_handle_out.state::<AppState>();
                            add_log_to_state(&state.logs, "info", trimmed.to_string());
                            if let Ok(mut file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&log_file_out)
                            {
                                use std::io::Write;
                                let _ = writeln!(file, "[{}] [INFO] {}", timestamp, trimmed);
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };

        let app_handle_err = app_handle.clone();
        let log_file_err = log_file_path.clone();
        let stderr_handle = if let Some(stderr) = stderr {
            Some(std::thread::spawn(move || {
                let reader = std::io::BufReader::new(stderr);
                for line in reader.lines() {
                    if let Ok(l) = line {
                        // Each \n-terminated line may contain multiple \r-separated
                        // cargo progress updates. Split and handle each segment.
                        for part in l.split('\r') {
                            let clean = strip_ansi_codes(part);
                            let trimmed = clean.trim();
                            if trimmed.is_empty() {
                                continue;
                            }
                            let level =
                                if trimmed.starts_with("error") || trimmed.contains("error[") {
                                    "error"
                                } else if trimmed.starts_with("warning") {
                                    "warn"
                                } else {
                                    "info"
                                };
                            let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                            let state = app_handle_err.state::<AppState>();
                            add_log_to_state(&state.logs, level, trimmed.to_string());
                            if let Ok(mut file) = std::fs::OpenOptions::new()
                                .create(true)
                                .append(true)
                                .open(&log_file_err)
                            {
                                use std::io::Write;
                                let _ = writeln!(
                                    file,
                                    "[{}] [{}] {}",
                                    timestamp,
                                    level.to_uppercase(),
                                    trimmed
                                );
                            }
                        }
                    }
                }
            }))
        } else {
            None
        };

        let exit_status = child.wait();

        // Wait for both readers to finish draining the pipe before we
        // write the completion log entry and clear build_running. This
        // guarantees the frontend's final poll sees all output.
        if let Some(h) = stdout_handle {
            let _ = h.join();
        }
        if let Some(h) = stderr_handle {
            let _ = h.join();
        }

        let state = app_handle.state::<AppState>();
        match exit_status {
            Ok(s) if s.success() => {
                add_log(&state, "info", "Build completed successfully".to_string());
            }
            Ok(s) => {
                add_log(
                    &state,
                    "error",
                    format!("Build failed with code: {:?}", s.code()),
                );
            }
            Err(e) => {
                add_log(&state, "error", format!("Build process error: {}", e));
            }
        }
        *state.build_running.lock().unwrap() = false;
    });

    Ok("Build started".to_string())
}

#[tauri::command]
fn show_window(window: Window) {
    window.show().ok();
    window.set_focus().ok();
    window.unminimize().ok();
}

#[tauri::command]
fn minimize_window(window: Window) {
    window.minimize().ok();
}

#[tauri::command]
fn exit_app(app: tauri::AppHandle, state: State<AppState>) {
    let running = *state.osagent_running.lock().unwrap();
    if running {
        let mut process = state.osagent_process.lock().unwrap();
        if let Some(mut child) = process.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
        *state.osagent_running.lock().unwrap() = false;
    }

    app.exit(0);
}

#[tauri::command]
fn check_osagent_path(state: State<AppState>) -> String {
    state.osagent_path.to_string_lossy().to_string()
}

// --- Tray Helpers ---

fn update_tray_menu(window: &Window, running: bool) {
    let app_handle = window.app_handle();

    let open_launcher = CustomMenuItem::new("open_launcher".to_string(), "Open Launcher");
    let open_ui = CustomMenuItem::new("open_ui".to_string(), "Open Web UI");
    let start = CustomMenuItem::new("start_osagent".to_string(), "Start OSAgent");
    let stop = CustomMenuItem::new("stop_osagent".to_string(), "Stop OSAgent");
    let exit = CustomMenuItem::new("exit".to_string(), "Exit");

    let tray_menu = SystemTrayMenu::new()
        .add_item(open_launcher)
        .add_item(open_ui)
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(if running { stop } else { start })
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(exit);

    let _ = app_handle.tray_handle().set_menu(tray_menu);
}

fn build_tray() -> SystemTray {
    let open_launcher = CustomMenuItem::new("open_launcher".to_string(), "Open Launcher");
    let open_ui = CustomMenuItem::new("open_ui".to_string(), "Open Web UI");
    let start = CustomMenuItem::new("start_osagent".to_string(), "Start OSAgent");
    let exit = CustomMenuItem::new("exit".to_string(), "Exit");

    let tray_menu = SystemTrayMenu::new()
        .add_item(open_launcher)
        .add_item(open_ui)
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(start)
        .add_native_item(SystemTrayMenuItem::Separator)
        .add_item(exit);

    SystemTray::new()
        .with_menu(tray_menu)
        .with_tooltip("OSAgent Launcher")
}

// --- Output Reader ---

fn read_output_to_file<R: std::io::Read>(
    reader: R,
    app_handle: tauri::AppHandle,
    log_path: PathBuf,
) {
    let buf = BufReader::new(reader);
    for line in buf.lines() {
        match line {
            Ok(l) => {
                let trimmed = l.trim();
                if !trimmed.is_empty() {
                    let level = if trimmed.contains("ERROR")
                        || trimmed.contains("error")
                        || trimmed.contains("Error")
                    {
                        "error"
                    } else if trimmed.contains("WARN")
                        || trimmed.contains("warn")
                        || trimmed.contains("Warning")
                    {
                        "warn"
                    } else {
                        "info"
                    };

                    let timestamp = chrono::Local::now().format("%H:%M:%S").to_string();
                    let entry = LogEntry {
                        timestamp: timestamp.clone(),
                        level: level.to_string(),
                        message: trimmed.to_string(),
                    };

                    if let Ok(mut file) = std::fs::OpenOptions::new()
                        .create(true)
                        .append(true)
                        .open(&log_path)
                    {
                        use std::io::Write;
                        let _ = writeln!(
                            file,
                            "[{}] [{}] {}",
                            timestamp,
                            level.to_uppercase(),
                            trimmed
                        );
                    }

                    let state = app_handle.state::<AppState>();
                    add_log_to_state(&state.logs, level, trimmed.to_string());

                    let _ = app_handle.emit_all("log-line", entry);
                }
            }
            Err(_) => break,
        }
    }
}

// --- Process Monitor ---

fn start_process_monitor(app_handle: tauri::AppHandle) {
    std::thread::spawn(move || loop {
        std::thread::sleep(std::time::Duration::from_secs(2));

        let state = app_handle.state::<AppState>();
        let mut running = state.osagent_running.lock().unwrap();

        if *running {
            let mut process = state.osagent_process.lock().unwrap();
            if let Some(ref mut child) = *process {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        let code = status.code().unwrap_or(-1);
                        add_log(
                            &state,
                            "warn",
                            format!("OSAgent exited with code: {}", code),
                        );
                        *running = false;
                        state.osagent_pid.lock().unwrap().take();

                        let _ = app_handle.emit_all(
                            "osagent-status-changed",
                            AgentStatus {
                                running: false,
                                pid: None,
                                osagent_path: state.osagent_path.to_string_lossy().to_string(),
                                config_path: state.config_path.to_string_lossy().to_string(),
                            },
                        );
                    }
                    Ok(None) => {}
                    Err(e) => {
                        add_log(&state, "error", format!("Failed to check process: {}", e));
                        *running = false;
                        state.osagent_pid.lock().unwrap().take();
                    }
                }
            }
        }
    });
}

// --- Main Entry ---

fn main() {
    tracing_subscriber::fmt()
        .with_max_level(Level::INFO)
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let osagent_path = get_osagent_path();
    let config_path = get_config_path();

    info!("OSAgent Launcher starting");
    info!("OSAgent path: {}", osagent_path.display());
    info!("Config path: {}", config_path.display());

    let app_state = AppState {
        osagent_process: Mutex::new(None),
        osagent_pid: Mutex::new(None),
        osagent_running: Mutex::new(false),
        build_running: Mutex::new(false),
        osagent_path,
        config_path,
        logs: Mutex::new(Vec::new()),
    };

    tauri::Builder::default()
        .system_tray(build_tray())
        .on_system_tray_event(|app, event| match event {
            SystemTrayEvent::LeftClick {
                position: _,
                size: _,
                ..
            } => {
                if let Some(window) = app.get_window("main") {
                    if window.is_visible().unwrap_or(false) {
                        window.hide().ok();
                    } else {
                        window.show().ok();
                        window.set_focus().ok();
                    }
                }
            }
            SystemTrayEvent::MenuItemClick { id, .. } => match id.as_str() {
                "open_launcher" => {
                    if let Some(window) = app.get_window("main") {
                        window.show().ok();
                        window.set_focus().ok();
                        window.unminimize().ok();
                    }
                }
                "open_ui" => {
                    open_web_ui();
                }
                "start_osagent" => {
                    if let Some(window) = app.get_window("main") {
                        let _ = start_osagent(window, app.state());
                    }
                }
                "stop_osagent" => {
                    if let Some(window) = app.get_window("main") {
                        let _ = stop_osagent(window, app.state());
                    }
                }
                "exit" => {
                    let state = app.state::<AppState>();
                    let mut process = state.osagent_process.lock().unwrap();
                    if let Some(mut child) = process.take() {
                        let _ = child.kill();
                    }
                    app.exit(0);
                }
                _ => {}
            },
            SystemTrayEvent::DoubleClick {
                position: _,
                size: _,
                ..
            } => {
                if let Some(window) = app.get_window("main") {
                    window.show().ok();
                    window.set_focus().ok();
                }
            }
            _ => {}
        })
        .manage(app_state)
        .on_window_event(|event| match event.event() {
            WindowEvent::CloseRequested { api, .. } => {
                event.window().hide().ok();
                api.prevent_close();
            }
            _ => {}
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            get_logs,
            get_build_running,
            start_osagent,
            stop_osagent,
            restart_osagent,
            open_web_ui,
            hide_to_tray,
            show_window,
            minimize_window,
            exit_app,
            check_osagent_path,
            build_osagent,
        ])
        .setup(|app| {
            start_process_monitor(app.handle());

            let state = app.state::<AppState>();
            add_log(&state, "info", "OSAgent Launcher initialized".into());

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
