---
title: "Scheduled Tasks"
description: "Cron jobs, reminders, recurring tasks, and daily briefings."
weight: 10
toc: true
---

## What this does

The `schedule` tool runs work on a schedule: reminders, agent prompts, or
daily briefings — notified via web, Discord, or both.

## Examples

```text
Brief me every weekday at 8am on my calendar, the weather, and top tech news.
```

```text
Remind me in 30 minutes to check the oven.
```

`when` accepts `'in 30m'`, `'at 3pm'`, `'@daily'`, or cron (`'0 9 * * 1-5'`
for every weekday at 9am).

## Config notes

- `schedule`, `coordinator`, and friends live in the deferred catalog — the
  agent loads them via `tool_search` when needed.
- Checkpoints (`checkpoint_enabled`, `checkpoint_interval`) let long-running
  automation resume sensibly.

## Related tasks

- [Tool overview]({{< relref "../tools/overview.md" >}})
- [Research & context]({{< relref "../tools/research.md" >}})
