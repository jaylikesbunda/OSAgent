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
- Follow-up queue: messages sent mid-turn wait in a panel above the composer,
  where each can be edited, reordered, steered (interrupt and run next), or
  removed; Enter follows your queue/steer preference, Ctrl+Enter uses the other

## Workspace files and changes

Open the file browser from the chat header or a tool's preview button. The
Files view loads each folder when expanded, supports multiple workspace roots,
and opens up to eight file tabs. Filter applies to the loaded tree. Source shows
line numbers; Markdown files also offer a rendered Preview.

Changes shows edits recorded in this chat, combining the first recorded
baseline with the latest contents for each file. Choose Diff, then Split view
to compare before and after. Tool snapshots are labelled separately from files
read directly from the workspace. This view does not represent every Git change.

The file browser reads within the chat's workspace roots. Text previews are
limited to 2 MB; binary and non-UTF-8 files report why they cannot be displayed.

## Persistent goals

Choose the target icon beside Context to open the inline goal editor. An
existing goal appears above the message field with its objective, status,
round usage, and Pause/Resume control. Options contains the next round budget
and Clear goal. Round usage measures the budget consumed, not completion.

Use `/goal Fix the failing build` to start a goal that continues across
agent turns. The default budget is five rounds; `/goal --rounds 10 Fix the
failing build` chooses a budget from 1 to 100.

- `/goal` or `/goal status` shows the objective, phase, and round count.
- `/goal pause` stops future goal rounds; the current turn can finish.
- `/goal resume` renews the round budget and resumes an unfinished goal.
- `/goal resume --rounds 3` resumes with a new budget.
- `/goal clear` removes the goal without erasing the conversation.

Stop also pauses an active goal. Exhausting the budget pauses it; it does
not count as completion. Goals survive restarts, but autonomous continuation
requires an explicit `/goal resume` after restarting. Clear an unfinished
goal before setting a different objective.

Stop interrupts the current run and holds queued messages. Send a new
message or use a queued message's send-now control to continue the queue;
use `/goal resume` to resume goal work. Pending messages are retained.

Grouped tools start collapsed. **Settings → Appearance → Tool Groups**
can show two, five, or all tool rows by default.

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Skills]({{< relref "../extend/skills.md" >}})
- [Workflows]({{< relref "../extend/workflows.md" >}})
