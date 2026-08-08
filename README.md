<p align="center">
  <img src="frontend/images/thinking-indicator.png" alt="OSA Logo" width="120">
</p>

<h1 align="center">OSAgent</h1>

<p align="center"><strong>Open source local-first AI agent. Rust-powered, zero runtime deps.</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square" alt="Rust"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20Linux%20%7C%20macOS-lightgrey?style=flat-square" alt="Platform">
  <a href="https://opensourceagent.net"><img src="https://img.shields.io/badge/website-opensourceagent.net-blue?style=flat-square" alt="Website"></a>
</p>

---

An AI agent that lives on your machine, not in the cloud. Point it at a folder and it refactors code, fixes failing scripts, researches the web, or briefs you every morning. Talks to 100+ model providers through a web UI, a Discord bot, or your voice.

Nothing is sent anywhere you didn't configure. Point it at a local Ollama instance and it runs entirely off-grid.

## Why OSAgent

- **Stays on, unnoticed** — starts in ~540ms and idles at ~14MB of RAM (1/50th of a typical Electron agent), so it can run the background jobs, Discord sessions, and daily briefings around the clock.
- **Runs anywhere** — a single binary with no Node, Python, or Docker: a 2GB VPS, a Raspberry Pi, a locked-down corporate laptop, a container fleet.
- **Your machine, your model** — local via Ollama, or any API key.

## Quick Start

| Platform | Asset |
|---|---|
| Windows | `osagent-windows-x86_64-setup.exe` |
| Linux (x86_64) | `osagent-linux-x86_64.deb` |
| macOS (Apple Silicon) | `osagent-macos-arm64.dmg` |

Download the installer, pick a provider (OAuth or API key), pick a workspace — done. Your browser opens to `http://localhost:8765`.

Prefer the terminal:

```bash
osagent start                      # Start with default config
osagent start -w /path/to/project  # Start with a specific workspace
```

## What it does

- **Models** — 100+ providers (OpenRouter, OpenAI, Anthropic, Google, Ollama, Bedrock, Azure, ...), OAuth login for GitHub Copilot / Google / OpenAI Codex
- **Interfaces** — web UI, Discord bot with per-channel sessions, voice (Whisper STT + Piper TTS)
- **Tools** — 30+ built-in: file edit, code execution, grep/glob, LSP, web fetch/search, calendar, weather, persistent memory
- **Scheduling** — cron jobs, reminders, recurring tasks, daily briefings
- **Extending** — visual workflow editor, installable `.oskill` skill bundles

Coding tools and assistant tools ship side by side — trim `[tools].allowed` to whichever half you want.

## Configuration

Config lives at `~/.osagent/config.toml`. Minimal:

```toml
[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."
model = "anthropic/claude-sonnet-4"

[agent]
workspace = "~/.osagent/workspace"
```

Everything else is in [`config.example.toml`](config.example.toml).

## Skills

A skill is a zip of `SKILL.md` (instructions) + `manifest.toml` (metadata), renamed to `.oskill` and installed via **Settings → Skills** in the web UI. Examples in `examples/skills/`.

## Building from Source

```powershell
git clone https://github.com/jaylikesbunda/OSAgent.git
cd OSAgent
.\build-launcher.ps1 -Installer
```

`build-launcher.ps1` is Windows-only; Linux/macOS use `launcher/build.sh`. See [RELEASING.md](RELEASING.md) for the release flow.

## Contributing & License

Bug reports and PRs welcome — see [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), [CHANGELOG.md](CHANGELOG.md). [MIT](LICENSE).
