# OSA - Open Source Agent

A secure, local-first AI agent with a web UI, workspace management, and tool execution capabilities.

## Features

- **Web UI** - Modern chat interface with model selection, workspace management, and tool output visualization
- **Multi-Provider Support** - OpenRouter, OpenAI, Anthropic, Google AI, Ollama, Groq, DeepSeek, and xAI
- **Workspace Management** - Organize work into isolated workspaces with configurable permissions
- **Tool Execution** - Execute code (Python, Node, Bash), search files, manage tasks, and more
- **Discord Bot** - Optional Discord integration for chat-based access
- **Audit Logging** - Track all agent actions for security and compliance

## Requirements

- Rust 1.70+ (for building from source)
- OpenAI/Anthropic/etc. API key for your chosen provider
- Linux, macOS, or Windows

## Quick Start

### 1. Install OSA

**macOS/Linux:**
```bash
curl -sSL https://raw.githubusercontent.com/osagent/osagent/main/install.sh | bash
```

**Windows:**
Download the latest release from [GitHub](https://github.com/osagent/osagent/releases/latest)

**From Source:**
```bash
cargo build --release
```

### 2. Run Setup Wizard

```bash
osagent setup
```

This interactive wizard will:
- Generate a secure password for the web UI
- Help you configure your AI provider and API key
- Set up default workspaces

### 3. Start the Agent

```bash
osagent start
```

Then open http://localhost:8765 in your browser and log in with your password.

## Configuration

Config file: `~/.osagent/config.toml`

### Provider Setup

OSA auto-detects API keys from environment variables:
- `OPENROUTER_API_KEY`
- `OPENAI_API_KEY`
- `ANTHROPIC_API_KEY`
- `GOOGLE_API_KEY`
- `GROQ_API_KEY`
- `DEEPSEEK_API_KEY`
- `XAI_API_KEY`

Or configure directly in `config.toml`:

```toml
[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."
model = "anthropic/claude-sonnet-4"

[agent]
workspace = "~/.osagent/workspace"
```

### Available Models

| Provider   | Example Models |
|------------|----------------|
| OpenRouter | Claude Sonnet 4, GPT-4.1, Gemini 2.5 Pro, Llama 3.1 |
| OpenAI     | GPT-4.1, GPT-4o, o3-mini |
| Anthropic  | Claude Sonnet 4, Claude 3.5 Sonnet |
| Google     | Gemini 2.5 Pro, Gemini 2.0 Flash |
| Ollama     | Llama 3.1 70B, Qwen3 32B, Mistral 7B |
| Groq       | Llama 3.3 70B, Mixtral 8x7B |
| DeepSeek   | DeepSeek R1, DeepSeek V3 |
| xAI        | Grok 3 |

## Usage

### CLI Commands

```bash
osagent start          # Start the agent server
osagent setup         # Run the interactive setup wizard
osagent config show    # Display current configuration
osagent config edit   # Edit configuration in $EDITOR
osagent service install  # Install as system service
osagent update        # Check for updates
```

### Web UI

- **New Chat** - Start a new conversation
- **Model Selector** - Switch between providers/models
- **Workspaces** - Manage isolated working directories
- **Tools** - Enable/disable tool execution capabilities

### Tools

OSA can execute various tools based on your configuration:
- `bash`, `code_python`, `code_node`, `code_bash` - Run code
- `read_file`, `write_file`, `edit_file` - File operations
- `grep`, `glob` - Search files
- `todowrite`, `task` - Task management
- `web_fetch`, `web_search` - Web access

Tools are disabled by default for security. Edit `config.toml` to enable them:

```toml
[tools]
allowed = ["bash", "read_file", "write_file", "grep", "glob"]
```

## Security

- Password-protected web UI with bcrypt hashing
- Audit logging of all agent actions
- Workspace permission controls (read-only, read-write)
- Configurable allowed commands for shell execution
- LAN access warning on first connection

## Skills

Skills extend OSA with specialized integrations for external services. Install skill bundles (.oskill files) via Settings → Skills.

### Creating Skills

See `examples/skills/SKILL_CREATOR.md` for a complete guide, or start with the `TEMPLATE` folder.

**Quick create a skill:**
```bash
mkdir my-skill && cd my-skill
# Create SKILL.md - instructions for the agent
# Create manifest.toml - metadata
zip -r ../my-skill.oskill *
# Install via Settings → Skills
```

**Example skills in `examples/skills/`:**
- `github/` - GitHub CLI integration
- `spotify/` - Spotify playback control
- `word/` - Microsoft Word documents

## Troubleshooting

**Connection refused:**
```bash
osagent start --verbose
```

**Model not responding:**
- Check your API key is valid
- Verify the model ID is correct for your provider
- Check `~/.osagent/audit.log` for errors

**Tools not working:**
- Ensure tools are listed in `allowed` config
- Check timeout settings in `[tools.*]` sections

## Building from Source

```bash
cargo build --release
./target/release/osagent start
```

## Release Artifacts

- GitHub Actions builds `osagent` for Windows, Linux, and macOS.
- The optional desktop launcher in `launcher/` is built separately and published as its own release artifact.

## License

GPL-3.0
