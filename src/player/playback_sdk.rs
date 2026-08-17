/// The HTML document loaded into the hidden WebView.
///
/// It boots the Spotify Web Playback SDK, registers a `Spotify.Player` and
/// forwards every interesting event to Rust over the `window.ipc.postMessage`
/// bridge. Rust talks back through the `window._relay` object.
pub const SDK_HTML: &str = r#"<!DOCTYPE html><html><head>
<script src="https://sdk.scdn.co/spotify-player.js"></script>
<script>
// Fetch a fresh web-player access token from Spotify's internal endpoint. This
// works only inside the WebView because it relies on the HttpOnly session
// cookies (sp_dc) that the shared data directory holds — Rust can never see
// them, so the SDK never asks Rust for a token.
function fetchAccessToken() {
  return fetch(
    'https://open.spotify.com/get_access_token?reason=transport&productType=web_player',
    { credentials: 'include' }
  ).then(r => r.json());
}
window.onSpotifyWebPlaybackSDKReady = () => {
  const player = new Spotify.Player({
    name: 'Spotify DX',
    getOAuthToken: cb => {
      fetchAccessToken()
        .then(d => {
          window.ipc.postMessage(JSON.stringify({
            type: 'token_refresh',
            token: d.accessToken,
            expiresMs: d.accessTokenExpirationTimestampMs,
            isAnon: d.isAnonymous
          }));
          cb(d.accessToken);
        })
        .catch(e => {
          window.ipc.postMessage(JSON.stringify({ type: 'token_error', msg: e.toString() }));
        });
    },
    volume: 0.8,
  });
  player.addListener('ready', ({ device_id }) =>
    window.ipc.postMessage(JSON.stringify({ type: 'ready', device_id })));
  player.addListener('not_ready', () =>
    window.ipc.postMessage(JSON.stringify({ type: 'not_ready' })));
  player.addListener('player_state_changed', state =>
    window.ipc.postMessage(JSON.stringify({ type: 'state', payload: state })));
  player.addListener('authentication_error', ({ message }) =>
    window.ipc.postMessage(JSON.stringify({ type: 'auth_error', message })));
  player.addListener('initialization_error', ({ message }) =>
    window.ipc.postMessage(JSON.stringify({ type: 'init_error', message })));
  window._player = player;
  window._relay = {
    play:  () => player.resume(),
    pause: () => player.pause(),
    next:  () => player.nextTrack(),
    prev:  () => player.previousTrack(),
    seek:  (ms) => player.seek(ms),
    volume: (v) => player.setVolume(v),
    refreshToken: () => {
      fetchAccessToken()
        .then(d => window.ipc.postMessage(JSON.stringify({
          type: 'token_refresh_result',
          token: d.accessToken,
          expiresMs: d.accessTokenExpirationTimestampMs,
          isAnon: d.isAnonymous
        })))
        .catch(e => window.ipc.postMessage(JSON.stringify({
          type: 'token_error', msg: e.toString()
        })));
    },
    connect: () => player.connect(),
  };
  player.connect();
};
</script></head><body></body></html>"#;

use crate::spotify::models::{AlbumRef, ArtistRef, Track};

/// Structured playback state decoded from a Web Playback SDK
/// `player_state_changed` payload.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SdkState {
    pub track: Option<Track>,
    pub is_playing: bool,
    pub position_ms: u64,
    pub duration_ms: u64,
    pub shuffle: bool,
    pub repeat: u8,
}

/// Interpret a `player_state_changed` payload into the local model. Unknown or
/// malformed shapes degrade to a default state rather than panicking.
pub fn parse_sdk_state(payload: &serde_json::Value) -> SdkState {
    let mut state = SdkState::default();

    if let Some(paused) = payload.get("paused").and_then(|v| v.as_bool()) {
        state.is_playing = !paused;
    }
    if let Some(pos) = payload.get("position").and_then(|v| v.as_u64()) {
        state.position_ms = pos;
    }
    if let Some(dur) = payload.get("duration").and_then(|v| v.as_u64()) {
        state.duration_ms = dur;
    }
    if let Some(shuffle) = payload.get("shuffle").and_then(|v| v.as_bool()) {
        state.shuffle = shuffle;
    }
    if let Some(repeat) = payload.get("repeat_mode").and_then(|v| v.as_u64()) {
        state.repeat = repeat.min(2) as u8;
    }

    state.track = payload
        .get("track_window")
        .and_then(|w| w.get("current_track"))
        .and_then(track_from_sdk);
    state
}

/// Map an SDK `current_track` object onto the app's `Track` model.
fn track_from_sdk(value: &serde_json::Value) -> Option<Track> {
    let id = value.get("id").and_then(|v| v.as_str())?.to_owned();
    Some(Track {
        id,
        name: value.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
        duration_ms: value
            .get("duration_ms")
            .and_then(|v| v.as_u64())
            .unwrap_or_default(),
        uri: value.get("uri").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
        explicit: value
            .get("explicit")
            .and_then(|v| v.as_bool())
            .unwrap_or_default(),
        preview_url: None,
        popularity: 0,
        artists: value
            .get("artists")
            .and_then(|a| a.as_array())
            .map(|artists| {
                artists
                    .iter()
                    .map(|artist| ArtistRef {
                            id: artist.get("uri").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                            name: artist.get("name").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                            uri: artist.get("uri").and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                        })
                    .collect()
            })
            .unwrap_or_default(),
        album: {
            let album = value.get("album");
            AlbumRef {
                id: album
                    .and_then(|a| a.get("uri"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                name: album.and_then(|a| a.get("name")).and_then(|v| v.as_str()).unwrap_or_default().to_owned(),
                uri: album
                    .and_then(|a| a.get("uri"))
                    .and_then(|v| v.as_str())
                    .unwrap_or_default()
                    .to_owned(),
                images: album
                    .and_then(|a| a.get("images"))
                    .and_then(|imgs| imgs.as_array())
                    .map(|imgs| {
                        imgs.iter()
                            .filter_map(|img| {
                                Some(crate::spotify::models::SpotifyImage {
                                    url: img.get("url").and_then(|v| v.as_str())?.to_owned(),
                                    width: img.get("width").and_then(|v| v.as_u64()).map(|w| w as u32),
                                    height: img.get("height").and_then(|v| v.as_u64()).map(|h| h as u32),
                                })
                            })
                            .collect()
                    })
                    .unwrap_or_default(),
                album_type: None,
                release_date: None,
            }
        },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_player_state_payload() {
        let payload = serde_json::json!({
            "paused": false,
            "position": 12345,
            "duration": 267111,
            "shuffle": true,
            "repeat_mode": 2,
            "track_window": {
                "current_track": {
                    "id": "4NHQUGzhtTLFpeF5Z4S3zy",
                    "uri": "spotify:track:4NHQUGzhtTLFpeF5Z4S3zy",
                    "name": "Mercy",
                    "duration_ms": 267111,
                    "explicit": true,
                    "artists": [ { "name": "Kanye West" } ],
                    "album": { "name": "The Life of Pablo", "images": [] }
                }
            }
        });
        let state = parse_sdk_state(&payload);
        assert!(state.is_playing);
        assert_eq!(state.position_ms, 12345);
        assert_eq!(state.duration_ms, 267111);
        assert!(state.shuffle);
        assert_eq!(state.repeat, 2);
        let track = state.track.expect("track present");
        assert_eq!(track.name, "Mercy");
        assert_eq!(track.artists[0].name, "Kanye West");
    }

    #[test]
    fn handles_paused_empty_state() {
        let state = parse_sdk_state(&serde_json::json!({ "paused": true }));
        assert!(!state.is_playing);
        assert!(state.track.is_none());
    }
}