---
title: "Web UI"
description: "Embedded chat, model picker, skills, and workflows at localhost:8765."
weight: 10
toc: true
---

## What this does

The built-in web UI is the primary interface: chat with the agent, switch
models, manage skills (**Settings → Skills**), and open the visual workflow
editor (experimental — enable it under **Settings** first). Same-origin
requests work with no CORS setup.

## Open it

1. Start OSAgent.
2. Open `http://localhost:8765` (or your configured `[server]` bind/port).
3. Sign in if `password_enabled = true` (use `osagent setup` to set the password).

## Highlights

- Chat composer with model selector and thinking controls
- Tool cards for file edits, command runs, and subagent activity
- Subagent status/resume controls with preserved timeout results
- Context ring showing provider-reported input + cache usage
- Transcript with code blocks, thinking blocks, and attachments

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Skills]({{< relref "../extend/skills.md" >}})
- [Workflows]({{< relref "../extend/workflows.md" >}})
