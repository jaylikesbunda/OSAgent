//! Chromium discovery, isolated launch and teardown.
//!
//! Every browser gets a throwaway profile directory, a loopback-only
//! debugging port chosen by the OS (read back from `DevToolsActivePort`,
//! never guessed) and is killed as a process tree on close or drop.

use crate::config::BrowserConfig;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::time::{Duration, Instant};
use tokio::process::{Child, Command};

pub const INSTALL_HINT: &str = "No Chrome/Chromium/Edge/Brave binary was found. Install one, or set [browser] executable_path in the OSA config (or the OSAGENT_BROWSER environment variable).";

/// Locate a Chromium-family executable. An explicit config path wins, then
/// the environment override, then well-known install locations, then PATH.
pub fn find_browser(configured: &str) -> Option<PathBuf> {
    let configured = configured.trim();
    if !configured.is_empty() {
        let path = PathBuf::from(shellexpand::tilde(configured).to_string());
        return path.is_file().then_some(path);
    }
    if let Ok(value) = std::env::var("OSAGENT_BROWSER") {
        let path = PathBuf::from(value.trim());
        if path.is_file() {
            return Some(path);
        }
    }
    for candidate in well_known_paths() {
        if candidate.is_file() {
            return Some(candidate);
        }
    }
    for name in [
        "google-chrome",
        "google-chrome-stable",
        "chromium",
        "chromium-browser",
        "microsoft-edge",
        "microsoft-edge-stable",
        "brave-browser",
        "chrome",
        "msedge",
    ] {
        if let Ok(path) = which::which(name) {
            return Some(path);
        }
    }
    None
}

fn well_known_paths() -> Vec<PathBuf> {
    let mut paths = Vec::new();
    if cfg!(windows) {
        let roots: Vec<PathBuf> = ["ProgramFiles", "ProgramFiles(x86)", "LOCALAPPDATA"]
            .iter()
            .filter_map(|key| std::env::var_os(key).map(PathBuf::from))
            .collect();
        for root in roots {
            for relative in [
                "Google/Chrome/Application/chrome.exe",
                "Microsoft/Edge/Application/msedge.exe",
                "BraveSoftware/Brave-Browser/Application/brave.exe",
                "Chromium/Application/chrome.exe",
            ] {
                paths.push(root.join(relative));
            }
        }
    } else if cfg!(target_os = "macos") {
        for path in [
            "/Applications/Google Chrome.app/Contents/MacOS/Google Chrome",
            "/Applications/Microsoft Edge.app/Contents/MacOS/Microsoft Edge",
            "/Applications/Brave Browser.app/Contents/MacOS/Brave Browser",
            "/Applications/Chromium.app/Contents/MacOS/Chromium",
        ] {
            paths.push(PathBuf::from(path));
        }
    }
    paths
}

/// Launch flags for an isolated, headless, automation-only browser.
pub fn launch_args(config: &BrowserConfig, profile_dir: &Path) -> Vec<String> {
    let mut args = vec![
        format!("--user-data-dir={}", profile_dir.display()),
        "--remote-debugging-address=127.0.0.1".to_string(),
        "--remote-debugging-port=0".to_string(),
        "--no-first-run".to_string(),
        "--no-default-browser-check".to_string(),
        "--disable-extensions".to_string(),
        "--disable-sync".to_string(),
        "--disable-default-apps".to_string(),
        "--disable-background-networking".to_string(),
        "--disable-component-update".to_string(),
        "--disable-popup-blocking".to_string(),
        "--password-store=basic".to_string(),
        "--use-mock-keychain".to_string(),
        "--mute-audio".to_string(),
        "--disable-features=Translate,OptimizationHints,MediaRouter".to_string(),
        format!(
            "--window-size={},{}",
            config.viewport_width, config.viewport_height
        ),
    ];
    if config.headless {
        args.push("--headless=new".to_string());
        args.push("--disable-gpu".to_string());
    }
    if config.no_sandbox {
        args.push("--no-sandbox".to_string());
    }
    if !config.user_agent.trim().is_empty() {
        args.push(format!("--user-agent={}", config.user_agent.trim()));
    }
    args.push("about:blank".to_string());
    args
}

pub struct BrowserProcess {
    child: Option<Child>,
    pid: Option<u32>,
    profile_dir: PathBuf,
    /// `ws://127.0.0.1:<port>/devtools/browser/<id>`
    pub ws_url: String,
}

impl BrowserProcess {
    pub async fn launch(
        executable: &Path,
        config: &BrowserConfig,
        profile_dir: PathBuf,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(&profile_dir)
            .map_err(|error| format!("could not create browser profile dir: {error}"))?;

        let mut command = Command::new(executable);
        command
            .args(launch_args(config, &profile_dir))
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        #[cfg(windows)]
        {
            const CREATE_NO_WINDOW: u32 = 0x0800_0000;
            command.creation_flags(CREATE_NO_WINDOW);
        }

        let child = command
            .spawn()
            .map_err(|error| format!("could not start {}: {error}", executable.display()))?;
        let pid = child.id();
        let mut process = Self {
            child: Some(child),
            pid,
            profile_dir: profile_dir.clone(),
            ws_url: String::new(),
        };

        // Chromium writes `DevToolsActivePort` (port, then browser path)
        // once the debugging server is listening on the port it picked.
        let marker = profile_dir.join("DevToolsActivePort");
        let deadline = Instant::now() + Duration::from_secs(20);
        loop {
            if let Some(child) = process.child.as_mut() {
                if let Ok(Some(status)) = child.try_wait() {
                    return Err(format!(
                        "browser exited during startup ({status}); try [browser] no_sandbox or a different executable_path"
                    ));
                }
            }
            if let Ok(contents) = std::fs::read_to_string(&marker) {
                let mut lines = contents.lines();
                if let (Some(port), Some(path)) = (lines.next(), lines.next()) {
                    if port.trim().parse::<u16>().is_ok() && path.starts_with('/') {
                        process.ws_url = format!("ws://127.0.0.1:{}{}", port.trim(), path.trim());
                        return Ok(process);
                    }
                }
            }
            if Instant::now() >= deadline {
                return Err("timed out waiting for the browser debugging endpoint".to_string());
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Wait briefly for a graceful exit, then kill the whole process tree.
    pub async fn shutdown(&mut self) {
        if let Some(child) = self.child.as_mut() {
            let exited = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            if exited.is_err() {
                kill_tree(self.pid);
                let _ = tokio::time::timeout(Duration::from_secs(2), child.wait()).await;
            }
        }
        self.child = None;
        self.remove_profile();
    }

    fn remove_profile(&self) {
        // Chromium can hold files for a moment after exit.
        for _ in 0..5 {
            if std::fs::remove_dir_all(&self.profile_dir).is_ok() || !self.profile_dir.exists() {
                if let Some(parent) = self.profile_dir.parent() {
                    let _ = std::fs::remove_dir(parent);
                }
                return;
            }
            std::thread::sleep(Duration::from_millis(100));
        }
    }
}

impl Drop for BrowserProcess {
    fn drop(&mut self) {
        if self.child.is_some() {
            kill_tree(self.pid);
            self.remove_profile();
        }
    }
}

fn kill_tree(pid: Option<u32>) {
    let Some(pid) = pid else { return };
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        let _ = std::process::Command::new("taskkill")
            .args(["/PID", &pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
    #[cfg(unix)]
    {
        // Launched with `process_group(0)`, so the pid is the group id.
        let _ = std::process::Command::new("kill")
            .args(["-KILL", &format!("-{pid}")])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
    }
}

/// Remove profile directories left behind by a crashed run. Only entries
/// untouched for over an hour are removed, so a second live OSA process
/// sharing this directory is left alone.
pub fn cleanup_stale_profiles(root: &Path) {
    let Ok(entries) = std::fs::read_dir(root) else {
        return;
    };
    for entry in entries.flatten() {
        let stale = entry
            .metadata()
            .and_then(|meta| meta.modified())
            .ok()
            .and_then(|modified| modified.elapsed().ok())
            .is_some_and(|age| age > Duration::from_secs(3600));
        if stale {
            let _ = std::fs::remove_dir_all(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn launch_args_isolate_profile_and_bind_loopback() {
        let config = BrowserConfig::default();
        let args = launch_args(&config, Path::new("/tmp/osa/profile"));

        assert!(args.iter().any(|a| a.starts_with("--user-data-dir=")));
        assert!(args.contains(&"--remote-debugging-address=127.0.0.1".to_string()));
        assert!(args.contains(&"--remote-debugging-port=0".to_string()));
        assert!(args.contains(&"--headless=new".to_string()));
        assert!(args.contains(&"--disable-extensions".to_string()));
        assert!(!args.contains(&"--no-sandbox".to_string()));
        assert_eq!(args.last().map(String::as_str), Some("about:blank"));
    }

    #[test]
    fn launch_args_honor_overrides() {
        let config = BrowserConfig {
            headless: false,
            no_sandbox: true,
            user_agent: "OSA-Test/1.0".to_string(),
            ..BrowserConfig::default()
        };
        let args = launch_args(&config, Path::new("p"));

        assert!(!args.contains(&"--headless=new".to_string()));
        assert!(args.contains(&"--no-sandbox".to_string()));
        assert!(args.contains(&"--user-agent=OSA-Test/1.0".to_string()));
    }

    #[test]
    fn missing_explicit_path_does_not_fall_back() {
        assert!(find_browser("/definitely/not/a/browser").is_none());
    }

    #[test]
    fn stale_cleanup_keeps_fresh_dirs() {
        let root = tempfile::tempdir().unwrap();
        let fresh = root.path().join("fresh");
        std::fs::create_dir_all(&fresh).unwrap();
        cleanup_stale_profiles(root.path());
        assert!(fresh.exists());
    }
}
