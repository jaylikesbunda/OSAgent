---
name: spotify
description: "Control Spotify playback with natural language"
icon_url: "https://upload.wikimedia.org/wikipedia/commons/thumb/1/19/Spotify_logo_without_text.svg/64px-Spotify_logo_without_text.svg.png"
config:
  - name: SPOTIFY_CLIENT_ID
    type: api_key
    description: "Client ID from your Spotify Developer app"
    required: true
  - name: SPOTIFY_CLIENT_SECRET
    type: api_key
    description: "Client Secret from your Spotify Developer app"
    required: true
  - name: SPOTIFY_REFRESH_TOKEN
    type: api_key
    description: "Refresh token from the authorize action (leave blank until first authorization)"
    required: false
token_refresh:
  token_url: "https://accounts.spotify.com/api/token"
  grant_type: "refresh_token"
  refresh_token_field: "SPOTIFY_REFRESH_TOKEN"
  access_token_field: "SPOTIFY_ACCESS_TOKEN"
  client_id_field: "SPOTIFY_CLIENT_ID"
  client_secret_field: "SPOTIFY_CLIENT_SECRET"
  authorize_url: "https://accounts.spotify.com/authorize"
  scopes: "user-modify-playback-state user-read-playback-state user-read-currently-playing playlist-modify-public playlist-modify-private playlist-read-private user-library-read user-library-modify"
  callback_port: 8888
  redirect_path: "/callback"
actions:
  - name: status
    description: "Get the currently playing track and playback state"
    type: http
    method: GET
    url: "https://api.spotify.com/v1/me/player/currently-playing"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
  - name: pause
    description: "Pause current Spotify playback"
    type: http
    method: PUT
    url: "https://api.spotify.com/v1/me/player/pause"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
  - name: resume
    description: "Resume Spotify playback on the active device"
    type: http
    method: PUT
    url: "https://api.spotify.com/v1/me/player/play"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
  - name: next_track
    description: "Skip to the next track in the queue"
    type: http
    method: POST
    url: "https://api.spotify.com/v1/me/player/next"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
  - name: previous_track
    description: "Go back to the previous track"
    type: http
    method: POST
    url: "https://api.spotify.com/v1/me/player/previous"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
  - name: set_volume
    description: "Set the playback volume (0-100)"
    type: http
    method: PUT
    url: "https://api.spotify.com/v1/me/player/volume"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    query:
      volume_percent: "{{ args.volume }}"
    parameters:
      - name: volume
        type: number
        description: "Volume level from 0 to 100"
        required: true
  - name: search_tracks
    description: "Search Spotify tracks and return candidate URIs"
    type: http
    method: GET
    url: "https://api.spotify.com/v1/search"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    query:
      q: "{{ args.query }}"
      type: "track"
      limit: "5"
    response_transform: "tracks.items"
    parameters:
      - name: query
        type: string
        description: "Song, artist, or album search query"
        required: true
  - name: play_uri
    description: "Play a specific Spotify track URI on the active device"
    type: http
    method: PUT
    url: "https://api.spotify.com/v1/me/player/play"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    body:
      uris:
        - "{{ args.uri }}"
    parameters:
      - name: uri
        type: string
        description: "Spotify track URI such as spotify:track:..."
        required: true
  - name: get_playlists
    description: "List the user's Spotify playlists"
    type: http
    method: GET
    url: "https://api.spotify.com/v1/me/playlists"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    query:
      limit: "50"
    response_transform: "items"
    parameters:
      - name: limit
        type: number
        description: "Number of playlists to return (max 50, default 50)"
        required: false
  - name: get_playlist_tracks
    description: "Get tracks from a specific playlist"
    type: http
    method: GET
    url: "https://api.spotify.com/v1/playlists/{{ args.playlist_id }}/tracks"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    query:
      limit: "20"
    response_transform: "items"
    parameters:
      - name: playlist_id
        type: string
        description: "Spotify playlist ID"
        required: true
  - name: create_playlist
    description: "Create a new Spotify playlist for the current user"
    type: http
    method: POST
    url: "https://api.spotify.com/v1/me/playlists"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    body:
      name: "{{ args.name }}"
      description: "{{ args.description }}"
      public: false
    parameters:
      - name: name
        type: string
        description: "Playlist name"
        required: true
      - name: description
        type: string
        description: "Playlist description (optional)"
        required: false
  - name: add_tracks_to_playlist
    description: "Add tracks to a Spotify playlist"
    type: http
    method: POST
    url: "https://api.spotify.com/v1/playlists/{{ args.playlist_id }}/tracks"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    body:
      uris: "{{ args.uris }}"
    parameters:
      - name: playlist_id
        type: string
        description: "Spotify playlist ID"
        required: true
      - name: uris
        type: string
        description: "Array of Spotify track URIs to add"
        required: true
  - name: save_track
    description: "Save a track to the user's Liked Songs"
    type: http
    method: PUT
    url: "https://api.spotify.com/v1/me/tracks"
    headers:
      Authorization: "Bearer {{ config.SPOTIFY_ACCESS_TOKEN }}"
    body:
      ids:
        - "{{ args.track_id }}"
    parameters:
      - name: track_id
        type: string
        description: "Spotify track ID (without the 'spotify:track:' prefix)"
        required: true
---
# Spotify Skill

Control Spotify playback through the Spotify Web API.

## Setup (one-time)

1. Go to the [Spotify Developer Dashboard](https://developer.spotify.com/dashboard) and create an app.
2. Add a redirect URI of `http://127.0.0.1:8888/callback` in the app settings (under "Edit Settings" > "Redirect URIs").
3. Copy the **Client ID** and **Client Secret** into the skill configuration fields.
4. Click the **Authorize** button in the skill settings. It will open your browser, you log into Spotify, and tokens are captured and saved automatically.

## How to use this skill

The agent should use the `skill_action` tool with `skill: "spotify"` and the appropriate `action` name.

### Playing a song

Always search first, then play the best match:

1. Call `search_tracks(query="song name or artist")` — returns a formatted list of tracks with names, artists, and URIs.
2. Pick the best match from the results.
3. Call `play_uri(uri="spotify:track:...")` with the track's URI.

**Example:** User says "play Bohemian Rhapsody"
```
skill_action(skill="spotify", action="search_tracks", args={query: "Bohemian Rhapsody"})
→ returns: "1. Bohemian Rhapsody | by Queen | uri: spotify:track:4u7EnebtmKWzUH433cf5Qv"
skill_action(skill="spotify", action="play_uri", args={uri: "spotify:track:4u7EnebtmKWzUH433cf5Qv"})
```

### Playback controls

- `pause` — pause current playback (no args needed)
- `resume` — resume playback (no args needed)
- `next_track` — skip to next track (no args needed)
- `previous_track` — go to previous track (no args needed)
- `set_volume(volume)` — set volume 0-100. Example: `set_volume(volume=50)` for half volume.
- `status` — check what's currently playing and playback state. Use this when the user asks "what's playing?" or "is music playing?"

### Playlists

- `get_playlists` — list the user's playlists. Returns playlist names and IDs.
- `get_playlist_tracks(playlist_id)` — get tracks from a specific playlist. Extract the playlist ID from the `get_playlists` result or from a playlist URI (`spotify:playlist:xxxxx` → ID is `xxxxx`).
- `create_playlist(name, description?)` — create a new playlist. Example: `create_playlist(name="Chill Vibes", description="Relaxing tracks")`.
- `add_tracks_to_playlist(playlist_id, uris)` — add one or more tracks to a playlist. The `uris` should be an array of Spotify track URIs. Example: `add_tracks_to_playlist(playlist_id="abc123", uris=["spotify:track:xxx", "spotify:track:yyy"])`.

**Example workflow — create a playlist and add songs:**
```
skill_action(skill="spotify", action="create_playlist", args={name: "Road Trip"})
→ returns playlist info with ID
skill_action(skill="spotify", action="search_tracks", args={query: "Life is a Highway"})
→ returns track URI
skill_action(skill="spotify", action="add_tracks_to_playlist", args={playlist_id: "abc123", uris: ["spotify:track:xxx"]})
```

### Liked Songs

- `save_track(track_id)` — save a track to the user's Liked Songs. The `track_id` is the ID portion of the track URI (e.g., from `spotify:track:4u7EnebtmKWzUH433cf5Qv`, the ID is `4u7EnebtmKWzUH433cf5Qv`).

### Important rules

- Always use `search_tracks` before `play_uri` — never guess a track URI.
- The `search_tracks` response is formatted as a readable list. Extract the URI from the `uri: spotify:track:...` part.
- If the user asks to play something from a playlist, use `get_playlist_tracks` first to find the track URI, then `play_uri`.
- If playback fails, try `resume` — sometimes the player just needs to be unpaused.
- Volume is 0-100. Map natural language: "quiet" → 20, "medium" → 50, "loud" → 80, "max" → 100.
