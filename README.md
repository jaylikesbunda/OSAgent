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

An AI agent that belongs on your desktop, not in the cloud.

Point it at a folder and ask it to do things: refactor a codebase, run a script and fix what breaks, research something across a dozen pages, or check your calendar and brief you every morning. It talks to 100+ model providers, runs as a single binary with no runtime to install, and reaches you through a web UI, a Discord bot, or your voice.

Nothing is sent anywhere you didn't configure. Point it at a local Ollama instance and it runs entirely off-grid.

## Quick Start

Download the latest release for your platform from [GitHub Releases](https://github.com/jaylikesbunda/OSAgent/releases):

| Platform | Asset |
|---|---|
| Windows | `osagent-windows-x86_64-setup.exe` |
| Linux (x86_64) | `osagent-linux-x86_64.deb` |
| macOS (Apple Silicon) | `osagent-macos-arm64.dmg` |

Then run the launcher and follow the wizard: pick a provider (OAuth or API key), pick a workspace folder, done. Your browser opens to `http://localhost:8765`.

Prefer the terminal:

```bash
osagent start                      # Start with default config
osagent start -w /path/to/project  # Start with a specific workspace
```

## Features

**Models**
- 100+ providers: OpenRouter, OpenAI, Anthropic, Google AI, GitHub Copilot, OpenAI Codex, Ollama, Groq, DeepSeek, xAI, AWS Bedrock, Azure, and more
- OAuth login for GitHub Copilot, Google, and OpenAI Codex, so no API keys needed

**Interfaces**
- Web UI: chat interface at `localhost:8765`
- Discord bot: slash commands, per-channel/per-user sessions, live tool progress
- Voice I/O: Whisper STT + Piper TTS, with browser fallback

**What it can do**
- 30+ built-in tools: file read/write/patch, code execution (Python/Node/Bash), grep/glob, LSP, web fetch and search, process control
- Desktop assistant tools: calendar, weather, news, system status, persistent memory
- Jobs scheduling: cron-based reminders, recurring tasks, daily briefings

**Extending it**
- Visual workflow editor: node-based drag-and-drop automation with conditions, loops, and branching
- Skills system: installable `.oskill` bundles for custom integrations

It ships coding tools and assistant tools side by side, so trim `[tools].allowed` to whichever half you actually want.

## Performance

~543ms to ready, ~13.7MB RSS at ready, ~50MB installed. Measured with in-repo runtime benchmarks (release, provider-free workloads, 10 runs on 2026-04-08):

```bash
cargo run --release --bin osagent-bench -- --profiles release --iterations 10
```

## Configuration

Config lives at `~/.osagent/config.toml`. A minimal one is just a provider:

```toml
[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."
model = "anthropic/claude-sonnet-4"

[agent]
workspace = "~/.osagent/workspace"
```

<details>
<summary>Full config reference</summary>

```toml
[server]
bind = "127.0.0.1"
port = 8765
password = "$2b$12$..."  # bcrypt hash (optional)

[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."
model = "anthropic/claude-sonnet-4"

[agent]
workspace = "~/.osagent/workspace"
max_tokens = 4096

[tools]
allowed = ["bash", "read_file", "write_file", "grep", "glob", "code_python"]

[discord]
token = "your-bot-token"
allowed_users = ["1234567890"]
```

</details>

## Skills

A skill is a zipped folder containing `SKILL.md` (agent instructions) and `manifest.toml` (metadata), renamed to `.oskill`. Build one and install it via **Settings → Skills** in the Web UI:

```bash
cd my-skill
zip -r ../my-skill.oskill *
```

See `examples/skills/` for examples.

## Building from Source

```powershell
git clone https://github.com/jaylikesbunda/OSAgent.git
cd OSAgent
.\build-launcher.ps1 -Installer
```

`build-launcher.ps1` is Windows-only. On Linux and macOS, run `launcher/build.sh`, which checks formatting, runs clippy, and builds the release binary. Packaged installers (`.deb` / `.dmg`) are produced by the release pipeline rather than locally:

- Windows: `launcher/target/release/bundle/nsis/*.exe` (installer)
- Linux: `launcher/target/release/bundle/deb/*.deb`
- macOS: `launcher/target/release/bundle/macos/*.app`

See [RELEASING.md](RELEASING.md) for the full release flow.

## Contributing

Bug reports and pull requests are welcome; see [CONTRIBUTING.md](CONTRIBUTING.md). For security issues, please follow [SECURITY.md](SECURITY.md) rather than opening a public issue. Release notes live in [CHANGELOG.md](CHANGELOG.md).

## License

[MIT](LICENSE)
