---
title: "Research & Context"
description: "Web fetch/search/news, calendar, weather, memory, and notes."
weight: 30
toc: true
---

## Web

- `web_fetch` / `web_search` (Bing backend included for burst-tolerant general
  queries with direct result URLs) / `news`
- `calendar` and `weather` for briefings and planning

## Memory

- Persistent memory and decision stores (`record_memory`, `record_decision`)
- Rolling working notes plus real `/compact` (verify-and-correct handoff that
  resets context)
- Prompt caching reuses stable system-prompt prefixes (`prompt_cache_enabled`,
  on by default) to cut tokens and latency

## Planning

Todos, goals, plans, notes, and questions (`todowrite`, `create_goal`,
`plan_exit`, `update_notes`, `question`) structure multi-step work;
`coordinator` and `task` orchestrate it.

## Related tasks

- [Automation]({{< relref "../automation/scheduled-tasks.md" >}})
- [Tool overview]({{< relref "overview.md" >}})
