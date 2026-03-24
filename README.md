<p align="center">
  <a href="https://github.com/osagent/osagent">
    <img src="frontend/images/thinking-indicator.png" alt="OSA Logo" width="120">
  </a>
</p>

<h1 align="center">OSA - Open Source Agent</h1>

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

**macOS / Linux**
```bash
curl -sSL https://raw.githubusercontent.com/osagent/osagent/main/install.sh | bash
```

**Windows**
Download from [Releases](https://github.com/osagent/osagent/releases/latest)

**From Source**
```bash
cargo build --release
```

### Quick Start

```bash
osagent setup    # Configure your API key
osagent start    # Launch at http://localhost:8765
```

### Features

- **Multi-Provider** — OpenRouter, OpenAI, Anthropic, Google, Ollama, Groq, DeepSeek, xAI
- **Web UI** — Modern chat with workspaces and tool visualization
- **Tool Execution** — Bash, Python, Node, file ops, web search
- **Skills System** — Extend with custom integrations
- **Audit Logging** — Full action history for compliance
- **Local-First** — Runs entirely on your machine

### Configuration

Set your API key:
```bash
export OPENROUTER_API_KEY=sk-or-v1-...
# or: OPENAI_API_KEY, ANTHROPIC_API_KEY, GOOGLE_API_KEY, GROQ_API_KEY, DEEPSEEK_API_KEY, XAI_API_KEY
```

Or configure directly in `~/.osagent/config.toml`:
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

### CLI Commands

```bash
osagent start            # Start the agent server
osagent setup            # Run the interactive setup wizard
osagent config show      # Display current configuration
osagent config edit      # Edit configuration in $EDITOR
osagent service install  # Install as system service
osagent update           # Check for updates
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
