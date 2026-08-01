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

