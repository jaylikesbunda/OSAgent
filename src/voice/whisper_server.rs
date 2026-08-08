//! Persistent whisper.cpp server.
//!
//! The CLI path spawns a process and loads the whole model from disk for every
//! utterance. For a two-second command that model load dominates: `base` is
//! ~148MB and `small` ~488MB, against maybe 100ms of actual inference.
//!
//! `whisper-server` ships in the same release archive we already download, and
//! the Windows installer already copies every file out of the release
//! directory, so the binary is usually present without any install change.
//! Keeping one resident process removes the reload entirely.
//!
//! Everything here is best-effort: if the binary is missing, the port is taken,
//! or the process dies, callers fall back to the CLI path.

use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::Mutex;
use std::time::{Duration, Instant};

use tracing::{info, warn};

use super::get_models_dir;

/// How long to wait for a freshly spawned server to accept connections. Model
/// load is the bulk of this, so it scales with model size.
const STARTUP_TIMEOUT: Duration = Duration::from_secs(90);

static SERVER: Mutex<Option<ServerHandle>> = Mutex::new(None);

struct ServerHandle {
    child: Child,
    port: u16,
    model_path: PathBuf,
}

impl ServerHandle {
    /// `try_wait` reaps the child if it exited, so a crashed server is detected
    /// rather than leaving us posting into a closed port.
    fn is_alive(&mut self) -> bool {
        matches!(self.child.try_wait(), Ok(None))
    }

    fn kill(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

pub fn server_binary_path() -> PathBuf {
    let dir = get_models_dir();
    if cfg!(windows) {
        dir.join("whisper-server.exe")
    } else {
        dir.join("whisper-server")
    }
}

pub fn is_available() -> bool {
    server_binary_path().exists()
}

/// Asks the OS for an unused port by binding to 0 and immediately releasing it.
///
/// Inherently racy, but the alternative is a fixed port that collides with
/// whatever else the user is running, and a failed bind is recoverable here.
fn pick_port() -> Result<u16, String> {
    let listener = std::net::TcpListener::bind("127.0.0.1:0")
        .map_err(|e| format!("Could not reserve a port for whisper-server: {e}"))?;
    let port = listener
        .local_addr()
        .map_err(|e| format!("Could not read reserved port: {e}"))?
        .port();
    drop(listener);
    Ok(port)
}

fn wait_until_ready(port: u16, deadline: Instant, child: &mut Child) -> Result<(), String> {
    while Instant::now() < deadline {
        // A server that died during model load will never accept, so check for
        // exit rather than spinning until the timeout.
        if let Ok(Some(status)) = child.try_wait() {
            return Err(format!("whisper-server exited during startup ({status})"));
        }

        if std::net::TcpStream::connect_timeout(
            &([127, 0, 0, 1], port).into(),
            Duration::from_millis(250),
        )
        .is_ok()
        {
            return Ok(());
        }

        std::thread::sleep(Duration::from_millis(150));
    }

    Err("whisper-server did not become ready in time".to_string())
}

/// Returns the port of a running server for `model_path`, starting one if
/// needed. Switching models restarts the process, since the server holds a
/// single model for its lifetime.
pub fn ensure_running(model_path: &Path, threads: usize) -> Result<u16, String> {
    let mut guard = SERVER
        .lock()
        .map_err(|_| "whisper-server mutex poisoned".to_string())?;

    if let Some(handle) = guard.as_mut() {
        if handle.is_alive() && handle.model_path == model_path {
            return Ok(handle.port);
        }
        // Wrong model or dead process: replace it.
        handle.kill();
        *guard = None;
    }

    let binary = server_binary_path();
    if !binary.exists() {
        return Err("whisper-server binary is not installed".to_string());
    }

    let port = pick_port()?;

    let mut command = Command::new(&binary);
    command
        .args([
            "-m",
            &model_path.to_string_lossy(),
            "-t",
            &threads.to_string(),
            "--host",
            "127.0.0.1",
            "--port",
            &port.to_string(),
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null());

    // On Windows a child spawned after the web server has bound can inherit its
    // listening socket. If the child then outlives OSA, the port stays occupied
    // by a dead PID and the next start fails with "address already in use".
    //
    // DETACHED_PROCESS gives the child its own console and no inherited console
    // handles; CREATE_NO_WINDOW keeps it from flashing a window.
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        const DETACHED_PROCESS: u32 = 0x0000_0008;
        const CREATE_NO_WINDOW: u32 = 0x0800_0000;
        command.creation_flags(DETACHED_PROCESS | CREATE_NO_WINDOW);
    }

    let mut child = command
        .spawn()
        .map_err(|e| format!("Failed to start whisper-server: {e}"))?;

    let deadline = Instant::now() + STARTUP_TIMEOUT;
    if let Err(err) = wait_until_ready(port, deadline, &mut child) {
        let _ = child.kill();
        let _ = child.wait();
        return Err(err);
    }

    info!(
        "whisper-server ready on port {} with model {}",
        port,
        model_path.display()
    );

    *guard = Some(ServerHandle {
        child,
        port,
        model_path: model_path.to_path_buf(),
    });

    Ok(port)
}

/// Kills whisper-server processes left over from a previous run.
///
/// `shutdown` only runs when OSA exits cleanly. When it is killed — by the
/// launcher, a crash, or Task Manager — the child survives, keeps a few hundred
/// megabytes resident, and on Windows can keep OSA's own listening socket alive
/// so the next start fails to bind its port.
///
/// OSA is the only thing that spawns this binary, so sweeping every instance at
/// startup is safe.
pub fn kill_stale_servers() {
    let result = if cfg!(windows) {
        Command::new("taskkill")
            .args(["/F", "/IM", "whisper-server.exe"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    } else {
        Command::new("pkill")
            .args(["-f", "whisper-server"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
    };

    // A non-zero status just means there was nothing to kill.
    match result {
        Ok(status) if status.success() => {
            info!("Cleared a leftover whisper-server from a previous run")
        }
        Ok(_) => {}
        Err(err) => warn!("Could not check for leftover whisper-server: {}", err),
    }
}

/// Stops the resident server. Called on shutdown so a ~500MB process is not
/// left behind holding a model.
pub fn shutdown() {
    let Ok(mut guard) = SERVER.lock() else {
        return;
    };
    if let Some(handle) = guard.as_mut() {
        info!("Stopping whisper-server");
        handle.kill();
    }
    *guard = None;
}

/// Transcribes a WAV file through the resident server.
pub async fn transcribe(
    audio_path: &Path,
    language: Option<&str>,
    model_path: &Path,
    threads: usize,
) -> Result<String, String> {
    let port = {
        let model_path = model_path.to_path_buf();
        tokio::task::spawn_blocking(move || ensure_running(&model_path, threads))
            .await
            .map_err(|e| format!("spawn_blocking error: {e}"))??
    };

    let bytes = tokio::fs::read(audio_path)
        .await
        .map_err(|e| format!("Failed to read audio for transcription: {e}"))?;

    let part = reqwest::multipart::Part::bytes(bytes)
        .file_name("audio.wav")
        .mime_str("audio/wav")
        .map_err(|e| format!("Invalid audio part: {e}"))?;

    let mut form = reqwest::multipart::Form::new()
        .part("file", part)
        .text("response_format", "json")
        // Greedy, matching the CLI path: for short conversational input the
        // accuracy difference does not justify the extra beams.
        .text("beam_size", "1");

    if let Some(lang) = language {
        form = form.text("language", lang.to_string());
    }

    let client = reqwest::Client::new();
    let response = client
        .post(format!("http://127.0.0.1:{port}/inference"))
        .multipart(form)
        .send()
        .await
        .map_err(|e| format!("whisper-server request failed: {e}"))?;

    if !response.status().is_success() {
        return Err(format!(
            "whisper-server returned {}",
            response.status()
        ));
    }

    let payload: serde_json::Value = response
        .json()
        .await
        .map_err(|e| format!("whisper-server returned invalid JSON: {e}"))?;

    let text = payload
        .get("text")
        .and_then(|value| value.as_str())
        .ok_or_else(|| "whisper-server response had no text field".to_string())?;

    Ok(text.trim().to_string())
}

/// Marks the resident server unusable after a failed request, so the next call
/// starts a fresh one instead of retrying a wedged process.
pub fn mark_unhealthy() {
    if let Ok(mut guard) = SERVER.lock() {
        if let Some(handle) = guard.as_mut() {
            warn!("Restarting whisper-server after a failed request");
            handle.kill();
        }
        *guard = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn binary_name_matches_platform() {
        let path = server_binary_path();
        let name = path.file_name().unwrap().to_string_lossy().to_string();
        if cfg!(windows) {
            assert_eq!(name, "whisper-server.exe");
        } else {
            assert_eq!(name, "whisper-server");
        }
    }

    /// The reserved port must actually be bindable after we release it,
    /// otherwise the server would fail to start on a port we just handed it.
    #[test]
    fn picked_port_is_usable() {
        let port = pick_port().expect("pick a port");
        assert!(port > 0);
        let listener = std::net::TcpListener::bind(("127.0.0.1", port));
        assert!(listener.is_ok(), "reserved port {port} was not bindable");
    }

    #[test]
    fn picked_ports_are_not_all_identical() {
        // Two reservations held at once must differ, or concurrent starts would
        // collide on the same port.
        let a = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let b = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        assert_ne!(
            a.local_addr().unwrap().port(),
            b.local_addr().unwrap().port()
        );
    }

    /// Starting must fail cleanly, not panic or hang, when the binary is absent.
    /// This is the path every user without whisper-server installed takes.
    #[test]
    fn ensure_running_errors_without_binary() {
        if is_available() {
            // whisper-server is installed on this machine; the negative case
            // cannot be exercised without removing it.
            return;
        }
        let err = ensure_running(Path::new("nonexistent-model.bin"), 4)
            .expect_err("should refuse to start without a binary");
        assert!(err.contains("not installed"), "unexpected error: {err}");
    }

    #[test]
    fn shutdown_is_safe_when_nothing_is_running() {
        shutdown();
        shutdown();
    }

    #[test]
    fn mark_unhealthy_is_safe_when_nothing_is_running() {
        mark_unhealthy();
    }
}
