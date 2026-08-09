#!/usr/bin/env bash
# Validate an OTA archive before it is published.
#
# The updater downloads these tarballs and swaps the launcher binary in place,
# so a malformed archive is not a failed update — it is a broken install on
# every machine that picks it up. Fail the release here instead.
set -euo pipefail

ARCHIVE="${1:?Usage: $0 <archive.tar.gz>}"
LAUNCHER_NAME="osagent-launcher"
MIN_BYTES=1048576

fail() {
    echo "OTA archive verification failed: $1" >&2
    exit 1
}

[ -f "$ARCHIVE" ] || fail "'$ARCHIVE' not found"

# 1. Real gzip data, not an error page or a mislabeled package. This is the
#    exact class of bug that shipped a .deb under a .tar.gz name.
MAGIC=$(head -c 2 "$ARCHIVE" | od -An -tx1 | tr -d ' \n')
[ "$MAGIC" = "1f8b" ] || fail "'$ARCHIVE' is not gzip data (magic bytes: ${MAGIC:-empty})"

if command -v file >/dev/null 2>&1; then
    echo "--- file(1) ---"
    file "$ARCHIVE"
fi

# 2. Plausible size. A few KB means the build produced a stub.
if command -v stat >/dev/null 2>&1; then
    SIZE=$(stat -f%z "$ARCHIVE" 2>/dev/null || stat -c%s "$ARCHIVE")
    [ "$SIZE" -ge "$MIN_BYTES" ] || fail "'$ARCHIVE' is only $SIZE bytes, expected at least $MIN_BYTES"
    echo "Size: $SIZE bytes"
fi

# 3. The gzip stream decompresses cleanly end to end.
gzip -t "$ARCHIVE" || fail "'$ARCHIVE' failed gzip integrity check"

# 4. Entry paths are relative and contain the launcher the updater looks for.
echo "--- Contents ---"
tar -tzf "$ARCHIVE" || fail "'$ARCHIVE' could not be listed as a tar archive"

while IFS= read -r entry; do
    case "$entry" in
        /*|*..*)
            fail "'$ARCHIVE' contains an unsafe entry path: $entry"
            ;;
    esac
done < <(tar -tzf "$ARCHIVE")

tar -tzf "$ARCHIVE" | grep -qx "$LAUNCHER_NAME" \
    || fail "'$ARCHIVE' does not contain '$LAUNCHER_NAME' at the archive root"

# 5. The extracted launcher is an executable binary for a real platform.
WORKDIR=$(mktemp -d)
trap 'rm -rf "$WORKDIR"' EXIT
tar -xzf "$ARCHIVE" -C "$WORKDIR" || fail "'$ARCHIVE' could not be extracted"

EXTRACTED="$WORKDIR/$LAUNCHER_NAME"
[ -f "$EXTRACTED" ] || fail "extracted '$LAUNCHER_NAME' is missing"

# Check the mode recorded in the archive rather than the extracted file: the
# permission bits on disk depend on the filesystem doing the extracting, but the
# bits stored in the tar header are what every client will restore.
ARCHIVED_MODE=$(tar -tvzf "$ARCHIVE" | awk -v name="$LAUNCHER_NAME" '$NF == name {print $1; exit}')
case "$ARCHIVED_MODE" in
    *x*) echo "Archived mode: $ARCHIVED_MODE" ;;
    "")  fail "could not read archived mode for '$LAUNCHER_NAME'" ;;
    *)   fail "'$LAUNCHER_NAME' is not executable in the archive (mode: $ARCHIVED_MODE)" ;;
esac

BIN_MAGIC=$(head -c 4 "$EXTRACTED" | od -An -tx1 | tr -d ' \n')
case "$BIN_MAGIC" in
    7f454c46)      echo "Launcher format: ELF" ;;          # Linux
    cffaedfe|cefaedfe) echo "Launcher format: Mach-O" ;;   # macOS (64/32-bit LE)
    cafebabe)      echo "Launcher format: Mach-O universal" ;;
    *)             fail "extracted '$LAUNCHER_NAME' is not a native executable (magic: $BIN_MAGIC)" ;;
esac

# 6. The published checksum matches the bytes we just verified.
CHECKSUM_FILE="${ARCHIVE}.sha256"
if [ -f "$CHECKSUM_FILE" ]; then
    EXPECTED=$(awk '{print $1}' "$CHECKSUM_FILE")
    if command -v sha256sum >/dev/null 2>&1; then
        ACTUAL=$(sha256sum "$ARCHIVE" | awk '{print $1}')
    else
        ACTUAL=$(shasum -a 256 "$ARCHIVE" | awk '{print $1}')
    fi
    [ "$EXPECTED" = "$ACTUAL" ] \
        || fail "checksum mismatch: $CHECKSUM_FILE says $EXPECTED, archive hashes to $ACTUAL"
    echo "SHA-256 verified: $ACTUAL"
else
    fail "checksum file '$CHECKSUM_FILE' is missing"
fi

echo "OTA archive verified: $ARCHIVE"
