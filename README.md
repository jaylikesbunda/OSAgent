<p align="center">
  <img src="frontend/images/thinking-indicator.png" alt="OSA Logo" width="120">
</p>

<h1 align="center">OSAgent</h1>

<p align="center"><strong>Your open source agent. Rust-powered, with zero runtime deps.</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/built%20with-Rust-orange?style=flat-square" alt="Rust"></a>
  <img src="https://img.shields.io/badge/platform-Windows%20%7C%20macOS%20%7C%20Linux-lightgrey?style=flat-square" alt="Platform">
</p>

---

## Why OSAgent?

| | OSAgent |
|---|---|
| **Runtime** | Single binary, zero deps |
| **Cold start** | ~3ms |
| **Memory** | ~20MB |
| **Discord bot** | Built-in |
| **OAuth providers** | GitHub Copilot, Codex |
| **Workflow editor** | Visual node-based |

## Features

- **200+ models** — OpenRouter, OpenAI, Anthropic, Google AI, GitHub Copilot, OpenAI Codex, Ollama, Groq, DeepSeek, xAI, AWS Bedrock, Azure, and more
- **OAuth authentication** — Sign in with GitHub Copilot, Google, or OpenAI Codex (no API keys needed)
- **Discord bot** — Slash commands, per-channel sessions, thinking indicators, tool execution feedback
- **Web UI** — Modern chat interface at `localhost:8765`
- **24 tools** — File ops, code execution (Python/Node/Bash), grep/glob, web fetch, LSP, and more
- **Voice I/O** — Whisper STT + Piper TTS with browser fallback
- **Visual workflow editor** — Node-based drag-and-drop automation (experimental)
- **Skills system** — Installable `.oskill` bundles for custom integrations

## Quick Start

### Download

Grab the latest release for your platform:

```
# Windows
osagent-launcher.exe

# macOS
osagent-launcher

# Linux
./osagent-launcher
```

### Setup Wizard

1. Run the launcher
2. Choose your provider (OAuth or API key)
3. Select a workspace folder
4. Done — browser opens to `http://localhost:8765`

### CLI

```bash
# Start with default config
osagent start

# Start with a specific workspace
osagent start -w /path/to/project

# Install as system service (Linux/macOS)
osagent service install
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
```

## Discord Bot

1. Create a bot at [Discord Developer Portal](https://discord.com/developers/applications)
2. Add token to config:

```toml
[discord]
token = "your-bot-token"
allowed_users = ["1234567890"]
```

3. Invite bot to server with `applications.commands` scope
4. Use `/help` to see available commands

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

```bash
git clone https://github.com/jaylikesbunda/OSAgent.git
cd osagent
cargo build --release
```

The binary will be at `target/release/osagent`.

## License

[MIT](LICENSE)
