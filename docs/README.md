# OSAgent docs (Hugo)

Hugo docs site for OSA, styled after the GhostESP Hugo docs
(sidebar navigation, Fuse.js search, light/dark theme).

## Preview

Requires Hugo extended (v0.120+):

```powershell
cd docs
hugo server
```

## Build

```powershell
cd docs
hugo --minify
```

Output goes to `docs/public/` (gitignored).

## Layout

- `hugo.toml` — site config (no versioning by default; the theme supports it if added later)
- `content/` — markdown docs, grouped by section with `weight` front matter
- `assets/` — CSS/JS/images processed by Hugo pipes
- `themes/osagent/` — layouts (topbar, sidebar, search, cards, prev/next nav)
- `static/` — copied verbatim (`robots.txt`, images)
