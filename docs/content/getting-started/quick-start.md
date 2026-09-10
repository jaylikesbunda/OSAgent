---
title: "Quick Start"
description: "Pick a provider, pick a workspace, and run your first task."
weight: 20
toc: true
---

## What this does

Takes you from a fresh install to a working agent session in a few minutes.

## Before you start

- OSAgent [installed]({{< relref "installation.md" >}})
- A provider API key **or** OAuth login **or** local Ollama running

## Run your first task

1. Open `http://localhost:8765` in your browser.
2. Pick a provider (OAuth or API key) and a workspace folder.
3. Send a first prompt, for example:

```text
List the files in my workspace and summarize what this project does.
```

4. Try a coding task next:

```text
Fix the failing script in ./scripts and explain what was wrong.
```

## Expected result

The agent replies in the chat UI, showing tool calls (file reads, edits,
command runs) as cards in the transcript. Approve any permission prompts for
actions outside the workspace.

## Tips

- Point the workspace at a copy of your project first while you learn the
  permission model.
- Coding and assistant tools ship side by side — list the ones you don't want
  under `[tools].denied` (see [Configuration]({{< relref "configuration.md" >}})).
- Warm-starts take ~0.5s; idle memory is ~20 MB fresh / ~50 MB under use.
  Reproduce with `cargo run --release --bin osagent-bench`.

## Related tasks

- [Configuration]({{< relref "configuration.md" >}})
- [Model providers]({{< relref "../models/providers.md" >}})
- [Tools overview]({{< relref "../tools/overview.md" >}})
