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
- Keep the OS sandbox at `workspace-write` (the default) and raise it to
  `danger-full-access` only for sessions that genuinely need it.

## OS sandbox

The sandbox is an OS capability axis, separate from the approval prompts:
approvals decide *who signs off*, the sandbox decides *what is even possible*
once a tool runs. Configure it under `[sandbox]` or in Settings -> Security ->
OS Sandbox.

| Mode | Behavior |
| --- | --- |
| `read-only` | Nothing may be modified anywhere. Mutating tools fail; spawned processes can only read. |
| `workspace-write` | Reads anywhere; spawned processes can only write inside the workspace roots, `extra_roots`, and the OSA scratch dir. The default. |
| `danger-full-access` | No OS confinement. |

Enforcement is real, not advisory:

- **Windows** — spawned processes run under a privilege-stripped restricted
  token at Low integrity; the writable roots carry mandatory-label ACLs, so
  writes outside them fail at the filesystem. Every process tree lands in a
  kill-on-close Job Object. With `network = "deny"` the process additionally
  runs in an AppContainer and WFP firewall rules keyed to its package SID
  block outbound connections (adding rules needs admin rights — without them
  OSA reports filesystem-only enforcement instead of pretending).
- **Linux** — Landlock confines writes to the roots and a seccomp deny-list
  blocks mount/ptrace-class syscalls (and socket creation with
  `network = "deny"`).
- **macOS** — commands run under Seatbelt (`sandbox-exec`) with a generated
  profile per mode.

In-process tools (file writes/edits/deletes) enforce `read-only` directly;
writes outside the workspace in `workspace-write` mode still go through the
external-path approval prompt. Prompt approvals ("Allow once" / "Always
allow") are scoped to the chat that asked — they never leak into other
sessions — and the composer's approval behavior (`agent.approval_policy`) is
a per-chat ceiling: `deny` refuses everything outside the workspace even if
an earlier approval exists. The bash command blocklist remains as defense in
depth — the OS sandbox is the boundary behind it. Settings shows the live
`SandboxStatus` (backend, enforced, degradations) so OSA never claims
confinement it did not get.

## Spend limits

`[usage]` records every provider request with its token split and computed
cost (per-model prices from the bundled catalog, or your own
`[usage.price_overrides]`). Costs appear on every turn, in `/usage`
(Settings -> Usage & Budgets, or `osagent usage`). When a `session`/`daily`/
`monthly` limit trips, the cost guard either routes turns through the
`small_model` slot or blocks them until the window resets.

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [Discord bot]({{< relref "../interfaces/discord.md" >}})
