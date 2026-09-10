---
title: "Security"
description: "Report vulnerabilities and harden your OSAgent install."
weight: 10
toc: true
---

## Report a vulnerability

Do **not** open a public GitHub issue for security vulnerabilities. Use
GitHub's private reporting form:

<https://github.com/jaylikesbunda/OSAgent/security/advisories/new>

Include a clear description, affected versions/commits, reproduction steps or
proof of concept, and any suggested mitigation. Give maintainers reasonable
time to fix before disclosing publicly.

## Harden your install

- Enable `password_enabled` and set the password with `osagent setup`.
- Keep `jwt_secret` long and random — rotating it signs out all sessions.
- Leave `cors_allowed_origins` empty unless you need cross-origin API access.
- Keep Discord `trusted_*` lists narrow; DMs need `allow_dms` **plus** an
  explicit `allowed_users` entry.
- Point the workspace at the smallest folder the agent needs, and deny tools
  you don't use via `[tools].denied`.

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Discord bot]({{< relref "../interfaces/discord.md" >}})
