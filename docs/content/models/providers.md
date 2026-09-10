---
title: "Providers"
description: "Configure OpenRouter, OpenAI, Anthropic, Google, Ollama, Bedrock, and more."
weight: 10
toc: true
---

## What this does

Each `[[providers]]` block in `~/.osagent/config.toml` adds one backend.
Set the top-level `default_provider` / `default_model` keys to choose the
active pair.

## OpenRouter (recommended)

Access to 200+ models behind one key:

```toml
[[providers]]
provider_type = "openrouter"
api_key = "sk-or-v1-..."  # or OPENROUTER_API_KEY env var
base_url = "https://openrouter.ai/api/v1"
model = "anthropic/claude-sonnet-4"
```

## Local Ollama (fully offline)

Nothing leaves your machine:

```toml
[[providers]]
provider_type = "ollama"
api_key = ""
base_url = "http://localhost:11434/v1"
model = "llama3.1:70b"
```

## OpenAI / Anthropic / Google

```toml
[[providers]]
provider_type = "openai"
api_key = "sk-..."  # or OPENAI_API_KEY env var
base_url = "https://api.openai.com/v1"
model = "gpt-4.1"

[[providers]]
provider_type = "anthropic"
api_key = "sk-ant-..."  # or ANTHROPIC_API_KEY env var
base_url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-20250514"

[[providers]]
provider_type = "google"
api_key = "AI..."  # or GOOGLE_API_KEY env var
base_url = "https://generativelanguage.googleapis.com/v1beta/openai"
model = "gemini-2.5-pro-preview-05-06"
```

## AWS Bedrock

```toml
[[providers]]
provider_type = "amazon-bedrock"
auth_type = "aws"
aws_profile = "default"  # or AWS_PROFILE env var
region = "us-east-1"
base_url = "https://bedrock-runtime.us-east-1.amazonaws.com"
model = "anthropic.claude-3-5-sonnet-20241022-v2:0"
```

## Per-subagent models

Run explore subagents on a cheaper model while the main agent stays on your
default. Value is `"provider_id:model"` or just a model name (uses the default
provider):

```toml
[agent.subagent_models]
"explore" = "openrouter:deepseek/deepseek-r1"
```

## Related tasks

- [OAuth login]({{< relref "oauth.md" >}})
- [Configuration]({{< relref "../getting-started/configuration.md" >}})
