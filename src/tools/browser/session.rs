//! One live browser: process, DevTools connection, tabs and the last
//! snapshot. All page-level operations live here; `mod.rs` only parses
//! tool arguments and formats results.

use super::cdp::{CdpClient, CdpError, CdpEvent};
use super::input::{self, KeySpec};
use super::policy::EgressPolicy;
use super::sandbox::BrowserProcess;
use super::snapshot::{self, RefEntry, Snapshot};
use crate::config::BrowserConfig;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tokio::sync::{broadcast, Notify};
use tokio::task::JoinHandle;

const MAX_NOTES: usize = 20;
const MAX_ANNOTATED_BOXES: usize = 60;
const MAX_FULL_PAGE_HEIGHT: f64 = 8000.0;

pub type SessionResult<T> = Result<T, String>;

fn cdp_message(error: CdpError) -> String {
    let text = error.to_string();
    if text.contains("No node with given id")
        || text.contains("Could not find node")
        || text.contains("does not belong to the document")
        || text.contains("Node with given id does not belong")
    {
        return "that element is no longer on the page; call `snapshot` for fresh refs".to_string();
    }
    text
}

/// State shared with the background event task.
pub struct SessionShared {
    /// target id -> flattened CDP session id, page targets only.
    targets: Mutex<HashMap<String, String>>,
    notes: Mutex<Vec<String>>,
    attached: Notify,
    policy: EgressPolicy,
    viewport: (u32, u32),
    timeout: Duration,
}

impl SessionShared {
    fn note(&self, message: String) {
        let mut notes = self.notes.lock().expect("notes lock");
        if notes.last() != Some(&message) && notes.len() < MAX_NOTES {
            notes.push(message);
        }
    }
}

pub struct TabInfo {
    pub target_id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
}

pub struct Screenshot {
    pub base64: String,
    pub mime: &'static str,
    pub extension: &'static str,
}

pub struct BrowserSession {
    client: Arc<CdpClient>,
    process: BrowserProcess,
    shared: Arc<SessionShared>,
    config: BrowserConfig,
    active: String,
    snapshot: Snapshot,
    snapshot_target: Option<String>,
    seen_targets: HashSet<String>,
    event_task: JoinHandle<()>,
}

impl BrowserSession {
    pub async fn launch(
        config: &BrowserConfig,
        executable: &Path,
        profile_dir: PathBuf,
        cookies: Vec<super::import::Cookie>,
    ) -> SessionResult<Self> {
        let process = BrowserProcess::launch(executable, config, profile_dir).await?;
        let client = CdpClient::connect(&process.ws_url)
            .await
            .map_err(|error| error.to_string())?;

        let shared = Arc::new(SessionShared {
            targets: Mutex::new(HashMap::new()),
            notes: Mutex::new(Vec::new()),
            attached: Notify::new(),
            policy: EgressPolicy::from_config(config),
            viewport: (config.viewport_width, config.viewport_height),
            timeout: Duration::from_millis(config.action_timeout_ms.max(1000)),
        });

        // Subscribe before enabling auto-attach so the first page's
        // attach event cannot be missed.
        let events = client.subscribe();
        let event_task = tokio::spawn(run_event_loop(client.clone(), shared.clone(), events));

        let timeout = shared.timeout;
        client
            .send(
                "Target.setAutoAttach",
                json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
                None,
                timeout,
            )
            .await
            .map_err(cdp_message)?;
        // Downloads need the user's consent; a research browser never
        // writes files on its own.
        let _ = client
            .send(
                "Browser.setDownloadBehavior",
                json!({"behavior": "deny"}),
                None,
                timeout,
            )
            .await;

        if !cookies.is_empty() {
            let mut restored = 0usize;
            for cookie in &cookies {
                let ok = client
                    .send(
                        "Storage.setCookies",
                        json!({"cookies": [super::import::to_cdp_param(cookie)]}),
                        None,
                        timeout,
                    )
                    .await
                    .is_ok();
                restored += usize::from(ok);
            }
            shared.note(format!(
                "signed-in sessions loaded: {restored} of {} imported cookies",
                cookies.len()
            ));
        }

        let mut session = Self {
            client,
            process,
            shared,
            config: config.clone(),
            active: String::new(),
            snapshot: Snapshot::default(),
            snapshot_target: None,
            seen_targets: HashSet::new(),
            event_task,
        };
        session.active = session.first_page_target().await?;
        session.seen_targets.insert(session.active.clone());
        Ok(session)
    }

    pub fn is_alive(&self) -> bool {
        !self.client.is_closed()
    }

    pub async fn close(mut self) {
        let _ = self
            .client
            .send("Browser.close", json!({}), None, Duration::from_secs(2))
            .await;
        self.event_task.abort();
        self.process.shutdown().await;
    }

    // ------------------------------------------------------------ plumbing

    async fn first_page_target(&self) -> SessionResult<String> {
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let pages = self.page_targets().await?;
            for (target_id, _) in &pages {
                if self.session_for_target(target_id).is_some() {
                    return Ok(target_id.clone());
                }
            }
            if Instant::now() >= deadline {
                // Auto-attach did not pick the initial page up; attach by hand.
                if let Some((target_id, _)) = pages.first() {
                    self.attach(target_id).await?;
                    return Ok(target_id.clone());
                }
                return self.create_target("about:blank").await;
            }
            let _ = tokio::time::timeout(
                Duration::from_millis(100),
                self.shared.attached.notified(),
            )
            .await;
        }
    }

    async fn page_targets(&self) -> SessionResult<Vec<(String, Value)>> {
        let response = self
            .client
            .send("Target.getTargets", json!({}), None, self.shared.timeout)
            .await
            .map_err(cdp_message)?;
        Ok(response["targetInfos"]
            .as_array()
            .map(|infos| {
                infos
                    .iter()
                    .filter(|info| info["type"] == "page")
                    .filter_map(|info| {
                        Some((info["targetId"].as_str()?.to_string(), info.clone()))
                    })
                    .collect()
            })
            .unwrap_or_default())
    }

    fn session_for_target(&self, target_id: &str) -> Option<String> {
        self.shared
            .targets
            .lock()
            .expect("targets lock")
            .get(target_id)
            .cloned()
    }

    async fn attach(&self, target_id: &str) -> SessionResult<String> {
        let response = self
            .client
            .send(
                "Target.attachToTarget",
                json!({"targetId": target_id, "flatten": true}),
                None,
                self.shared.timeout,
            )
            .await
            .map_err(cdp_message)?;
        let session_id = response["sessionId"]
            .as_str()
            .ok_or("attachToTarget returned no session")?
            .to_string();
        setup_target(&self.client, &self.shared, &session_id, "page", false).await;
        self.shared
            .targets
            .lock()
            .expect("targets lock")
            .insert(target_id.to_string(), session_id.clone());
        Ok(session_id)
    }

    async fn wait_for_session(&self, target_id: &str) -> SessionResult<String> {
        let deadline = Instant::now() + Duration::from_secs(5);
        loop {
            if let Some(session) = self.session_for_target(target_id) {
                return Ok(session);
            }
            if Instant::now() >= deadline {
                return self.attach(target_id).await;
            }
            let _ = tokio::time::timeout(
                Duration::from_millis(100),
                self.shared.attached.notified(),
            )
            .await;
        }
    }

    async fn create_target(&self, url: &str) -> SessionResult<String> {
        let response = self
            .client
            .send(
                "Target.createTarget",
                json!({"url": url}),
                None,
                self.shared.timeout,
            )
            .await
            .map_err(cdp_message)?;
        let target_id = response["targetId"]
            .as_str()
            .ok_or("createTarget returned no target")?
            .to_string();
        self.wait_for_session(&target_id).await?;
        Ok(target_id)
    }

    fn sid(&self) -> SessionResult<String> {
        self.session_for_target(&self.active)
            .ok_or_else(|| "the active tab is gone; use `tabs` to pick another".to_string())
    }

    async fn page_cmd(&self, method: &str, params: Value) -> SessionResult<Value> {
        let sid = self.sid()?;
        self.client
            .send(method, params, Some(&sid), self.shared.timeout)
            .await
            .map_err(cdp_message)
    }

    async fn page_cmd_slow(&self, method: &str, params: Value) -> SessionResult<Value> {
        let sid = self.sid()?;
        let timeout = Duration::from_millis(self.config.navigation_timeout_ms.max(1000));
        self.client
            .send(method, params, Some(&sid), timeout)
            .await
            .map_err(cdp_message)
    }

    pub fn take_notes(&self) -> Vec<String> {
        std::mem::take(&mut *self.shared.notes.lock().expect("notes lock"))
    }

    pub async fn page_state(&self) -> SessionResult<(String, String)> {
        let response = self
            .client
            .send(
                "Target.getTargetInfo",
                json!({"targetId": self.active}),
                None,
                self.shared.timeout,
            )
            .await
            .map_err(cdp_message)?;
        let info = &response["targetInfo"];
        Ok((
            info["url"].as_str().unwrap_or("").to_string(),
            info["title"].as_str().unwrap_or("").to_string(),
        ))
    }

    fn invalidate_snapshot(&mut self) {
        self.snapshot = Snapshot::default();
        self.snapshot_target = None;
    }

    // ---------------------------------------------------------- navigation

    pub async fn navigate(&mut self, url: &str) -> SessionResult<()> {
        let url = normalize_url(url)?;
        self.shared.policy.check(&url).await?;
        let sid = self.sid()?;
        let mut events = self.client.subscribe();
        let response = self.page_cmd("Page.navigate", json!({"url": url})).await?;
        self.invalidate_snapshot();
        if let Some(error) = response["errorText"].as_str() {
            let blocked = self.take_notes();
            let detail = if blocked.is_empty() {
                String::new()
            } else {
                format!(" ({})", blocked.join("; "))
            };
            return Err(format!("navigation to {url} failed: {error}{detail}"));
        }
        // Same-document navigations (hash changes) carry no loaderId and
        // never fire a load event.
        if response.get("loaderId").is_some() {
            self.wait_for_load(&sid, &mut events).await;
        }
        Ok(())
    }

    pub async fn history(&mut self, delta: i64) -> SessionResult<bool> {
        let sid = self.sid()?;
        let history = self
            .page_cmd("Page.getNavigationHistory", json!({}))
            .await?;
        let current = history["currentIndex"].as_i64().unwrap_or(0);
        let entries = history["entries"].as_array().cloned().unwrap_or_default();
        let target = current + delta;
        let Some(entry) = usize::try_from(target).ok().and_then(|i| entries.get(i)) else {
            return Ok(false);
        };
        let mut events = self.client.subscribe();
        self.page_cmd(
            "Page.navigateToHistoryEntry",
            json!({"entryId": entry["id"]}),
        )
        .await?;
        self.invalidate_snapshot();
        self.wait_for_load(&sid, &mut events).await;
        Ok(true)
    }

    pub async fn reload(&mut self) -> SessionResult<()> {
        let sid = self.sid()?;
        let mut events = self.client.subscribe();
        self.page_cmd("Page.reload", json!({})).await?;
        self.invalidate_snapshot();
        self.wait_for_load(&sid, &mut events).await;
        Ok(())
    }

    /// Wait for the page's load event, bounded by the navigation timeout.
    async fn wait_for_load(&self, sid: &str, events: &mut broadcast::Receiver<CdpEvent>) -> bool {
        let deadline = Duration::from_millis(self.config.navigation_timeout_ms.max(1000));
        let wait = async {
            loop {
                match events.recv().await {
                    Ok(event)
                        if event.method == "Page.loadEventFired"
                            && event.session_id.as_deref() == Some(sid) =>
                    {
                        return true;
                    }
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => return false,
                }
            }
        };
        let loaded = tokio::time::timeout(deadline, wait).await.unwrap_or(false);
        if !loaded {
            self.shared
                .note("page did not finish loading before the timeout; showing what is there".to_string());
        }
        // Let late scripts render before the caller reads the page.
        tokio::time::sleep(Duration::from_millis(250)).await;
        loaded
    }

    /// After an interaction: if it started a navigation, wait for it.
    async fn settle(&self, sid: &str, mut events: broadcast::Receiver<CdpEvent>) {
        let window = tokio::time::sleep(Duration::from_millis(600));
        tokio::pin!(window);
        let started = loop {
            tokio::select! {
                _ = &mut window => break false,
                event = events.recv() => match event {
                    Ok(event) if event.session_id.as_deref() == Some(sid)
                        && event.method == "Page.frameStartedLoading" => break true,
                    Ok(_) | Err(broadcast::error::RecvError::Lagged(_)) => {}
                    Err(broadcast::error::RecvError::Closed) => break false,
                }
            }
        };
        if started {
            self.wait_for_load(sid, &mut events).await;
        } else {
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
    }

    // ------------------------------------------------------------ snapshot

    pub async fn snapshot(&mut self, interactive_only: bool) -> SessionResult<&Snapshot> {
        let response = self
            .page_cmd("Accessibility.getFullAXTree", json!({}))
            .await?;
        let nodes = response["nodes"].as_array().cloned().unwrap_or_default();
        let mut snap = snapshot::render(&nodes, interactive_only, 0);

        // Same-process iframes are separate AX trees; fold them in so
        // embedded forms and widgets are reachable. Cross-process frames
        // are not (they need their own debugging session).
        if let Ok(tree) = self.page_cmd("Page.getFrameTree", json!({})).await {
            let mut frames = Vec::new();
            collect_child_frames(&tree["frameTree"], &mut frames, 0);
            for (frame_id, label) in frames.into_iter().take(8) {
                let Ok(response) = self
                    .page_cmd("Accessibility.getFullAXTree", json!({"frameId": frame_id}))
                    .await
                else {
                    continue;
                };
                let nodes = response["nodes"].as_array().cloned().unwrap_or_default();
                let inner = snapshot::render(&nodes, interactive_only, snap.refs.len());
                snap.extend_frame(format!("iframe {label}:"), inner);
            }
        }

        self.snapshot = snap;
        self.snapshot_target = Some(self.active.clone());
        Ok(&self.snapshot)
    }

    /// One page of the last snapshot's outline and the total page count.
    pub fn snapshot_page(&self, page: usize) -> Option<(String, usize)> {
        self.snapshot_target
            .as_ref()
            .map(|_| self.snapshot.page(page, self.config.max_snapshot_chars.max(500)))
    }

    pub fn last_fingerprint(&self) -> Option<u64> {
        self.snapshot_target
            .as_ref()
            .map(|_| self.snapshot.fingerprint())
    }

    pub async fn read_text(&self, max_chars: usize) -> SessionResult<String> {
        let value = self
            .evaluate("document.body ? document.body.innerText : (document.documentElement ? document.documentElement.innerText : '')")
            .await?;
        let text = value.as_str().unwrap_or_default();
        let tidy = text
            .lines()
            .map(str::trim_end)
            .collect::<Vec<_>>()
            .join("\n");
        if tidy.chars().count() > max_chars {
            let cut: String = tidy.chars().take(max_chars).collect();
            Ok(format!("{cut}\n… (truncated at {max_chars} characters)"))
        } else {
            Ok(tidy)
        }
    }

    pub async fn evaluate(&self, expression: &str) -> SessionResult<Value> {
        let response = self
            .page_cmd(
                "Runtime.evaluate",
                json!({
                    "expression": expression,
                    "returnByValue": true,
                    "awaitPromise": true,
                    "timeout": self.config.action_timeout_ms,
                }),
            )
            .await?;
        if let Some(exception) = response.get("exceptionDetails") {
            let message = exception["exception"]["description"]
                .as_str()
                .or_else(|| exception["text"].as_str())
                .unwrap_or("script error");
            return Err(format!("javascript error: {message}"));
        }
        Ok(response["result"]["value"].clone())
    }

    // ---------------------------------------------------------- screenshot

    pub async fn screenshot(&mut self, full_page: bool, annotate: bool) -> SessionResult<(Screenshot, Option<String>)> {
        let mut legend = None;
        if annotate {
            let (text, boxes) = self.annotation_boxes().await?;
            legend = Some(text);
            if !boxes.is_empty() {
                let _ = self.evaluate(&input::overlay_script(&boxes)).await;
            }
        }
        let result = self.capture(full_page).await;
        if annotate {
            let _ = self.evaluate(input::REMOVE_OVERLAY_JS).await;
        }
        Ok((result?, legend))
    }

    async fn annotation_boxes(&mut self) -> SessionResult<(String, Vec<(String, f64, f64, f64, f64)>)> {
        let refs: Vec<(String, RefEntry)> = self.snapshot(true).await?.refs.clone();
        let (width, height) = (
            f64::from(self.shared.viewport.0),
            f64::from(self.shared.viewport.1),
        );
        let mut boxes = Vec::new();
        let mut shown = Vec::new();
        for (id, entry) in refs.iter().take(MAX_ANNOTATED_BOXES * 2) {
            if boxes.len() >= MAX_ANNOTATED_BOXES {
                break;
            }
            let Ok(response) = self
                .page_cmd("DOM.getBoxModel", json!({"backendNodeId": entry.backend_node_id}))
                .await
            else {
                continue;
            };
            let Some(rect) = input::box_model_quad(&response).and_then(|q| input::quad_rect(&q)) else {
                continue;
            };
            let (x, y, w, h) = rect;
            if w < 1.0 || h < 1.0 || x + w < 0.0 || y + h < 0.0 || x > width || y > height {
                continue;
            }
            boxes.push((id.clone(), x, y, w, h));
            shown.push(format!("{id}: {} \"{}\"", entry.role, entry.name));
        }
        Ok((shown.join("\n"), boxes))
    }

    async fn capture(&self, full_page: bool) -> SessionResult<Screenshot> {
        let png = self.config.screenshot_format.eq_ignore_ascii_case("png");
        let mut qualities = vec![self.config.screenshot_quality.clamp(10, 100)];
        for fallback in [50, 30] {
            if fallback < qualities[0] {
                qualities.push(fallback);
            }
        }
        let mut last = None;
        // PNG is lossless, so only a JPEG retry can shrink an oversized capture.
        let attempts: Vec<(bool, u32)> = if png {
            std::iter::once((true, 100))
                .chain(qualities.iter().map(|q| (false, *q)))
                .collect()
        } else {
            qualities.iter().map(|q| (false, *q)).collect()
        };
        for (as_png, quality) in attempts {
            let mut params = json!({
                "format": if as_png { "png" } else { "jpeg" },
                "captureBeyondViewport": full_page,
            });
            if !as_png {
                params["quality"] = json!(quality);
            }
            if full_page {
                let metrics = self.page_cmd("Page.getLayoutMetrics", json!({})).await?;
                let size = metrics
                    .get("cssContentSize")
                    .or_else(|| metrics.get("contentSize"));
                if let Some(size) = size {
                    params["clip"] = json!({
                        "x": 0,
                        "y": 0,
                        "width": size["width"].as_f64().unwrap_or(f64::from(self.shared.viewport.0)),
                        "height": size["height"].as_f64().unwrap_or(f64::from(self.shared.viewport.1)).min(MAX_FULL_PAGE_HEIGHT),
                        "scale": 1,
                    });
                }
            }
            let response = self.page_cmd_slow("Page.captureScreenshot", params).await?;
            let data = response["data"]
                .as_str()
                .ok_or("screenshot returned no data")?
                .to_string();
            let shot = Screenshot {
                mime: if as_png { "image/png" } else { "image/jpeg" },
                extension: if as_png { "png" } else { "jpg" },
                base64: data,
            };
            let bytes = shot.base64.len() / 4 * 3;
            if bytes <= self.config.max_screenshot_bytes {
                return Ok(shot);
            }
            last = Some(shot);
        }
        // Smallest attempt still over budget: better to return it than nothing.
        last.ok_or_else(|| "screenshot failed".to_string())
    }

    // --------------------------------------------------------- interaction

    /// Resolve the model's description of an element to a DOM node.
    pub async fn resolve_target(
        &mut self,
        reference: Option<&str>,
        selector: Option<&str>,
        text: Option<&str>,
    ) -> SessionResult<i64> {
        if let Some(reference) = reference {
            let reference = reference.trim().trim_start_matches("ref=").trim_start_matches('@');
            if self.snapshot_target.as_deref() != Some(self.active.as_str()) {
                return Err(format!(
                    "ref `{reference}` is from an older page; call `snapshot` first"
                ));
            }
            return self
                .snapshot
                .refs
                .iter()
                .find(|(id, _)| id == reference)
                .map(|(_, entry)| entry.backend_node_id)
                .ok_or_else(|| format!("unknown ref `{reference}`; call `snapshot` for current refs"));
        }
        if let Some(selector) = selector {
            let expression = format!(
                "document.querySelector({})",
                serde_json::to_string(selector).unwrap_or_default()
            );
            let response = self
                .page_cmd(
                    "Runtime.evaluate",
                    json!({"expression": expression, "returnByValue": false}),
                )
                .await?;
            let object_id = response["result"]["objectId"]
                .as_str()
                .ok_or_else(|| format!("no element matches selector `{selector}`"))?
                .to_string();
            let described = self
                .page_cmd("DOM.describeNode", json!({"objectId": object_id}))
                .await?;
            return described["node"]["backendNodeId"]
                .as_i64()
                .ok_or_else(|| "could not resolve the selected element".to_string());
        }
        if let Some(text) = text {
            let fresh = self.snapshot(false).await?.clone();
            let matches = snapshot::find_ref(&fresh, text);
            return match matches.as_slice() {
                [] => Err(format!("no interactive element matches \"{text}\"")),
                [(_, entry), ..] => Ok(entry.backend_node_id),
            };
        }
        Err("provide `ref`, `selector` or `text` to identify the element".to_string())
    }

    async fn element_center(&self, backend_node_id: i64) -> SessionResult<(f64, f64)> {
        let _ = self
            .page_cmd(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": backend_node_id}),
            )
            .await;
        let response = self
            .page_cmd("DOM.getBoxModel", json!({"backendNodeId": backend_node_id}))
            .await?;
        let rect = input::box_model_quad(&response)
            .and_then(|quad| input::quad_rect(&quad))
            .filter(|(_, _, w, h)| *w > 0.0 && *h > 0.0)
            .ok_or("the element has no visible area (hidden or collapsed)")?;
        Ok(input::rect_center(rect))
    }

    async fn mouse(&self, kind: &str, x: f64, y: f64, extra: Value) -> SessionResult<()> {
        let mut params = json!({"type": kind, "x": x, "y": y});
        if let (Some(target), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
            for (key, value) in extra {
                target.insert(key.clone(), value.clone());
            }
        }
        self.page_cmd("Input.dispatchMouseEvent", params).await.map(|_| ())
    }

    pub async fn click(&mut self, backend_node_id: i64, double: bool) -> SessionResult<()> {
        let sid = self.sid()?;
        let events = self.client.subscribe();
        let (x, y) = self.element_center(backend_node_id).await?;
        self.mouse("mouseMoved", x, y, json!({})).await?;
        let clicks = if double { 2 } else { 1 };
        for count in 1..=clicks {
            self.mouse(
                "mousePressed",
                x,
                y,
                json!({"button": "left", "buttons": 1, "clickCount": count}),
            )
            .await?;
            self.mouse(
                "mouseReleased",
                x,
                y,
                json!({"button": "left", "buttons": 0, "clickCount": count}),
            )
            .await?;
        }
        self.settle(&sid, events).await;
        Ok(())
    }

    pub async fn hover(&mut self, backend_node_id: i64) -> SessionResult<()> {
        let (x, y) = self.element_center(backend_node_id).await?;
        self.mouse("mouseMoved", x, y, json!({})).await?;
        tokio::time::sleep(Duration::from_millis(200)).await;
        Ok(())
    }

    async fn call_on_node(
        &self,
        backend_node_id: i64,
        function: &str,
        arguments: Value,
    ) -> SessionResult<Value> {
        let resolved = self
            .page_cmd("DOM.resolveNode", json!({"backendNodeId": backend_node_id}))
            .await?;
        let object_id = resolved["object"]["objectId"]
            .as_str()
            .ok_or("could not resolve the element")?
            .to_string();
        let response = self
            .page_cmd(
                "Runtime.callFunctionOn",
                json!({
                    "objectId": object_id,
                    "functionDeclaration": function,
                    "arguments": arguments,
                    "returnByValue": true,
                }),
            )
            .await?;
        if let Some(exception) = response.get("exceptionDetails") {
            return Err(format!(
                "page script failed: {}",
                exception["exception"]["description"]
                    .as_str()
                    .unwrap_or("unknown error")
            ));
        }
        Ok(response["result"]["value"].clone())
    }

    pub async fn type_text(
        &mut self,
        backend_node_id: i64,
        text: &str,
        clear: bool,
        submit: bool,
    ) -> SessionResult<()> {
        let sid = self.sid()?;
        let events = self.client.subscribe();
        self.call_on_node(
            backend_node_id,
            "function(clear){this.scrollIntoView({block:'center'});this.focus();\
             if(clear){if(typeof this.select==='function'){this.select();}\
             else if(this.isContentEditable){const r=document.createRange();r.selectNodeContents(this);\
             const s=getSelection();s.removeAllRanges();s.addRange(r);}}}",
            json!([{"value": clear}]),
        )
        .await?;
        if text.is_empty() {
            if clear {
                self.press_key(&input::parse_key("Delete")?).await?;
            }
        } else {
            self.page_cmd("Input.insertText", json!({"text": text})).await?;
        }
        if submit {
            self.press_key(&input::parse_key("Enter")?).await?;
        }
        self.settle(&sid, events).await;
        Ok(())
    }

    async fn press_key(&self, key: &KeySpec) -> SessionResult<()> {
        let mut down = json!({
            "type": if key.text.is_some() { "keyDown" } else { "rawKeyDown" },
            "key": key.key,
            "code": key.code,
            "windowsVirtualKeyCode": key.vk,
            "modifiers": key.modifiers,
        });
        if let Some(text) = &key.text {
            down["text"] = json!(text);
        }
        self.page_cmd("Input.dispatchKeyEvent", down).await?;
        self.page_cmd(
            "Input.dispatchKeyEvent",
            json!({
                "type": "keyUp",
                "key": key.key,
                "code": key.code,
                "windowsVirtualKeyCode": key.vk,
                "modifiers": key.modifiers,
            }),
        )
        .await?;
        Ok(())
    }

    pub async fn press(&mut self, spec: &str) -> SessionResult<()> {
        let sid = self.sid()?;
        let events = self.client.subscribe();
        let key = input::parse_key(spec)?;
        self.press_key(&key).await?;
        self.settle(&sid, events).await;
        Ok(())
    }

    /// Scroll the page (or an element into view). Returns a short position summary.
    pub async fn scroll(
        &mut self,
        direction: &str,
        amount: Option<i64>,
        element: Option<i64>,
    ) -> SessionResult<String> {
        if let Some(backend_node_id) = element {
            self.page_cmd(
                "DOM.scrollIntoViewIfNeeded",
                json!({"backendNodeId": backend_node_id}),
            )
            .await?;
        } else {
            match direction {
                "top" => {
                    self.evaluate("window.scrollTo(0, 0)").await?;
                }
                "bottom" => {
                    self.evaluate("window.scrollTo(0, document.documentElement.scrollHeight)")
                        .await?;
                }
                "up" | "down" => {
                    let pixels = amount
                        .unwrap_or_else(|| i64::from(self.shared.viewport.1) * 8 / 10)
                        .abs();
                    let delta = if direction == "up" { -pixels } else { pixels };
                    self.mouse(
                        "mouseWheel",
                        f64::from(self.shared.viewport.0) / 2.0,
                        f64::from(self.shared.viewport.1) / 2.0,
                        json!({"deltaX": 0, "deltaY": delta}),
                    )
                    .await?;
                }
                other => return Err(format!("unknown scroll direction `{other}`")),
            }
        }
        tokio::time::sleep(Duration::from_millis(200)).await;
        let position = self
            .evaluate(
                "JSON.stringify([Math.round(scrollY),document.documentElement.scrollHeight,innerHeight])",
            )
            .await
            .ok()
            .and_then(|v| v.as_str().and_then(|s| serde_json::from_str::<Vec<i64>>(s).ok()));
        Ok(match position.as_deref() {
            Some([y, total, view]) => {
                let at_bottom = y + view >= *total - 2;
                format!(
                    "scrolled to y={y} of {total}px{}",
                    if at_bottom { " (bottom of page)" } else if *y == 0 { " (top of page)" } else { "" }
                )
            }
            _ => "scrolled".to_string(),
        })
    }

    pub async fn select_option(&mut self, backend_node_id: i64, value: &str) -> SessionResult<String> {
        let result = self
            .call_on_node(
                backend_node_id,
                "function(v){if(this.tagName!=='SELECT')return 'not-a-select';\
                 const want=String(v).trim().toLowerCase();\
                 const opts=[...this.options];\
                 const hit=opts.find(o=>o.value.toLowerCase()===want)||opts.find(o=>o.text.trim().toLowerCase()===want)||opts.find(o=>o.text.toLowerCase().includes(want));\
                 if(!hit)return 'no-match:'+opts.map(o=>o.text.trim()).slice(0,20).join(' | ');\
                 this.value=hit.value;\
                 this.dispatchEvent(new Event('input',{bubbles:true}));\
                 this.dispatchEvent(new Event('change',{bubbles:true}));\
                 return 'ok:'+hit.text.trim();}",
                json!([{"value": value}]),
            )
            .await?;
        let result = result.as_str().unwrap_or_default();
        if let Some(chosen) = result.strip_prefix("ok:") {
            Ok(chosen.to_string())
        } else if result == "not-a-select" {
            Err("that element is not a <select>; use `click` for custom dropdowns".to_string())
        } else if let Some(options) = result.strip_prefix("no-match:") {
            Err(format!("no option matches \"{value}\". Options: {options}"))
        } else {
            Err("could not select the option".to_string())
        }
    }

    /// Type into whatever currently has focus (canvas apps, widgets that
    /// manage their own focus, or right after a click).
    pub async fn type_focused(&mut self, text: &str, submit: bool) -> SessionResult<()> {
        let sid = self.sid()?;
        let events = self.client.subscribe();
        if !text.is_empty() {
            self.page_cmd("Input.insertText", json!({"text": text})).await?;
        }
        if submit {
            self.press_key(&input::parse_key("Enter")?).await?;
        }
        self.settle(&sid, events).await;
        Ok(())
    }

    /// Set one form control from a loosely typed value: text inputs are
    /// replaced, `<select>` picks by value or label, checkboxes and radios
    /// are toggled to match.
    pub async fn fill_field(&mut self, backend_node_id: i64, value: &Value) -> SessionResult<String> {
        let kind = self
            .call_on_node(
                backend_node_id,
                "function(){const t=this.tagName;if(t==='SELECT')return 'select';                 if(t==='INPUT'&&(this.type==='checkbox'||this.type==='radio'))return 'check:'+this.checked;                 return 'text';}",
                json!([]),
            )
            .await?;
        let kind = kind.as_str().unwrap_or("text");
        let text = match value {
            Value::String(s) => s.clone(),
            other => other.to_string(),
        };
        if kind == "select" {
            let chosen = self.select_option(backend_node_id, &text).await?;
            Ok(format!("selected \"{chosen}\""))
        } else if let Some(current) = kind.strip_prefix("check:") {
            let want = truthy(value);
            if want != (current == "true") {
                self.click(backend_node_id, false).await?;
            }
            Ok(format!("{}", if want { "checked" } else { "unchecked" }))
        } else {
            self.type_text(backend_node_id, &text, true, false).await?;
            Ok("filled".to_string())
        }
    }

    /// Poll until a condition holds. Returns whether it did before `timeout`.
    pub async fn wait_for(
        &self,
        appears: Option<&str>,
        gone: Option<&str>,
        selector: Option<&str>,
        timeout: Duration,
    ) -> SessionResult<bool> {
        let literal = |s: &str| serde_json::to_string(&s.to_lowercase()).unwrap_or_default();
        let body = "(document.body?document.body.innerText:'').toLowerCase()";
        let mut checks = Vec::new();
        if let Some(text) = appears {
            checks.push(format!("{body}.includes({})", literal(text)));
        }
        if let Some(text) = gone {
            checks.push(format!("!{body}.includes({})", literal(text)));
        }
        if let Some(selector) = selector {
            checks.push(format!(
                "!!document.querySelector({})",
                serde_json::to_string(selector).unwrap_or_default()
            ));
        }
        let expression = checks.join(" && ");
        let deadline = Instant::now() + timeout;
        loop {
            if self.evaluate(&expression).await?.as_bool().unwrap_or(false) {
                return Ok(true);
            }
            if Instant::now() >= deadline {
                return Ok(false);
            }
            tokio::time::sleep(Duration::from_millis(250)).await;
        }
    }

    // ---------------------------------------------------------------- tabs

    pub async fn tabs(&self) -> SessionResult<Vec<TabInfo>> {
        Ok(self
            .page_targets()
            .await?
            .into_iter()
            .map(|(target_id, info)| TabInfo {
                active: target_id == self.active,
                title: info["title"].as_str().unwrap_or("").to_string(),
                url: info["url"].as_str().unwrap_or("").to_string(),
                target_id,
            })
            .collect())
    }

    pub async fn new_tab(&mut self, url: Option<&str>) -> SessionResult<()> {
        let target_id = self.create_target("about:blank").await?;
        self.seen_targets.insert(target_id.clone());
        self.active = target_id;
        self.invalidate_snapshot();
        if let Some(url) = url {
            self.navigate(url).await?;
        }
        Ok(())
    }

    async fn find_tab(&self, which: &str) -> SessionResult<String> {
        let tabs = self.tabs().await?;
        let which = which.trim();
        if let Ok(index) = which.parse::<usize>() {
            if let Some(tab) = index.checked_sub(1).and_then(|i| tabs.get(i)) {
                return Ok(tab.target_id.clone());
            }
        }
        tabs.iter()
            .find(|tab| tab.target_id.starts_with(which) && !which.is_empty())
            .map(|tab| tab.target_id.clone())
            .ok_or_else(|| format!("no tab `{which}`; use `tabs` to list them"))
    }

    pub async fn switch_tab(&mut self, which: &str) -> SessionResult<()> {
        let target_id = self.find_tab(which).await?;
        let _ = self
            .client
            .send(
                "Target.activateTarget",
                json!({"targetId": target_id}),
                None,
                self.shared.timeout,
            )
            .await;
        self.wait_for_session(&target_id).await?;
        self.active = target_id;
        self.invalidate_snapshot();
        Ok(())
    }

    pub async fn close_tab(&mut self, which: Option<&str>) -> SessionResult<()> {
        let target_id = match which {
            Some(which) => self.find_tab(which).await?,
            None => self.active.clone(),
        };
        self.client
            .send(
                "Target.closeTarget",
                json!({"targetId": target_id}),
                None,
                self.shared.timeout,
            )
            .await
            .map_err(cdp_message)?;
        self.shared
            .targets
            .lock()
            .expect("targets lock")
            .remove(&target_id);
        self.seen_targets.remove(&target_id);
        if self.active == target_id {
            self.invalidate_snapshot();
            let remaining = self.page_targets().await?;
            self.active = match remaining.first() {
                Some((id, _)) => {
                    self.wait_for_session(id).await?;
                    id.clone()
                }
                None => self.create_target("about:blank").await?,
            };
        }
        Ok(())
    }

    /// Tabs opened since the last check (popups, `target=_blank`). When a
    /// single new tab appears the session follows it, since that is where
    /// the click the model just made took it.
    pub async fn adopt_new_tabs(&mut self) -> Option<String> {
        let current: Vec<String> = self
            .shared
            .targets
            .lock()
            .expect("targets lock")
            .keys()
            .cloned()
            .collect();
        let fresh: Vec<String> = current
            .into_iter()
            .filter(|id| !self.seen_targets.contains(id))
            .collect();
        if fresh.is_empty() {
            return None;
        }
        self.seen_targets.extend(fresh.iter().cloned());
        if fresh.len() == 1 {
            self.active = fresh[0].clone();
            self.invalidate_snapshot();
            // Give the popup a moment to load before the caller snapshots it.
            tokio::time::sleep(Duration::from_millis(500)).await;
            Some("the action opened a new tab; switched to it".to_string())
        } else {
            Some(format!(
                "the action opened {} new tabs; use `tabs` to switch",
                fresh.len()
            ))
        }
    }
}

fn collect_child_frames(node: &Value, out: &mut Vec<(String, String)>, depth: usize) {
    if depth > 4 {
        return;
    }
    for child in node["childFrames"].as_array().into_iter().flatten() {
        let frame = &child["frame"];
        if let Some(id) = frame["id"].as_str() {
            let name = frame["name"].as_str().filter(|n| !n.is_empty());
            let url: String = frame["url"].as_str().unwrap_or("").chars().take(80).collect();
            let label = match name {
                Some(name) => format!("\"{name}\""),
                None => format!("\"{url}\""),
            };
            out.push((id.to_string(), label));
        }
        collect_child_frames(child, out, depth + 1);
    }
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Bool(b) => *b,
        Value::Number(n) => n.as_f64().is_some_and(|n| n != 0.0),
        Value::String(s) => matches!(
            s.trim().to_ascii_lowercase().as_str(),
            "true" | "yes" | "on" | "1" | "checked"
        ),
        _ => false,
    }
}

fn normalize_url(raw: &str) -> SessionResult<String> {
    let trimmed = raw.trim();
    if trimmed.is_empty() {
        return Err("`url` is required".to_string());
    }
    let lowered = trimmed.to_ascii_lowercase();
    if ["javascript:", "file:", "chrome:", "view-source:", "devtools:"]
        .iter()
        .any(|scheme| lowered.starts_with(scheme))
    {
        return Err("only http(s) URLs can be opened".to_string());
    }
    if trimmed.contains("://") || lowered.starts_with("about:") || lowered.starts_with("data:") {
        return Ok(trimmed.to_string());
    }
    Ok(format!("https://{trimmed}"))
}

async fn setup_target(
    client: &Arc<CdpClient>,
    shared: &Arc<SessionShared>,
    session_id: &str,
    target_type: &str,
    waiting_for_debugger: bool,
) {
    let send = |method: &'static str, params: Value| {
        let client = client.clone();
        let session_id = session_id.to_string();
        let timeout = shared.timeout;
        async move {
            let _ = client.send(method, params, Some(&session_id), timeout).await;
        }
    };

    if matches!(target_type, "page" | "iframe") {
        send(
            "Target.setAutoAttach",
            json!({"autoAttach": true, "waitForDebuggerOnStart": true, "flatten": true}),
        )
        .await;
    }
    if shared.policy.is_active() {
        send("Fetch.enable", json!({"patterns": [{"urlPattern": "*"}]})).await;
    }
    if target_type == "page" {
        send("Page.enable", json!({})).await;
        send(
            "Emulation.setDeviceMetricsOverride",
            json!({
                "width": shared.viewport.0,
                "height": shared.viewport.1,
                "deviceScaleFactor": 1,
                "mobile": false,
            }),
        )
        .await;
    }
    if waiting_for_debugger {
        send("Runtime.runIfWaitingForDebugger", json!({})).await;
    }
}

async fn run_event_loop(
    client: Arc<CdpClient>,
    shared: Arc<SessionShared>,
    mut events: broadcast::Receiver<CdpEvent>,
) {
    loop {
        let event = match events.recv().await {
            Ok(event) => event,
            Err(broadcast::error::RecvError::Lagged(_)) => continue,
            Err(broadcast::error::RecvError::Closed) => return,
        };
        match event.method.as_str() {
            "Target.attachedToTarget" => {
                let client = client.clone();
                let shared = shared.clone();
                tokio::spawn(async move {
                    let params = &event.params;
                    let Some(session_id) = params["sessionId"].as_str() else {
                        return;
                    };
                    let target_type = params["targetInfo"]["type"].as_str().unwrap_or("other");
                    let waiting = params["waitingForDebugger"].as_bool().unwrap_or(false);
                    setup_target(&client, &shared, session_id, target_type, waiting).await;
                    if target_type == "page" {
                        if let Some(target_id) = params["targetInfo"]["targetId"].as_str() {
                            shared
                                .targets
                                .lock()
                                .expect("targets lock")
                                .insert(target_id.to_string(), session_id.to_string());
                            shared.attached.notify_waiters();
                        }
                    }
                });
            }
            "Target.detachedFromTarget" => {
                if let Some(session_id) = event.params["sessionId"].as_str() {
                    shared
                        .targets
                        .lock()
                        .expect("targets lock")
                        .retain(|_, value| value != session_id);
                }
            }
            "Fetch.requestPaused" => {
                let client = client.clone();
                let shared = shared.clone();
                tokio::spawn(async move {
                    let (Some(request_id), Some(url)) = (
                        event.params["requestId"].as_str(),
                        event.params["request"]["url"].as_str(),
                    ) else {
                        return;
                    };
                    let session = event.session_id.as_deref();
                    match shared.policy.check(url).await {
                        Ok(()) => {
                            let _ = client
                                .send(
                                    "Fetch.continueRequest",
                                    json!({"requestId": request_id}),
                                    session,
                                    shared.timeout,
                                )
                                .await;
                        }
                        Err(reason) => {
                            shared.note(format!("blocked request to {url}: {reason}"));
                            let _ = client
                                .send(
                                    "Fetch.failRequest",
                                    json!({"requestId": request_id, "errorReason": "BlockedByClient"}),
                                    session,
                                    shared.timeout,
                                )
                                .await;
                        }
                    }
                });
            }
            "Page.javascriptDialogOpening" => {
                let client = client.clone();
                let shared = shared.clone();
                tokio::spawn(async move {
                    let kind = event.params["type"].as_str().unwrap_or("alert").to_string();
                    let message = event.params["message"].as_str().unwrap_or("").to_string();
                    // Alerts only inform and beforeunload would otherwise
                    // trap navigation; confirm/prompt are declined rather
                    // than answered on the user's behalf.
                    let accept = matches!(kind.as_str(), "alert" | "beforeunload");
                    let _ = client
                        .send(
                            "Page.handleJavaScriptDialog",
                            json!({"accept": accept}),
                            event.session_id.as_deref(),
                            shared.timeout,
                        )
                        .await;
                    let verb = if accept { "accepted" } else { "dismissed" };
                    shared.note(format!("javascript {kind} dialog {verb}: {message}"));
                });
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn normalize_adds_https_and_rejects_script_urls() {
        assert_eq!(normalize_url("example.com/a").unwrap(), "https://example.com/a");
        assert_eq!(normalize_url("http://x.test").unwrap(), "http://x.test");
        assert_eq!(normalize_url("about:blank").unwrap(), "about:blank");
        assert!(normalize_url("javascript:alert(1)").is_err());
        assert!(normalize_url("file:///etc/passwd").is_err());
        assert!(normalize_url("  ").is_err());
    }

    #[test]
    fn stale_node_errors_become_actionable() {
        let error = CdpError::Protocol {
            code: -32000,
            message: "No node with given id found".to_string(),
        };
        assert!(cdp_message(error).contains("call `snapshot`"));
    }
}
