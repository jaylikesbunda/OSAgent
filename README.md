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

An AI agent that belongs on your desktop, not in the cloud. Single binary, zero runtime dependencies, built with Rust for performance and reliability.

## Features

- **100+ LLM providers** — OpenRouter, OpenAI, Anthropic, Google AI, GitHub Copilot, OpenAI Codex, Ollama, Groq, DeepSeek, xAI, AWS Bedrock, Azure, and more
- **OAuth login** — Sign in with GitHub Copilot, Google, or OpenAI Codex (no API keys needed)
- **Discord bot** — Slash commands, per-channel/per-user sessions, real-time tool progress, thinking indicators
- **Web UI** — Modern chat interface at `localhost:8765`
- **30+ built-in tools** — File ops, code execution (Python/Node/Bash), grep/glob, web fetch, LSP, and more
- **Voice I/O** — Whisper STT + Piper TTS with browser fallback
- **Visual workflow editor** — Node-based drag-and-drop automation with conditions, loops, and branching logic
- **Skills system** — Installable `.oskill` bundles for custom integrations
- **Jobs scheduling** — Cron-based reminders, recurring tasks, and daily briefings

## Benchmarks

Measured with in-repo runtime benchmarks (release, provider-free workloads, 10 runs on 2026-04-08):

| Metric | OSAgent |
|---|---|
| Startup to ready | ~543ms |
| Ready RSS | ~13.68MB |
| Idle RSS | ~22.66MB |
| Install size | ~50MB single binary |
| Runtime deps | Zero |

```bash
cargo run --release --bin osagent-bench -- --profiles release --iterations 10
```

## Quick Start

### Download

Download the latest release for your platform from [GitHub Releases](https://github.com/jaylikesbunda/OSAgent/releases):

| Platform | Asset |
|---|---|
| Windows | `osagent-windows-x86_64-setup.exe` or `.zip` |
| Linux (x86_64) | `osagent-linux-x86_64.tar.gz` |
| Linux (ARM64) | `osagent-linux-arm64.tar.gz` |
| macOS (Apple Silicon) | `osagent-macos-arm64.tar.gz` |
| macOS (Intel) | `osagent-macos-x86_64.tar.gz` |

Auto-updates are served via `https://osa.fuckyourcdn.com/releases/latest.json`.

### Setup Wizard

1. Run the launcher
2. Choose your provider (OAuth or API key)
3. Select a workspace folder
4. Done — browser opens to `http://localhost:8765`

### CLI

```bash
osagent start                    # Start with default config
osagent start -w /path/to/project  # Start with a specific workspace
```

## Configuration

Config stored at `~/.osagent/config.toml`:

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

## Skills

Extend OSAgent with custom integrations:

```bash
mkdir my-skill && cd my-skill
# Create SKILL.md (agent instructions) and manifest.toml (metadata)
zip -r ../my-skill.oskill *
# Install via Settings → Skills in the Web UI
```

See `examples/skills/` for examples.

## Building from Source

```powershell
git clone https://github.com/jaylikesbunda/OSAgent.git
cd osagent
.\build-launcher.ps1 -Checks
```

Release artifacts:
- Windows: `launcher/target/release/osagent-launcher.exe`
- Linux: `launcher/target/release/osagent-launcher`

See `RELEASING.md` for the full release flow.

## FAQ

**What do I need to get started?** Bring your own model. Use GitHub Copilot or OpenAI Codex via OAuth, any OpenRouter/Anthropic API key, or Ollama for fully local models. Download the binary, run it, open `localhost:8765`.

**Can I run it fully offline?** Yes. Point at a local Ollama instance and you're off-grid.

**How does Discord integration work?** Add your bot token to config. OSAgent becomes a Discord bot with per-channel sessions, slash commands, and real-time tool progress. Same binary as the web UI.

## License

[MIT](LICENSE)
