---
title: "Configuration"
description: "Providers, workspaces, tools, and permissions in config.toml."
weight: 30
toc: true
---

## What this does

Configures where OSAgent listens, which models it can use, what folder it may
touch, and which tools are available. Config lives at `~/.osagent/config.toml`.

## Minimal config

```toml
[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."
model = "anthropic/claude-sonnet-4"

[agent]
workspace = "~/.osagent/workspace"
```

The full annotated template is [`config.example.toml`](https://github.com/jaylikesbunda/OSAgent/blob/main/config.example.toml)
in the repo root.

## Key sections

| Section | What it controls |
|---|---|
| `[server]` | Bind address, port, password, JWT secret, CORS origins |
| `[[providers]]` | One block per provider (API key, OAuth, AWS, local Ollama) |
| `[agent]` | Workspace(s), tokens, temperature, checkpoints, subagent limits |
| `[tools]` | `denied` / `allowed` lists, per-tool limits and timeouts |
| `[discord]` | Discord bot token and access lists (optional) |

API keys are also auto-detected from environment variables
(`OPENROUTER_API_KEY`, `OPENAI_API_KEY`, `ANTHROPIC_API_KEY`,
`GOOGLE_API_KEY`, `GROQ_API_KEY`, `DEEPSEEK_API_KEY`, `XAI_API_KEY`).

## Trim the toolset

Coding and assistant tools ship side by side. Remove what you don't need with
`denied`:

```toml
[tools]
denied = ["news", "calendar"]
```

(To keep every tool loaded in all requests instead of on demand, deny
`tool_search` — see [Tool overview]({{< relref "../tools/overview.md" >}}).)

## Apply changes

Restart OSAgent after editing `config.toml`. CORS changes and JWT secret
changes always require a restart (rotating the secret signs out all sessions).
Inspect the live config anytime with `osagent config show`.

## Related tasks

- [Model providers]({{< relref "../models/providers.md" >}})
- [OAuth login]({{< relref "../models/oauth.md" >}})
- [Tools overview]({{< relref "../tools/overview.md" >}})
- [Security]({{< relref "../operations/security.md" >}})
