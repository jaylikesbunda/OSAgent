---
title: "Scheduled Tasks"
description: "Cron jobs, reminders, recurring tasks, and daily briefings."
weight: 10
toc: true
---

## What this does

The `schedule` tool runs work on a schedule: reminders, agent prompts, or
daily briefings — notified via web, Discord, or both. Schedules are evaluated
in the machine's local timezone. One-time jobs and recurring jobs are stored
separately, so `in 30m` and `at 3pm` do not accidentally repeat forever.

## Examples

```text
Brief me every weekday at 8am on my calendar, the weather, and top tech news.
```

```text
Remind me in 30 minutes to check the oven.
```

`when` accepts `'in 30m'`, `'at 3pm'`, `'@daily'`, or cron (`'0 9 * * 1-5'`
for every weekday at 9am). The `schedule_type` field can explicitly set
`one_shot` or `recurring`; when omitted, `in ...` and `at ...` are one-shot
and other forms are recurring. Cron expressions support minute/hour,
day-of-month/month/day-of-week fields, ranges, lists, and steps. The Jobs
panel also provides **Run now** for testing a job without waiting for its next
scheduled occurrence.

## Config notes

- `schedule`, `coordinator`, and friends live in the deferred catalog — the
  agent loads them via `tool_search` when needed.
- Checkpoints (`checkpoint_enabled`, `checkpoint_interval`) let long-running
  automation resume sensibly.

## Related tasks

- [Tool overview]({{< relref "../tools/overview.md" >}})
- [Research & context]({{< relref "../tools/research.md" >}})
