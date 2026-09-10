---
title: "OAuth Login"
description: "Sign in with OpenAI, Anthropic, Google, GitHub Copilot, Qwen, or Chutes."
weight: 20
toc: true
---

## What this does

Easiest path: sign in from the web UI — one click, no API key to paste.
Built-in login covers OpenAI, Anthropic, Google, GitHub Copilot, Qwen, and
Chutes (browser flow, or a device code for Copilot and Qwen). Tokens are
encrypted at rest with AES-GCM and can be refreshed or revoked per provider.

## Custom OAuth apps (advanced)

To use your own OAuth app credentials instead of the built-in login, set
`auth_type = "oauth"` on a provider. Client credentials can come from config
or environment variables (`{PROVIDER}_OAUTH_CLIENT_ID` /
`{PROVIDER}_OAUTH_CLIENT_SECRET`). Examples:

## OpenAI with OAuth

```toml
[[providers]]
provider_type = "openai"
auth_type = "oauth"
oauth_client_id = "your-client-id"          # or OPENAI_OAUTH_CLIENT_ID
oauth_client_secret = "your-client-secret"  # or OPENAI_OAUTH_CLIENT_SECRET
oauth_scopes = ["api.full-access"]
base_url = "https://api.openai.com/v1"
model = "gpt-4.1"
```

## Anthropic with OAuth

```toml
[[providers]]
provider_type = "anthropic"
auth_type = "oauth"
oauth_client_id = "your-client-id"
oauth_client_secret = "your-client-secret"
oauth_scopes = ["api:read", "api:write"]
base_url = "https://api.anthropic.com/v1"
model = "claude-sonnet-4-20250514"
```

## GitHub Copilot with OAuth

```toml
[[providers]]
provider_type = "github-copilot"
auth_type = "oauth"
oauth_client_id = "your-github-oauth-app-client-id"
oauth_client_secret = "your-github-oauth-app-client-secret"
oauth_scopes = ["read:user", "workflow"]
base_url = "https://api.githubcopilot.com/chat/completions"
model = "gpt-4o"
```

Google, Qwen, and Chutes custom apps follow the same `auth_type = "oauth"`
pattern — see `config.example.toml` for the exact keys, scopes, and URLs.

## Related tasks

- [Providers]({{< relref "providers.md" >}})
- [Security]({{< relref "../operations/security.md" >}})
