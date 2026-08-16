use crate::app_error::AppError;
use crate::spotify::client;
use crate::spotify::session;

const PLAYER_BASE: &str = "https://api.spotify.com/v1/me/player";

/// Resume playback of a track or context URI on the given Connect device.
pub async fn play(
    device_id: &str,
    uri: &str,
    position_ms: Option<u64>,
) -> Result<(), AppError> {
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/play?device_id={device_id}");
    let mut body = serde_json::Map::new();
    body.insert("uris".into(), serde_json::json!([uri]));
    if let Some(pos) = position_ms {
        body.insert("position_ms".into(), serde_json::json!(pos));
    }
    let resp = client::filtered_put_auth(&url, &token, serde_json::Value::Object(body)).await?;
    match resp.status().as_u16() {
        200 | 204 => Ok(()),
        status => Err(AppError::Playback(format!("play failed with {status}"))),
    }
}

/// Pause on the given device.
pub async fn pause(device_id: &str) -> Result<(), AppError> {
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/pause?device_id={device_id}");
    let resp = client::filtered_put_auth(&url, &token, serde_json::json!({})).await?;
    match resp.status().as_u16() {
        200 | 204 => Ok(()),
        status => Err(AppError::Playback(format!("pause failed with {status}"))),
    }
}

/// Skip to the next / previous track.
pub async fn skip(device_id: &str, next: bool) -> Result<(), AppError> {
    let token = session::ensure_token().await?;
    let url = if next {
        format!("{PLAYER_BASE}/next?device_id={device_id}")
    } else {
        format!("{PLAYER_BASE}/previous?device_id={device_id}")
    };
    let resp = client::filtered_post_auth(&url, &token, serde_json::json!({})).await?;
    match resp.status().as_u16() {
        200 | 204 => Ok(()),
        status => Err(AppError::Playback(format!("skip failed with {status}"))),
    }
}

/// Seek to an absolute ms position.
pub async fn seek(device_id: &str, position_ms: u64) -> Result<(), AppError> {
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/seek?position_ms={position_ms}&device_id={device_id}");
    let resp = client::filtered_put_auth(&url, &token, serde_json::json!({})).await?;
    match resp.status().as_u16() {
        200 | 204 => Ok(()),
        status => Err(AppError::Playback(format!("seek failed with {status}"))),
    }
}

/// Set playback volume on the device.
pub async fn set_volume(device_id: &str, volume_percent: u8) -> Result<(), AppError> {
    let token = session::ensure_token().await?;
    let url = format!(
        "{PLAYER_BASE}/volume?volume_percent={volume_percent}&device_id={device_id}"
    );
    let resp = client::filtered_put_auth(&url, &token, serde_json::json!({})).await?;
    match resp.status().as_u16() {
        200 | 204 => Ok(()),
        status => Err(AppError::Playback(format!("volume failed with {status}"))),
    }
}

/// Fetch the current Connect playback state directly from the API.
pub async fn get_playback_state() -> Result<serde_json::Value, AppError> {
    let token = session::ensure_token().await?;
    let resp = client::filtered_get_auth(PLAYER_BASE, &token).await?;
    resp.error_for_status()
        .map_err(AppError::from)?
        .json()
        .await
        .map_err(AppError::from)
}

/// Report randomized device ids to make each launch look fresh.
pub fn random_device_id() -> String {
    use rand::Rng as _;
    let mut id = String::with_capacity(32);
    for _ in 0..32 {
        id.push(rand::thread_rng().gen_range(b'a'..=b'z') as char);
    }
    id
}