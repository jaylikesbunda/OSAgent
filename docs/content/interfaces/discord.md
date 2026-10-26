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

## Message context

Every message the agent receives is attributed: the prompt names the author
(server nickname when set, otherwise their display name) and says who wrote the
message being replied to. Several people can therefore talk to the same
conversation without the agent losing track of who said what.

Replies carry their context to the agent:

- Replying to a message includes that message's text.
- Images attached to the reply **or to the message it replies to** are
  downloaded and handed to the model directly, so "look at this" works on the
  picture in the original message even when the reply itself only contains the
  mention.
- Replying to a voice message transcribes it, and audio attached to the reply
  is transcribed too.

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
| `moderation_enabled` | Register the `/mod` moderation tools — `trusted_users` only |

Optional extras: `community_context` system prompt, `docs_url`, and GitHub
new-issue/PR announcements (`github_repo`, `github_tracking_channel`,
`github_poll_seconds`).

## Moderation tools

`/mod` covers basic community-server moderation: `warn`, `timeout`, `untimeout`,
`kick`, `ban`, `unban`, `purge`, and `slowmode`. Every part of it is narrow by
design:

- **Opt-in.** `moderation_enabled = true` (default off). `/mod` is only
  registered while the flag is on — toggle it in the web UI (Discord → Trusted
  Full Access) or with `/discord-setup mod-tools on`, and the command list
  updates immediately.
- **Trusted users only.** `trusted_users` is the only key. Trusted *roles* and
  community grants (allowed users, allowed roles, all-member access) never
  unlock `/mod`, and DMs never reach it.
- **Known servers only.** The invocation must land in a server listed in
  `trusted_guilds` or `allowed_guilds` (channel lists deliberately do not
  narrow moderation).
- **Discord permissions still apply.** The invoker *and* the bot need the
  matching permission (Moderate Members, Kick Members, Ban Members, Manage
  Messages, or Manage Channels), and actions only land on targets below both
  highest role positions.
- **Protected targets.** The server owner, other bots, and anyone in
  `trusted_users` / `trusted_roles` can never be targeted.

```toml
[discord]
moderation_enabled = true
```

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Voice]({{< relref "voice.md" >}})
- [Security]({{< relref "../operations/security.md" >}})
