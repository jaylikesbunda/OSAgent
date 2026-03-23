#!/bin/bash
set -e

# Colors
RED='\033[0;31m'
GREEN='\033[0;32m'
YELLOW='\033[1;33m'
NC='\033[0m' # No Color

echo -e "${GREEN}OSA Installer${NC}"
echo "=================="
echo ""

# Detect OS and architecture
OS=$(uname -s | tr '[:upper:]' '[:lower:]')
ARCH=$(uname -m)

case "$OS" in
  darwin) 
    OS="macos"
    ;;
  mingw*|msys*|cygwin*) 
    OS="windows"
    EXE_EXT=".exe"
    ;;
  linux) 
    OS="linux"
    ;;
  *) 
    echo -e "${RED}Unsupported OS: $OS${NC}"
    exit 1
    ;;
esac

case "$ARCH" in
  x86_64|amd64) 
    ARCH="x86_64"
    ;;
  arm64|aarch64) 
    ARCH="arm64"
    ;;
  *) 
    echo -e "${RED}Unsupported architecture: $ARCH${NC}"
    exit 1
    ;;
esac

echo -e "Detected: ${YELLOW}${OS}-${ARCH}${NC}"
echo ""

# Determine binary name
if [ "$OS" = "windows" ]; then
  BINARY_NAME="osagent-${OS}-${ARCH}.exe"
else
  BINARY_NAME="osagent-${OS}-${ARCH}"
fi

# Download URL
REPO="${REPO:-osagent/osagent}"
VERSION="${VERSION:-latest}"
if [ "$VERSION" = "latest" ]; then
  URL="https://github.com/${REPO}/releases/latest/download/${BINARY_NAME}"
else
  URL="https://github.com/${REPO}/releases/download/${VERSION}/${BINARY_NAME}"
fi

# Download binary
echo -e "${YELLOW}Downloading OSA...${NC}"
if command -v curl &> /dev/null; then
  curl -fsSL "$URL" -o osagent
elif command -v wget &> /dev/null; then
  wget -q "$URL" -O osagent
else
  echo -e "${RED}Error: Neither curl nor wget found${NC}"
  exit 1
fi

# Make executable (Unix-like systems)
if [ "$OS" != "windows" ]; then
  chmod +x osagent
fi

# Move to installation directory
INSTALL_DIR="${INSTALL_DIR:-/usr/local/bin}"
echo -e "${YELLOW}Installing to ${INSTALL_DIR}...${NC}"

if [ -w "$INSTALL_DIR" ]; then
  mv osagent "${INSTALL_DIR}/osagent"
else
  echo -e "${YELLOW}Need sudo to install to ${INSTALL_DIR}${NC}"
  sudo mv osagent "${INSTALL_DIR}/osagent"
fi

echo ""
echo -e "${GREEN}✓ OSA installed successfully!${NC}"
echo ""

# Run setup wizard
read -p "Run setup wizard now? [Y/n] " -n 1 -r
echo
if [[ ! $REPLY =~ ^[Nn]$ ]]; then
  "${INSTALL_DIR}/osagent" setup
fi

# Optional: Install as service
echo ""
read -p "Install as system service? [y/N] " -n 1 -r
echo
if [[ $REPLY =~ ^[Yy]$ ]]; then
  if [ "$OS" = "linux" ]; then
    echo -e "${YELLOW}Installing systemd service...${NC}"
    cat <<EOF | sudo tee /etc/systemd/system/osagent.service > /dev/null
[Unit]
Description=OSA AI Assistant
After=network.target

[Service]
Type=simple
User=$USER
ExecStart=${INSTALL_DIR}/osagent start
Restart=on-failure
RestartSec=5

[Install]
WantedBy=multi-user.target
EOF
    sudo systemctl daemon-reload
    sudo systemctl enable osagent
    echo -e "${GREEN}✓ Service installed. Run 'sudo systemctl start osagent' to start.${NC}"
  elif [ "$OS" = "macos" ]; then
    echo -e "${YELLOW}Installing launchd service...${NC}"
    cat <<EOF > ~/Library/LaunchAgents/com.osagent.plist
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0">
<dict>
    <key>Label</key>
    <string>com.osagent</string>
    <key>ProgramArguments</key>
    <array>
        <string>${INSTALL_DIR}/osagent</string>
        <string>start</string>
    </array>
    <key>RunAtLoad</key>
    <true/>
    <key>KeepAlive</key>
    <true/>
</dict>
</plist>
EOF
    launchctl load ~/Library/LaunchAgents/com.osagent.plist
    echo -e "${GREEN}✓ Service installed and started.${NC}"
  elif [ "$OS" = "windows" ]; then
    echo -e "${YELLOW}For Windows, use Task Scheduler or NSSM to install as service.${NC}"
    echo "See: https://nssm.cc/"
  fi
fi

echo ""
echo -e "${GREEN}Installation complete!${NC}"
echo ""
echo "Next steps:"
echo "  1. Edit config: ${INSTALL_DIR}/osagent config edit"
echo "  2. Start agent: ${INSTALL_DIR}/osagent start"
echo "  3. Open http://localhost:8765 in your browser"
echo ""
