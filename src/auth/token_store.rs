use anyhow::Context as _;
use keyring::Entry;
use serde::{Deserialize, Serialize};

const SERVICE: &str = "spotify-dx";
const ACCESS_USER: &str = "access_token";
const REFRESH_USER: &str = "refresh_token";

/// Token pair as persisted. Also the on-disk fallback format.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct StoredTokens {
    pub access_token: Option<String>,
    pub refresh_token: Option<String>,
}

/// Storage: prefer the platform keychain (Secret Service / Keychain /
/// Credential Manager); fall back to a plaintext file under the app data dir
/// when no keychain is available. Prompts/daemons like gnome-keyring or kwallet
/// are commonly absent on headless/minimal desktops, so the configurable path is
/// what actually makes a session survive a restart there.
const FALLBACK_FILE: &str = "tokens.json";

fn entry_for(account: &str) -> anyhow::Result<Entry> {
    Entry::new(SERVICE, account).context("keyring entry could not be created")
}

/// Persist both tokens to the platform keychain (Keychain / Credential Manager /
/// Secret Service), falling back to a local file when no keychain exists.
pub fn save(access: &str, refresh: &str) -> anyhow::Result<()> {
    match set_token(ACCESS_USER, access).and_then(|()| set_token(REFRESH_USER, refresh)) {
        Ok(()) => Ok(()),
        Err(keyring_err) => {
            tracing::debug!("token_store: keychain unavailable ({keyring_err:#}); using file fallback");
            save_to_file(access, refresh)
        }
    }
}

fn set_token(account: &str, value: &str) -> anyhow::Result<()> {
    entry_for(account)?
        .set_password(value)
        .context(anyhow::anyhow!("failed to store token for {account}"))
}

/// Load a persisted token pair, tolerating a missing / half-written state.
pub fn load() -> Option<StoredTokens> {
    keychain_tokens().or_else(load_from_file)
}

fn keychain_tokens() -> Option<StoredTokens> {
    let access = get_token(ACCESS_USER);
    let refresh = get_token(REFRESH_USER);
    if access.is_none() && refresh.is_none() {
        return None;
    }
    Some(StoredTokens {
        access_token: access,
        refresh_token: refresh,
    })
}

fn get_token(account: &str) -> Option<String> {
    entry_for(account)
        .and_then(|entry| {
            entry
                .get_password()
                .map_err(|err| anyhow::anyhow!(err.to_string()))
        })
        .ok()
}

/// Wipe every credential the app has written. Missing entries are tolerated.
pub fn clear() -> anyhow::Result<()> {
    for account in [ACCESS_USER, REFRESH_USER] {
        if let Err(err) = delete_token(account) {
            tracing::warn!("token_store: could not delete {account}: {err:#}");
        }
    }
    if delete_file().is_err() {
        tracing::debug!("token_store: no fallback file to delete");
    }
    Ok(())
}

fn delete_token(account: &str) -> anyhow::Result<()> {
    entry_for(account)?
        .delete_credential()
        .map_err(|err| anyhow::anyhow!(err.to_string()))?;
    Ok(())
}

fn fallback_path() -> std::path::PathBuf {
    crate::util::data_dir().join(FALLBACK_FILE)
}

fn save_to_file(access: &str, refresh: &str) -> anyhow::Result<()> {
    let path = fallback_path();
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).context("fallback token directory could not be created")?;
    }
    let tokens = StoredTokens {
        access_token: Some(access.to_owned()),
        refresh_token: (!refresh.is_empty()).then(|| refresh.to_owned()),
    };
    let json = serde_json::to_vec_pretty(&tokens).context("tokens could not be serialized")?;
    std::fs::write(&path, json).context("fallback token file could not be written")?;
    tracing::info!("token_store: persisted tokens to {}", path.display());
    Ok(())
}

fn load_from_file() -> Option<StoredTokens> {
    let path = fallback_path();
    let Ok(bytes) = std::fs::read(&path) else {
        return None;
    };
    serde_json::from_slice::<StoredTokens>(&bytes).ok()
}

fn delete_file() -> anyhow::Result<()> {
    let path = fallback_path();
    if path.exists() {
        std::fs::remove_file(&path).context("fallback token file could not be deleted")?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Exercises the platform keychain only when one is available; CI boxes
    /// without a secret-service daemon simply skip the round-trip.
    #[test]
    fn round_trips_tokens() {
        if save("test-access", "test-refresh").is_err() {
            eprintln!("keychain unavailable on this machine; skipping round-trip");
            return;
        }
        let loaded = load().expect("tokens must be loadable after save");
        assert_eq!(loaded.access_token.as_deref(), Some("test-access"));
        assert_eq!(loaded.refresh_token.as_deref(), Some("test-refresh"));
        let _ = clear();
    }

    #[test]
    fn file_fallback_round_trips() {
        let path = fallback_path();
        let _ = std::fs::remove_file(&path);
        save_to_file("file-access", "file-refresh").expect("file write must succeed");
        let loaded = load_from_file().expect("file must be readable");
        assert_eq!(loaded.access_token.as_deref(), Some("file-access"));
        assert_eq!(loaded.refresh_token.as_deref(), Some("file-refresh"));
        let _ = delete_file();
    }
}