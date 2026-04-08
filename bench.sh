#!/usr/bin/env bash
set -euo pipefail

# Full reproducible runtime benchmark (debug + release)
cargo run --release --bin osagent-bench -- --profiles debug,release --iterations 10 "$@"
