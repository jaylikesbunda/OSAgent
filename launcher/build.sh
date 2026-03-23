#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

echo "[1/3] Checking formatting..."
cargo fmt --all -- --check

echo "[2/3] Running clippy..."
cargo clippy --all-targets -- -D warnings

echo "[3/3] Building launcher release binary..."
cargo build --release
