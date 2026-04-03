#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

TAG="${1:?Usage: $0 <tag> [artifact_dir]}"
ARTIFACT_DIR="${2:-release}"
CDN_BASE_URL="https://osa.fuckyourcdn.com"
BUCKET="${R2_BUCKET:-osagent-releases}"
PREFIX="${R2_RELEASE_PREFIX:-releases}"
LINUX_ARCHIVE="osagent-linux-x86_64.tar.gz"
LINUX_CHECKSUM="${LINUX_ARCHIVE}.sha256"
WINDOWS_ARCHIVE="osagent-windows-x86_64.zip"
WINDOWS_CHECKSUM="${WINDOWS_ARCHIVE}.sha256"

require_file() {
    local path="$1"
    if [ ! -f "$path" ]; then
        echo "Error: Required release artifact '$path' not found"
        exit 1
    fi
}

if [ -z "${R2_ACCOUNT_ID:-}" ] || [ -z "${R2_ACCESS_KEY_ID:-}" ] || [ -z "${R2_SECRET_ACCESS_KEY:-}" ]; then
    echo "Error: Set R2_ACCOUNT_ID, R2_ACCESS_KEY_ID, and R2_SECRET_ACCESS_KEY environment variables."
    exit 1
fi

ENDPOINT="https://${R2_ACCOUNT_ID}.r2.cloudflarestorage.com"
export AWS_ACCESS_KEY_ID="$R2_ACCESS_KEY_ID"
export AWS_SECRET_ACCESS_KEY="$R2_SECRET_ACCESS_KEY"
export AWS_DEFAULT_REGION="auto"

if [ ! -d "$ARTIFACT_DIR" ]; then
    echo "Error: Artifact directory '$ARTIFACT_DIR' not found"
    exit 1
fi

require_file "${ARTIFACT_DIR}/${LINUX_ARCHIVE}"
require_file "${ARTIFACT_DIR}/${LINUX_CHECKSUM}"
require_file "${ARTIFACT_DIR}/${WINDOWS_ARCHIVE}"
require_file "${ARTIFACT_DIR}/${WINDOWS_CHECKSUM}"

R2_PATH="${PREFIX}/${TAG}"
VERSION="${TAG#v}"
MANIFEST_CHANNEL="stable"

case "${TAG,,}" in
    *alpha*|*beta*|*rc*)
        MANIFEST_CHANNEL="beta"
        ;;
esac

echo "=== Uploading ${TAG} to R2 ==="
echo "CDN URL:  ${CDN_BASE_URL}/${R2_PATH}/"
echo ""

LINUX_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${LINUX_CHECKSUM}")
WIN_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${WINDOWS_CHECKSUM}")

cat > "${ARTIFACT_DIR}/release-manifest.json" <<EOF
{
  "tag": "${TAG}",
  "version": "${VERSION}",
  "released_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "channel": "${MANIFEST_CHANNEL}",
  "assets": {
    "linux-x86_64": {
      "archive": "${LINUX_ARCHIVE}",
      "url": "${CDN_BASE_URL}/${R2_PATH}/${LINUX_ARCHIVE}"
    },
    "windows-x86_64": {
      "archive": "${WINDOWS_ARCHIVE}",
      "url": "${CDN_BASE_URL}/${R2_PATH}/${WINDOWS_ARCHIVE}"
    }
  },
  "sha256": {
    "linux-x86_64": "${LINUX_SHA}",
    "windows-x86_64": "${WIN_SHA}"
  }
}
EOF

echo "--- Manifest ---"
cat "${ARTIFACT_DIR}/release-manifest.json"
echo ""

echo "--- Uploading to ${R2_PATH}/ ---"
aws s3 cp "$ARTIFACT_DIR/" "s3://${BUCKET}/${R2_PATH}/" \
    --endpoint-url "$ENDPOINT" \
    --recursive \
    --no-progress

echo "--- Updating latest.json ---"
aws s3 cp "${ARTIFACT_DIR}/release-manifest.json" "s3://${BUCKET}/${PREFIX}/latest.json" \
    --endpoint-url "$ENDPOINT" \
    --content-type "application/json" \
    --no-progress

echo ""
echo "=== Done ==="
echo "Latest: ${CDN_BASE_URL}/${PREFIX}/latest.json"
echo "Files:  ${CDN_BASE_URL}/${R2_PATH}/"
