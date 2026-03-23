---
name: github
description: "GitHub operations via `gh` CLI"
emoji: "🐙"
requires:
  bins: ["gh"]
config:
  - name: GITHUB_TOKEN
    type: api_key
    description: "GitHub personal access token with repo scope"
    required: true
  - name: DEFAULT_REPO
    type: string
    description: "Default repository in owner/repo format (e.g. octocat/hello-world)"
    required: false
---
# GitHub Skill

Use the `gh` CLI to interact with GitHub.

## Commands

### Create Issue
```bash
gh issue create --title "{{ skill.env.ISSUE_TITLE }}" --body "{{ skill.env.ISSUE_BODY }}"
```

### List Issues
```bash
gh issue list --state {{ skill.env.STATE | default: "open" }}
```

### Create PR
```bash
gh pr create --title "{{ skill.env.PR_TITLE }}" --body "{{ skill.env.PR_BODY }}"
```

## Configuration

| Variable | Description | Required |
|----------|-------------|----------|
| `GITHUB_TOKEN` | GitHub personal access token | Yes |

## Setup

1. Install `gh`: `brew install gh` or download from [cli.github.com](https://cli.github.com)
2. Authenticate: `gh auth login`
3. Configure your token in the skill settings
