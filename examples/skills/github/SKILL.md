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
actions:
  - name: list_issues
    description: "List open issues in the configured repository"
    type: http
    method: GET
    url: "https://api.github.com/repos/{{ config.DEFAULT_REPO }}/issues"
    headers:
      Authorization: "Bearer {{ config.GITHUB_TOKEN }}"
      Accept: "application/vnd.github+json"
    query:
      state: "open"
      per_page: "10"
  - name: create_issue
    description: "Create a new issue in the configured repository"
    type: http
    method: POST
    url: "https://api.github.com/repos/{{ config.DEFAULT_REPO }}/issues"
    headers:
      Authorization: "Bearer {{ config.GITHUB_TOKEN }}"
      Accept: "application/vnd.github+json"
    body:
      title: "{{ args.title }}"
      body: "{{ args.body }}"
    parameters:
      - name: title
        type: string
        description: "Issue title"
        required: true
      - name: body
        type: string
        description: "Issue body in markdown"
        required: false
  - name: list_prs
    description: "List open pull requests in the configured repository"
    type: http
    method: GET
    url: "https://api.github.com/repos/{{ config.DEFAULT_REPO }}/pulls"
    headers:
      Authorization: "Bearer {{ config.GITHUB_TOKEN }}"
      Accept: "application/vnd.github+json"
    query:
      state: "open"
      per_page: "10"
  - name: create_pr
    description: "Create a new pull request in the configured repository"
    type: http
    method: POST
    url: "https://api.github.com/repos/{{ config.DEFAULT_REPO }}/pulls"
    headers:
      Authorization: "Bearer {{ config.GITHUB_TOKEN }}"
      Accept: "application/vnd.github+json"
    body:
      title: "{{ args.title }}"
      body: "{{ args.body }}"
      head: "{{ args.head }}"
      base: "{{ args.base }}"
    parameters:
      - name: title
        type: string
        description: "PR title"
        required: true
      - name: head
        type: string
        description: "Head branch name"
        required: true
      - name: base
        type: string
        description: "Base branch name (default: main)"
        required: false
      - name: body
        type: string
        description: "PR body in markdown"
        required: false
---
# GitHub Skill

Use the GitHub REST API to manage issues and pull requests.

## Runtime Actions

- `list_issues` lists open issues in the configured repo.
- `create_issue(title, body?)` creates a new issue.
- `list_prs` lists open pull requests.
- `create_pr(title, head, base?, body?)` creates a new pull request.

## Setup

1. Create a GitHub personal access token with `repo` scope.
2. Paste the token into the `GITHUB_TOKEN` field in Skills settings.
3. Set `DEFAULT_REPO` to your repository in `owner/repo` format.

This skill does not require the `gh` CLI to be installed for runtime actions.
The `requires: bins: ["gh"]` is kept for backward compatibility with skill documentation.
