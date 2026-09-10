---
title: "Workflows"
description: "Repeatable multi-step jobs in the visual workflow editor."
weight: 30
toc: true
---

## What this does

The visual workflow editor chains prompts, tools, and conditions into
repeatable jobs — morning briefings, triage sweeps, release checklists. It is
experimental: enable it under **Settings** first (look for the Workflows
toggle), then open it from the header button.

## When to use what

| Need | Use |
|---|---|
| One-off task | Chat prompt |
| Repeatable multi-step job | Workflow |
| Reusable agent capability | Skill or MCP server |
| Time-based trigger | `schedule` tool |

## Related tasks

- [Scheduled tasks]({{< relref "../automation/scheduled-tasks.md" >}})
- [Skills]({{< relref "skills.md" >}})
