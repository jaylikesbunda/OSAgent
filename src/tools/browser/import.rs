//! Import logged-in sessions (cookies) from the user's own browsers into
//! the agent browser's cookie jar.
//!
//! Flow: the user picks a profile in settings, scans it to see which
//! domains hold cookies, selects the domains to share, and imports. Only
//! the selected cookies are copied into `<config>/browser/cookies.json`,
//! and are injected into each new agent browser session. Cookie values are
//! never returned by the API or shown to the model; only domains and counts.
//!
//! Support:
//! - Firefox / LibreWolf: plaintext `cookies.sqlite`.
//! - Chrome / Brave / Edge / Opera: AES-GCM (`v10`/`v11`) cookies on
//!   Windows, key unwrapped through the current user's DPAPI. Chromium's
//!   app-bound encryption (`v20`, Chrome/Edge 127+) is deliberately not
//!   bypassed; those cookies are reported as skipped.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use rusqlite::{Connection, OpenFlags};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum BrowserKind {
    Firefox,
    Librewolf,
    Chrome,
    Brave,
    Edge,
    Opera,
}

impl BrowserKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::Firefox => "Firefox",
            Self::Librewolf => "LibreWolf",
            Self::Chrome => "Chrome",
            Self::Brave => "Brave",
            Self::Edge => "Edge",
            Self::Opera => "Opera",
        }
    }

    fn is_gecko(self) -> bool {
        matches!(self, Self::Firefox | Self::Librewolf)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ProfileInfo {
    /// Stable handle the UI sends back, e.g. `chrome:Default`.
    pub id: String,
    pub browser: BrowserKind,
    pub browser_label: &'static str,
    pub name: String,
    #[serde(skip)]
    pub path: PathBuf,
    /// Chromium user-data root (holds `Local State`); unused for Gecko.
    #[serde(skip)]
    pub root: PathBuf,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Cookie {
    pub name: String,
    pub value: String,
    /// As stored: a leading dot marks a domain cookie, none a host-only cookie.
    pub domain: String,
    pub path: String,
    /// Unix seconds; `None` for session cookies.
    pub expires: Option<f64>,
    pub secure: bool,
    pub http_only: bool,
    /// `Strict`, `Lax` or `None`.
    pub same_site: Option<String>,
}

impl Cookie {
    pub fn bare_domain(&self) -> &str {
        self.domain.trim_start_matches('.')
    }
}

#[derive(Debug, Default)]
pub struct ReadOutcome {
    pub cookies: Vec<Cookie>,
    /// Reason -> number of cookies that could not be read.
    pub skipped: BTreeMap<String, usize>,
}

// ---------------------------------------------------------------- discovery

fn home() -> Option<PathBuf> {
    dirs_next::home_dir()
}

fn env_path(key: &str) -> Option<PathBuf> {
    std::env::var_os(key).map(PathBuf::from)
}

fn gecko_roots() -> Vec<(BrowserKind, PathBuf)> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        if let Some(appdata) = env_path("APPDATA") {
            roots.push((BrowserKind::Firefox, appdata.join("Mozilla/Firefox/Profiles")));
            roots.push((BrowserKind::Librewolf, appdata.join("librewolf/Profiles")));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = home() {
            let support = home.join("Library/Application Support");
            roots.push((BrowserKind::Firefox, support.join("Firefox/Profiles")));
            roots.push((BrowserKind::Librewolf, support.join("librewolf/Profiles")));
        }
    } else if let Some(home) = home() {
        roots.push((BrowserKind::Firefox, home.join(".mozilla/firefox")));
        roots.push((BrowserKind::Librewolf, home.join(".librewolf")));
    }
    roots
}

fn chromium_roots() -> Vec<(BrowserKind, PathBuf)> {
    let mut roots = Vec::new();
    if cfg!(windows) {
        if let Some(local) = env_path("LOCALAPPDATA") {
            roots.push((BrowserKind::Chrome, local.join("Google/Chrome/User Data")));
            roots.push((
                BrowserKind::Brave,
                local.join("BraveSoftware/Brave-Browser/User Data"),
            ));
            roots.push((BrowserKind::Edge, local.join("Microsoft/Edge/User Data")));
        }
        if let Some(appdata) = env_path("APPDATA") {
            roots.push((BrowserKind::Opera, appdata.join("Opera Software/Opera Stable")));
            roots.push((BrowserKind::Opera, appdata.join("Opera Software/Opera GX Stable")));
        }
    } else if cfg!(target_os = "macos") {
        if let Some(home) = home() {
            let support = home.join("Library/Application Support");
            roots.push((BrowserKind::Chrome, support.join("Google/Chrome")));
            roots.push((BrowserKind::Brave, support.join("BraveSoftware/Brave-Browser")));
            roots.push((BrowserKind::Edge, support.join("Microsoft Edge")));
            roots.push((BrowserKind::Opera, support.join("com.operasoftware.Opera")));
        }
    } else if let Some(home) = home() {
        let config = home.join(".config");
        roots.push((BrowserKind::Chrome, config.join("google-chrome")));
        roots.push((BrowserKind::Brave, config.join("BraveSoftware/Brave-Browser")));
        roots.push((BrowserKind::Edge, config.join("microsoft-edge")));
        roots.push((BrowserKind::Opera, config.join("opera")));
    }
    roots
}

fn chromium_cookie_file(profile: &Path) -> Option<PathBuf> {
    [profile.join("Network/Cookies"), profile.join("Cookies")]
        .into_iter()
        .find(|path| path.is_file())
}

pub fn detect_profiles() -> Vec<ProfileInfo> {
    let mut profiles = Vec::new();

    for (kind, root) in gecko_roots() {
        let Ok(entries) = std::fs::read_dir(&root) else { continue };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.join("cookies.sqlite").is_file() {
                let name = entry.file_name().to_string_lossy().to_string();
                profiles.push(ProfileInfo {
                    id: format!("{}:{}", slug(kind), name),
                    browser: kind,
                    browser_label: kind.label(),
                    name,
                    path,
                    root: root.clone(),
                });
            }
        }
    }

    for (kind, root) in chromium_roots() {
        if !root.is_dir() {
            continue;
        }
        // Opera keeps its single profile directly in the root.
        let mut candidates = vec![root.clone()];
        if let Ok(entries) = std::fs::read_dir(&root) {
            candidates.extend(entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()));
        }
        for path in candidates {
            if chromium_cookie_file(&path).is_none() {
                continue;
            }
            let name = if path == root {
                "Default".to_string()
            } else {
                path.file_name().map(|n| n.to_string_lossy().to_string()).unwrap_or_default()
            };
            let id = format!("{}:{}:{}", slug(kind), name, root.display());
            profiles.push(ProfileInfo {
                id,
                browser: kind,
                browser_label: kind.label(),
                name,
                path,
                root: root.clone(),
            });
        }
    }
    profiles
}

fn slug(kind: BrowserKind) -> &'static str {
    match kind {
        BrowserKind::Firefox => "firefox",
        BrowserKind::Librewolf => "librewolf",
        BrowserKind::Chrome => "chrome",
        BrowserKind::Brave => "brave",
        BrowserKind::Edge => "edge",
        BrowserKind::Opera => "opera",
    }
}

pub fn find_profile(id: &str) -> Option<ProfileInfo> {
    detect_profiles().into_iter().find(|profile| profile.id == id)
}

// ------------------------------------------------------------------ reading

pub fn read_cookies(profile: &ProfileInfo) -> Result<ReadOutcome, String> {
    let mut outcome = if profile.browser.is_gecko() {
        read_gecko(&profile.path)?
    } else {
        read_chromium(profile)?
    };
    let now = now_secs();
    outcome
        .cookies
        .retain(|cookie| cookie.expires.map_or(true, |at| at > now));
    Ok(outcome)
}

fn now_secs() -> f64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs_f64())
        .unwrap_or(0.0)
}

/// Copy a (possibly WAL-mode) SQLite database next to its sidecars so it can
/// be read without touching the live file. Fails with a clear message if
/// the browser holds an exclusive lock.
fn snapshot_db(source: &Path, browser: &str) -> Result<(tempfile::TempDir, PathBuf), String> {
    let dir = tempfile::tempdir().map_err(|e| format!("temp dir failed: {e}"))?;
    let target = dir.path().join("cookies.db");
    std::fs::copy(source, &target).map_err(|error| {
        format!("could not read the cookie database ({error}). Close {browser} completely and try again.")
    })?;
    for suffix in ["-wal", "-shm"] {
        let mut sidecar = source.as_os_str().to_owned();
        sidecar.push(suffix);
        let sidecar = PathBuf::from(sidecar);
        if sidecar.is_file() {
            let mut dest = target.as_os_str().to_owned();
            dest.push(suffix);
            let _ = std::fs::copy(&sidecar, PathBuf::from(dest));
        }
    }
    Ok((dir, target))
}

fn read_gecko(profile: &Path) -> Result<ReadOutcome, String> {
    let (_guard, db) = snapshot_db(&profile.join("cookies.sqlite"), "the browser")?;
    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("could not open cookie database: {e}"))?;
    let mut statement = conn
        .prepare("SELECT name, value, host, path, expiry, isSecure, isHttpOnly, sameSite FROM moz_cookies")
        .map_err(|e| format!("unexpected cookie database layout: {e}"))?;
    let rows = statement
        .query_map([], |row| {
            let expiry: i64 = row.get(4)?;
            // Newer Firefox builds store milliseconds.
            let expiry_secs = if expiry > 100_000_000_000 { expiry / 1000 } else { expiry };
            let same_site: i64 = row.get(7).unwrap_or(0);
            Ok(Cookie {
                name: row.get(0)?,
                value: row.get(1)?,
                domain: row.get(2)?,
                path: row.get(3)?,
                expires: (expiry_secs > 0).then_some(expiry_secs as f64),
                secure: row.get::<_, i64>(5)? != 0,
                http_only: row.get::<_, i64>(6)? != 0,
                same_site: match same_site {
                    1 => Some("Lax".to_string()),
                    2 => Some("Strict".to_string()),
                    _ => Some("None".to_string()),
                },
            })
        })
        .map_err(|e| format!("could not read cookies: {e}"))?;
    Ok(ReadOutcome {
        cookies: rows.filter_map(Result::ok).collect(),
        skipped: BTreeMap::new(),
    })
}

fn read_chromium(profile: &ProfileInfo) -> Result<ReadOutcome, String> {
    let cookie_file = chromium_cookie_file(&profile.path).ok_or("no cookie database in this profile")?;
    let label = profile.browser.label();
    let (_guard, db) = snapshot_db(&cookie_file, label)?;
    let key = chromium_key(&profile.root);

    let conn = Connection::open_with_flags(&db, OpenFlags::SQLITE_OPEN_READ_ONLY)
        .map_err(|e| format!("could not open cookie database: {e}"))?;
    let mut statement = conn
        .prepare("SELECT host_key, name, value, encrypted_value, path, expires_utc, is_secure, is_httponly, samesite FROM cookies")
        .map_err(|e| format!("unexpected cookie database layout: {e}"))?;

    let mut outcome = ReadOutcome::default();
    let rows = statement
        .query_map([], |row| {
            Ok((
                row.get::<_, String>(0)?,
                row.get::<_, String>(1)?,
                row.get::<_, String>(2)?,
                row.get::<_, Vec<u8>>(3)?,
                row.get::<_, String>(4)?,
                row.get::<_, i64>(5)?,
                row.get::<_, i64>(6)? != 0,
                row.get::<_, i64>(7)? != 0,
                row.get::<_, i64>(8).unwrap_or(-1),
            ))
        })
        .map_err(|e| format!("could not read cookies: {e}"))?;

    for row in rows.flatten() {
        let (host, name, plain, encrypted, path, expires_utc, secure, http_only, same_site) = row;
        let value = if !plain.is_empty() || encrypted.is_empty() {
            plain
        } else {
            match decrypt_chromium_value(key.as_ref(), &encrypted, &host) {
                Ok(value) => value,
                Err(reason) => {
                    *outcome.skipped.entry(reason).or_insert(0) += 1;
                    continue;
                }
            }
        };
        outcome.cookies.push(Cookie {
            name,
            value,
            domain: host,
            path,
            // WebKit epoch (1601) in microseconds.
            expires: (expires_utc > 0).then(|| expires_utc as f64 / 1_000_000.0 - 11_644_473_600.0),
            secure,
            http_only,
            same_site: match same_site {
                0 => Some("None".to_string()),
                1 => Some("Lax".to_string()),
                2 => Some("Strict".to_string()),
                _ => None,
            },
        });
    }
    Ok(outcome)
}

fn chromium_key(root: &Path) -> Result<Vec<u8>, String> {
    let state = std::fs::read_to_string(root.join("Local State"))
        .map_err(|e| format!("no Local State: {e}"))?;
    let state: serde_json::Value = serde_json::from_str(&state).map_err(|e| e.to_string())?;
    let encoded = state["os_crypt"]["encrypted_key"]
        .as_str()
        .ok_or("no encryption key in Local State")?;
    let wrapped = base64::engine::general_purpose::STANDARD
        .decode(encoded)
        .map_err(|e| e.to_string())?;
    let blob = wrapped.strip_prefix(b"DPAPI").ok_or("unrecognised key format")?;
    dpapi_unprotect(blob)
}

#[cfg(windows)]
fn dpapi_unprotect(blob: &[u8]) -> Result<Vec<u8>, String> {
    use std::os::windows::process::CommandExt;
    let encoded = base64::engine::general_purpose::STANDARD.encode(blob);
    let script = format!(
        "Add-Type -AssemblyName System.Security; \
         $b=[Convert]::FromBase64String('{encoded}'); \
         [Convert]::ToBase64String([Security.Cryptography.ProtectedData]::Unprotect($b,$null,'CurrentUser'))"
    );
    let output = std::process::Command::new("powershell")
        .args(["-NoProfile", "-NonInteractive", "-Command", &script])
        .creation_flags(0x0800_0000)
        .output()
        .map_err(|e| format!("could not run powershell: {e}"))?;
    if !output.status.success() {
        return Err("Windows could not unlock the browser's cookie key for this user".to_string());
    }
    base64::engine::general_purpose::STANDARD
        .decode(String::from_utf8_lossy(&output.stdout).trim())
        .map_err(|e| e.to_string())
}

#[cfg(not(windows))]
fn dpapi_unprotect(_blob: &[u8]) -> Result<Vec<u8>, String> {
    Err("importing from Chromium-based browsers is only supported on Windows".to_string())
}

/// Decrypt a Chromium `v10`/`v11` AES-256-GCM cookie value.
fn decrypt_chromium_value(
    key: Result<&Vec<u8>, &String>,
    encrypted: &[u8],
    host: &str,
) -> Result<String, String> {
    let prefix = encrypted.get(..3).unwrap_or_default();
    if prefix == b"v20" {
        return Err("app-bound encryption (Chrome/Edge 127+) is not supported".to_string());
    }
    if prefix != b"v10" && prefix != b"v11" {
        return Err("unrecognised cookie encryption".to_string());
    }
    let key = key.map_err(|e| e.clone())?;
    let plain = decrypt_gcm(key, encrypted)?;
    // Chromium 130+ prepends SHA-256(host_key) to the plaintext.
    let digest = Sha256::digest(host.as_bytes());
    let body = plain.strip_prefix(digest.as_slice()).unwrap_or(&plain);
    String::from_utf8(body.to_vec()).map_err(|_| "cookie value was not valid text".to_string())
}

fn decrypt_gcm(key: &[u8], encrypted: &[u8]) -> Result<Vec<u8>, String> {
    if encrypted.len() < 3 + 12 + 16 {
        return Err("truncated cookie value".to_string());
    }
    let cipher = Aes256Gcm::new_from_slice(key).map_err(|_| "invalid cookie key".to_string())?;
    let nonce = Nonce::from_slice(&encrypted[3..15]);
    cipher
        .decrypt(nonce, &encrypted[15..])
        .map_err(|_| "cookie could not be decrypted".to_string())
}

// --------------------------------------------------------------------- jar

#[derive(Debug, Default, Serialize, Deserialize)]
pub struct CookieJar {
    #[serde(default)]
    pub imported_at: Option<String>,
    /// Human-readable origins, e.g. `Firefox (default-release)`.
    #[serde(default)]
    pub sources: Vec<String>,
    #[serde(default)]
    pub cookies: Vec<Cookie>,
}

#[derive(Debug, Serialize)]
pub struct DomainCount {
    pub domain: String,
    pub count: usize,
}

pub fn jar_path(config_dir: &Path) -> PathBuf {
    config_dir.join("browser").join("cookies.json")
}

impl CookieJar {
    pub fn load(config_dir: &Path) -> Self {
        std::fs::read_to_string(jar_path(config_dir))
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, config_dir: &Path) -> Result<(), String> {
        let path = jar_path(config_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        let text = serde_json::to_string(self).map_err(|e| e.to_string())?;
        std::fs::write(&path, text).map_err(|e| e.to_string())?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600));
        }
        Ok(())
    }

    pub fn clear(config_dir: &Path) -> Result<(), String> {
        match std::fs::remove_file(jar_path(config_dir)) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(e.to_string()),
        }
    }

    pub fn domains(&self) -> Vec<DomainCount> {
        domain_counts(&self.cookies)
    }

    /// Merge cookies, replacing ones with the same (domain, path, name).
    pub fn merge(&mut self, source: String, cookies: Vec<Cookie>) {
        for cookie in cookies {
            self.cookies.retain(|c| {
                !(c.domain == cookie.domain && c.path == cookie.path && c.name == cookie.name)
            });
            self.cookies.push(cookie);
        }
        if !self.sources.contains(&source) {
            self.sources.push(source);
        }
        self.imported_at = Some(chrono::Utc::now().to_rfc3339());
    }

    pub fn remove_domain(&mut self, domain: &str) -> usize {
        let before = self.cookies.len();
        let domain = domain.trim_start_matches('.');
        self.cookies.retain(|c| c.bare_domain() != domain);
        before - self.cookies.len()
    }
}

pub fn domain_counts(cookies: &[Cookie]) -> Vec<DomainCount> {
    let mut counts: BTreeMap<String, usize> = BTreeMap::new();
    for cookie in cookies {
        *counts.entry(cookie.bare_domain().to_ascii_lowercase()).or_insert(0) += 1;
    }
    let mut list: Vec<DomainCount> = counts
        .into_iter()
        .map(|(domain, count)| DomainCount { domain, count })
        .collect();
    list.sort_by(|a, b| b.count.cmp(&a.count).then(a.domain.cmp(&b.domain)));
    list
}

/// Keep cookies whose domain is, or is a subdomain of, a selected domain.
pub fn filter_by_domains(cookies: Vec<Cookie>, selected: &[String]) -> Vec<Cookie> {
    let selected: Vec<String> = selected
        .iter()
        .map(|d| d.trim().trim_start_matches('.').to_ascii_lowercase())
        .filter(|d| !d.is_empty())
        .collect();
    cookies
        .into_iter()
        .filter(|cookie| {
            let domain = cookie.bare_domain().to_ascii_lowercase();
            selected
                .iter()
                .any(|s| domain == *s || domain.ends_with(&format!(".{s}")))
        })
        .collect()
}

/// CDP `Storage.setCookies` parameters for one cookie.
pub fn to_cdp_param(cookie: &Cookie) -> serde_json::Value {
    let mut param = serde_json::json!({
        "name": cookie.name,
        "value": cookie.value,
        "path": cookie.path,
        "secure": cookie.secure,
        "httpOnly": cookie.http_only,
    });
    if cookie.domain.starts_with('.') {
        param["domain"] = serde_json::json!(cookie.domain);
    } else {
        // Host-only: a `url` keeps it host-only, and is required for
        // `__Host-` prefixed cookies.
        let scheme = if cookie.secure { "https" } else { "http" };
        param["url"] = serde_json::json!(format!("{scheme}://{}{}", cookie.domain, cookie.path));
    }
    if let Some(expires) = cookie.expires {
        param["expires"] = serde_json::json!(expires);
    }
    // `None` requires Secure; browsers reject it otherwise.
    match cookie.same_site.as_deref() {
        Some("Strict") => param["sameSite"] = serde_json::json!("Strict"),
        Some("Lax") => param["sameSite"] = serde_json::json!("Lax"),
        Some("None") if cookie.secure => param["sameSite"] = serde_json::json!("None"),
        _ => {}
    }
    param
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cookie(domain: &str, name: &str) -> Cookie {
        Cookie {
            name: name.to_string(),
            value: "v".to_string(),
            domain: domain.to_string(),
            path: "/".to_string(),
            expires: None,
            secure: true,
            http_only: false,
            same_site: Some("Lax".to_string()),
        }
    }

    #[test]
    fn reads_firefox_cookie_database() {
        let dir = tempfile::tempdir().unwrap();
        let conn = Connection::open(dir.path().join("cookies.sqlite")).unwrap();
        conn.execute_batch(
            "CREATE TABLE moz_cookies (name TEXT, value TEXT, host TEXT, path TEXT, expiry INTEGER, isSecure INTEGER, isHttpOnly INTEGER, sameSite INTEGER);
             INSERT INTO moz_cookies VALUES ('sid','abc','.example.com','/',4102444800,1,1,2);
             INSERT INTO moz_cookies VALUES ('gone','x','.example.com','/',1000,0,0,0);",
        )
        .unwrap();
        drop(conn);

        let profile = ProfileInfo {
            id: "firefox:t".into(),
            browser: BrowserKind::Firefox,
            browser_label: "Firefox",
            name: "t".into(),
            path: dir.path().to_path_buf(),
            root: dir.path().to_path_buf(),
        };
        let outcome = read_cookies(&profile).unwrap();

        assert_eq!(outcome.cookies.len(), 1, "expired cookie is dropped");
        let sid = &outcome.cookies[0];
        assert_eq!((sid.name.as_str(), sid.value.as_str()), ("sid", "abc"));
        assert!(sid.secure && sid.http_only);
        assert_eq!(sid.same_site.as_deref(), Some("Strict"));
    }

    #[test]
    fn decrypts_v10_values_and_strips_host_hash() {
        let key = [7u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [1u8; 12];

        let mut plain = Sha256::digest(b".example.com").to_vec();
        plain.extend_from_slice(b"secret-token");
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&nonce);
        blob.extend(cipher.encrypt(Nonce::from_slice(&nonce), plain.as_slice()).unwrap());

        let key_vec = key.to_vec();
        let value = decrypt_chromium_value(Ok(&key_vec), &blob, ".example.com").unwrap();
        assert_eq!(value, "secret-token");
    }

    #[test]
    fn app_bound_cookies_are_skipped_not_bypassed() {
        let key = vec![0u8; 32];
        let error = decrypt_chromium_value(Ok(&key), b"v20xxxxxxxxxxxxxxxxxxxxxxxxxxxxxxxx", "h").unwrap_err();
        assert!(error.contains("app-bound"));
    }

    #[test]
    fn wrong_key_fails_cleanly() {
        let key = [7u8; 32];
        let cipher = Aes256Gcm::new_from_slice(&key).unwrap();
        let nonce = [2u8; 12];
        let mut blob = b"v10".to_vec();
        blob.extend_from_slice(&nonce);
        blob.extend(cipher.encrypt(Nonce::from_slice(&nonce), b"x".as_slice()).unwrap());

        let wrong = vec![9u8; 32];
        assert!(decrypt_chromium_value(Ok(&wrong), &blob, "h").is_err());
    }

    #[test]
    fn domain_filter_matches_subdomains_only() {
        let all = vec![
            cookie(".github.com", "a"),
            cookie("api.github.com", "b"),
            cookie(".notgithub.com", "c"),
            cookie(".example.org", "d"),
        ];
        let kept = filter_by_domains(all, &["github.com".to_string()]);
        let names: Vec<_> = kept.iter().map(|c| c.name.as_str()).collect();
        assert_eq!(names, ["a", "b"]);
    }

    #[test]
    fn counts_group_by_bare_domain() {
        let counts = domain_counts(&[cookie(".a.com", "1"), cookie("a.com", "2"), cookie(".b.com", "3")]);
        assert_eq!((counts[0].domain.as_str(), counts[0].count), ("a.com", 2));
    }

    #[test]
    fn jar_roundtrip_merge_and_remove() {
        let dir = tempfile::tempdir().unwrap();
        let mut jar = CookieJar::load(dir.path());
        jar.merge("Firefox (x)".into(), vec![cookie(".a.com", "s"), cookie(".b.com", "t")]);
        let mut updated = cookie(".a.com", "s");
        updated.value = "new".into();
        jar.merge("Firefox (x)".into(), vec![updated]);
        jar.save(dir.path()).unwrap();

        let mut loaded = CookieJar::load(dir.path());
        assert_eq!(loaded.cookies.len(), 2);
        assert_eq!(loaded.sources.len(), 1);
        assert_eq!(loaded.remove_domain("a.com"), 1);
        CookieJar::clear(dir.path()).unwrap();
        assert!(CookieJar::load(dir.path()).cookies.is_empty());
    }

    #[test]
    fn cdp_param_keeps_host_only_cookies_host_only() {
        let host_only = to_cdp_param(&cookie("example.com", "h"));
        assert!(host_only.get("domain").is_none());
        assert_eq!(host_only["url"], "https://example.com/");

        let domain = to_cdp_param(&cookie(".example.com", "d"));
        assert_eq!(domain["domain"], ".example.com");

        let mut insecure = cookie("example.com", "n");
        insecure.secure = false;
        insecure.same_site = Some("None".into());
        assert!(to_cdp_param(&insecure).get("sameSite").is_none());
    }
}
