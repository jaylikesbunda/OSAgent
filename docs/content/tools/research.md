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
- `browser` drives a sandboxed headless Chromium for JavaScript-heavy or
  interactive pages. Pages come back as an outline with `[ref=eN]` handles;
  act with `click`, `fill` (several fields at once), `type`, `press`,
  `scroll`, `wait`, `select`, `tabs`. Use `read` for article text and
  `screenshot` (optionally `annotate`) only when a visual check is needed.
  Each session gets a throwaway profile, local and private network addresses
  are blocked, downloads are refused and `eval` is off unless enabled. Signed-in
  sessions can be imported per site under Settings → Browser. Configure it in
  the `[browser]` section of the config.

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
