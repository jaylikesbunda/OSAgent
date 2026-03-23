---
name: my-skill
description: "Description of what this skill does"
emoji: "🔧"
requires:
  bins: ["required-cli-tool"]
config:
  - name: MY_API_KEY
    type: api_key
    description: "API key for the service (shown as password input)"
    required: true
  - name: MY_VARIABLE
    type: string
    description: "Description of this config value"
    required: true
  - name: OPTIONAL_VAR
    type: string
    description: "Optional setting with a default"
    required: false
    default: "default-value"
---
# My Skill

Brief description of what this skill enables the agent to do.

## Commands

### Example Action
Describe when to use this command.

```bash
my-cli-tool action --arg "{{ skill.env.MY_VARIABLE }}"
```

## Configuration

| Variable | Description | Required |
|----------|-------------|----------|
| `MY_VARIABLE` | Description of this config | Yes |
| `OPTIONAL_VAR` | Optional setting | No |

## Setup

1. Install the CLI: `brew install my-cli-tool` (or download from https://example.com)
2. Get your API key from https://example.com/keys
3. Enter your credentials in the skill settings below

## Usage

When enabled, the agent can use this skill to perform actions like:
- "Do the thing with my-skill"
- "Use my-skill to accomplish X"
