---
title: "Troubleshooting"
description: "Fix common setup, provider, and tool problems."
weight: 30
toc: true
---

## Browser does not open / cannot reach localhost:8765

- Confirm OSAgent is running and check `[server]` bind/port in
  `~/.osagent/config.toml`.
- Try `http://127.0.0.1:8765` in case `localhost` resolution differs.
- Check firewall rules for local ports.

## Provider errors

- Verify the API key or OAuth credentials for the active provider.
- Check `default_provider` / `default_model` spelling against the
  `[[providers]]` blocks.
- For Ollama: confirm the daemon is up at the configured `base_url`.

## Agent cannot edit files

- Confirm the target is inside the configured workspace.
- Approve the permission prompt (outside-workspace access asks by default).
- Check the `[tools].denied` list and `permission_rules`.

## Discord bot stays silent

- Rebuild with `--features discord`.
- Verify `token`, `allowed_guilds` / `allowed_channels`, and that the bot has
  been invited with message permissions.
- DMs require `allow_dms = true` **plus** an explicit `allowed_users` entry.

## Still stuck?

Open an issue with repro steps, expected vs. actual behavior, and your OS and
OSA version: <https://github.com/jaylikesbunda/OSAgent/issues/new>

## Related tasks

- [Configuration]({{< relref "../getting-started/configuration.md" >}})
- [FAQ]({{< relref "faq.md" >}})
