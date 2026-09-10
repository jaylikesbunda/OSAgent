---
title: "FAQ"
description: "Quick answers to common OSAgent questions."
weight: 40
toc: true
---

## Is anything sent to the cloud?

Only what you configure. Point OSA at local Ollama and it runs entirely
off-grid. Otherwise it talks to whichever providers you configured — nothing
else.

## What are the runtime dependencies?

None. A single binary with no Node, Python, or Docker.

## Which platforms are supported?

Windows, Linux, and macOS (installers on the releases page).

## How heavy is it?

Warm-starts in ~0.5s, idles at ~20 MB fresh / ~50 MB under use — roughly a
tenth of a typical Electron agent. Fine on a Raspberry Pi or 2 GB VPS.

## How do skills differ from MCP servers and workflows?

- **Skills** (`.oskill` bundles) add reusable agent capabilities.
- **MCP servers** connect external tool servers over MCP.
- **Workflows** chain steps into repeatable jobs in the visual editor.

## Where is the config?

`~/.osagent/config.toml`. Start from `config.example.toml` in the repo.

## Related tasks

- [Quick start]({{< relref "../getting-started/quick-start.md" >}})
- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Troubleshooting]({{< relref "troubleshooting.md" >}})
