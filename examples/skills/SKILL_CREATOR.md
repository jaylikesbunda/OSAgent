# Skill Creator Guide

Skills extend OSA's capabilities by connecting to external services and APIs.
This guide shows you how to create a skill that the agent can execute at runtime
without compiling, installing dependencies, or restarting.

## Skill Anatomy

A skill is a `.oskill` bundle (ZIP file) containing:

```
my-skill/
├── manifest.toml    # Skill metadata
├── SKILL.md         # Frontmatter + runtime actions + optional docs
├── scripts/         # Optional: script-backed actions
└── icon.png         # Optional icon (256x256)
```

## Step 1: Create the Directory Structure

```bash
mkdir -p my-skill/scripts
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

The YAML frontmatter between `---` lines defines the skill metadata, required
configuration, and **runtime actions**. The agent calls actions via the built-in
`skill_action` tool. Your secrets are never shown to the agent.

```markdown
---
name: my-service
description: "My service integration"
emoji: "🔧"
requires:
  bins: ["optional-cli"]
config:
  - name: API_KEY
    type: api_key
    description: "Your API key"
    required: true
  - name: BASE_URL
    type: string
    description: "API base URL"
    required: false
    default: "https://api.example.com"
actions:
  - name: status
    description: "Get current status from the service"
    type: http
    method: GET
    url: "{{ config.BASE_URL }}/status"
    headers:
      Authorization: "Bearer {{ config.API_KEY }}"
      Accept: "application/json"
  - name: create_record
    description: "Create a new record"
    type: http
    method: POST
    url: "{{ config.BASE_URL }}/records"
    headers:
      Authorization: "Bearer {{ config.API_KEY }}"
      Content-Type: "application/json"
    body:
      name: "{{ args.name }}"
      value: "{{ args.value }}"
    parameters:
      - name: name
        type: string
        description: "Record name"
        required: true
      - name: value
        type: string
        description: "Record value"
        required: true
  - name: generate_report
    description: "Generate a report file"
    type: script
    script: "scripts/generate_report.py"
    parameters:
      - name: format
        type: string
        description: "Report format (json, csv, html)"
        required: false
---

# My Service Skill

Brief description of what this skill enables.

## Runtime Actions

- `status` shows current service status.
- `create_record(name, value)` creates a record.
- `generate_report(format?)` generates a report file.

## Configuration

| Variable | Description | Required |
|----------|-------------|----------|
| `API_KEY` | Your API key from example.com | Yes |
| `BASE_URL` | API base URL (default: https://api.example.com) | No |

## Setup

1. Get an API key from https://example.com/keys
2. Paste it in the skill configuration settings
```

### Frontmatter Reference

| Field | Type | Description |
|-------|------|-------------|
| `name` | string | Skill identifier |
| `description` | string | Brief description |
| `emoji` | string | Emoji for UI display |
| `requires.bins` | string[] | Required CLI binaries |
| `requires.files` | string[] | Required files |
| `config` | list | Configuration fields the user sets in the UI |
| `actions` | list | Runtime actions the agent can execute |

### Config Field Types

| Type | UI Input |
|------|----------|
| `api_key` | Password input, masked in UI |
| `password` | Password input, masked in UI |
| `string` | Text input |
| `number` | Numeric input |
| `boolean` | Toggle |

### Action Types

#### HTTP Actions

Call REST APIs directly. The engine handles auth headers and response parsing.

```yaml
type: http
method: GET|POST|PUT|DELETE|PATCH
url: "https://api.example.com/endpoint"
headers:
  Authorization: "Bearer {{ config.API_KEY }}"
query:
  param: "{{ args.value }}"
body:
  key: "{{ args.arg }}"
```

**Template variables:**
- `{{ config.VAR }}` — resolved from the user's saved skill config
- `{{ args.param }}` — resolved from the agent's call arguments

#### Script Actions

Run a bundled script. Scripts receive environment variables:

```yaml
type: script
script: "scripts/myscript.py"
```

**Environment variables available to scripts:**
- `OSA_SKILL_NAME` — the skill name
- `OSA_SKILL_ACTION` — the action name
- `OSA_SKILL_ARGS_JSON` — full JSON of agent-supplied arguments
- `OSA_SKILL_ARG_<KEY>` — each argument as an uppercase env var
- All skill config values (e.g. `API_KEY`, `BASE_URL`)

**Supported script extensions:**
- `.py` → runs with `python`
- `.sh` → runs with `sh`
- `.ps1` → runs with `powershell`
- `.js` → runs with `node`

## Step 4: Add an Icon (Optional)

Create a 256x256px PNG icon. Name it `icon.png` in the skill folder.

## Step 5: Create the .oskill Bundle

```bash
# Navigate to parent directory
cd ..

# Create ZIP bundle (zip is needed because .oskill is a zip with a different extension)
zip -r my-service.oskill my-service/

# Or on Windows (rename zip to oskill after)
Compress-Archive -Path my-service -DestinationPath my-service.zip -Force
Rename-Item my-service.zip my-service.oskill
```

## Step 6: Install and Test

1. Open OSA Settings → Skills
2. Drag and drop `my-service.oskill` onto the upload zone
3. Configure your API keys in the skill settings
4. Click "Test" to verify connectivity

## How the Agent Uses Skills

1. The agent calls `skill_list()` to discover available skills and their actions.
2. The agent calls `skill_action(skill="my-service", action="status")`.
3. The backend resolves config, templates arguments, and executes the action.
4. The agent receives the result. Config secrets are never exposed.

## Example Skills

See `examples/skills/` for reference implementations:

- **spotify** — Spotify playback control via Web API (HTTP actions)
- **github** — GitHub REST API integration (HTTP actions)
- **word** — Word document creation (script action)

## Best Practices

1. **Use HTTP actions when possible** — no external CLI needed, zero dependencies
2. **Use script actions sparingly** — for logic that cannot be expressed declaratively
3. **Document required configuration** — help users get started
4. **Keep actions focused** — one action per atomic operation
5. **Test with real credentials** — use the Test button in the UI

## Troubleshooting

**Skill not appearing?**
- Check manifest.toml syntax is valid
- Ensure SKILL.md exists in skill folder
- Bundle must be `.oskill` extension (ZIP internally)

**Action not found?**
- Ensure the action name matches what `skill_list` shows
- Check the YAML `type:` is `http` or `script`

**Config errors?**
- Verify required config fields are set in the skill settings UI
- Check `{{ config.VAR }}` names match the config field names exactly

**Script errors?**
- Ensure the script file exists at the path in the `script:` field
- Check script has the correct file extension
- Verify the script runs standalone outside OSA
