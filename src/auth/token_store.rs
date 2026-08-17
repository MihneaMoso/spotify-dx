use keyring::Entry;

const SERVICE: &str = "spotify-dx";
const KEY_TOKEN: &str = "access_token";
const KEY_EXPIRY: &str = "expires_at_ms"; // decimal string

/// Persist the short-lived access token + expiry.
pub fn save(access_token: &str, expires_at_ms: u64) {
    let kr_token = Entry::new(SERVICE, KEY_TOKEN).unwrap();
    let kr_expiry = Entry::new(SERVICE, KEY_EXPIRY).unwrap();
    let _ = kr_token.set_password(access_token);
    let _ = kr_expiry.set_password(&expires_at_ms.to_string());
    // File fallback for headless desktops without a secret-service daemon:
    save_to_file(access_token, expires_at_ms);
}

/// Load the persisted token + expiry, tolerating missing / half-written state.
/// Tries the OS keychain first, then falls back to the on-disk file (for
/// headless desktops without a secret-service daemon).
pub fn load() -> Option<(String, u64)> {
    let keychain = (|| {
        let token = Entry::new(SERVICE, KEY_TOKEN).ok()?.get_password().ok()?;
        let expiry = Entry::new(SERVICE, KEY_EXPIRY)
            .ok()?
            .get_password()
            .ok()?
            .parse::<u64>()
            .ok()?;
        Some((token, expiry))
    })();
    keychain.or_else(load_from_file)
}

/// Wipe every credential the app has written. Missing entries are tolerated.
pub fn clear() {
    for key in [KEY_TOKEN, KEY_EXPIRY] {
        if let Ok(e) = Entry::new(SERVICE, key) {
            let _ = e.delete_credential();
        }
    }
    let _ = std::fs::remove_file(fallback_path());
}

/// The on-disk fallback (keychain-free).
const FALLBACK_FILE: &str = "session.json";

fn fallback_path() -> std::path::PathBuf {
    crate::util::data_dir().join(FALLBACK_FILE)
}

fn save_to_file(access_token: &str, expires_at_ms: u64) {
    let path = fallback_path();
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let json = serde_json::json!({
        "access_token": access_token,
        "expires_at_ms": expires_at_ms,
    });
    let _ = std::fs::write(&path, serde_json::to_vec_pretty(&json).unwrap_or_default());
}

fn load_from_file() -> Option<(String, u64)> {
    let bytes = std::fs::read(fallback_path()).ok()?;
    let value: serde_json::Value = serde_json::from_slice(&bytes).ok()?;
    let token = value.get("access_token")?.as_str()?.to_owned();
    let expiry = value.get("expires_at_ms")?.as_u64()?;
    Some((token, expiry))
}
