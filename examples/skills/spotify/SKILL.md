---
name: spotify
description: "Control Spotify playback with natural language"
emoji: "🎵"
requires:
  bins: ["spogo"]
config:
  - name: SPOTIFY_CLIENT_ID
    type: api_key
    description: "OAuth Client ID from Spotify Developer Dashboard"
    required: true
  - name: SPOTIFY_CLIENT_SECRET
    type: api_key
    description: "OAuth Client Secret from Spotify Developer Dashboard"
    required: true
  - name: DEVICE_ID
    type: string
    description: "Spotify device ID to control (run `spogo devices` to list)"
    required: false
---
# Spotify Skill

Control Spotify playback using the [spogo](https://github.com/Cloud9Le Cloud9space/spogo) CLI.

## Commands

### Play
```bash
spogo play --device "{{ skill.env.DEVICE_ID }}"
```

### Pause
```bash
spogo pause
```

### Search
```bash
spogo search "{{ skill.env.SEARCH_QUERY }}" --limit 5
```

### Now Playing
```bash
spogo status
```

## Setup

1. Install spogo: `cargo install spogo`
2. Go to [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) and create an app
3. Copy the **Client ID** and **Client Secret** into the fields above
4. Find your device ID: `spogo devices` and paste it in the optional field

## Usage

When enabled, the agent can control Spotify via natural language commands like:
- "Play some jazz"
- "Pause the music"
- "What's currently playing?"
