#!/usr/bin/env bash
set -euo pipefail

cd "$(dirname "$0")"

PROFILE="${1:-release}"

echo "Building OSAgent ($PROFILE)"

echo "[1/4] Checking formatting..."
cargo fmt -- --check

echo "[2/4] Running clippy..."
cargo clippy --all-targets --all-features -- -D warnings

echo "[3/4] Running tests..."
cargo test --all-features --verbose

echo "[4/4] Building core with Discord..."
cargo build --$PROFILE --features discord

echo "[5/5] Building launcher with embedded core..."
cargo build --manifest-path launcher/Cargo.toml --$PROFILE

echo "Done! Launcher: launcher/target/$PROFILE/osagent-launcher"
