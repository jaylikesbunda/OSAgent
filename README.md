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

An AI agent that lives on your machine, not in the cloud. Point it at a folder and it refactors code, fixes failing scripts, researches the web, or briefs you every morning. Talks to 200+ model providers through a web UI, a Discord bot, or your voice.

## What it does

- **Local-first** — nothing is sent anywhere you didn't configure; point it at a local Ollama instance and it runs entirely off-grid.
- **Lightweight** — a single binary with no Node, Python, or Docker: warm-starts in ~0.5s and idles at ~20 MB fresh / ~50 MB under use (roughly a tenth of a typical Electron agent), happy on a Raspberry Pi or a 2GB VPS. Reproduce with `cargo run --release --bin osagent-bench`.
- **Models** — 200+ providers and 7,300+ models (OpenRouter, OpenAI, Anthropic, Google, Ollama, Bedrock, Azure, …), plus OAuth login for OpenAI, Anthropic, Google, GitHub Copilot, Qwen, and Chutes.
- **Interfaces** — embedded web UI · Discord bot with per-channel sessions · voice (Whisper STT + Piper TTS).
- **Tools** — 40+ built-ins: file edit/patch, bash + Python/Node execution, grep/glob/code search, LSP, web fetch/search/news, calendar, weather, persistent memory, todos/goals/plans, background subagents.
- **Automation** — cron jobs, reminders, recurring tasks, daily briefings.
- **Extendable** — visual workflow editor · installable `.oskill` skill bundles · MCP servers · sandboxed tool scripts.

Coding and assistant tools ship side by side — trim `[tools].allowed` to whichever half you want.

## Quick Start

Download and run [`osagent-windows-x86_64-setup.exe`](https://github.com/jaylikesbunda/OSAgent/releases/latest) from the [**latest release**](https://github.com/jaylikesbunda/OSAgent/releases/latest). Pick a provider (OAuth or API key), pick a workspace, done — your browser opens at `http://localhost:8765`.

Linux (`.deb`) and macOS (`.dmg`) installers are on the same releases page.

<details>
<summary><strong>Configuration</strong></summary>

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

</details>

## Skills

A skill is a zip of `SKILL.md` (instructions) + `manifest.toml` (metadata), renamed to `.oskill` and installed via **Settings → Skills** in the web UI. Examples in `examples/skills/`.

<details>
<summary><strong>Building from Source</strong></summary>

```powershell
git clone https://github.com/jaylikesbunda/OSAgent.git
cd OSAgent
.\build-launcher.ps1 -Installer
```

`build-launcher.ps1` is Windows-only; Linux/macOS use `launcher/build.sh`. See [RELEASING.md](RELEASING.md) for the release flow.

</details>

## Contributing & License

Bug reports and PRs welcome; see [CONTRIBUTING.md](CONTRIBUTING.md), [SECURITY.md](SECURITY.md), [CHANGELOG.md](CHANGELOG.md). [MIT](LICENSE).
