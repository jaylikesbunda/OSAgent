#!/bin/bash
set -e

echo "=== OSAgent Package Builder ==="
echo ""

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

PROFILE="release"
SKIP_BUILD=false

while [[ $# -gt 0 ]]; do
    case $1 in
        --debug) PROFILE="debug"; shift ;;
        --skip-build) SKIP_BUILD=true; shift ;;
        *) shift ;;
    esac
done

echo "Profile: $PROFILE"
echo ""

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
    darwin) OS="macos" ;;
    mingw*|msys*|cygwin*) OS="windows" ;;
esac

case "$ARCH" in
    x86_64|amd64) ARCH="x86_64" ;;
    arm64|aarch64) ARCH="arm64" ;;
esac

DIST_NAME="osagent-${OS}-${ARCH}"

# Build osagent core
if [ "$SKIP_BUILD" = false ]; then
    echo "[1/3] Building osagent core ($PROFILE)..."
    cargo build --profile "$PROFILE"
fi

# Build launcher
if [ "$SKIP_BUILD" = false ]; then
    echo "[2/3] Building launcher ($PROFILE)..."
    cd launcher
    cargo tauri build
    cd ..
fi

# Create package directory
echo "[3/3] Creating package..."
DIST_DIR="dist/$DIST_NAME"
rm -rf "$DIST_DIR"
mkdir -p "$DIST_DIR"

# Copy core binary
if [ "$OS" = "windows" ]; then
    cp "target/$PROFILE/osagent.exe" "$DIST_DIR/"
else
    cp "target/$PROFILE/osagent" "$DIST_DIR/"
    chmod +x "$DIST_DIR/osagent"
fi

# Copy launcher from tauri build output
if [ "$OS" = "macos" ]; then
    # macOS .app bundle
    APP_SRC="launcher/src-tauri/target/release/bundle/macos/OSAgent Launcher.app"
    if [ -d "$APP_SRC" ]; then
        cp -R "$APP_SRC" "$DIST_DIR/"
    fi
elif [ "$OS" = "linux" ]; then
    # Linux AppImage or binary
    LAUNCHER_SRC="launcher/src-tauri/target/release/osagent-launcher"
    if [ -f "$LAUNCHER_SRC" ]; then
        cp "$LAUNCHER_SRC" "$DIST_DIR/osagent-launcher"
        chmod +x "$DIST_DIR/osagent-launcher"
    fi
    # Also check for AppImage
    APPIMAGE_SRC=$(find launcher/src-tauri/target/release/bundle -name "*.AppImage" 2>/dev/null | head -1)
    if [ -n "$APPIMAGE_SRC" ]; then
        cp "$APPIMAGE_SRC" "$DIST_DIR/"
    fi
else
    LAUNCHER_SRC="launcher/src-tauri/target/release/osagent-launcher.exe"
    if [ -f "$LAUNCHER_SRC" ]; then
        cp "$LAUNCHER_SRC" "$DIST_DIR/"
    fi
fi

# Create README
cat > "$DIST_DIR/README.txt" << EOF
OSAgent - Your Open Source Agent

To start:
  1. Run osagent-launcher (or open "OSAgent Launcher.app" on macOS)
  2. Follow the setup wizard
  3. OSA will start at http://localhost:8765
EOF

# Create zip archive
cd dist
zip -r "${DIST_NAME}.zip" "$DIST_NAME"

echo ""
echo "✓ Package created at: dist/$DIST_NAME"
echo "✓ Archive created at: dist/${DIST_NAME}.zip"
echo ""
echo "Contents:"
ls -la "$DIST_NAME"
echo ""
echo "Done!"
