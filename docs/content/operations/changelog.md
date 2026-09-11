---
title: "Changelog"
description: "Recent OSAgent changes (see CHANGELOG.md for full history)."
weight: 50
toc: true
---

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
