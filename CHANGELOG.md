v0.5.1 changes:

* Added cache hit-rate text beside tok/s and fixed context token accounting
* Added cache-result reasons for misses and hits
* Fixed Discord model/provider controls rejecting authorized users
* Fixed context UI overstating current usage with cumulative tokens
* Added subagent status/resume controls and preserved timeout results
* Fixed subagent context rings, transcript ordering, and read-only Git access

v0.5.0 changes:

* Slimmed the sidebar with collapsible subagent groups and per-session workspace labels
* Background subagents now wake the parent conversation automatically when they finish: results arrive as a continuation turn immediately if the agent is idle, or right after its current run and any queued messages finish, instead of waiting for your next message
* Added `subagent_auto_resume`, `subagent_auto_resume_max_turns` (runaway-loop cap on consecutive background-driven turns), and `subagent_task_max_retries` config options
* Failed subagent runs now retry at the task level with exponential backoff (30s → 10 min) on transient provider errors, resuming the same session so completed work is kept; the card shows "provider error — resuming (attempt n/m)"
* Subagents that hit their iteration budget now report an honest `partial` status with resume instructions instead of claiming success, and foreground calls surface it as a resumable task
* Orphaned subagent tasks from a crashed process are failed at startup and delivered to the parent conversation instead of being silently lost
* Background subagent completions now raise a toast (and a desktop notification when the tab is hidden), and partial tasks get their own amber badge
* Fixed fresh installs creating a `subagent_tasks` table without the `notified_at`/`background` columns, which broke background subagents until an in-place upgrade ran
* Thinking levels now come from models.dev `reasoning_options` metadata instead of hardcoded provider/model rules, so every model exposes its real efforts/budget windows; the catalog also refreshes at startup
* Fixed replying to a voice message with no text not transcribing — referenced audio now counts as content
* Added YouTube voice playback in Discord via a new `discord-voice` feature (Songbird 0.6, required for Discord's 2026 voice E2EE), with piped/invidious fallback and Now Playing/Queued embeds
* Music needs no manual `yt-dlp` install — it auto-downloads on first `/play` or bot start and updates weekly; custom path/args still honored
* Added Discord Music settings to the web UI: enable, yt-dlp path/extra args, queue limits, max duration, auto-leave, piped instances
* Fixed `Access Denied` for community members on music commands — they now honor `allow_community_members`
* Music voice connections now retry, and playback failures are reported in Discord instead of silently stalling
* Removed the obsolete missing-`ffmpeg` warning; Songbird decodes audio in-process
* Fixed `opencode-go` rejecting temperatures with more than two decimals
* Fixed provider errors showing twice in the web transcript
* Added search-before-asking: vague requests now fan out parallel reformulated searches (case variants, synonyms, error fragments) before OSA asks for clarification
* Added a `sessions` tool to list, read, and search past conversations, gated by a new `session_access` permission; compaction archives removed messages so they stay searchable
* Code search no longer needs MeiliSearch — replaced with a batched-grep quick-context tool (no binary download, background server, or stale index)
* Code search is now an always-loaded first-class tool, with prompt guidance to reach for it before grep on vague queries
* Simplified the web settings UI: fewer tabs, rare options collapsed into Advanced sections

v0.4.6 changes:

* `tool_search` now covers low-frequency built-ins (code execution, LSP, weather, calendar, news, codesearch, process, system status, persona, task, memory/decision management, goals, coordinator, schedule, skill actions) alongside MCP servers; their schemas stay out of every request until needed
* Added a one-line per-tool manifest of deferred built-ins to the prompt so the model knows the capability exists and can search for it; skills summary shrinks to a count + pointer to `skill_list`
* Tool activation is now session-scoped: tools loaded via `tool_search` stay loaded only for the session that loaded them, so a new session always starts from the lean core set; config `always_active` MCP tools remain preloaded for every session, and deleted sessions drop their activation state
* Subagents gain `tool_search` and the tool manifests, so they can load deferred built-ins and MCP tools instead of losing them from the shared registry
* `public_web_fetch` is now excluded from full-access profiles (Default/Code/Creative) and only offered to Discord community sessions
* Removed duplicated prompt rules: "be concise", "tool schemas are authoritative", and the lint/test mandate each existed twice across sections
* Reworded the git safety rule to permit read-only git (status/diff/log) while still requiring approval for mutating operations, matching actual behavior
* Trimmed implementation-detail prose from edit_file, apply_patch, read_file, and bash tool descriptions; moved the decision-memory protocol note into the record_decision tool schema
* Shortened the per-turn "tool calls completed" follow-up nudge
* Discord now transcribes voice messages and audio attachments with local Whisper, decoding OGG/Opus, MP3, M4A, FLAC and more in-process before transcription
* Replying to a message and tagging the bot now includes the message you replied to, so OSA sees both your reply and its context
* Added Unsloth (Local) provider auto-detection like Ollama — probes `http://localhost:8888/v1/models` fallback `:8000` with `Bearer sk-unsloth-…` and unauthenticated fallback for `unsloth studio --no-auth`, live models appear as `installed` in the picker within 20s
* Slimmed the outside-workspace permission prompt from `1120px` to `640px` max-width so the approval dock is centered and less wide
* Fixed replying to a voice message in Discord not transcribing — now also transcribes audio attachments on the referenced message when you reply tagging OSA
* Added Discord bot config export/import with tokens — `GET /api/discord/export` returns the full `DiscordConfig` (including `token`/`github_token`) and `POST /api/discord/import` restores it, with `Settings → Discord → Backup & Restore → Export/Import` handling file download/upload and preserving existing tokens when imported file omits them

v0.4.5 changes:

* Reworked streamed transcript rendering around one keyed model and renderer, eliminating chat flicker and tool-card ordering issues across thinking, tool, and response segments

v0.4.4 changes:

* Added opt-in community-member chat scoped to configured servers and channels
* Hardened Discord access controls and cleaned up Discord settings

v0.4.3 changes:

* Fixed native WebUI chat when speech synthesis is unavailable
* Added secure Discord community chat and multi-part replies
* Added per-tool timeouts, bounded large-result spill files, repeat-call reminders, and structured tool outcomes
* Added structured context compaction with configurable pruning and transcript limits
* Added per-message feedback and resumable revision-fenced goals

v0.4.0 changes:

## Auto-updates (OTA)

* Fixed OTA updates failing on Linux and macOS with a "Failed to extract tar.gz" error: the release manifest advertised a `.deb`/`.dmg` while the client saved it under a hardcoded `.tar.gz` filename and fed a package to the gzip decoder. The download filename now comes from the manifest instead of a per-platform guess
* The release pipeline now builds and publishes real `osagent-linux-x86_64.tar.gz` and `osagent-macos-arm64.tar.gz` OTA archives alongside the `.deb`/`.dmg`/`.exe` installers, so in-place updates work on every platform
* Downloaded payloads are identified by their magic bytes rather than their file extension, so a package, an HTML error page, or a truncated body is reported for what it is instead of failing deep inside an archive decoder
* Update downloads are now verified against the manifest SHA-256 before anything is staged, and a mismatched or corrupt payload is discarded rather than installed
* Downloads retry up to 4 times with exponential backoff and resume from the bytes already on disk via HTTP range requests, and are checked against `Content-Length` so a silently truncated body is caught while retrying is still cheap
* Downloads are written to a `.part` file and only renamed into place after the size and checksum pass, so a crash mid-download can never leave a corrupt payload for the next run to pick up; an already-downloaded payload whose hash still matches is reused instead of re-fetched
* Archive extraction now rejects entries that escape the destination directory (zip-slip / tar traversal), preserves the executable bit from zip archives, and unpacks into a wiped subdirectory so leftovers from a failed attempt cannot be picked up
* The pending/prepared update marker files are written atomically, arming an update whose staged file is missing is refused, and stale payloads from earlier attempts are cleaned up before a new download starts
* Update download timeout raised from 5 to 30 minutes, with a separate 30s connect timeout, so a large installer on a slow link no longer fails partway through
* On macOS the updater now clears the quarantine attribute and re-signs the app bundle ad-hoc after swapping the launcher binary — without this the modified bundle fails its signature check and macOS refuses to launch it
* Platforms that ship only an installer now report the verified file and its path instead of silently staging something the launcher cannot apply

## Release pipeline

* Release archives are validated before publishing: gzip integrity, entry paths, presence and executable mode of the launcher, native binary format, minimum size, and checksum agreement (`verify-ota-archive.sh`)
* `upload-to-r2.sh` re-verifies every checksum, confirms the OTA archives really are gzip, validates the generated manifest JSON, and HEAD-checks every published payload before flipping `latest.json`, so the manifest can never point at a missing or broken file
* The release manifest now distinguishes the OTA archive (`assets.<platform>.url`) from the manual installer (`assets.<platform>.installer`), with separate `sha256.<platform>` and `sha256.<platform>-installer` entries
* GitHub releases now host the built binaries as release assets and the release notes link to `github.com` download URLs instead of the CDN, which is not intended for direct downloads; a post-publish step fails the job if any advertised asset is not downloadable

## Web search

* Added structured search over free, key-less JSON APIs — Wikipedia, Hacker News (Algolia), GitHub, Stack Exchange, crates.io, npm, Reddit and arXiv. A `site:` filter naming one of those hosts is answered by its API directly, which neither gets challenged nor breaks when markup changes
* When every general backend fails, the structured APIs are now tried as a last resort instead of returning "No search results found", and the error message lists the sites that can be targeted with `site:`
* Reddit is queried through its RSS endpoint: anonymous `/search.json` now returns HTTP 403, while the feed still answers requests that carry a real user agent
* Added a `time_range` parameter (`day`/`week`/`month`/`year`) plumbed into every backend that supports recency filtering, so "what happened in the past month" can actually be expressed
* Fixed a results page being discarded as a bot challenge whenever its text merely contained words like "challenge", "captcha" or "access denied" — which hit security-related queries the hardest. Detection now relies on unambiguous phrases, or on those words appearing in the title of a page too small to be a page of results
* Raised search timeouts from 2s per backend / 4.5s overall to 8s / 15s: public SearXNG instances routinely need several seconds and were being timed out before they could ever answer

## Launcher & native app

* Added a native Web UI window (Tauri 2, rendering via the OS WebView2 runtime — no bundled Electron/Chromium): the web UI now opens in its own app window instead of only in a browser, and the launcher and tray offer both targets — "Open Web UI" (native window) and "Open in Browser"
* The native Web UI window is frameless with a dark custom titlebar matching the launcher (drag to move, minimize, and close-to-tray via an injected titlebar that shrinks the page content to fit beneath it)
* Starting the launcher when setup is complete now auto-starts OSAgent, waits for the web server to come up, opens the native Web UI window, and keeps the launcher minimized in the tray (tray icon click reopens it); first-run setup still shows the launcher window so onboarding is not skipped
* The launcher now enforces a single instance: launching again from the Start Menu shortcut focuses the running launcher instead of spawning a second copy
* Added a Tauri capability for the remote Web UI origin so the embedded page may drag, minimize and hide its own window (Tauri denies IPC from remote origins by default), with the launcher window's default permissions preserved explicitly so nothing regressed
* Closing either window now hides it to the tray instead of exiting the app; the tray "Exit" item terminates OSAgent and quits cleanly

## Voice mode & streaming STT

* Added a hands-free voice mode: the agent is told (via a dedicated system message) to write a short `<speak>` spoken summary first and the full written answer second; the frontend reads the `<speak>` block aloud as it streams, strips it before rendering for the reader, and every assistant message gains a "Speak" button (click again to stop)
* Voice mode is stored per-session (session metadata, so it survives reloads) and applies to every surface; finished transcripts send straight to the agent in voice mode, and interim words are shown live while speaking
* Added a persistent whisper.cpp server (`whisper-server`): the model stays resident instead of being reloaded from disk (148MB–488MB) for every two-second utterance, with a 90s model-load timeout, dynamic port, best-effort startup, and automatic fallback to the one-shot CLI path when the binary is missing or the port is taken
* Partial/streaming transcription is enabled only when the resident server is available — without it every partial would reload the model from disk per chunk
* Whisper now prefers English-only model builds (`tiny.en`/`base.en`/`small.en`) when the configured language is English, and threads are capped based on physical cores to avoid thread-overhead dominating inference
* New config: `speak_tool_progress` (narrate each tool call while working, default off) and `silence_auto_stop` (end recording after a pause, default on; voice mode always auto-stops on silence)
* Mic capture is now resampled and downmixed to 16 kHz mono on the client (up to ~3× less upload, no server-side resample), has a live input level monitor so a muted or wrong-device mic is visible immediately, supports explicit device selection (kept in localStorage since device IDs are browser-scoped), and recording is cancelled when TTS starts so the agent never hears its own speech
* Session storage now runs on a pooled SQLite connection with an explicit transcript table and a backfill migration for existing sessions, plus message search and message truncation (truncate refuses while a turn is in flight) backing the new `/api/session-search` and `/api/sessions/:id/messages/truncate` endpoints

## MCP servers & tool discovery

* Added MCP (Model Context Protocol) client support over stdio and HTTP, with paginated tool discovery, `readOnly`/`destructive` hints, and per-server timeouts
* Tool schemas are deferred: the model sees one line per server, not every tool, so connecting a 50-tool server costs about a line of context instead of 50 schemas
* Added `tool_search` — the agent searches the catalog in plain language and the matches become callable immediately; `select:<name>` re-fetches a specific schema
* Added `tool_script` — runs a Python or Node script that drives many tools at once, so intermediate results never reach the conversation. Scripts declare `uses` up front and the bridge rejects anything else
* Activated tools are appended after the native tool block, so discovering a tool invalidates only the tail of the provider's cached prompt prefix
* Plan mode only sees MCP tools the server marks read-only; the roleplay persona gets no MCP access at all
* Added Settings > MCP Servers: add, edit, enable, remove, test a connection before saving, and browse a server's tools. Servers connect in the background so startup is not blocked

## CLI

* Running the binary bare now defaults to `start` instead of printing a usage error
* Added `-p/--port` to override the web UI port, with precedence: flag > `OSAGENT_PORT` env var > config file
* Fixed `-v`/verbose logging never taking effect (the log level was fixed before the flag was parsed)
* The restart sentinel now lives at an absolute `~/.osagent/restart_flag` instead of a relative path that dropped a dotfile into whatever directory the process was launched from
* On startup the agent kills stale `whisper-server` processes, which could otherwise hold the listen socket open on Windows after a hard kill

v0.3.0 changes:

## Provider error handling

* Provider errors are now parsed structurally instead of classified by sniffing the message text: the HTTP status code, the provider's error code (e.g. `context_length_exceeded`, `rate_limit_exceeded`) and any `Retry-After` hint are extracted from the response once and carried on the error, so rate-limit, context-overflow and retryable classifications no longer depend on the exact wording each provider uses. Text-based matching remains as a fallback for proxies that return plain-text bodies
* Retries now honor the server's `Retry-After` hint instead of ignoring it: `retry-after-ms`, `retry-after` (seconds or HTTP date) and `x-ratelimit-reset` are all understood, so a 429 that says "wait 30s" no longer retries after 8s and fails again, or sleeps through a short window. The hint is capped at 5 minutes so a misbehaving server cannot stall the agent loop
* 5xx responses are now always retried, even when the error text doesn't say "status code 5xx" — previously a 503 from a gateway with non-standard wording was treated as permanent and aborted the turn
* Retry backoff is now exponential (`base × 2^attempt`, capped) instead of linear, matching provider expectations for transient failures
* OpenAI-style 404 "model not found" responses remain retryable for providers known to return them spuriously
* Retries are now announced live instead of only after the fact: before each attempt the provider emits a retry event carrying the attempt number, the maximum, and the server-requested delay, and the chat UI shows a "Provider request failed — retrying in ~Ns (attempt N/M)" notice that clears when the turn resumes. Previously the only signal was a single event after the request had already succeeded
* When the provider's last-resort context compression kicks in (request exceeded the context window despite the pre-emptive estimate), the resulting summary is now persisted into the session instead of being discarded after that one request — the model sees it on later turns and history stays bounded

## Tools & agent runtime

* Failed tool calls are now recorded distinctly: the tool message persisted to the session carries `success: false` plus the error text in metadata (and session tool events include the error message), so failures are no longer indistinguishable from a tool that returned the string "Error: ..."
* Argument/schema validation errors (missing or invalid parameters) now end with explicit guidance telling the model to rewrite the input to satisfy the expected schema, so it stops resubmitting the same malformed call
* The tool loop's allowlist and loop-guard error messages are now marked as failed tool results with the same success/error metadata instead of bare tool messages

## Chat UI diagnostics

* Added a debug overlay (toggleable from Settings > Agent > Debug Overlay, or with Shift+D) that shows a live stream of agent events and every tool-card placement decision — anchors resolved, slots found or missing, cards recovered, and cards orphaned in the transcript's virtualized window — plus a Copy button that dumps the diagnostics and current transcript state, so "tool cards missing until session reload" style issues can be diagnosed instead of guessed at. Off by default
* Tool cards are now anchored under both the requested and the resolved message index, and a pending-mount pass runs after every transcript render: cards that arrived before their anchor message was rendered are re-attached as soon as it appears, instead of staying invisible until a session reload
* The 2.5s backend tool-sync now logs how many cards it created, skipped as out-of-window, or left in place

## Subagents

* Added nesting control: new `agent.subagent_depth` config (default 1) is a hard limit on how many subagent levels can be spawned — the main agent counts as 0, so a subagent can no longer spawn another subagent beyond the configured depth, and the attempt fails with an explicit "Subagent depth limit reached" error instead of silently forking
* Subagent cards now show live execution detail like opencode: while running, a status strip displays the current tool being executed (`↳ read_file`) and flips to `↳ retrying in ~5s (attempt 2/4)` when the provider retries (retry events are now routed to the parent session with the subagent id so the card can react); completed/failed tools are appended to the tool list in green/red
* Completed subagent cards show duration (`N tools · 2m 14s`) computed server-side on completion and client-side when restored from history
* Subagent retries also surface as a "retrying" badge state on the card, clearing when work resumes

## Streaming performance

* Fixed chat streaming render cost: assistant responses are now rendered incrementally — only the newly received chunk is parsed and appended each animation frame instead of re-parsing and re-rendering the entire accumulated message, so per-frame cost stays flat as messages grow instead of climbing linearly (most noticeable on slower machines)
* Fixed lang-less code fences (``` with no language) rendering as plain paragraphs instead of code blocks in the incremental renderer

## Agent prompting & context

* The system prompt now carries concrete behavior rules: keep replies concise (a few lines unless the user asks for detail), batch independent tool calls into a single message, run the repo's lint/typecheck/test/build command when one exists and fix what it reports, explain non-trivial shell commands before running them, never commit or push without being explicitly asked, and avoid adding code comments unless asked
* The runtime environment block now reports whether the workspace is a git repository (`- Is directory a git repo: yes/no`) alongside model, workspace, working directory and platform, for both the main agent and subagents
* Open-ended discovery (searches that will need multiple rounds of globbing and grepping) is now delegated to explore subagents via the task tool instead of being run inline; the grep/glob tool descriptions, their when-not-to-use guidance and the large-output spill hints all point the model at that delegation path

## Coding tools

* read_file now reads images and PDFs instead of rejecting them as binary: PNG/JPEG/GIF/WebP files are attached to the conversation as images the model can see (capped at 10MB), and PDFs have their text extracted with the same offset/limit pagination as text files
* apply_patch hunk matching is now fuzzy: it falls back from byte-exact to whitespace-insensitive and then similarity-based matching, so patches with slightly stale context lines apply instead of failing
* apply_patch now runs post-apply diagnostics through the LSP server when one is available and reports per-file errors/warnings in the tool output
* The LSP transport was a stub (responses were logged but never routed back to callers); it now implements real LSP framing — Content-Length header parsing, request/response correlation, an initialize/initialized handshake, buffered publishDiagnostics — and the client gains pull diagnostics (`textDocument/diagnostic`) with push diagnostics as fallback, plus a check that the server binary actually exists on PATH

## Search tool timeouts

* Fixed grep/glob tools stalling turns for exactly 60s and then producing late results: rg was run through a detached `spawn_blocking` child that a tokio timeout could abandon but never kill, so timed-out searches kept running (and the tool loop sat waiting); both grep and glob now run rg as a `tokio::process::Command` with `kill_on_drop`, so a timeout actually terminates the child
* The fallback directory walk (used when rg is unavailable) is now cancellable: a shared cancel flag set on timeout stops the walk, which previously kept scanning (including binary files) long after the caller gave up
* The fallback walk now skips heavy directories (`.pio`, `node_modules`, `target`, `dist`, `.git`, `.venv`, ...) unless the search root itself is one, sniffs the first 4KB to skip binary files without reading whole files, reads line-by-line instead of loading entire files, and caps results at 10,000 matches with an explicit "results truncated" note

## Chat streaming fixes

* Fixed tool cards disappearing from the chat after the assistant message that invoked them loses its anchor: assistant messages that carry `tool_calls` (the "tool prelude" messages) were treated as hidden when their content was empty, so the transcript entry hosting their cards was removed on the next re-render and the cards could only be restored by reloading the session; preludes with tool calls now stay in the transcript as card anchors
* Fixed the message "still writing" indicator staying stuck on completed segments: the 2.5s tool-sync kept re-adopting the last assistant message as a streaming element even after it was finalized at a tool boundary, re-adding the streaming cursor every poll; finalized segments (marked `boundaryAfter=tool` or already hosting tool cards) are now content-synced quietly instead of being re-streamed
* Fixed duplicated text fragments in streamed messages (e.g. the tail of a line, with raw `**`/backtick characters, appearing twice): when the 2.5s snapshot sync had already applied a full response and a delayed or re-delivered `response_chunk` for the same content then arrived, the chunk was appended a second time; content and thinking chunk appends now skip text that is already present at the end of the current segment, which also covers dual SSE/WebSocket delivery
* Thinking blocks now stay collapsed by default: auto-expanding while streaming, at creation, and on re-adoption was removed, so the "Thinking" preview stays minimized until the user opens it (manual open/close toggles still stick)

v0.2.0 changes:

## Security & permissions

* Added user approval prompts for explicit file and directory access outside the active workspace
* Fixed outside-workspace paths blocked before reaching the permission system (resolved .. paths now trigger the approval popup)
* Removed per-tool .. rejection; paths with .. are now canonicalized and checked against workspace boundaries through the authorization system
* Fixed bash tool silently bypassing the outside-workspace approval prompt: it only checked the `workdir` argument for external paths, so absolute paths embedded directly in the command text (e.g. `dir "C:\Users\name\Documents"`) ran with no approval; the command text is now scanned for absolute paths too
* Outside-workspace approval now also triggers on environment-variable paths in bash commands (`%USERPROFILE%\Documents`, `$env:USERPROFILE`, `$HOME`, `~`), which the shell expands after the check ran and which previously slipped through unprompted
* Blocked workspace switching during active turns and preserved parent workspaces for subagents

## Tools & agent runtime

* Bash tool description now tells the model which shell it's actually running (cmd on Windows, sh elsewhere) so it stops guessing POSIX syntax on Windows and failing commands
* Fixed bash tool silently mangling quoted paths on Windows: Rust's default arg-quoting re-escaped embedded quotes before handing them to `cmd /C`, which cmd parses differently, so a quoted path (e.g. `dir "C:\Users\name\Documents"`) would fail while the same path unquoted worked
* Fixed read-only bash mode rejecting harmless commands: mutating keywords were matched as raw substrings, so "Format-Table" tripped the "rm" rule and "different" tripped "ren"; matching is now word-boundary aware
* Subagent results are now delivered straight to the waiting caller over a channel instead of being discovered by polling every 200ms and re-read from the database, and an aborted or cancelled subagent now always reports a terminal status instead of leaving the caller to time out
* Added per-turn workspace context and corrected the model-visible tool list
* Added model, Git worktree/status, global instruction, and skill context for agents and subagents
* Added persona-based capability profiles that expose every allowed tool in the selected profile
* Preserved nested workspace instructions at the start of file-read results
* Removed duplicate textual tool catalogs and made provider schemas authoritative
* Added tool-schema token estimates to context tracking and the context inspector

## Sessions

* Sessions are now auto-named from the first exchange
* Fixed sessions never auto-naming from the first exchange: both the frontend trigger and the auto-name endpoint treated the default "Session N" placeholder as an already-set name, so the request was never sent and title generation never ran; both now only skip for a genuinely custom name
* Fixed js/inspector.js never being bundled: it was missing from the frontend build's load order, so every function it defines was undefined at runtime. `OSA.scheduleSessionInspectorRefresh()` is called unguarded at the end of the tool-complete and response-complete handlers, so each of those handlers threw partway through — which is what actually stopped sessions from being auto-named, and silently skipped the session-inspector refresh after tool calls
* The auto-name endpoint now logs why a title was not produced (model error, or an unusable response) instead of silently returning no name, and tolerates models that wrap the title in quotes, prefix it with "Title:", or emit a reasoning preamble

## Code search

* Code search now installs MeiliSearch itself instead of permanently disabling itself with a "binary not found" warning: nothing ever populated the `~/.osagent/search` location it already probed. The download runs detached so it never blocks startup (indexer construction is synchronous and the binary is ~110MB), verifies the transfer against Content-Length, and only moves the binary into place once complete so an interrupted download cannot leave a truncated binary that looks installed. Code search enables on the next start

## Voice

* Fixed Whisper and Piper runtime installs reporting success while installing nothing: extraction shelled out to PowerShell's Expand-Archive, whose errors are non-terminating so the process still exits 0, and the code then skipped its copy step silently when the expected binary was absent. Extraction now runs in-process via the zip crate with real errors, per-entry progress, and zip-slip protection; downloads are size-checked against Content-Length so a truncated archive is reported instead of being extracted; and a missing binary is now a hard error rather than a silent success
* Fixed Whisper runtime install looking for its binary at one hardcoded path: it now searches the extracted tree and accepts whisper-cli.exe, whisper.exe or main.exe, and copies runtime DLLs from beside the binary it actually found
* Fixed voice model and voice downloads appearing to freeze partway: the progress SSE stream treated a lagging broadcast receiver as fatal and closed itself, so the download continued with the UI stuck at its last percentage (and the browser logging a "Voice progress SSE error"). Lag is now skipped rather than ending the stream, the stream sends keep-alives, progress frames are rate-limited to whole-percent changes so clients stop falling behind, and the channel has more headroom
* Voice runtime installs now report extraction progress per file instead of jumping from download straight to "installed"
* Fixed the launcher's voice installer treating a failed extraction as success for the same Expand-Archive reason, and it now verifies extraction actually produced files
* Launcher voice downloads now report progress for chunked/no-content-length downloads and show visible errors instead of sitting at 0%

## Discord

* Discord: added /settings, a single control panel with select menus for provider, model, persona and workspace that re-renders itself after every change, so nothing has to be typed from memory
* Discord: /model set now validates against the model catalog and offers near-matches plus an explicit "Use anyway" confirmation instead of silently accepting a typo that fails on the next turn
* Discord: added /provider list and /provider use; switching provider and model now goes through one atomic switch, so the running provider and the config file can no longer disagree (previously the model was set on the active provider but written to the default provider's config entry, and reverted on restart)
* Discord: reorganised commands into subcommands (/session, /model, /provider, /persona, /workspace, /permissions) with autocomplete on every id
* Discord: each turn now renders one status message that updates in place with the running tool, elapsed time and failures, instead of one embed per tool, and responses carry a model · provider · persona footer
* Discord: long responses no longer split in the middle of a code fence; a block that spans messages is closed and reopened
* Discord: provider errors are reported with the likely cause and fix (bad API key, unknown model, rate limit, context overflow) instead of a raw error dump
* Discord: agent questions now reach Discord again — the session-to-channel mapping was only written by an unreachable code path, so every /ask question was dropped and /answer always reported "no pending question"
* Discord: questions are tracked per session, /answer resolves a bare option number to that option, and answering is restricted to the user whose session asked
* Discord: fixed duplicated notifications — every gateway reconnect spawned another event listener and re-registered all slash commands, so workflow and schedule notices were delivered once per reconnect
* Discord: workflow approval buttons now require authorization; previously anyone who could see the message could approve or reject a workflow step
* Discord: /answer now requires authorization
* Discord: an empty discord.allowed_users list now denies everyone instead of allowing everyone, and in servers the bot only responds when mentioned or replied to rather than to every message in every channel it can see
* Discord: /lsp and /subagent now actually run the request instead of printing "use /chat"; /mode reports that it is a hint rather than implying tool access changed
* Discord: added a queue notice when a channel already has a turn running, and per-channel locks are now reclaimed instead of accumulating for the lifetime of the process

## Chat UI

* Redesigned chat composer into a rounded floating card with glass background that sits on top of the conversation, with a gradient fade on messages so content scrolls under the card
* Fixed duplicate/bulk tool-call cards appearing at the top of a running turn (then interleaved again lower down) when a tool card was orphaned before its transcript slot existed; reload no longer needed to clear them
* Fixed subagent cards appearing at the very top of the chat until reload: when the timestamp-to-message anchor lookup failed during a live turn it fell back to message index 0, pinning the card to the top; live cards now fall back to the latest message instead
* Error and cancelled notices are now anchored inline where they occurred instead of being pinned to the bottom of the chat below later messages, and are rendered compactly against the left edge of the message column
* Fixed markdown rendering: links `[text](url)` and bare URLs are now clickable, numbered lists are supported
* Added markdown table rendering
* File preview panel now renders beside the chat as a side pane instead of stacking underneath it
* Typing a bare slash command (e.g. /settings) now runs that command instead of sending it to the agent, and the slash menu now lists all commands as soon as you type /
* Rebuilt the model picker with an opaque, scrollable, viewport-aware popover and keyboard navigation

## Workspace & settings UI

* Reworked workspace and persona popups with better breathing room, alignment, and visual hierarchy
* Improved workspace selection: active workspace now marked with a checkmark indicator, trigger chip shows the workspace name with a ro/rw permission badge, and an empty state shows a clear "Add workspace" CTA
* Workspace menu rows now show rounded permission pill and a dedicated icon-style edit button
* Simplified workspace creation to just folder path + read/write toggle
* Combined workspace and persona buttons into one context button with tabs to save mobile space
* Restyled outside-workspace permission popup as a clean bottom dock
* Permission popup now appears above the composer bar instead of at viewport bottom
* Settings modal now closes reliably on Save Changes (post-save refreshes no longer block the close)
* Fixed settings load/save crash from removed provider fields (base_url/model) that left most settings form fields blank and blocked saving

## Streaming, queueing & cancellation

* Fixed active_runs corruption on concurrent send that caused Stop button to be unreliable
* Fixed queued run dispatch not setting isProcessing, causing Stop to send instead of cancel
* Fixed response_complete idle flicker when queue has more items
* Fixed error and spawn wrapper paths not emitting terminal events, causing UI to hang
* Fixed streaming cancel emitting Error instead of Cancelled
* Fixed queue item id mismatch between optimistic add and dispatch event
* Added persistent cancel flag alongside Notify for reliable cancellation between iterations
* Added self-healing state recovery: syncRunningSessionSnapshot now detects and resets stuck processing state even when the Error event is missed

## Build & updates

* Fixed the launcher embedding a stale core binary: build.rs watched the path OSAGENT_CORE_SOURCE pointed at but never declared rerun-if-env-changed, so switching build profiles (e.g. -Fast) left cargo checking the previous profile's binary and the installer silently shipped an old osagent.exe
* Update checker no longer fails on manifests with platform entries that omit archive/url (e.g. windows-x86_64 using only an installer URL)


v0.1.1 changes:

* Added missing context overflow detection patterns (Mistral, Ollama, z.ai, vLLM, llama.cpp, LM Studio, MiniMax, Kimi, Copilot, Bedrock, HTTP 413)
* Added is\_openai\_retryable() for 404 retry handling (OpenAI sometimes returns 404 for available models)
* Added schema transforms: Kimi/Moonshot ($ref sibling stripping, tuple items fix) and Google/Gemini (integer enum-to-string, required sanitization, non-object type cleanup)
* Added DeepSeek reasoning part fix (ensure all assistant messages have reasoning\_content field)
* Added GPT-5 text\_verbosity: "low" default option for non-Codex models
* Added GitHub Copilot store: false default option
* Added Google Vertex ADC auth token support (GCLOUD\_ACCESS\_TOKEN env var)
* Added AWS Bedrock profile-based credential loading (\~/.aws/credentials)
* Added AWS Bedrock cross-region inference prefix support
* Added Azure env var fallback chain (AZURE\_OPENAI\_KEY / AZURE\_OPENAI\_API\_KEY, AZURE\_RESOURCE\_NAME)
* Fixed OpenRouter header duplication (removed from provider\_transforms, kept in provider\_auth with consistent values
* Added GCP service account file detection (GOOGLE\_APPLICATION\_CREDENTIALS)
* Fixed chat streaming flicker: thinking block no longer collapses/re-expands on thinking\_end → response\_start transition
* Fixed chat streaming flicker: virtual scroll reconciliation now preserves streaming DOM state (rawText, streaming/expanded classes) through patchMessageElement
* Fixed chat streaming flicker: scheduleFormattedRender skips innerHTML update when text hasn't changed (renderedText cache)
* Fixed chat streaming scroll jump: auto-scroll now runs after DOM update in requestAnimationFrame instead of before
* Fixed chat streaming layout thrash: thinking preview and body updates are batched into the same animation frame
* Fixed non-streaming response flash: completeAssistantResponse and commitStreamingAssistantSegment now populate DOM from session data when rawText is empty, preventing remove-and-recreate cycle
* Fixed response text duplication: removed duplicate ResponseStart/ResponseChunk emission in backend fallback path that caused word-by-word doubled text ("YouYou're're welcome welcome!!")
* Fixed response text duplication: frontend now resets accumulated streaming content when a new ResponseStart arrives (fallback retry), preventing content accumulation across attempts







v0.1.0-rc2 changes:

* Added discord bot setup to launcher
* Added file read before caching
* Added subagent coordinator
* Improved internal prompt with automated evaluation and testing
* Added read before write check
* Replaced completion heuristic with finish\_reason based logic\\
* Expanded parallel safe tools list
* Added updater logic
* Support for workspaces with multiple allowed dirs
