---
title: "Changelog"
description: "Recent OSAgent changes (see CHANGELOG.md for full history)."
weight: 50
toc: true
---

## v0.6.1

- Streaming replies respect an intentional scroll-up and resume following only
  after returning to the bottom or sending another message.
- Reopened sessions preserve newer background-streamed response text and clear
  stale typing state instead of temporarily showing truncated replies.
- Final replies and idle status are saved before completion is broadcast;
  reopening refreshes the snapshot before advancing the event cursor.
- Background completions show an unread sidebar dot immediately and keep it
  visible through icon refreshes until the chat is opened.
- Discord requests through OpenCode Go include the required stable session
  header and OSAgent user agent, preventing provider 400 errors.
- Code search streams bounded results and skips generated/dependency trees;
  grep and glob avoid duplicate fallback scans after timeouts.
- Web search decodes DuckDuckGo result links and extracts clean snippets, ranks
  a small candidate pool by relevance, preserves `site:` filters, and no longer
  substitutes unrelated site-specific API results.
- Community Discord turns skip unnecessary workspace Git snapshots, and their
  status changes to Wrapping up when the model finishes.
- Mobile chat fits the visible viewport when browser chrome or the keyboard
  changes, with a focused composer action menu, simpler header and chat drawer.
- Mobile user messages use one bubble without a duplicate frame or extra right padding.

## v0.6.0

- Runtime skill authoring: OSA can create/update/delete skills live via
  `skill_create` / `skill_update` / `skill_delete` (plain `SKILL.md`, no
  restart); `.oskill` bundles kept for import only.

## v0.5.3

- Rolling working notes + real `/compact` (verify-and-correct handoff, resets context)
- Provider-reported context ring (input + cache) with estimate fallback
- Bing backend for web search with direct result URLs
- Subagent card flicker and stuck-stream fixes
- Read-before-edit, per-file edit locks, line-ending/BOM preservation
- `edit_file` / `write_file` return diff + LSP diagnostics

## Earlier

Full history lives in [`CHANGELOG.md`](https://github.com/jaylikesbunda/OSAgent/blob/main/CHANGELOG.md)
in the repo root.
