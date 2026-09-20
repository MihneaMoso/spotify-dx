use crate::app_error::AppError;
use crate::spotify::client;
use crate::spotify::session;
#[cfg(not(target_os = "android"))]
use crate::state::AUTH_STATE;
#[cfg(not(target_os = "android"))]
use dioxus::prelude::ReadableExt;

const PLAYER_BASE: &str = "https://api.spotify.com/v1/me/player";

/// Map a Connect transport status to a typed error. 401 (dead token), 403
/// (region/forbidden), and 404 (no active device) each mean something
/// actionable — collapsing them into `"failed with {status}"` forced every
/// caller (and every debugger) to guess. The response body rides along for
/// diagnosis instead of being discarded.
async fn check_transport(op: &str, resp: reqwest::Response) -> Result<(), AppError> {
    let status = resp.status().as_u16();
    if status == 200 || status == 204 {
        return Ok(());
    }
    let body = resp.text().await.unwrap_or_default();
    let detail = if body.is_empty() {
        String::new()
    } else {
        format!(": {}", body.chars().take(200).collect::<String>())
    };
    match status {
        401 => Err(AppError::SessionExpired),
        403 => Err(AppError::Forbidden(format!("{op} forbidden{detail}"))),
        404 => Err(AppError::Playback(format!(
            "{op}: no active device{detail}"
        ))),
        _ => Err(AppError::Playback(format!(
            "{op} failed with {status}{detail}"
        ))),
    }
}

fn require_premium() -> Result<(), AppError> {
    // Android runs this on bridge worker threads (no Dioxus runtime), where
    // even signal reads panic — consult the runtime-free session mirror the
    // bridge maintains instead. Desktop reads the live signal.
    #[cfg(target_os = "android")]
    let premium = crate::settings::session_premium();
    #[cfg(not(target_os = "android"))]
    let premium = AUTH_STATE.peek().is_premium();
    if !premium {
        return Err(AppError::PremiumRequired(
            "Playback requires Spotify Premium. Browsing is available for all accounts.".into(),
        ));
    }
    Ok(())
}

/// Resume playback of a track or context URI on the given Connect device.
/// Context URIs (`spotify:album:…`, `spotify:playlist:…`, `spotify:artist:…`)
/// go in `context_uri` per the API contract — sending them as track `uris`
/// was rejected and context playback never worked through this entry point.
pub async fn play(device_id: &str, uri: &str, position_ms: Option<u64>) -> Result<(), AppError> {
    require_premium()?;
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/play?device_id={device_id}");
    let mut body = serde_json::Map::new();
    if uri.starts_with("spotify:track:") || !uri.starts_with("spotify:") {
        body.insert("uris".into(), serde_json::json!([uri]));
    } else {
        body.insert("context_uri".into(), serde_json::json!(uri));
    }
    if let Some(pos) = position_ms {
        body.insert("position_ms".into(), serde_json::json!(pos));
    }
    let resp = client::filtered_put_auth(&url, &token, serde_json::Value::Object(body)).await?;
    check_transport("play", resp).await
}

/// Pause on the given device.
pub async fn pause(device_id: &str) -> Result<(), AppError> {
    require_premium()?;
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/pause?device_id={device_id}");
    let resp = client::filtered_put_auth(&url, &token, serde_json::json!({})).await?;
    check_transport("pause", resp).await
}

/// Skip to the next / previous track.
pub async fn skip(device_id: &str, next: bool) -> Result<(), AppError> {
    require_premium()?;
    let token = session::ensure_token().await?;
    let url = if next {
        format!("{PLAYER_BASE}/next?device_id={device_id}")
    } else {
        format!("{PLAYER_BASE}/previous?device_id={device_id}")
    };
    let resp = client::filtered_post_auth(&url, &token, serde_json::json!({})).await?;
    check_transport("skip", resp).await
}

/// Seek to an absolute ms position.
pub async fn seek(device_id: &str, position_ms: u64) -> Result<(), AppError> {
    require_premium()?;
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/seek?position_ms={position_ms}&device_id={device_id}");
    let resp = client::filtered_put_auth(&url, &token, serde_json::json!({})).await?;
    check_transport("seek", resp).await
}

/// Set playback volume on the device.
pub async fn set_volume(device_id: &str, volume_percent: u8) -> Result<(), AppError> {
    require_premium()?;
    let token = session::ensure_token().await?;
    let url = format!("{PLAYER_BASE}/volume?volume_percent={volume_percent}&device_id={device_id}");
    let resp = client::filtered_put_auth(&url, &token, serde_json::json!({})).await?;
    check_transport("volume", resp).await
}

/// Fetch the current Connect playback state directly from the API.
/// Success with an empty body (204 / nothing playing) is a normal empty
/// state — not a fetch failure.
pub async fn get_playback_state() -> Result<serde_json::Value, AppError> {
    require_premium()?;
    let token = session::ensure_token().await?;
    let resp = client::filtered_get_auth(PLAYER_BASE, &token).await?;
    if resp.status() == reqwest::StatusCode::NO_CONTENT {
        return Ok(serde_json::Value::Null);
    }
    let bytes = resp
        .error_for_status()
        .map_err(AppError::from)?
        .bytes()
        .await
        .map_err(AppError::from)?;
    if bytes.is_empty() {
        return Ok(serde_json::Value::Null);
    }
    serde_json::from_slice(&bytes)
        .map_err(|e| AppError::Spotify(format!("bad playback state body: {e}")))
}
