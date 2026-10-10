---
title: "Discord Bot"
description: "One conversation per user, community mode, and trusted access."
weight: 20
toc: true
---

## What this does

Runs OSAgent inside Discord. Trusted users get one conversation that carries
across every channel and DM they use. Community members get one conversation
per server. Requires building with the `discord` feature. Audio attachments are
decoded to 16-bit WAV in-process before Whisper transcription.

> **Note:** `symphonia`/`ogg`/`opus` decode deps are gated behind the
> `discord` feature, and `songbird` playback behind `discord-voice`, so default
> builds stay light.

## Minimal config

```toml
[discord]
enabled = true
token = "your-discord-bot-token"
allowed_users = ["123456789012345678"]
```

## Access model

| Setting | Meaning |
|---|---|
| `allowed_users` / `allowed_roles` | Who may use restricted community chat |
| `allowed_guilds` / `allowed_channels` | Where the bot responds (empty = anywhere) |
| `allow_community_members` | Let all members of `allowed_guilds` use community chat |
| `trusted_users` / `trusted_roles` / `trusted_guilds` / `trusted_channels` | Full machine-capable access — keep narrow |
| `allow_dms` | DMs need this **plus** an explicit `allowed_users` entry |
| `community_mode` | Separate public support access from trusted access |

Optional extras: `community_context` system prompt, `docs_url`, and GitHub
new-issue/PR announcements (`github_repo`, `github_tracking_channel`,
`github_poll_seconds`).

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Voice]({{< relref "voice.md" >}})
- [Security]({{< relref "../operations/security.md" >}})
