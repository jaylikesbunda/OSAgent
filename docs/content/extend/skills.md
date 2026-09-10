---
title: "Skills"
description: "Install .oskill bundles or let OSA author skills live."
weight: 10
toc: true
---

## What this does

A skill is a zip of `SKILL.md` (instructions) + `manifest.toml` (metadata),
renamed to `.oskill`. Install via **Settings → Skills** in the web UI.
Examples live in `examples/skills/`.

## Install a skill

1. Open the web UI → **Settings → Skills**.
2. Upload the `.oskill` file.
3. The agent can now load it via the `skill` tool.

## Live authoring

OSA can also create, update, and delete skills at runtime via
`skill_create` / `skill_update` / `skill_delete` — plain `SKILL.md` files, no
restart. `.oskill` bundles are kept for import only.

## Related tasks

- [MCP servers]({{< relref "mcp.md" >}})
- [Workflows]({{< relref "workflows.md" >}})
