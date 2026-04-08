#!/usr/bin/env bash
set -euo pipefail

# Quick reproducible runtime benchmark (debug + release)
cargo run --release --bin osagent-bench -- --profiles debug,release --iterations 3 "$@"
