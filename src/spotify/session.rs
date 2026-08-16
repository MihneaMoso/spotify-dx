use crate::app_error::AppError;
use crate::auth;
use crate::auth::token_store;
use crate::state::{AUTH_STATE, AuthState};
use chrono::Utc;
use dioxus::signals::Readable;

/// Snapshot the current access token without subscribing callers.
pub fn current_token() -> Option<String> {
    AUTH_STATE.peek().access_token.clone()
}

/// True when we believe the current token is still valid.
pub fn has_valid_session() -> bool {
    let auth = AUTH_STATE.peek();
    matches!(
        (auth.access_token.as_deref(), auth.expires_at),
        (Some(_), Some(expiry)) if Utc::now() < expiry
    )
}

/// Make sure we hold a (reasonably) live access token, refreshing if needed.
///
/// Safe to call from UI-thread async tasks: it reads the signal only through
/// `peek` and writes only after the awaited network round-trip completes.
pub async fn ensure_token() -> Result<String, AppError> {
    let snapshot = AuthState {
        access_token: AUTH_STATE.peek().access_token.clone(),
        refresh_token: AUTH_STATE.peek().refresh_token.clone(),
        expires_at: AUTH_STATE.peek().expires_at,
        ..AuthState::default()
    };

    let still_valid = snapshot.access_token.as_deref().is_some() && snapshot.expires_at.map(|e| Utc::now() < e).unwrap_or(false);
    if still_valid {
        return snapshot.access_token.ok_or_else(|| AppError::Auth("no token".into()));
    }

    let refresh = snapshot
        .refresh_token
        .ok_or_else(|| AppError::Auth("no refresh token available".into()))?;
    refresh_and_store(&refresh).await
}

/// Exchange a refresh token for a new pair and update both keychain + state.
pub async fn refresh_and_store(refresh: &str) -> Result<String, AppError> {
    let (access, new_refresh, expires_in) = auth::refresh(refresh).await.map_err(|err| {
        AppError::Auth(format!("refresh failed: {err:#}"))
    })?;
    if !new_refresh.is_empty() {
        token_store::save(&access, &new_refresh).ok();
    }
    AUTH_STATE.write().access_token = Some(access.clone());
    AUTH_STATE.write().refresh_token = (!new_refresh.is_empty()).then_some(new_refresh);
    AUTH_STATE.write().expires_at = Some(Utc::now() + chrono::Duration::seconds(expires_in));
    Ok(access)
}