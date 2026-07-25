v0.2.0 changes:

* Redesigned chat composer into a rounded floating card with glass background that sits on top of the conversation, with a gradient fade on messages so content scrolls under the card
* Reworked workspace and persona popups with better breathing room, alignment, and visual hierarchy
* Improved workspace selection: active workspace now marked with a checkmark indicator, trigger chip shows the workspace name with a ro/rw permission badge, and an empty state shows a clear "Add workspace" CTA
* Workspace menu rows now show rounded permission pill and a dedicated icon-style edit button
* Typing a bare slash command (e.g. /settings) now runs that command instead of sending it to the agent, and the slash menu now lists all commands as soon as you type /
* Settings modal now closes reliably on Save Changes (post-save refreshes no longer block the close)
* Fixed settings load/save crash from removed provider fields (base_url/model) that left most settings form fields blank and blocked saving
* Update checker no longer fails on manifests with platform entries that omit archive/url (e.g. windows-x86_64 using only an installer URL)
* File preview panel now renders beside the chat as a side pane instead of stacking underneath it
* Launcher voice downloads now report progress for chunked/no-content-length downloads and show visible errors instead of sitting at 0%
* Added user approval prompts for explicit file and directory access outside the active workspace
* Added per-turn workspace context and corrected the model-visible tool list
* Blocked workspace switching during active turns and preserved parent workspaces for subagents
* Added model, Git worktree/status, global instruction, and skill context for agents and subagents
* Added persona-based capability profiles that expose every allowed tool in the selected profile
* Preserved nested workspace instructions at the start of file-read results
* Removed duplicate textual tool catalogs and made provider schemas authoritative
* Added tool-schema token estimates to context tracking and the context inspector
* Rebuilt the model picker with an opaque, scrollable, viewport-aware popover and keyboard navigation
* Fixed active_runs corruption on concurrent send that caused Stop button to be unreliable
* Fixed queued run dispatch not setting isProcessing, causing Stop to send instead of cancel
* Fixed response_complete idle flicker when queue has more items
* Fixed error and spawn wrapper paths not emitting terminal events, causing UI to hang
* Fixed streaming cancel emitting Error instead of Cancelled
* Fixed queue item id mismatch between optimistic add and dispatch event
* Added persistent cancel flag alongside Notify for reliable cancellation between iterations
* Fixed outside-workspace paths blocked before reaching the permission system (resolved .. paths now trigger the approval popup)
* Removed per-tool .. rejection; paths with .. are now canonicalized and checked against workspace boundaries through the authorization system
* Simplified workspace creation to just folder path + read/write toggle
* Restyled outside-workspace permission popup as a clean bottom dock
* Fixed markdown rendering: links `[text](url)` and bare URLs are now clickable, numbered lists are supported
* Sessions are now auto-named from the first exchange
* Added markdown table rendering
* Permission popup now appears above the composer bar instead of at viewport bottom
* Added self-healing state recovery: syncRunningSessionSnapshot now detects and resets stuck processing state even when the Error event is missed
* Combined workspace and persona buttons into one context button with tabs to save mobile space


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

