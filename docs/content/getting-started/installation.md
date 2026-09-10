---
title: "Installation"
description: "Download and install OSAgent on Windows, Linux, or macOS."
weight: 10
toc: true
---

## What this does

Installs the OSAgent single binary (plus launcher/updater) on your machine.
No Node, Python, or Docker required.

## Before you start

- Windows 10/11, a modern Linux distro, or macOS
- A model provider account (or a local Ollama instance for fully offline use)

## Install

1. Open the [**latest release**](https://github.com/jaylikesbunda/OSAgent/releases/latest).
2. Download the installer for your platform:
   - Windows: `osagent-windows-x86_64-setup.exe`
   - Linux: `.deb` package
   - macOS: `.dmg` image
3. Run the installer and launch OSAgent.
4. Your browser opens at `http://localhost:8765`.

## Expected result

`osagent setup` (included in the install) sets a web-UI password, offers 8
provider presets, and writes `~/.osagent/config.toml`. Then `osagent start`
serves the web UI — open `http://localhost:8765` and log in.

## Build from source instead

```powershell
git clone https://github.com/jaylikesbunda/OSAgent.git
cd OSAgent
.\build-launcher.ps1 -Installer
```

`build-launcher.ps1` is Windows-only; Linux/macOS use `launcher/build.sh`.
See [Building from source]({{< relref "../operations/building.md" >}}) for details.

## Related tasks

- [Quick start]({{< relref "quick-start.md" >}})
- [Configuration]({{< relref "configuration.md" >}})
- [Troubleshooting]({{< relref "../operations/troubleshooting.md" >}})
