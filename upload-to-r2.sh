#!/usr/bin/env bash
set -euo pipefail

SCRIPT_DIR="$(cd "$(dirname "$0")" && pwd)"
cd "$SCRIPT_DIR"

TAG="${1:?Usage: $0 <tag> [artifact_dir]}"
ARTIFACT_DIR="${2:-release}"
CDN_BASE_URL="https://osa.fuckyourcdn.com"
BUCKET="${R2_BUCKET:-osagent-releases}"
PREFIX="${R2_RELEASE_PREFIX:-releases}"
# Packages: what a human downloads and installs by hand.
LINUX_PACKAGE="osagent-linux-x86_64.deb"
LINUX_PACKAGE_CHECKSUM="${LINUX_PACKAGE}.sha256"
MACOS_PACKAGE="osagent-macos-arm64.dmg"
MACOS_PACKAGE_CHECKSUM="${MACOS_PACKAGE}.sha256"
WINDOWS_INSTALLER="osagent-windows-x86_64-setup.exe"
WINDOWS_INSTALLER_CHECKSUM="${WINDOWS_INSTALLER}.sha256"

# OTA archives: what the in-app updater downloads and swaps in place. These are
# what `assets.<platform>.url` points at, so they must be real tarballs — a
# package published under an archive URL is what broke updates before.
LINUX_ARCHIVE="osagent-linux-x86_64.tar.gz"
LINUX_ARCHIVE_CHECKSUM="${LINUX_ARCHIVE}.sha256"
MACOS_ARCHIVE="osagent-macos-arm64.tar.gz"
MACOS_ARCHIVE_CHECKSUM="${MACOS_ARCHIVE}.sha256"

require_file() {
    local path="$1"
    if [ ! -f "$path" ]; then
        echo "Error: Required release artifact '$path' not found"
        exit 1
    fi
}

# Never publish a manifest that points at bytes we have not checked. A wrong
# hash here becomes a failed update on every client that trusts the manifest.
verify_checksum() {
    local file="$1"
    local checksum_file="${file}.sha256"
    local expected actual
    expected=$(awk '{print $1}' "$checksum_file")
    if command -v sha256sum >/dev/null 2>&1; then
        actual=$(sha256sum "$file" | awk '{print $1}')
    else
        actual=$(shasum -a 256 "$file" | awk '{print $1}')
    fi
    if [ "$expected" != "$actual" ]; then
        echo "Error: Checksum mismatch for '$file'"
        echo "  ${checksum_file} says: $expected"
        echo "  actual:                $actual"
        exit 1
    fi
    echo "Verified ${file}: ${actual}"
}

# Confirm an OTA archive really is gzip data before it is advertised as one.
verify_archive_format() {
    local file="$1"
    local magic
    magic=$(head -c 2 "$file" | od -An -tx1 | tr -d ' \n')
    if [ "$magic" != "1f8b" ]; then
        echo "Error: '$file' is not gzip data (magic bytes: ${magic:-empty})"
        exit 1
    fi
    if ! tar -tzf "$file" | grep -qx "osagent-launcher"; then
        echo "Error: '$file' does not contain 'osagent-launcher'"
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

for artifact in \
    "$LINUX_PACKAGE" "$LINUX_PACKAGE_CHECKSUM" \
    "$MACOS_PACKAGE" "$MACOS_PACKAGE_CHECKSUM" \
    "$WINDOWS_INSTALLER" "$WINDOWS_INSTALLER_CHECKSUM" \
    "$LINUX_ARCHIVE" "$LINUX_ARCHIVE_CHECKSUM" \
    "$MACOS_ARCHIVE" "$MACOS_ARCHIVE_CHECKSUM"; do
    require_file "${ARTIFACT_DIR}/${artifact}"
done

echo "--- Verifying artifacts ---"
for artifact in "$LINUX_PACKAGE" "$MACOS_PACKAGE" "$WINDOWS_INSTALLER" \
                "$LINUX_ARCHIVE" "$MACOS_ARCHIVE"; do
    verify_checksum "${ARTIFACT_DIR}/${artifact}"
done
verify_archive_format "${ARTIFACT_DIR}/${LINUX_ARCHIVE}"
verify_archive_format "${ARTIFACT_DIR}/${MACOS_ARCHIVE}"
echo ""

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

LINUX_PACKAGE_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${LINUX_PACKAGE_CHECKSUM}")
MACOS_PACKAGE_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${MACOS_PACKAGE_CHECKSUM}")
WIN_INSTALLER_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${WINDOWS_INSTALLER_CHECKSUM}")
LINUX_ARCHIVE_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${LINUX_ARCHIVE_CHECKSUM}")
MACOS_ARCHIVE_SHA=$(awk '{print $1}' "${ARTIFACT_DIR}/${MACOS_ARCHIVE_CHECKSUM}")

# Schema note: `url` is the OTA archive the updater swaps in place, `installer`
# is the manual package. `sha256.<platform>` covers the archive and
# `sha256.<platform>-installer` covers the package.
cat > "${ARTIFACT_DIR}/release-manifest.json" <<EOF
{
  "tag": "${TAG}",
  "version": "${VERSION}",
  "released_at": "$(date -u +%Y-%m-%dT%H:%M:%SZ)",
  "channel": "${MANIFEST_CHANNEL}",
  "assets": {
    "linux-x86_64": {
      "archive": "${LINUX_ARCHIVE}",
      "url": "${CDN_BASE_URL}/${R2_PATH}/${LINUX_ARCHIVE}",
      "installer": "${CDN_BASE_URL}/${R2_PATH}/${LINUX_PACKAGE}"
    },
    "macos-arm64": {
      "archive": "${MACOS_ARCHIVE}",
      "url": "${CDN_BASE_URL}/${R2_PATH}/${MACOS_ARCHIVE}",
      "installer": "${CDN_BASE_URL}/${R2_PATH}/${MACOS_PACKAGE}"
    },
    "windows-x86_64": {
      "installer": "${CDN_BASE_URL}/${R2_PATH}/${WINDOWS_INSTALLER}"
    }
  },
  "sha256": {
    "linux-x86_64": "${LINUX_ARCHIVE_SHA}",
    "linux-x86_64-installer": "${LINUX_PACKAGE_SHA}",
    "macos-arm64": "${MACOS_ARCHIVE_SHA}",
    "macos-arm64-installer": "${MACOS_PACKAGE_SHA}",
    "windows-x86_64-installer": "${WIN_INSTALLER_SHA}"
  }
}
EOF

# Validate with whichever parser is actually usable. Clients refuse to update
# at all if latest.json does not parse, so this is worth checking before upload.
# `command -v` alone is not enough: Windows ships a python3 shim that is not a
# working interpreter, so confirm the tool runs before trusting its verdict.
if command -v jq >/dev/null 2>&1 && echo '{}' | jq -e . >/dev/null 2>&1; then
    jq -e . "${ARTIFACT_DIR}/release-manifest.json" >/dev/null \
        || { echo "Error: generated manifest is not valid JSON"; exit 1; }
    echo "Manifest JSON validated with jq"
elif command -v python3 >/dev/null 2>&1 && python3 -c "pass" >/dev/null 2>&1; then
    python3 -c "import json,sys; json.load(open(sys.argv[1]))" \
        "${ARTIFACT_DIR}/release-manifest.json" \
        || { echo "Error: generated manifest is not valid JSON"; exit 1; }
    echo "Manifest JSON validated with python3"
else
    echo "Warning: no JSON parser available, skipping manifest validation"
fi

echo "--- Manifest ---"
cat "${ARTIFACT_DIR}/release-manifest.json"
echo ""

echo "--- Uploading to ${R2_PATH}/ ---"
aws s3 cp "$ARTIFACT_DIR/" "s3://${BUCKET}/${R2_PATH}/" \
    --endpoint-url "$ENDPOINT" \
    --recursive \
    --no-progress

# Confirm every payload the manifest advertises is actually fetchable before
# flipping latest.json. Clients read latest.json first; a pointer that leads to
# a 404 turns every update check into a failed download.
echo "--- Verifying published payloads ---"
for asset in "$LINUX_ARCHIVE" "$MACOS_ARCHIVE" "$LINUX_PACKAGE" "$MACOS_PACKAGE" "$WINDOWS_INSTALLER"; do
    url="${CDN_BASE_URL}/${R2_PATH}/${asset}"
    if ! curl -fsSIL --retry 5 --retry-delay 3 --max-time 60 "$url" >/dev/null; then
        echo "Error: published asset is not reachable: $url"
        exit 1
    fi
    echo "OK: $url"
done
echo ""

# latest.json goes last: the payloads it names are already live.
echo "--- Updating latest.json ---"
aws s3 cp "${ARTIFACT_DIR}/release-manifest.json" "s3://${BUCKET}/${PREFIX}/latest.json" \
    --endpoint-url "$ENDPOINT" \
    --content-type "application/json" \
    --no-progress

echo ""
echo "=== Done ==="
echo "Latest: ${CDN_BASE_URL}/${PREFIX}/latest.json"
echo "Files:  ${CDN_BASE_URL}/${R2_PATH}/"
