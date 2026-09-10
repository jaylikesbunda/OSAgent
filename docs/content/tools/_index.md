---
title: "Tools"
description: "40+ built-in tools for files, code, web, memory, and planning."
keywords: ["tools", "bash", "lsp", "memory", "todos", "subagents"]
weight: 400
toc: false
---

OSA ships 40+ built-in tools behind a permission model:

1. **[Overview]({{< relref "overview.md" >}})** — the full catalogue and the deferred-loading pattern.
2. **[Files & code]({{< relref "files-code.md" >}})** — edits with diffs, `apply_patch`, fuzzy edit, LSP, code search.
3. **[Research]({{< relref "research.md" >}})** — web fetch/search/news, calendar, weather.

Core tools (files, edits, bash, grep/glob, web, subagent, todos, skills,
questions) are always loaded. Low-frequency tools load on demand via
`tool_search` — deny it in `[tools]` to keep everything loaded.
