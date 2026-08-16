pub mod pkce;
pub mod token_store;

use crate::auth::pkce::PkceCodeVerifier;
use crate::spotify::api;
use crate::state::AuthState;
use anyhow::Context as _;
use chrono::{Duration as ChronoDuration, Utc};
use oauth2::reqwest::async_http_client;
use oauth2::{
    AuthType, AuthUrl, AuthorizationCode, ClientId, PkceCodeVerifier as OAuthVerifier,
    RedirectUrl, RefreshToken, TokenUrl,
};
use rand::distributions::{Alphanumeric, DistString};
use std::collections::HashMap;

pub const AUTH_URL: &str = "https://accounts.spotify.com/authorize";
pub const TOKEN_URL: &str = "https://accounts.spotify.com/api/token";
pub const CLIENT_ID: &str = env!("SPOTIFY_CLIENT_ID");
/// The custom URI scheme registered for deep-link driven login on mobile.
pub const REDIRECT_SCHEME: &str = "spotifydx://callback";
/// Loopback port the OAuth callback listener binds to. Spotify's dashboard only
/// accepts concrete redirect URIs (no port wildcards), so this must stay fixed
/// and match `http://127.0.0.1:{CALLBACK_PORT}/callback` in the app settings.
pub const CALLBACK_PORT: u16 = 8888;

const SCOPE: &str = "user-read-private user-read-email streaming \
                    user-library-read user-library-modify user-top-read \
                    user-read-currently-playing user-read-playback-state \
                    user-modify-playback-state playlist-read-private \
                    playlist-read-collaborative";

/// Load any persisted tokens, refreshing them when necessary. Returns
/// `Ok(None)` when the user has never logged in (or the session is dead).
pub async fn init() -> anyhow::Result<Option<AuthState>> {
    let Some(stored) = token_store::load() else {
        return Ok(None);
    };

    let mut state = AuthState::default();
    if let Some(stored_refresh) = stored.refresh_token.as_deref() {
        match refresh(stored_refresh).await {
            Ok((access, new_refresh, expires_in)) => {
                let _ = token_store::save(&access, &new_refresh);
                state.access_token = Some(access);
                state.refresh_token = (!new_refresh.is_empty()).then_some(new_refresh);
                state.expires_at = Some(Utc::now() + ChronoDuration::seconds(expires_in));
            }
            Err(err) => tracing::warn!("auth: initial refresh failed: {err:#}"),
        }
    }

    // Fall back to the stored access token when refresh did not produce one.
    if state.access_token.is_none() {
        if let Some(access) = stored.access_token {
            // No way to inspect remaining lifetime, so we treat it as valid and
            // let a 401 during the first fetch trigger a fresh refresh.
            state.access_token = Some(access);
            state.refresh_token = stored.refresh_token;
            state.expires_at = Some(Utc::now() + ChronoDuration::seconds(3600));
        }
    }

    if let (Some(token), None) = (&state.access_token, &state.user_id) {
        if let Ok(profile) = api::get_current_user_profile(token).await {
            state.user_id = Some(profile.id);
            state.user_display_name = profile.display_name;
            state.user_avatar_url = profile.images.first().map(|img| img.url.clone());
        } else if state.expires_at.map(|e| e < Utc::now()).unwrap_or(true) {
            return Ok(None);
        }
    }

    if state.access_token.is_none() {
        return Ok(None);
    }
    Ok(Some(state))
}

/// Drive the full OAuth2 PKCE flow:
///   1. generate a verifier + challenge
///   2. open the system browser on the authorize screen
///   3. listen on a loopback port for the redirect
///   4. exchange the authorization code for a token pair
///   5. persist the pair to the keychain
pub async fn login() -> anyhow::Result<AuthState> {
    let verifier = PkceCodeVerifier::new_random();
    let challenge = verifier.code_challenge();
    let state = generate_state();

    let listener = tokio::net::TcpListener::bind(("127.0.0.1", CALLBACK_PORT))
        .await
        .with_context(|| {
            format!(
                "could not bind the OAuth callback listener on port {CALLBACK_PORT} \
                 (is another spotify-dx instance running?)"
            )
        })?;
    let port = listener.local_addr().context("no listener address")?.port();
    let redirect_uri = format!("http://127.0.0.1:{port}/callback");

    let authorize_url = url::Url::parse_with_params(
        AUTH_URL,
        &[
            ("client_id", CLIENT_ID),
            ("response_type", "code"),
            ("redirect_uri", &redirect_uri),
            ("scope", SCOPE),
            ("state", &state),
            (
                "code_challenge",
                challenge
                    .secret()
                    .ok_or_else(|| anyhow::anyhow!("pkce challenge missing"))?,
            ),
            ("code_challenge_method", "S256"),
        ],
    )
    .context("failed to build the authorize URL")?;

    tracing::info!("auth: open your browser at {authorize_url}");
    if let Err(err) = webbrowser::open(authorize_url.as_str()) {
        // Keep going — the user may paste the URL manually.
        tracing::warn!("auth: could not open a browser ({err}), copy the URL instead");
        anyhow::bail!("copy the following URL into your browser:\n{authorize_url}");
    }

    let code = listen_for_code(&listener, &state).await?;
    tracing::info!("auth: got authorization code; exchanging for tokens");
    let (access, refresh, expires_in) = exchange_code(&code, &verifier, &redirect_uri).await?;
    tracing::info!("auth: token exchange succeeded (expires in {expires_in}s)");

    let mut auth = AuthState {
        access_token: Some(access.clone()),
        refresh_token: (!refresh.is_empty()).then_some(refresh.clone()),
        expires_at: Some(Utc::now() + ChronoDuration::seconds(expires_in)),
        ..AuthState::default()
    };
    if let Ok(profile) = api::get_current_user_profile(&access).await {
        auth.user_id = Some(profile.id);
        auth.user_display_name = profile.display_name;
        auth.user_avatar_url = profile.images.first().map(|img| img.url.clone());
    }

    match token_store::save(&access, &refresh) {
        Ok(()) => tracing::info!("auth: tokens persisted"),
        Err(err) => tracing::warn!("auth: could not persist tokens: {err:#}"),
    }
    Ok(auth)
}

/// Forget the session: clear the keychain and reset the global state.
pub fn logout() {
    if let Err(err) = token_store::clear() {
        tracing::warn!("auth: keychain clear failed: {err:#}");
    }
    crate::state::AUTH_STATE.write().reset();
}

/// Holding cell for the session produced during `bootstrap()` (before the
/// dioxus runtime exists) so `App` can pick it up on first render. This keeps
/// the login gate entirely out of the startup path.
static BOOT_AUTH: once_cell::sync::Lazy<parking_lot::Mutex<Option<AuthState>>> =
    once_cell::sync::Lazy::new(|| parking_lot::Mutex::new(None));

/// Store a session for the first render to consume.
pub fn set_boot_auth(auth: AuthState) {
    *BOOT_AUTH.lock() = Some(auth);
}

/// Take the session left by `set_boot_auth`, if any.
pub fn take_boot_auth() -> Option<AuthState> {
    BOOT_AUTH.lock().take()
}

/// Refresh a token pair. Returns `(access_token, refresh_token, expires_in_secs)`.
/// Spotify hands out a fresh refresh token on every refresh call.
pub async fn refresh(refresh_token: &str) -> anyhow::Result<(String, String, i64)> {
    let client = make_client(None)?;
    let token = client
        .exchange_refresh_token(&RefreshToken::new(refresh_token.to_owned()))
        .request_async(&async_http_client)
        .await
        .context("refresh token request failed")?;
    Ok(extract_tokens(token))
}

async fn exchange_code(
    code: &str,
    verifier: &PkceCodeVerifier,
    redirect_uri: &str,
) -> anyhow::Result<(String, String, i64)> {
    let client = make_client(Some(redirect_uri))?;
    let oauth_verifier = OAuthVerifier::new(verifier.secret().to_owned());
    let token = client
        .exchange_code(AuthorizationCode::new(code.to_owned()))
        .set_pkce_verifier(oauth_verifier)
        .request_async(&async_http_client)
        .await
        .context("authorization code exchange failed")?;
    Ok(extract_tokens(token))
}

fn make_client(
    redirect_uri: Option<&str>,
) -> anyhow::Result<oauth2::basic::BasicClient> {
    let mut client = oauth2::basic::BasicClient::new(
        ClientId::new(CLIENT_ID.to_owned()),
        None,
        AuthUrl::new(AUTH_URL.to_owned())?,
        Some(TokenUrl::new(TOKEN_URL.to_owned())?),
    )
    .set_auth_type(AuthType::RequestBody);
    if let Some(uri) = redirect_uri {
        client = client.set_redirect_uri(RedirectUrl::new(uri.to_owned())?);
    }
    Ok(client)
}

fn extract_tokens(
    token: oauth2::StandardTokenResponse<
        oauth2::EmptyExtraTokenFields,
        oauth2::basic::BasicTokenType,
    >,
) -> (String, String, i64) {
    use oauth2::TokenResponse as _;
    let access = token.access_token().secret().clone();
    let refresh = token
        .refresh_token()
        .map(|r| r.secret().clone())
        .unwrap_or_default();
    let expires_in = token
        .expires_in()
        .map(|d| d.as_secs() as i64)
        .unwrap_or(3600);
    (access, refresh, expires_in)
}

fn generate_state() -> String {
    Alphanumeric.sample_string(&mut rand::thread_rng(), 32)
}

/// Accept the loopback OAuth redirect and pull out `?code=` + `?state=`.
async fn listen_for_code(
    listener: &tokio::net::TcpListener,
    expected_state: &str,
) -> anyhow::Result<String> {
    use tokio::io::AsyncReadExt;

    tokio::time::timeout(std::time::Duration::from_secs(300), async {
        let (mut socket, _addr) = listener
            .accept()
            .await
            .context("callback listener failed")?;
        let mut buf = [0u8; 8192];
        let n = socket
            .read(&mut buf)
            .await
            .context("callback could not be read")?;
        let text = String::from_utf8_lossy(&buf[..n]);

        let request_line = text.lines().next().unwrap_or_default();
        let path_and_query = request_line
            .split_whitespace()
            .nth(1)
            .unwrap_or_default();
        let parsed = url::Url::parse(&format!("http://127.0.0.1{path_and_query}"))
            .context("callback URL unparsable")?;
        let query: HashMap<String, String> = parsed.query_pairs().into_owned().collect();

        if let Some(error) = query.get("error") {
            let body = format!(
                "<html><body><h1>Sign-in failed</h1><p>{error}</p></body></html>"
            );
            let _ = write_response(&mut socket, &body).await;
            anyhow::bail!("spotify denied the login request: {error}");
        }

        match query.get("state") {
            Some(got) if got == expected_state => {}
            _ => {
                let body = "<html><body><h1>State mismatch</h1></body></html>";
                let _ = write_response(&mut socket, body).await;
                anyhow::bail!("OAuth state check failed (possible CSRF)");
            }
        }

        let code = query
            .get("code")
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("no authorization code in callback"))?;
        let body = "<html><body><h1>Spotify DX</h1><p>You can close this window.</p></body></html>";
        let _ = write_response(&mut socket, body).await;
        Ok(code)
    })
    .await?
}

async fn write_response(
    socket: &mut tokio::net::TcpStream,
    body: &str,
) -> tokio::io::Result<()> {
    use tokio::io::AsyncWriteExt;
    let response = format!(
        "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nContent-Type: text/html\r\nConnection: close\r\n\r\n{}",
        body.len(),
        body
    );
    socket.write_all(response.as_bytes()).await?;
    Ok(())
}