# Releasing

## Release Contract

- GitHub runs CI only.
- GitLab is the release publisher.
- Cloudflare R2 is the auto-update source.
- The launcher is the only end-user distributable.
- The updater only replaces the launcher.
- The new launcher carries the new embedded core and updater.

## Required GitLab Variables

- `R2_ACCOUNT_ID`
- `R2_ACCESS_KEY_ID`
- `R2_SECRET_ACCESS_KEY`

Optional:

- `R2_BUCKET` if you do not want the default `osagent-releases`
- `R2_RELEASE_PREFIX` if you do not want the default `releases`
- `CDN_BASE_URL` if you do not want the default `https://releases.osagent.dev`
- `GITLAB_RELEASE_TOKEN` if your runner cannot create releases with `CI_JOB_TOKEN`

## Release Flow

1. Push a tag to GitLab.
2. GitLab builds Windows and Linux launcher archives.
3. GitLab publishes archive checksums as separate assets.
4. GitLab uploads the archives, checksums, and `release-manifest.json` to Cloudflare R2.
5. GitLab updates `releases/latest.json` on the CDN.
6. GitLab creates or updates the GitLab Release with links to the R2-hosted assets.

## Published Assets

- `osagent-windows-x86_64.zip`
- `osagent-windows-x86_64.zip.sha256`
- `osagent-linux-x86_64.tar.gz`
- `osagent-linux-x86_64.tar.gz.sha256`
- `release-manifest.json`
- `releases/latest.json`

## Local Validation

Run this before tagging a release:

```powershell
.\build-launcher.ps1 -Checks
```

That validates the same launcher-first build contract the release pipeline uses.
