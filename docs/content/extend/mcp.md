---
title: "MCP Servers"
description: "Connect external Model Context Protocol tool servers."
weight: 20
toc: true
---

## What this does

MCP servers expose extra tools to the agent over the Model Context Protocol.
They use the same deferred-catalog pattern as built-in low-frequency tools:
the agent sees the manifest and loads schemas via `tool_search`.

## Configure

Easiest path: **Settings → MCP Servers** in the web UI (includes a "Test
connection" button). By hand, add blocks to `~/.osagent/config.toml`:

```toml
[mcp]
enabled = true

# Local server over stdio:
[[mcp.servers]]
name = "filesystem"
command = "npx"
args = ["-y", "@modelcontextprotocol/server-filesystem", "~/notes"]

# Remote server over HTTP:
[[mcp.servers]]
name = "linear"
url = "https://mcp.linear.app/mcp"
headers = { Authorization = "Bearer YOUR_TOKEN" }
```

MCP tools load on demand through `tool_search` (capped by
`max_activated_tools`); naming a tool under `always_active` keeps it loaded in
every request. Verify with a prompt like:

```text
List your available tools and confirm the MCP server tools appear.
```

## Related tasks

- [Skills]({{< relref "skills.md" >}})
- [Tool overview]({{< relref "../tools/overview.md" >}})
