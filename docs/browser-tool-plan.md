# OSA sandboxed browser tool — implementation plan

Status: **proposal**, not yet implemented.
Scope: give OSA a stateful, sandboxed, headless-Chromium browser it can drive and
*see* (screenshots + accessibility snapshots) so web research is no longer limited
to raw HTML `web_fetch` and JSON `web_search` results.

Decisions locked with the maintainer:

1. **Real headless Chromium over CDP** (not a pure-DOM simulation).
2. **One stateful `browser` tool** with an `action` field (not ~20 separate tools).
3. **On by default, deferred via `tool_search`** (discoverable, not always-loaded).

---

## 1. What other lightweight browser tools do (reference survey)

| Project | Shape | Notable idea worth stealing |
|---|---|---|
| [chromiumoxide](https://github.com/mattsse/chromiumoxide) | async Rust CDP client, generated types (~60k LoC), `fetcher` feature auto-downloads Chromium | proves CDP-over-tokio; but the generated protocol crate is heavy |
| [headless_chrome](https://github.com/rust-headless-chrome/rust-headless-chrome) | sync Rust CDP client | simplest launch/keys code; blocking, less maintained |
| [cdpkit](https://lib.rs/crates/cdpkit) | new type-safe CDP client | confirms a small hand-rolled client is a real option |
| [Vercel agent-browser](https://github.com/vercel-labs/agent-browser) | Rust CLI, ~20 namespaced tools | **accessibility-tree snapshot with stable refs (`@e1`)**, compact text output, annotated screenshots, auto-install on first use |
| [browser-use](https://docs.browser-use.com/) | Python agent loop | `navigate / click / type / scroll / extract / get_state`; every action returns fresh state |
| [ABP / freezes-after-action](https://news.ycombinator.com/item?id=47336171) | CDP harness | after each action, freeze + report **navigation/alerts/downloads + screenshot** so the model re-syncs |
| PinchTab | 12 MB Go binary | accessibility-first refs; ~800 tokens/snapshot vs thousands for pixels |

**Lessons that shape the design**

- A **text/accessibility snapshot with element refs is the primary channel**; screenshots
  are for visual grounding and are token-expensive. Do not make pixels the only I/O.
- **Refs beat CSS selectors**: models rarely know the DOM. Assign `ref=e1..eN` from the
  accessibility tree, click by dispatching a real mouse event at the node's box center.
- **Report what happened after every action** (url/title changed, alert, navigation,
  new tab) so the agent stays synchronized — the ABP "freeze after action" insight.
- **Annotated screenshots** (numbered boxes matching refs) are the highest-value visual
  aid and are cheap to add once refs exist.
- Sandboxing is the differentiator vs. simply shelling out: isolated profile, localhost-only
  debug port, killed process tree, egress policy.

---

## 2. Architecture

```
Agent (LLM)
   │  one tool call: browser { action, ... , session_id }   ← session_id injected by runtime
   ▼
BrowserTool (src/tools/browser/mod.rs)          implements registry::Tool
   │  args.session_id keys the sandbox
   ▼
BrowserManager (src/tools/browser/manager.rs)   Arc-shared by the ToolRegistry
   │  one child process + CDP session per OSA session, idle-evicted
   ├── Sandbox (sandbox.rs)      binary discovery, launch flags, temp profile, kill tree
   ├── CdpClient (cdp.rs)        WebSocket JSON-RPC over tokio-tungstenite + reqwest /json endpoints
   └── Page state (snapshot.rs)  AX-tree → refs, readable text, screenshot → base64 data URL
```

Why hand-rolled CDP instead of `chromiumoxide`:

- `tokio-tungstenite`, `reqwest`, `serde_json`, `base64`, `which` are **already
  dependencies** — the browser tool adds essentially no dependency weight.
- We need ~15 CDP commands. `chromiumoxide_cdp` generates ~60k lines of Rust and
  materially worsens compile time and binary size for commands we never call.
- Full control over launch flags, profile isolation, and egress is required for the
  sandbox guarantees, and it is easier against our own thin client.

The cost is that we own protocol compatibility. Mitigation: pin the commands we use,
defensively parse responses, and keep a version handshake via `Browser.getVersion`.

---

## 3. Sandbox & security model

- **Isolated profile**: fresh `--user-data-dir` under the OSA data dir
  (`<data>/browser/<session_id>/profile`), deleted on close. The user's real
  profile/cookies/autofill are never touched.
- **Localhost-only debugging**: `--remote-debugging-address=127.0.0.1`,
  `--remote-debugging-port=0` (ephemeral). The actual port is parsed from the
  child's stderr `DevTools listening on ws://127.0.0.1:<port>/...`, never guessed,
  so we never attach to another app's browser.
- **Single node, no user data**: `--headless=new --disable-gpu --no-first-run
  --no-default-browser-check --disable-extensions --disable-sync
  --disable-default-apps --disable-background-networking --mute-audio
  --disable-features=Translate,OptimizationHints`. Do **not** pass `--no-sandbox`
  on platforms where the OS sandbox works.
- **Process hygiene**: child + descendants killed on `close`, on session prune, on
  idle timeout, and on `Drop` (Windows: `taskkill /T /F`; unix: process group kill).
- **Egress policy** (see Phase 3): the `public_web_fetch` tool already refuses
  non-public IPs. The browser must reach parity. Phase 1/2 ship `allowed_hosts` /
  `blocked_hosts` from config plus a documented gap; Phase 3 adds CDP `Fetch.enable`
  interception that resolves each request host and fails non-public addresses.
- **JS eval off by default** (`allow_javascript_eval = false`). `eval` actions and
  `javascript:` URLs are refused unless enabled.
- Browser process is **not** parallel-safe: the registry must keep `browser` out of
  `is_parallel_safe`, and the tool serializes calls per session with an async mutex.

---

## 4. Tool contract (one stateful `browser` tool)

Actions (superset of the reference tools, trimmed to what research needs):

| action | key args | returns |
|---|---|---|
| `open` | `url`, `new_tab?` | state + snapshot refs |
| `navigate` | `url` | state + refs |
| `back` / `forward` / `reload` | — | state |
| `snapshot` | `filter?` (`interactive`) | accessibility tree w/ `[ref=eN]` + title/url |
| `screenshot` | `full_page?`, `annotate?` | image attachment (PNG/JPEG data URL) |
| `read` | `max_chars?` | readable page text (reuses `WebFetchTool::extract_readable_html` as fallback) |
| `click` | `ref` \| `selector` \| `text` | state + refs |
| `type` | `ref` \| `selector`, `text`, `submit?` | state |
| `press` | `key` | state |
| `scroll` | `direction` \| `ref`, `amount` | state |
| `hover` | `ref` | state |
| `select` | `ref`, `value` | state |
| `find` | `text` | matching refs |
| `tabs` | subaction list/new/close/switch | tab list |
| `eval` | `js` (gated) | result |
| `close` | — | confirmation |

Design rules:

- Default response is **text** (title, url, refs, "what changed"). Screenshots only on
  explicit `screenshot` (or `annotate: true`) to control vision tokens.
- Output is size-bounded and spilled via the existing `spill`/`maybe_store_large_output`
  path; snapshots cap node count.
- After any mutating action, emit a compact state delta + (optionally) an annotated
  screenshot, per the "freeze after action" pattern.
- `session_id` is already injected into every tool call by
  `runtime.rs` (`tool_args["session_id"] = ...`), so the tool keys its process on it
  exactly like MCP/todo/goal tools do.

Example schema sketch:

```json
{
  "type": "object",
  "properties": {
    "action": { "type": "string", "enum": ["open","navigate","back","forward","reload",
      "snapshot","screenshot","read","click","type","press","scroll","hover","select",
      "find","tabs","eval","close"] },
    "url": { "type": "string" },
    "ref": { "type": "string", "description": "element ref from a snapshot, e.g. e12" },
    "selector": { "type": "string" },
    "text": { "type": "string" },
    "key": { "type": "string" },
    "direction": { "type": "string", "enum": ["up","down","top","bottom"] },
    "amount": { "type": "integer" },
    "value": { "type": "string" },
    "subaction": { "type": "string", "enum": ["list","new","close","switch"] },
    "full_page": { "type": "boolean" },
    "annotate": { "type": "boolean" },
    "max_chars": { "type": "integer" },
    "js": { "type": "string" }
  },
  "required": ["action"]
}
```

---

## 5. Config (`src/config.rs`)

New `BrowserConfig` on `Config` (`pub browser: BrowserConfig`), `serde(default)`,
mirrored into `config.example.toml`:

```toml
[browser]
enabled = true                 # on by default; discoverable via tool_search
executable_path = ""           # "" = auto-detect chrome/chromium/edge/brave
headless = true
user_agent = ""                # "" = browser default
viewport_width = 1280
viewport_height = 800
navigation_timeout_ms = 20000
action_timeout_ms = 10000
idle_timeout_seconds = 300     # reap the process after inactivity
max_concurrent_sessions = 2
screenshot_format = "jpeg"     # jpeg | png
screenshot_quality = 70        # jpeg only
max_screenshot_bytes = 1200000
allow_javascript_eval = false
allowed_hosts = []             # empty = all public hosts; patterns like "*.docs.rs"
blocked_hosts = []
```

- `enabled = false` skips registration entirely and drops it from the deferred manifest.
- `[tools] denied = ["browser"]` remains the escape hatch and is honored by
  `is_allowed` already.

---

## 6. Integration points (exact files)

| File | Change |
|---|---|
| `Cargo.toml` | none required. Optionally enable `tokio/process`+`io-util` (already covered by `features=["full"]`). |
| `src/tools/browser/mod.rs` | new — `BrowserTool` impl `Tool`; action dispatch; output bounding; attachments. |
| `src/tools/browser/manager.rs` | new — `BrowserManager` (session → process), idle reaper, lifecycle. |
| `src/tools/browser/cdp.rs` | new — HTTP `/json/*` discovery + WebSocket JSON-RPC command/event layer. |
| `src/tools/browser/sandbox.rs` | new — binary discovery (`which`), launch flags, temp profile, kill tree. |
| `src/tools/browser/snapshot.rs` | new — AX-tree → refs, text extraction, screenshot + annotation. |
| `src/tools/mod.rs` | `pub mod browser;` |
| `src/tools/registry.rs` | (a) construct `Arc<BrowserManager>` once in `with_full`; (b) insert `"browser"` into `tools`; (c) `native_catalog.register(...)`; (d) add arm to `build_tool` passing the shared manager (like `process_registry`); (e) do **not** add to `is_parallel_safe`. |
| `src/tools/registry.rs` `ToolProfile::allows` | Default/Code/Creative already allow unknown names; explicitly decide Plan/Community/Custom. Recommend: allowed in Default/Code/Creative, **not** Custom/Community; Plan = open question (see §9). |
| `src/config.rs` | add `pub browser: BrowserConfig` + default impl. |
| `config.example.toml` | documented block above. |
| `src/agent/prompt.rs` | optional: one line in research guidance ("for JS-heavy or interactive pages, use the `browser` tool"). |
| `docs/content/tools/research.md` | document the new tool. |
| `frontend/` | tool-result attachments already render as images; add a `kind:"browser"` renderer for state/URL if not covered. Verify. |
| `CHANGELOG.md` | entry. |

Note the shared-state pattern to copy: `ProcessRegistry` is built once in `with_full`
and threaded through `build_tool`/`tools_for_workspace` so per-workspace rebuilds
don't drop background processes. `BrowserManager` must follow the same path.

---

## 7. Phased implementation

**Phase 1 — core loop (navigate / see / read)**
1. `sandbox.rs`: discover Chromium/Chrome/Edge/Brave, build isolated profile, launch
   headless, parse debug port, kill-tree teardown.
2. `cdp.rs`: `/json/version` + `/json/new`, WebSocket connect, `Target.attachToTarget`
   (flatten), `Page.enable`, `Runtime.enable`, command/response correlation + event pump.
3. Actions: `open`, `navigate`, `back`, `forward`, `reload`, `read`, `close`.
4. `Page.captureScreenshot` → JPEG/PNG data URL → `ToolResult.attachments`.
5. `BrowserManager` with per-session processes + idle reaper; registry wiring.
6. Config + docs + unit tests.

**Phase 2 — interaction**
7. `Accessibility.getFullAXTree` → assign `ref=eN`; `snapshot` action.
8. `DOM.getBoxModel` + `Input.dispatchMouseEvent` → `click`/`hover` by ref.
9. `DOM.focus` + `Input.insertText` (+ `submit`) → `type`; `press` via
   `Input.dispatchKeyEvent`; `scroll`; `select`; `find`.
10. Tabs (`Target.*`), history, "what changed" deltas.
11. Screenshot annotation overlay (`annotate: true`).

**Phase 3 — safety & polish**
12. CDP `Fetch.enable` egress interception → refuse non-public IPs (parity with
    `public_web_fetch`); enforce `allowed_hosts`/`blocked_hosts`.
13. `eval` gating + `javascript:` URL rejection.
14. Frontend viewer for browser results; docs/CHANGELOG.
15. Optional: auto-provision Chromium behind a flag (deferred; avoid a +150 MB
    installer unless requested).

---

## 8. Testing

- **Unit (no browser needed)**
  - CDP message encode/decode + event correlation.
  - AX-tree → ref assignment from a captured JSON fixture.
  - URL/host policy (public vs private, allow/deny patterns).
  - Launch-arg construction and profile path isolation.
  - Screenshot byte/size capping and data-URL encoding.
  - Config default/parse round-trip.
- **Integration (`#[ignore]` unless a browser is present)**
  - Launch → navigate `https://example.com` → assert title/url.
  - Snapshot yields at least one ref; click a link → URL changes.
  - Screenshot produces a non-empty PNG/JPEG with correct magic bytes.
  - Idle reaper / `close` leaves no running process.
- **Manual**: `osagent` with a JS-heavy page (e.g. a live search results page),
  confirm snapshot refs + annotated screenshot.

---

## 9. Risks & open questions

- **Chrome availability**: not every machine has Chrome/Edge (Linux servers may not).
  Mitigation: clear, actionable tool error naming install options; `executable_path`
  override. Auto-download deliberately deferred.
- **Compile/binary size**: avoided by not adding `chromiumoxide_cdp`; confirm no
  unexpected transitive deps creep in.
- **Vision tokens**: screenshots are expensive. Default to text snapshots; cap
  dimensions/quality; consider a per-session screenshot budget.
- **CDP drift**: pin commands, handshake via `Browser.getVersion`, fail gracefully.
- **Resource use**: one process per session is heavy. Bounded by
  `max_concurrent_sessions` + idle reaper; consider sharing one browser process with
  one target/tab per session later (cheaper, slightly weaker isolation).
- **Open question — Plan profile**: should `browser` be available in read-only Plan
  mode (open/navigate/snapshot/screenshot/read/scroll only), or excluded entirely?
  Recommendation: include it, since `web_fetch` is already allowed in Plan.
- **Open question — egress timing**: ship Phase 1/2 with host allow/deny only and
  document the private-IP gap, or block the tool until Phase 3 interception lands?
  Recommendation: ship early but mark `browser` as network-mutating and warn in docs.
- **Open question — `web_search` integration**: should `web_search` optionally route
  through the browser to scrape a SERP when backends fail/are blocked? That would
  directly serve the original goal ("better web search results") and can be a
  follow-up once `open`/`snapshot`/`read` are stable.
