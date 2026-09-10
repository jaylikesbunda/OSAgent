---
title: "Tool Overview"
description: "Full catalogue, deferred loading, and permission trims."
weight: 10
toc: true
---

## Always-loaded core tools

Files, edits, bash, grep/glob, web, subagent, todos, skills, and questions.
Everything else loads on demand (see below).

## Deferred catalogue

Low-frequency tools (code execution, LSP, weather, calendar, news, codesearch,
system status, process, persona, task, memory/decision management, goals,
coordinator, schedule, skill actions) are **not** loaded into every request.
The agent sees a one-line manifest and calls `tool_search` to load a schema on
demand — the same deferred-catalog pattern MCP servers use.

```toml
[tools]
# To keep everything always loaded, deny tool_search here:
# denied = ["tool_search"]
```

## Trim the toolset

Coding and assistant tools ship side by side. Remove what you don't need with
`denied`:

```toml
[tools]
denied = ["weather", "news"]
```

## Safety rails

- Read-before-edit is enforced, with per-file edit locks and line-ending/BOM
  preservation.
- The loop guard blocks runaway repeats; `repeat_reminder` only injects
  advisory nudges at configurable thresholds (`[tools.repeat_reminder]`).
- Session-access policy (`session_access_default_action = "ask"`) gates reads
  of *other* conversations; per-conversation overrides live in
  `permission_rules`.

## Related tasks

- [Files & code]({{< relref "files-code.md" >}})
- [Research]({{< relref "research.md" >}})
- [Configuration]({{< relref "../getting-started/configuration.md" >}})
