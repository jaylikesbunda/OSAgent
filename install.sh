#!/bin/sh
set -eu

REPO="${REPO:-jaylikesbunda/OSAgent}"
VERSION="${VERSION:-latest}"

if [ "$(uname -s)" != "Linux" ] || [ "$(uname -m)" != "x86_64" ]; then
    printf '%s\n' "This installer currently supports Linux x86_64 only." >&2
    printf '%s\n' "Download the installer for your platform from https://github.com/${REPO}/releases." >&2
    exit 1
fi

if ! command -v apt-get >/dev/null 2>&1; then
    printf '%s\n' "OSAgent currently publishes a .deb installer for Debian and Ubuntu." >&2
    printf '%s\n' "The .tar.gz release is an in-app update payload, not a system package." >&2
    exit 1
fi

asset="osagent-linux-x86_64.deb"
if [ "$VERSION" = "latest" ]; then
    base_url="https://github.com/${REPO}/releases/latest/download"
else
    base_url="https://github.com/${REPO}/releases/download/${VERSION}"
fi

tmp_dir=$(mktemp -d)
trap 'rm -rf "$tmp_dir"' EXIT HUP INT TERM

download() {
    url=$1
    output=$2
    if command -v curl >/dev/null 2>&1; then
        curl -fsSL "$url" -o "$output"
    elif command -v wget >/dev/null 2>&1; then
        wget -q "$url" -O "$output"
    else
        printf '%s\n' "Install curl or wget and try again." >&2
        exit 1
    fi
}

printf '%s\n' "Downloading OSAgent ${asset}..."
download "${base_url}/${asset}" "${tmp_dir}/${asset}"
download "${base_url}/${asset}.sha256" "${tmp_dir}/${asset}.sha256"

expected=$(cut -d ' ' -f 1 "${tmp_dir}/${asset}.sha256")
actual=$(sha256sum "${tmp_dir}/${asset}" | cut -d ' ' -f 1)
if [ "$expected" != "$actual" ]; then
    printf '%s\n' "Checksum verification failed." >&2
    exit 1
fi

printf '%s\n' "Installing OSAgent..."
if [ "$(id -u)" -eq 0 ]; then
    apt-get install -y "${tmp_dir}/${asset}"
else
    sudo apt-get install -y "${tmp_dir}/${asset}"
fi

printf '%s\n' "OSAgent installed. Launch it from your application menu to finish setup."
