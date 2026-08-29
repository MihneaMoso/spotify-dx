//! TIDAL provider: resolves tracks via community TIDAL proxy instances.
//!
//! Uses a live uptime list (`tidal-uptime.geeked.wtf`) merged in front of a
//! static fallback pool. The uptime list is cached for ~5 minutes.
//!
//! Resolution flow:
//! 1. Odesli gives us a TIDAL URL like `https://tidal.com/browse/12345`.
//! 2. We extract the TIDAL track ID from that URL.
//! 3. We POST to a TIDAL proxy endpoint: `{instance}/api/dl/{track_id}`.
//! 4. The proxy returns a direct stream URL (FLAC/MP3).

use std::sync::OnceLock;
use std::time::{Duration, Instant};

use async_trait::async_trait;

use crate::streaming::odesli;
use crate::streaming::provider::{AudioFormat, Provider, Quality, Resolution, TrackQuery};

/// Static fallback TIDAL proxy instances (used when the uptime list is stale).
const FALLBACK_INSTANCES: &[&str] = &[
    "https://monochrome.nyc",     // Spotufi's primary
    "https://monochrome.us.to",
    "https://quack.wtf",
];

/// Uptime list URL.
const UPTIME_URL: &str = "https://tidal-uptime.geeked.wtf";

/// How long to cache the uptime list.
const UPTIME_TTL: Duration = Duration::from_secs(5 * 60);

struct UptimeState {
    instances: Vec<String>,
    fetched_at: Instant,
}

static UPTIME: OnceLock<std::sync::Mutex<UptimeState>> = OnceLock::new();

fn uptime_state() -> &'static std::sync::Mutex<UptimeState> {
    UPTIME.get_or_init(|| {
        std::sync::Mutex::new(UptimeState {
            instances: FALLBACK_INSTANCES.iter().map(|s| s.to_string()).collect(),
            fetched_at: Instant::now() - UPTIME_TTL * 2, // force first fetch
        })
    })
}

/// Refresh the live uptime list in the background. Non-blocking.
pub async fn refresh_uptime() {
    let Ok(resp) = reqwest::get(UPTIME_URL).await else {
        return;
    };
    let Ok(body) = resp.text().await else {
        return;
    };
    // The uptime list is newline-separated base URLs of healthy instances.
    let mut instances: Vec<String> = body
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| l.starts_with("http"))
        .collect();
    // Always include fallbacks at the end.
    for fb in FALLBACK_INSTANCES {
        let fb_str = fb.to_string();
        if !instances.contains(&fb_str) {
            instances.push(fb_str);
        }
    }
    if instances.is_empty() {
        return;
    }
    if let Ok(mut state) = uptime_state().lock() {
        state.instances = instances;
        state.fetched_at = Instant::now();
    }
}

fn get_instances() -> Vec<String> {
    let mut instances = Vec::new();
    if let Ok(state) = uptime_state().lock() {
        instances = state.instances.clone();
    }
    if instances.is_empty() {
        instances = FALLBACK_INSTANCES.iter().map(|s| s.to_string()).collect();
    }
    instances
}

pub struct TidalProvider {
    client: reqwest::Client,
}

impl Default for TidalProvider {
    fn default() -> Self {
        Self::new()
    }
}

impl TidalProvider {
    pub fn new() -> Self {
        Self {
            client: {
                let builder = reqwest::Client::builder();
                #[cfg(not(target_arch = "wasm32"))]
                let builder = builder.timeout(Duration::from_secs(10));
                builder.build().unwrap_or_default()
            },
        }
    }
}

#[async_trait(?Send)]
impl Provider for TidalProvider {
    fn name(&self) -> &'static str {
        "tidal"
    }

    fn is_available(&self) -> bool {
        // DISABLED: Odesli (song.link) — the only Spotify→TIDAL ID mapper — was
        // sunset (API now returns 401 PUBLIC_API_ACCESS_DEPRECATED and requires
        // a paid API key), and the community proxy instances return 404. Until a
        // working ID mapper exists, TIDAL cannot resolve; return false so the
        // resolver skips it without calling the dead Odesli API.
        false
    }

    async fn resolve(&self, query: &TrackQuery) -> Resolution {
        // Step 1: Get TIDAL URL from Odesli.
        let odesli_ids = match odesli::resolve(&query.spotify_id).await {
            Some(ids) => ids,
            None => return Resolution::Error("odesli mapping failed".into()),
        };
        let tidal_url = match odesli_ids.tidal_url {
            Some(url) => url,
            None => return Resolution::NotFound,
        };
        let track_id = match odesli::extract_id_from_url(&tidal_url) {
            Some(id) => id,
            None => return Resolution::Error("failed to extract TIDAL track ID".into()),
        };

        // Step 2: Try instances in order.
        let instances = get_instances();
        let mut last_error = String::new();
        for instance in &instances {
            let api_url = format!("{}/api/dl/{}", instance.trim_end_matches('/'), track_id);
            match self.client.get(&api_url).send().await {
                Ok(resp) if resp.status().is_success() => {
                    if let Ok(body) = resp.text().await {
                        // The response may be a direct URL or JSON with a URL field.
                        let url = extract_url_from_response(&body);
                        if !url.is_empty() {
                            let format = guess_format(&url);
                            return Resolution::Success {
                                url,
                                format,
                                quality: Quality::Lossless,
                            };
                        }
                    }
                }
                Ok(resp) if resp.status().as_u16() == 503 => {
                    return Resolution::Cooldown {
                        retry_after_secs: 60,
                    };
                }
                Ok(resp) => {
                    last_error = format!("HTTP {} from {}", resp.status(), instance);
                }
                Err(e) => {
                    last_error = format!("{}: {}", instance, e);
                }
            }
        }
        Resolution::Error(format!("all TIDAL instances failed: {last_error}"))
    }
}

/// Extract a URL from the proxy response (may be plain text or JSON).
fn extract_url_from_response(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.starts_with("http") {
        return trimmed.to_string();
    }
    // Try JSON: {"url": "..."}
    if let Ok(val) = serde_json::from_str::<serde_json::Value>(trimmed) {
        if let Some(url) = val.get("url").and_then(|u| u.as_str()) {
            return url.to_string();
        }
    }
    String::new()
}

/// Guess the audio format from a URL's file extension.
fn guess_format(url: &str) -> AudioFormat {
    let lower = url.to_lowercase();
    if lower.contains(".flac") {
        AudioFormat::Flac
    } else if lower.contains(".mp3") {
        AudioFormat::Mp3
    } else if lower.contains(".m4a") || lower.contains(".aac") {
        AudioFormat::Aac
    } else if lower.contains(".ogg") || lower.contains(".opus") {
        AudioFormat::Ogg
    } else {
        AudioFormat::Unknown
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_url_plain() {
        assert_eq!(
            extract_url_from_response("https://cdn.example.com/song.flac"),
            "https://cdn.example.com/song.flac"
        );
    }

    #[test]
    fn extract_url_json() {
        let body = r#"{"url": "https://cdn.example.com/song.flac", "quality": "hires"}"#;
        assert_eq!(
            extract_url_from_response(body),
            "https://cdn.example.com/song.flac"
        );
    }

    #[test]
    fn guess_format_from_extension() {
        assert_eq!(guess_format("https://x.com/s.flac"), AudioFormat::Flac);
        assert_eq!(guess_format("https://x.com/s.mp3"), AudioFormat::Mp3);
        assert_eq!(guess_format("https://x.com/s.m4a"), AudioFormat::Aac);
        assert_eq!(guess_format("https://x.com/s"), AudioFormat::Unknown);
    }
}
