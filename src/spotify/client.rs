use crate::adblock;
use crate::app_error::AppError;
use once_cell::sync::Lazy;
use reqwest::header::{
    HeaderMap, HeaderValue, ACCEPT_LANGUAGE, AUTHORIZATION, CONTENT_TYPE, USER_AGENT,
};
use reqwest::{Client, Response};

/// Chrome-on-macOS user agent, matching what open.spotify.com sees.
const CHROME_UA: &str = "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) \
                        AppleWebKit/537.36 (KHTML, like Gecko) \
                        Chrome/124.0.0.0 Safari/537.36";

/// The filtered HTTP client. Created once, shared process-wide.
static CLIENT: Lazy<Client> = Lazy::new(build_client);

/// Build a `reqwest::Client` that mimics an up-to-date Chrome desktop install.
pub fn build_client() -> Client {
    let mut headers = HeaderMap::new();
    headers.insert(
        USER_AGENT,
        HeaderValue::from_static(CHROME_UA),
    );
    headers.insert(ACCEPT_LANGUAGE, HeaderValue::from_static("en-US,en;q=0.9"));
    headers.insert(
        CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    let builder = Client::builder()
        .default_headers(headers)
        .user_agent(CHROME_UA);
    // Cookie jar, gzip/brotli and timeouts are native-only in reqwest; the
    // wasm/fetch backend negotiates those itself.
    #[cfg(not(target_arch = "wasm32"))]
    let builder = builder
        .cookie_store(true)
        .gzip(true)
        .brotli(true)
        .timeout(std::time::Duration::from_secs(20));
    builder
        .build()
        .expect("failed to build spotify http client")
}

/// Perform a GET through the ad-filter. Every outgoing request in the spotify
/// module must go through this gate — network I/O must not bypass it.
pub async fn filtered_get(url: &str) -> Result<Response, AppError> {
    if adblock::should_block(url) {
        adblock::record_drop();
        tracing::debug!("ad-block: dropped {url}");
        return Err(AppError::AdBlock(url.to_owned()));
    }
    CLIENT.get(url).send().await.map_err(AppError::from)
}

/// GET with a Bearer token attached, still through the ad-filter.
pub async fn filtered_get_auth(
    url: &str,
    access_token: &str,
) -> Result<Response, AppError> {
    if adblock::should_block(url) {
        adblock::record_drop();
        tracing::debug!("ad-block: dropped {url}");
        return Err(AppError::AdBlock(url.to_owned()));
    }
    CLIENT
        .get(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .send()
        .await
        .map_err(AppError::from)
}

/// PUT with a Bearer token (used by the Connect API).
pub async fn filtered_put_auth(
    url: &str,
    access_token: &str,
    body: serde_json::Value,
) -> Result<Response, AppError> {
    if adblock::should_block(url) {
        adblock::record_drop();
        tracing::debug!("ad-block: dropped {url}");
        return Err(AppError::AdBlock(url.to_owned()));
    }
    CLIENT
        .put(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .json(&body)
        .send()
        .await
        .map_err(AppError::from)
}

/// POST with a Bearer token (used by the Connect API).
pub async fn filtered_post_auth(
    url: &str,
    access_token: &str,
    body: serde_json::Value,
) -> Result<Response, AppError> {
    if adblock::should_block(url) {
        adblock::record_drop();
        tracing::debug!("ad-block: dropped {url}");
        return Err(AppError::AdBlock(url.to_owned()));
    }
    CLIENT
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .json(&body)
        .send()
        .await
        .map_err(AppError::from)
}

/// POST to Spotify's internal GraphQL API (`api-partner.spotify.com/pathfinder`).
/// Mirrors what the web player sends: an `app-platform` header plus browser
/// Origin/Referer so pathfinder accepts our web-player bearer token.
pub async fn filtered_post_pathfinder(
    url: &str,
    access_token: &str,
    body: serde_json::Value,
) -> Result<Response, AppError> {
    if adblock::should_block(url) {
        adblock::record_drop();
        tracing::debug!("ad-block: dropped {url}");
        return Err(AppError::AdBlock(url.to_owned()));
    }
    CLIENT
        .post(url)
        .header(AUTHORIZATION, format!("Bearer {access_token}"))
        .header("app-platform", "WebPlayer")
        .header("Origin", "https://open.spotify.com")
        .header("Referer", "https://open.spotify.com/")
        .json(&body)
        .send()
        .await
        .map_err(AppError::from)
}