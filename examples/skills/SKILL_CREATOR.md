# Skill Creator Guide

Skills extend OSA's capabilities by connecting to external services and APIs. This guide shows you how to create a skill.

## Skill Anatomy

A skill is a `.oskill` bundle (ZIP file) containing:

```
my-skill/
├── manifest.toml    # Skill metadata
├── SKILL.md         # Instructions for the agent
└── icon.png         # Optional icon (256x256)
```

## Step 1: Create the Directory Structure

```bash
mkdir -p my-skill
cd my-skill
```

## Step 2: Create manifest.toml

```toml
name = "my-service"
version = "1.0.0"
description = "Description of what this skill does"
author = "Your Name"
icon = "icon.png"    # Optional
```

**Rules:**
- `name`: lowercase, alphanumeric + hyphens/underscores only
- `version`: semver format (1.0.0)
- `icon`: must be PNG, will be displayed at 40x40px

## Step 3: Create SKILL.md

```markdown
---
name: my-service
description: "My service integration"
emoji: "🔧"
requires:
  bins: ["my-cli-tool"]
---
# My Service Skill

Brief description of what this skill enables.

## Commands

### Action Name
Description of when to use this command.

```bash
my-cli-tool action --arg "{{ skill.env.VAR_NAME }}"
```

## Configuration

| Variable | Description | Required |
|----------|-------------|----------|
| `API_KEY` | Your API key from example.com | Yes |
| `REGION` | Data region (us, eu, ap) | No |

## Setup

1. Install the CLI: `brew install my-cli-tool`
2. Get API key from [example.com/keys](https://example.com/keys)
3. Configure the skill with your credentials

## Usage

Explain how the agent uses this skill in natural language.
```

### SKILL.md Frontmatter

The YAML frontmatter (between `---` lines) defines:

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Skill identifier |
| `description` | string | Brief description |
| `emoji` | string | Emoji for UI display |
| `requires.bins` | string[] | Required CLI binaries |
| `requires.files` | string[] | Required files |

### Template Variables in Commands

Use `{{ skill.env.VARNAME }}` to reference configuration values:

```bash
api call --key "{{ skill.env.API_KEY }}" --region "{{ skill.env.REGION | default: 'us' }}"
```

### Jinja2 Features

Commands support Jinja2 templating:

```bash
{% for item in items %}
process --item "{{ item }}"
{% endfor %}
```

## Step 4: Add an Icon (Optional)

Create a 256x256px PNG icon. Name it `icon.png` in the skill folder.

## Step 5: Create the .oskill Bundle

```bash
# Navigate to parent directory
cd ..

# Create ZIP bundle
zip -r my-service.oskill my-service/

# Or on Windows
powershell -command "Compress-Archive -Path my-service -DestinationPath my-service.oskill"
```

## Step 6: Install and Test

1. Open OSA Settings → Skills
2. Drag and drop `my-service.oskill` onto the upload zone
3. Configure your API keys in the skill settings
4. Click "Test" to verify connectivity

## Example Skills

See `examples/skills/` for reference implementations:

- **github** - GitHub CLI integration
- **spotify** - Spotify playback control  
- **word** - Microsoft Word document creation

## CLI-Based vs API-Based Skills

### CLI-Based (Simpler)

Use shell commands with CLIs:

```bash
gh issue create --title "Bug"
spogo play
```

**Requirements:**
- CLI tool installed on system
- Auth configured (token, login, etc.)
- API keys passed via environment

### API-Based (More Complex)

Use HTTP requests directly:

```bash
curl -H "Authorization: Bearer {{ skill.env.API_KEY }}" \
     "{{ skill.env.BASE_URL }}/api/endpoint"
```

**Requirements:**
- API authentication (OAuth, API key, etc.)
- Network access
- Response parsing

## Best Practices

1. **Use CLI tools when available** - They handle auth and edge cases
2. **Document required binaries** - Add to `requires.bins`
3. **Provide sensible defaults** - Use `| default: 'value'`
4. **Include setup instructions** - Help users get started
5. **Test with real credentials** - Use the Test button

## Troubleshooting

**Skill not appearing?**
- Check manifest.toml syntax is valid
- Ensure SKILL.md exists in skill folder
- Bundle must be `.oskill` extension

**CLI not found?**
- Add to `requires.bins` array
- Document installation in Setup section

**API errors?**
- Verify API key is set correctly
- Check BASE_URL environment variable
- Test with curl directly first
