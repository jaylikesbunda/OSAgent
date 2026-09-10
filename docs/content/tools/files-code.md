---
title: "Files & Code"
description: "Edits with diffs and LSP diagnostics, patch, fuzzy edit, and code search."
weight: 20
toc: true
---

## Editing

`edit_file` / `write_file` return a diff plus LSP diagnostics, enforce
read-before-edit, and require re-reading the changed hunk. Fuzzy edit has a
disproportionate-match guard, and repo-wide grep skips build/dependency trees
by default (add project-specific skips via `exclude_dirs`).

```toml
[tools.bash]
mode = "permissive"  # or "allowlist" to restrict to allowed_commands only
blocked_commands = []
timeout_seconds = 30

[tools.code_python]
enabled = true
timeout_seconds = 60
max_output_bytes = 1048576  # 1MB
```

## Code intelligence

- `codesearch` for project-wide symbol search
- A diff plus LSP diagnostics on every edit
- `apply_patch` for multi-file changes
- `process` and `system_status` for inspecting runs and host state

## Background subagents

Offload work with `explore` (read-only research), `verify` (read-only checks),
or `general` (full tool access) subagents. Depth is capped by
`subagent_depth`, transient failures retry with backoff
(`subagent_task_max_retries`), and background completions can auto-resume the
parent (`subagent_auto_resume`, capped by `subagent_auto_resume_max_turns`).

## Related tasks

- [Tool overview]({{< relref "overview.md" >}})
- [Automation]({{< relref "../automation/scheduled-tasks.md" >}})
