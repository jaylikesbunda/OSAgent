//! Sandboxed, stateful browser the agent can drive and see.
//!
//! One `browser` tool with an `action` field. Each OSA session owns an
//! isolated headless Chromium (throwaway profile, loopback-only debugging,
//! egress policy) that is reaped when idle. Pages are observed through an
//! accessibility snapshot with stable `[ref=eN]` handles; screenshots are
//! opt-in because they cost vision tokens.

pub mod cdp;
pub mod import;
pub mod input;
pub mod policy;
pub mod sandbox;
pub mod session;
pub mod snapshot;

use crate::config::{BrowserConfig, Config};
use crate::error::{OSAgentError, Result};
use crate::tools::registry::{Tool, ToolAttachment, ToolExample, ToolResult};
use async_trait::async_trait;
use serde_json::{json, Value};
use session::BrowserSession;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

struct Entry {
    session: Arc<Mutex<BrowserSession>>,
    last_used: Arc<StdMutex<Instant>>,
    /// Cookie-jar modification time when this browser was launched. A newer
    /// jar (the user imported sessions) means the browser must restart to load them.
    jar_stamp: Option<std::time::SystemTime>,
}

/// Owns every live browser, keyed by OSA session id.
pub struct BrowserManager {
    config: BrowserConfig,
    config_dir: PathBuf,
    data_root: PathBuf,
    sessions: Mutex<HashMap<String, Entry>>,
    reaper_started: AtomicBool,
    cleaned_stale: AtomicBool,
}

impl BrowserManager {
    pub fn new(config: &Config) -> Arc<Self> {
        Arc::new(Self {
            config: config.browser.clone(),
            config_dir: config.config_dir(),
            data_root: config.config_dir().join("browser"),
            sessions: Mutex::new(HashMap::new()),
            reaper_started: AtomicBool::new(false),
            cleaned_stale: AtomicBool::new(false),
        })
    }

    pub fn config(&self) -> &BrowserConfig {
        &self.config
    }

    fn jar_stamp(&self) -> Option<std::time::SystemTime> {
        std::fs::metadata(import::jar_path(&self.config_dir))
            .and_then(|meta| meta.modified())
            .ok()
    }

    async fn session_for(
        self: &Arc<Self>,
        id: &str,
    ) -> std::result::Result<(Arc<Mutex<BrowserSession>>, Arc<StdMutex<Instant>>), String> {
        let mut sessions = self.sessions.lock().await;
        let stamp = self.jar_stamp();

        if let Some(entry) = sessions.get(id) {
            let alive = match entry.session.try_lock() {
                Ok(session) => session.is_alive(),
                // Busy means another call is using it, so it is alive.
                Err(_) => true,
            };
            let stale_jar = entry.jar_stamp != stamp && self.config.import_cookies;
            if alive && !stale_jar {
                *entry.last_used.lock().expect("last_used lock") = Instant::now();
                return Ok((entry.session.clone(), entry.last_used.clone()));
            }
            // Died, or launched before the cookie jar changed: relaunch below.
            if let Some(entry) = sessions.remove(id) {
                close_entry(entry).await;
            }
        }

        let executable = sandbox::find_browser(&self.config.executable_path)
            .ok_or_else(|| sandbox::INSTALL_HINT.to_string())?;

        if !self.cleaned_stale.swap(true, Ordering::SeqCst) {
            sandbox::cleanup_stale_profiles(&self.data_root);
        }

        // Make room: close the least recently used browser.
        let max = self.config.max_concurrent_sessions.max(1);
        while sessions.len() >= max {
            let oldest = sessions
                .iter()
                .min_by_key(|(_, entry)| *entry.last_used.lock().expect("last_used lock"))
                .map(|(key, _)| key.clone());
            let Some(key) = oldest else { break };
            if let Some(entry) = sessions.remove(&key) {
                close_entry(entry).await;
            }
        }

        let profile_dir = self
            .data_root
            .join(uuid::Uuid::new_v4().to_string())
            .join("profile");
        let cookies = if self.config.import_cookies {
            import::CookieJar::load(&self.config_dir).cookies
        } else {
            Vec::new()
        };
        let session =
            BrowserSession::launch(&self.config, &executable, profile_dir, cookies).await?;
        let session = Arc::new(Mutex::new(session));
        let last_used = Arc::new(StdMutex::new(Instant::now()));
        sessions.insert(
            id.to_string(),
            Entry {
                session: session.clone(),
                last_used: last_used.clone(),
                jar_stamp: stamp,
            },
        );
        drop(sessions);
        self.start_reaper();
        Ok((session, last_used))
    }

    pub async fn close(&self, id: &str) -> bool {
        let entry = self.sessions.lock().await.remove(id);
        match entry {
            Some(entry) => {
                close_entry(entry).await;
                true
            }
            None => false,
        }
    }

    pub async fn close_all(&self) {
        let entries: Vec<Entry> = self.sessions.lock().await.drain().map(|(_, e)| e).collect();
        for entry in entries {
            close_entry(entry).await;
        }
    }

    fn start_reaper(self: &Arc<Self>) {
        if self.reaper_started.swap(true, Ordering::SeqCst) {
            return;
        }
        let weak = Arc::downgrade(self);
        let idle = Duration::from_secs(self.config.idle_timeout_seconds.max(30));
        tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(15)).await;
                let Some(manager) = weak.upgrade() else { return };
                let expired: Vec<String> = {
                    let sessions = manager.sessions.lock().await;
                    sessions
                        .iter()
                        .filter(|(_, entry)| {
                            entry.last_used.lock().expect("last_used lock").elapsed() > idle
                                && entry.session.try_lock().is_ok()
                        })
                        .map(|(key, _)| key.clone())
                        .collect()
                };
                for key in expired {
                    manager.close(&key).await;
                }
            }
        });
    }
}

async fn close_entry(entry: Entry) {
    // If a call still holds the session, dropping the last Arc kills the
    // browser through `BrowserProcess::drop`.
    if let Ok(mutex) = Arc::try_unwrap(entry.session) {
        mutex.into_inner().close().await;
    }
}

pub struct BrowserTool {
    manager: Arc<BrowserManager>,
}

impl BrowserTool {
    pub fn new(manager: Arc<BrowserManager>) -> Self {
        Self { manager }
    }

    fn fail(message: impl Into<String>) -> OSAgentError {
        OSAgentError::ToolExecution(message.into())
    }

    async fn run(&self, args: &Value) -> Result<ToolResult> {
        let action = args["action"]
            .as_str()
            .ok_or_else(|| Self::fail("`action` is required"))?
            .to_ascii_lowercase();
        let session_id = args["session_id"].as_str().unwrap_or("default").to_string();

        if action == "close" {
            let closed = self.manager.close(&session_id).await;
            return Ok(ToolResult::new(if closed {
                "Browser closed."
            } else {
                "No browser was open."
            }));
        }

        let (session, last_used) = self
            .manager
            .session_for(&session_id)
            .await
            .map_err(Self::fail)?;
        let mut session = session.lock().await;
        let result = self.dispatch(&mut session, &action, args).await;
        *last_used.lock().expect("last_used lock") = Instant::now();
        result
    }

    async fn dispatch(
        &self,
        session: &mut BrowserSession,
        action: &str,
        args: &Value,
    ) -> Result<ToolResult> {
        let config = self.manager.config();
        let text = |key: &str| args[key].as_str().map(str::trim).filter(|v| !v.is_empty());
        let step = |error: String| Self::fail(error);

        match action {
            "open" | "navigate" | "goto" => {
                if args["new_tab"].as_bool().unwrap_or(false) {
                    session.new_tab(None).await.map_err(step)?;
                }
                match text("url") {
                    Some(url) => session.navigate(url).await.map_err(step)?,
                    None if action != "open" => return Err(Self::fail("`url` is required")),
                    None => {}
                }
                self.report(session, true, None, None).await
            }
            "back" | "forward" => {
                let delta = if action == "back" { -1 } else { 1 };
                let moved = session.history(delta).await.map_err(step)?;
                let note = (!moved).then(|| format!("no {action} history entry"));
                self.report(session, true, None, note).await
            }
            "reload" => {
                session.reload().await.map_err(step)?;
                self.report(session, true, None, None).await
            }
            "snapshot" => {
                let interactive = text("filter") == Some("interactive");
                session.snapshot(interactive).await.map_err(step)?;
                let page = args["page"].as_u64().unwrap_or(1).max(1) as usize;
                self.format_state(session, &[], true, page).await
            }
            "read" => {
                let max = args["max_chars"].as_u64().unwrap_or(12_000).clamp(500, 100_000) as usize;
                let body = session.read_text(max).await.map_err(step)?;
                let (url, title) = session.page_state().await.map_err(step)?;
                Ok(ToolResult::new(format!(
                    "URL: {url}\nTitle: {title}\n\n{body}"
                )))
            }
            "screenshot" => {
                let full = args["full_page"].as_bool().unwrap_or(false);
                let annotate = args["annotate"].as_bool().unwrap_or(false);
                let (shot, legend) = session.screenshot(full, annotate).await.map_err(step)?;
                let (url, title) = session.page_state().await.map_err(step)?;
                let mut out = format!("URL: {url}\nTitle: {title}\nScreenshot attached.");
                if let Some(legend) = legend.filter(|l| !l.is_empty()) {
                    out.push_str(&format!("\nLabelled elements:\n{legend}"));
                }
                let attachment = ToolAttachment {
                    filename: format!("screenshot.{}", shot.extension),
                    mime: shot.mime.to_string(),
                    data_url: format!("data:{};base64,{}", shot.mime, shot.base64),
                };
                let mut result = ToolResult::new(out);
                // The chat shows the same image in the tool's expanded card.
                result.metadata = json!({ "screenshots": [&attachment] });
                result.attachments.push(attachment);
                Ok(result)
            }
            "find" => {
                let needle = text("text").ok_or_else(|| Self::fail("`text` is required"))?;
                let snap = session.snapshot(false).await.map_err(step)?.clone();
                let matches = snapshot::find_ref(&snap, needle);
                if matches.is_empty() {
                    let literal = serde_json::to_string(&needle.to_lowercase()).unwrap_or_default();
                    let present = session
                        .evaluate(&format!(
                            "(document.body?document.body.innerText:'').toLowerCase().includes({literal})"
                        ))
                        .await
                        .ok()
                        .and_then(|v| v.as_bool())
                        .unwrap_or(false);
                    return Ok(ToolResult::new(if present {
                        format!("\"{needle}\" appears on the page but not in an interactive element. Use `read` or `scroll`.")
                    } else {
                        format!("No element matches \"{needle}\". It may be below the fold; try `scroll`.")
                    }));
                }
                let lines: Vec<String> = matches
                    .iter()
                    .take(15)
                    .map(|(id, entry)| format!("[ref={id}] {} \"{}\"", entry.role, entry.name))
                    .collect();
                Ok(ToolResult::new(lines.join("\n")))
            }
            "type" if text("ref").is_none() && text("selector").is_none() => {
                let before = session.last_fingerprint();
                let value = args["text"]
                    .as_str()
                    .ok_or_else(|| Self::fail("`text` is required for type"))?;
                session
                    .type_focused(value, args["submit"].as_bool().unwrap_or(false))
                    .await
                    .map_err(step)?;
                self.report(session, false, before, Some("typed into the focused element".to_string()))
                    .await
            }
            "fill" => {
                let before = session.last_fingerprint();
                let fields = args["fields"]
                    .as_array()
                    .filter(|f| !f.is_empty())
                    .ok_or_else(|| Self::fail("`fields` must be a non-empty list of {ref|selector|text, value}"))?;
                let mut done = Vec::new();
                for (index, field) in fields.iter().enumerate() {
                    let label = field["ref"].as_str().or(field["selector"].as_str()).or(field["text"].as_str()).unwrap_or("?").to_string();
                    let target = session
                        .resolve_target(
                            field["ref"].as_str(),
                            field["selector"].as_str(),
                            field["text"].as_str(),
                        )
                        .await
                        .map_err(|e| Self::fail(format!("field {} ({label}): {e}", index + 1)))?;
                    let outcome = session
                        .fill_field(target, &field["value"])
                        .await
                        .map_err(|e| Self::fail(format!("field {} ({label}): {e}", index + 1)))?;
                    done.push(format!("{label}: {outcome}"));
                }
                if args["submit"].as_bool().unwrap_or(false) {
                    session.press("Enter").await.map_err(step)?;
                    done.push("submitted (Enter)".to_string());
                }
                self.report(session, false, before, Some(done.join("; "))).await
            }
            "wait" => {
                let seconds = args["seconds"].as_f64().unwrap_or(0.0).clamp(0.0, 30.0);
                let conditions = (text("text"), text("text_gone"), text("selector"));
                if conditions == (None, None, None) {
                    tokio::time::sleep(Duration::from_secs_f64(seconds.max(1.0))).await;
                    return self.report(session, true, None, None).await;
                }
                let limit = if seconds > 0.0 { seconds } else { 10.0 };
                let met = session
                    .wait_for(conditions.0, conditions.1, conditions.2, Duration::from_secs_f64(limit))
                    .await
                    .map_err(step)?;
                let note = if met {
                    "wait condition met".to_string()
                } else {
                    format!("timed out after {limit}s; the condition was not met")
                };
                self.report(session, true, None, Some(note)).await
            }
            "click" | "double_click" | "hover" | "type" | "select" => {
                let before = session.last_fingerprint();
                let target = session
                    .resolve_target(text("ref"), text("selector"), if action == "type" { None } else { text("text") })
                    .await
                    .map_err(step)?;
                let mut note = None;
                match action {
                    "click" => session.click(target, false).await.map_err(step)?,
                    "double_click" => session.click(target, true).await.map_err(step)?,
                    "hover" => session.hover(target).await.map_err(step)?,
                    "type" => {
                        let value = args["text"]
                            .as_str()
                            .ok_or_else(|| Self::fail("`text` is required for type"))?;
                        session
                            .type_text(
                                target,
                                value,
                                args["clear"].as_bool().unwrap_or(true),
                                args["submit"].as_bool().unwrap_or(false),
                            )
                            .await
                            .map_err(step)?;
                    }
                    _ => {
                        let value = text("value").ok_or_else(|| Self::fail("`value` is required"))?;
                        note = Some(format!(
                            "selected \"{}\"",
                            session.select_option(target, value).await.map_err(step)?
                        ));
                    }
                }
                self.report(session, false, before, note).await
            }
            "press" => {
                let before = session.last_fingerprint();
                let key = text("key").ok_or_else(|| Self::fail("`key` is required"))?;
                session.press(key).await.map_err(step)?;
                self.report(session, false, before, None).await
            }
            "scroll" => {
                let element = match (text("ref"), text("selector")) {
                    (None, None) => None,
                    (reference, selector) => Some(
                        session
                            .resolve_target(reference, selector, None)
                            .await
                            .map_err(step)?,
                    ),
                };
                let direction = text("direction").unwrap_or("down");
                let summary = session
                    .scroll(direction, args["amount"].as_i64(), element)
                    .await
                    .map_err(step)?;
                // Scrolling keeps refs valid but reveals new content.
                session.snapshot(false).await.map_err(step)?;
                self.format_state(session, &[summary], true, 1).await
            }
            "tabs" => {
                let sub = text("subaction").unwrap_or("list");
                let tab = text("tab");
                match sub {
                    "list" => {}
                    "new" => session.new_tab(text("url")).await.map_err(step)?,
                    "close" => session.close_tab(tab).await.map_err(step)?,
                    "switch" => {
                        let tab = tab.ok_or_else(|| Self::fail("`tab` is required to switch"))?;
                        session.switch_tab(tab).await.map_err(step)?;
                    }
                    other => return Err(Self::fail(format!("unknown tabs subaction `{other}`"))),
                }
                let tabs = session.tabs().await.map_err(step)?;
                let lines: Vec<String> = tabs
                    .iter()
                    .enumerate()
                    .map(|(i, t)| {
                        format!(
                            "{}[{}] {} — {} (id {})",
                            if t.active { "* " } else { "  " },
                            i + 1,
                            if t.title.is_empty() { "(untitled)" } else { &t.title },
                            t.url,
                            t.target_id.chars().take(8).collect::<String>()
                        )
                    })
                    .collect();
                Ok(ToolResult::new(format!(
                    "Tabs (* = active):\n{}",
                    lines.join("\n")
                )))
            }
            "eval" => {
                if !config.allow_javascript_eval {
                    return Err(Self::fail(
                        "`eval` is disabled. The user can enable it with [browser] allow_javascript_eval = true",
                    ));
                }
                let js = text("js").ok_or_else(|| Self::fail("`js` is required"))?;
                let value = session.evaluate(js).await.map_err(step)?;
                let mut out = match value {
                    Value::String(s) => s,
                    other => other.to_string(),
                };
                if out.chars().count() > 10_000 {
                    out = format!("{}… (truncated)", out.chars().take(10_000).collect::<String>());
                }
                Ok(ToolResult::new(out))
            }
            other => Err(Self::fail(format!(
                "unknown action `{other}`. Actions: open, navigate, back, forward, reload, snapshot, read, screenshot, find, click, double_click, hover, type, select, press, scroll, tabs, eval, close"
            ))),
        }
    }

    /// State header plus a fresh snapshot. For interactions, the snapshot
    /// is omitted when the page did not visibly change.
    async fn report(
        &self,
        session: &mut BrowserSession,
        always_snapshot: bool,
        before: Option<u64>,
        note: Option<String>,
    ) -> Result<ToolResult> {
        let tab_note = session.adopt_new_tabs().await;
        let snap_fingerprint = session
            .snapshot(false)
            .await
            .map_err(Self::fail)?
            .fingerprint();
        let unchanged = !always_snapshot && tab_note.is_none() && before == Some(snap_fingerprint);
        let mut extra: Vec<String> = note.into_iter().chain(tab_note).collect();
        if unchanged {
            extra.push("page content unchanged".to_string());
        }
        self.format_state(session, &extra, !unchanged, 1).await
    }

    async fn format_state(
        &self,
        session: &mut BrowserSession,
        extra: &[String],
        include_snapshot: bool,
        page: usize,
    ) -> Result<ToolResult> {
        let (url, title) = session.page_state().await.map_err(Self::fail)?;
        let mut out = format!("URL: {url}\nTitle: {title}\n");
        let mut notes: Vec<String> = extra.to_vec();
        notes.extend(session.take_notes());
        for note in notes {
            out.push_str(&format!("Note: {note}\n"));
        }
        if include_snapshot {
            match session.snapshot_page(page) {
                Some((outline, total)) if !outline.is_empty() => {
                    let shown = page.clamp(1, total);
                    if total > 3 {
                        out.push_str(&format!(
                            "
Page snapshot, part {shown} of {total}. This is a long page: use `read` for the article text, `find` to locate a control, or snapshot with filter=interactive. All refs work even from parts not shown:
"
                        ));
                    } else if total > 1 {
                        out.push_str(&format!(
                            "
Page snapshot, part {shown} of {total} (all refs work, even from parts not shown; call snapshot with page=N for another part, or use find):
"
                        ));
                    } else {
                        out.push_str("
Page snapshot (refs are valid until the page changes):
");
                    }
                    out.push_str(&outline);
                }
                _ => out.push_str("
Page snapshot: (empty page)"),
            }
        }
        Ok(ToolResult::new(out))
    }
}

#[async_trait]
impl Tool for BrowserTool {
    fn name(&self) -> &str {
        "browser"
    }

    fn description(&self) -> &str {
        "Drive a real sandboxed web browser (headless Chromium) to read and interact with JavaScript-heavy or interactive pages. Observe pages through accessibility snapshots with [ref=eN] element handles; act by ref (click, type, select, press, scroll, hover). Keeps state across calls within a session: pages, tabs, logins in that browser. Local and private network addresses are blocked."
    }

    fn when_to_use(&self) -> &str {
        "Pages that need JavaScript, logins, clicking, forms, infinite scroll, or visual inspection; web_fetch returned an empty or blocked page"
    }

    fn when_not_to_use(&self) -> &str {
        "Plain pages and APIs that web_fetch or web_search can read; anything on localhost or a private network"
    }

    fn examples(&self) -> Vec<ToolExample> {
        vec![
            ToolExample {
                description: "open a page".to_string(),
                input: json!({"action": "open", "url": "https://example.com"}),
            },
            ToolExample {
                description: "click a link from the snapshot".to_string(),
                input: json!({"action": "click", "ref": "e3"}),
            },
        ]
    }

    fn parameters(&self) -> Value {
        json!({
            "type": "object",
            "properties": {
                "action": {
                    "type": "string",
                    "enum": ["open","navigate","back","forward","reload","snapshot","read","screenshot",
                             "find","click","double_click","hover","type","fill","select","press","scroll",
                             "wait","tabs","eval","close"],
                    "description": "open/navigate load a URL and return a snapshot. snapshot lists page content with refs. click/type/etc. act on an element by ref (from the latest snapshot), selector, or visible text. screenshot is for visual checks only (costly); prefer snapshot/read."
                },
                "url": {"type": "string", "description": "open/navigate/tabs new"},
                "ref": {"type": "string", "description": "Element ref from the latest snapshot, e.g. e12"},
                "selector": {"type": "string", "description": "CSS selector, when no ref is available"},
                "text": {"type": "string", "description": "click/hover: visible name of the element. type: the text to enter (into ref/selector, or the focused element if none given). find: text to look for. wait: text that must appear."},
                "value": {"type": "string", "description": "select: option value or label"},
                "key": {"type": "string", "description": "press: Enter, Tab, Escape, ArrowDown, a, Control+a, Shift+Tab ..."},
                "direction": {"type": "string", "enum": ["up","down","top","bottom"]},
                "amount": {"type": "integer", "description": "scroll pixels (default ~80% of the viewport)"},
                "submit": {"type": "boolean", "description": "type: press Enter afterwards"},
                "clear": {"type": "boolean", "description": "type: replace existing text (default true)"},
                "filter": {"type": "string", "enum": ["all","interactive"], "description": "snapshot: interactive limits output to actionable elements"},
                "full_page": {"type": "boolean", "description": "screenshot the whole page"},
                "annotate": {"type": "boolean", "description": "screenshot: draw ref labels over interactive elements"},
                "max_chars": {"type": "integer", "description": "read: character limit"},
                "new_tab": {"type": "boolean", "description": "open: use a new tab"},
                "subaction": {"type": "string", "enum": ["list","new","close","switch"], "description": "tabs"},
                "tab": {"type": "string", "description": "tabs close/switch: tab number or id from the list"},
                "page": {"type": "integer", "description": "snapshot: which part of a long outline to show (default 1). Refs from every part are usable."},
                "fields": {"type": "array", "description": "fill: form fields to set in one call", "items": {"type": "object", "properties": {"ref": {"type": "string"}, "selector": {"type": "string"}, "text": {"type": "string", "description": "visible label of the field"}, "value": {"description": "text, option label, or true/false for checkboxes"}}, "required": ["value"]}},
                "seconds": {"type": "number", "description": "wait: seconds to wait (max 30). With text/text_gone/selector it is the timeout."},
                "text_gone": {"type": "string", "description": "wait: text that must disappear"},
                "js": {"type": "string", "description": "eval: JavaScript expression (only if enabled by the user)"}
            },
            "required": ["action"]
        })
    }

    fn timeout_ms(&self) -> Option<u64> {
        Some(120_000)
    }

    async fn execute(&self, args: Value) -> Result<String> {
        self.run(&args).await.map(|result| result.output)
    }

    async fn execute_result(&self, args: Value) -> Result<ToolResult> {
        self.run(&args).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool() -> BrowserTool {
        BrowserTool::new(BrowserManager::new(&Config::default_config()))
    }

    #[test]
    fn schema_requires_action_and_lists_every_dispatched_action() {
        let schema = tool().parameters();
        assert_eq!(schema["required"][0], "action");
        let actions: Vec<&str> = schema["properties"]["action"]["enum"]
            .as_array()
            .unwrap()
            .iter()
            .filter_map(Value::as_str)
            .collect();
        for expected in ["open", "click", "type", "screenshot", "tabs", "close", "eval"] {
            assert!(actions.contains(&expected), "{expected} missing");
        }
    }

    #[tokio::test]
    async fn missing_action_is_rejected() {
        let error = tool().execute(json!({})).await.unwrap_err();
        assert!(error.to_string().contains("`action` is required"));
    }

    #[tokio::test]
    async fn close_without_browser_is_harmless() {
        let out = tool()
            .execute(json!({"action": "close", "session_id": "none"}))
            .await
            .unwrap();
        assert_eq!(out, "No browser was open.");
    }

    /// Needs a local Chrome/Edge: `cargo test --lib browser_live -- --ignored`.
    #[tokio::test]
    #[ignore]
    async fn browser_live_end_to_end() {
        let tool = tool();
        let page = "data:text/html,<title>Live</title><h1>Hello</h1><input aria-label=Name><button onclick=\"document.title='clicked '+document.querySelector('input').value\">Go</button>";
        let call = |v: Value| {
            let mut v = v;
            v["session_id"] = json!("live-test");
            v
        };

        let out = tool.execute(call(json!({"action":"open","url":page}))).await.unwrap();
        println!("{out}");
        assert!(out.contains("Title: Live"));
        assert!(out.contains("button \"Go\""));

        let out = tool.execute(call(json!({"action":"type","text":"Name","value":"x","ref":"e1"}))).await;
        println!("{out:?}");
        let out = tool.execute(call(json!({"action":"type","ref":"e1","text":"osa"}))).await.unwrap();
        println!("{out}");
        let out = tool.execute(call(json!({"action":"click","text":"Go"}))).await.unwrap();
        println!("{out}");
        assert!(out.contains("Title: clicked osa"), "{out}");

        let shot = tool.execute_result(call(json!({"action":"screenshot","annotate":true}))).await.unwrap();
        assert_eq!(shot.attachments.len(), 1);
        assert!(shot.attachments[0].data_url.starts_with("data:image/jpeg;base64,"));

        let blocked = tool
            .execute(call(json!({"action":"navigate","url":"http://127.0.0.1:9/"})))
            .await
            .unwrap_err();
        println!("{blocked}");
        assert!(blocked.to_string().contains("private"));

        let out = tool.execute(call(json!({"action":"tabs"}))).await.unwrap();
        println!("{out}");
        tool.execute(call(json!({"action":"close"}))).await.unwrap();
    }

    #[tokio::test]
    #[ignore]
    async fn browser_live_forms_frames_and_long_pages() {
        let tool = tool();
        let call = |v: Value| {
            let mut v = v;
            v["session_id"] = json!("live-test-2");
            v
        };
        let html = "<title>Form</title>            <label>Email <input id=em></label>            <select aria-label=Color><option value=r>Red</option><option value=b>Blue</option></select>            <label><input type=checkbox id=ck> Agree</label>            <button onclick=\"setTimeout(()=>{document.body.append('Saved OK')},700)\">Save</button>            <iframe srcdoc='<input aria-label=Inner id=in>'></iframe>";
        let page = format!("data:text/html,{}", html.replace('#', "%23"));

        let out = tool.execute(call(json!({"action":"open","url":page}))).await.unwrap();
        println!("{out}");
        assert!(out.contains("iframe"), "iframe content is listed");
        assert!(out.contains("textbox \"Inner\""));

        let out = tool.execute(call(json!({"action":"fill","fields":[
            {"text":"Email","value":"a@b.co"},
            {"text":"Color","value":"Blue"},
            {"text":"Agree","value":true},
            {"text":"Inner","value":"in-frame"}
        ]}))).await.unwrap();
        println!("{out}");

        let out = tool.execute(call(json!({"action":"click","text":"Save"}))).await.unwrap();
        println!("{out}");
        let out = tool.execute(call(json!({"action":"wait","text":"Saved OK","seconds":5}))).await.unwrap();
        assert!(out.contains("wait condition met"), "{out}");

        let state = tool
            .execute(call(json!({"action":"eval","js":"1"})))
            .await;
        assert!(state.is_err(), "eval stays gated");

        // Long real page: outline is paged, refs from later parts still work.
        let out = tool
            .execute(call(json!({"action":"open","url":"https://en.wikipedia.org/wiki/Rust_(programming_language)"})))
            .await;
        match out {
            Ok(out) => {
                println!("{}", &out.chars().take(1500).collect::<String>());
                assert!(out.contains("part 1 of"), "long pages are paged");
                let found = tool.execute(call(json!({"action":"find","text":"Cargo"}))).await.unwrap();
                println!("{found}");
                let page2 = tool.execute(call(json!({"action":"snapshot","page":2}))).await.unwrap();
                assert!(page2.contains("part 2 of"));
                if let Some(reference) = found.split("[ref=").nth(1).and_then(|r| r.split(']').next()) {
                    let clicked = tool.execute(call(json!({"action":"click","ref":reference}))).await.unwrap();
                    println!("{}", &clicked.chars().take(300).collect::<String>());
                }
            }
            Err(error) => println!("network test skipped: {error}"),
        }
        tool.execute(call(json!({"action":"close"}))).await.unwrap();
    }

    /// Imported cookies must reach the page: a local server echoes the
    /// Cookie header it receives. Run: `cargo test --lib browser_cookie_injection -- --ignored --nocapture`
    #[tokio::test]
    #[ignore]
    async fn browser_cookie_injection() {
        use crate::tools::browser::import::Cookie;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};

        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let port = listener.local_addr().unwrap().port();
        tokio::spawn(async move {
            loop {
                let Ok((mut socket, _)) = listener.accept().await else { return };
                let mut buf = vec![0u8; 8192];
                let n = socket.read(&mut buf).await.unwrap_or(0);
                let request = String::from_utf8_lossy(&buf[..n]).to_string();
                let cookie_header = request
                    .lines()
                    .find_map(|l| l.strip_prefix("Cookie: ").or_else(|| l.strip_prefix("cookie: ")))
                    .unwrap_or("NONE")
                    .to_string();
                let body = format!("<title>cookies</title><p>{cookie_header}</p>");
                let response = format!(
                    "HTTP/1.1 200 OK
Content-Type: text/html
Content-Length: {}
Connection: close

{}",
                    body.len(),
                    body
                );
                let _ = socket.write_all(response.as_bytes()).await;
            }
        });

        let config = BrowserConfig {
            block_private_network: false,
            ..BrowserConfig::default()
        };
        let exe = sandbox::find_browser("").expect("a Chromium browser is installed");
        let profile = std::env::temp_dir().join(format!("osa-cookie-test-{}", uuid::Uuid::new_v4()));
        let cookie = Cookie {
            name: "osa_test".into(),
            value: "signed-in".into(),
            domain: "127.0.0.1".into(),
            path: "/".into(),
            expires: None,
            secure: false,
            http_only: false,
            same_site: Some("Lax".into()),
        };
        let mut session = session::BrowserSession::launch(&config, &exe, profile, vec![cookie])
            .await
            .expect("launch");
        session
            .navigate(&format!("http://127.0.0.1:{port}/"))
            .await
            .expect("navigate");
        let text = session.read_text(500).await.unwrap();
        println!("page says: {text}");
        println!("notes: {:?}", session.take_notes());
        assert!(text.contains("osa_test=signed-in"), "cookie was not sent: {text}");
        session.close().await;
    }
}
