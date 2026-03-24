<p align="center">
  <a href="https://github.com/osagent/osagent">
    <img src="frontend/images/thinking-indicator.png" alt="OSA Logo" width="120">
  </a>
</p>

<h1 align="center">OSA - Your Open Source Agent</h1>

<p align="center"><strong>Your personal AI agent. Fast, local and secure.</strong></p>

<p align="center">
  <a href="LICENSE"><img src="https://img.shields.io/badge/license-MIT-blue?style=flat-square" alt="License"></a>
  <a href="https://www.rust-lang.org/"><img src="https://img.shields.io/badge/Rust-1.70+-orange?style=flat-square" alt="Rust"></a>
</p>

<p align="center">
  <a href="#installation">Install</a> •
  <a href="#quick-start">Quick Start</a> •
  <a href="#features">Features</a> •
  <a href="https://github.com/osagent/osagent/releases">Releases</a>
</p>

### Installation

**All Platforms**
Download the package for your platform from [Releases](https://github.com/osagent/osagent/releases/latest):
- Windows: `osagent-windows-x86_64.zip`
- Linux: `osagent-linux-x86_64.tar.gz`
- macOS: `osagent-macos-arm64.tar.gz`

Extract and run:
```bash
./osagent-launcher    # Opens setup wizard, then starts OSA
```

The package includes:
- `osagent` — The core agent binary
- `osagent-launcher` — GUI setup and management

The launcher will:
1. Guide you through setup (API key, workspace)
2. Start OSA at http://localhost:8765

No Rust installation required.

### Quick Start

```bash
./osagent-launcher    # Opens setup wizard, then starts OSA
```

### Features

- **Multi-Provider** — OpenRouter, OpenAI, Anthropic, Google, Ollama, Groq, DeepSeek, xAI
- **Web UI** — Modern chat with workspaces and tool visualization
- **Discord Bot** — Deploy as a Discord bot with slash commands, per-channel sessions, and thinking indicators
- **Tool Execution** — Bash, Python, Node, file ops, web search
- **Skills System** — Extend with custom integrations
- **Voice STT/TTS** — Whisper speech-to-text and Piper local text-to-speech with browser Web Speech API support
- **Local-First** — Runs entirely on your machine

### Configuration

Config is stored in `~/.osagent/config.toml`. The launcher handles setup, but you can edit manually:

```toml
[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."
model = "anthropic/claude-sonnet-4"
```

### Tools

Enable tools in `~/.osagent/config.toml`:
```toml
[tools]
allowed = ["bash", "read_file", "write_file", "grep", "glob", "code_python", "code_node"]
```

### Skills

Extend OSA with custom integrations. Create a skill:

```bash
mkdir my-skill && cd my-skill
# Create SKILL.md (agent instructions) and manifest.toml (metadata)
zip -r ../my-skill.oskill *
# Install via Settings → Skills
```

See `examples/skills/` for examples: GitHub, Spotify, Word.

### License

[MIT](LICENSE)
