# Releasing

## Release Contract

- GitHub Actions is the CI and release pipeline for `jaylikesbunda/OSAgent`.
- GitHub Releases host the files people download manually.
- Cloudflare R2 is the auto-update source for installed launchers.
- The launcher embeds the core agent and updater binaries.
- OTA `.tar.gz` archives contain only the launcher and are not first-time installers.
- Checksums are published for every installer and OTA archive.

## Required GitHub Secrets

Configure these repository or release-environment secrets before running the
release workflow:

- `R2_ACCOUNT_ID`
- `R2_ACCESS_KEY_ID`
- `R2_SECRET_ACCESS_KEY`

The workflow uses these defaults:

- `R2_BUCKET`: `osagent-releases`
- `R2_RELEASE_PREFIX`: `releases`
- `CDN_BASE_URL`: `https://osa.fuckyourcdn.com`

## Release Flow

1. Open the `CI` workflow in GitHub Actions.
2. Choose `Run workflow` on the `main` branch.
3. Enter a tag such as `v0.4.5` or `v0.4.5-rc1`.
4. Choose whether the GitHub Release is a prerelease.
5. GitHub Actions builds Linux, Windows, and macOS artifacts.
6. The workflow verifies OTA archives and uploads the release manifest and assets to Cloudflare R2.
7. The workflow creates or updates the matching GitHub Release and attaches the same assets for manual download.
8. The workflow verifies that every advertised GitHub download URL resolves.

## Published Assets

Manual installers:

- `osagent-linux-x86_64.deb`
- `osagent-windows-x86_64-setup.exe`
- `osagent-macos-arm64.dmg`

OTA archives:

- `osagent-linux-x86_64.tar.gz`
- `osagent-macos-arm64.tar.gz`
- `release-manifest.json`
- `latest.json` at the R2 release prefix

## Local Validation

On Windows, run this before releasing:

```powershell
.\build-launcher.ps1 -Checks
```

On Linux, use the same core checks directly:

```bash
cargo fmt --all -- --check
cargo clippy --all-targets --all-features -- -D warnings
cargo test --all-features --verbose
```

The authoritative release implementation is
`.github/workflows/ci.yml`. `upload-to-r2.sh` generates the update manifest,
and `verify-ota-archive.sh` validates launcher OTA archives before publishing.
