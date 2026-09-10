---
title: "Building from Source"
description: "Prerequisites, build flow, tests, and release notes."
weight: 20
toc: true
---

## Prerequisites

- Rust 1.78+
- Git
- PowerShell 5.1+ for the canonical Windows build flow

## Build

```powershell
git clone https://github.com/jaylikesbunda/OSAgent.git
cd OSAgent
.\build-launcher.ps1
```

For an installer: `.\build-launcher.ps1 -Installer`. Linux/macOS use
`launcher/build.sh`. The full release flow is documented in `RELEASING.md`.

## Checks before pushing

```powershell
.\build-launcher.ps1 -Checks
cargo test
cargo fmt --check
cargo clippy
```

`-Checks` runs the same core-first, updater-second, launcher-last flow used
for releases.

## Benchmarks

```powershell
cargo run --release --bin osagent-bench
```

Warm-starts in ~0.5s; idles at ~20 MB fresh / ~50 MB under use.

## Related tasks

- [Installation]({{< relref "../getting-started/installation.md" >}})
- [Contributing](https://github.com/jaylikesbunda/OSAgent/blob/main/CONTRIBUTING.md)
