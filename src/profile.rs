//! Local user profile: username + optional avatar picture.
//!
//! Persisted as JSON under `{data_dir}/profile.json` (native) / a
//! `localStorage` key (wasm) — the same seam `settings` uses, so nothing new is
//! required on any platform. Avatar bytes are stored base64 with their MIME
//! type; the settings screen and top-bar chip render them as a data URI.

use dioxus::prelude::*;
use serde::{Deserialize, Serialize};

/// Fallback name shown when no username has been set.
pub const DEFAULT_USERNAME: &str = "You";

/// Size cap for avatar uploads (stored base64 ~1.33x the raw size).
const MAX_AVATAR_BYTES: usize = 4 * 1024 * 1024;

/// Canonical storage key for the profile blob.
const PROFILE_KEY: &str = "profile.json";

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct UserProfile {
    pub username: String,
    /// MIME type of the uploaded image, e.g. "image/png".
    pub avatar_mime: Option<String>,
    /// Base64-encoded avatar bytes.
    pub avatar_b64: Option<String>,
}

impl UserProfile {
    pub fn display_name(&self) -> String {
        let name = self.username.trim();
        if name.is_empty() {
            DEFAULT_USERNAME.to_string()
        } else {
            name.to_string()
        }
    }

    /// Data-URI suitable for an `<img src>` attribute, or None.
    pub fn avatar_data_uri(&self) -> Option<String> {
        let mime = self.avatar_mime.as_deref().unwrap_or("image/png");
        let data = self.avatar_b64.as_deref()?;
        Some(format!("data:{mime};base64,{data}"))
    }

    pub fn initials(&self) -> String {
        let name = self.display_name();
        match name.split_whitespace().collect::<Vec<_>>().as_slice() {
            [first, rest @ ..] => {
                let mut s = String::new();
                if let Some(c) = first.chars().next() {
                    s.push(c);
                }
                if let Some(last) = rest.last().and_then(|w| w.chars().next()) {
                    s.push(last);
                }
                s.to_uppercase()
            }
            [] => "?".into(),
        }
    }

    pub fn has_avatar(&self) -> bool {
        self.avatar_b64.as_deref().is_some_and(|s| !s.is_empty())
    }
}

/// The persisted profile, read at startup. Mutated from the settings screen;
/// `save` persists on change.
pub static PROFILE: GlobalSignal<UserProfile> = Signal::global(UserProfile::default);

/// Error message shown inside the settings screen's profile section.
pub static PROFILE_ERROR: GlobalSignal<String> = Signal::global(String::new);

/// Seed [`PROFILE`] from disk. Called once at `bootstrap`.
pub fn init() {
    *PROFILE.write() = load();
}

/// Load persisted profile. ANY failure (missing, bad JSON, unknown fields)
/// falls back to defaults — the profile must never block boot.
pub fn load() -> UserProfile {
    #[cfg(not(target_arch = "wasm32"))]
    {
        load_from(&path()).unwrap_or_default()
    }
    #[cfg(target_arch = "wasm32")]
    {
        crate::platform::storage::get_bytes(PROFILE_KEY)
            .and_then(|raw| String::from_utf8(raw).ok())
            .and_then(|raw| serde_json::from_str::<UserProfile>(&raw).ok())
            .unwrap_or_default()
    }
}

/// Canonical native location for the profile blob.
#[cfg(not(target_arch = "wasm32"))]
pub fn path() -> std::path::PathBuf {
    crate::util::data_dir().join(PROFILE_KEY)
}

/// Load from a filesystem path (native tests/tools only).
#[cfg(not(target_arch = "wasm32"))]
pub fn load_from(path: &std::path::Path) -> Option<UserProfile> {
    let raw = std::fs::read_to_string(path).ok()?;
    serde_json::from_str(&raw).ok()
}

/// Persist [`PROFILE`]. Best-effort; the UI shows a toast on failure.
pub fn save() {
    let profile = PROFILE.peek().clone();
    if let Err(err) = persist(&profile) {
        crate::state::publish_error(crate::app_error::AppError::Other(anyhow::anyhow!(
            "could not save profile: {err}"
        )));
    }
}

fn persist(profile: &UserProfile) -> std::io::Result<()> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        persist_to(profile, &path())
    }
    #[cfg(target_arch = "wasm32")]
    {
        let json = serde_json::to_string(profile)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        crate::platform::storage::set_bytes(PROFILE_KEY, json.as_bytes());
        Ok(())
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn persist_to(profile: &UserProfile, path: &std::path::Path) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(profile)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
    std::fs::write(path, json)
}

/// Encode uploaded file bytes as the profile's avatar. Rejects anything over
/// the size cap or that doesn't look like an image by magic bytes.
pub fn set_avatar(profile: &mut UserProfile, mime: Option<String>, bytes: &[u8]) -> Result<(), String> {
    if bytes.len() > MAX_AVATAR_BYTES {
        return Err("Image too large (max 4 MB)".into());
    }
    if !looks_like_image(bytes) {
        return Err("Selected file is not an image".into());
    }
    profile.avatar_mime = Some(mime.unwrap_or_else(|| "image/png".into()));
    profile.avatar_b64 = Some(base64_encode(bytes));
    Ok(())
}

pub fn clear_avatar(profile: &mut UserProfile) {
    profile.avatar_mime = None;
    profile.avatar_b64 = None;
}

fn looks_like_image(bytes: &[u8]) -> bool {
    matches!(
        &bytes[..bytes.len().min(12)],
        [0x89, b'P', b'N', b'G', ..] | [0xFF, 0xD8, 0xFF, ..] | b"GIF8" | b"RIFF" // WEBP
    )
}

fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn initials_and_display_name() {
        let mut p = UserProfile::default();
        assert_eq!(&p.display_name(), DEFAULT_USERNAME);
        assert_eq!(&p.initials(), "Y");
        p.username = "Ada Lovelace".into();
        assert_eq!(&p.display_name(), "Ada Lovelace");
        assert_eq!(&p.initials(), "AL");
    }

    #[test]
    fn avatar_round_trip_and_rejects_non_images() {
        let mut p = UserProfile::default();
        assert!(set_avatar(&mut p, Some("image/png".into()), b"not an image").is_err());
        let png = [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
        set_avatar(&mut p, Some("image/png".into()), &png).unwrap();
        assert!(p.has_avatar());
        let uri = p.avatar_data_uri().unwrap();
        assert!(uri.starts_with("data:image/png;base64,"));
        clear_avatar(&mut p);
        assert!(!p.has_avatar());
        assert!(p.avatar_data_uri().is_none());
    }

    #[test]
    fn avatar_over_size_cap_rejected() {
        let mut p = UserProfile::default();
        let mut big =
            [0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a].repeat(MAX_AVATAR_BYTES / 8 + 1);
        big.push(0);
        assert!(set_avatar(&mut p, None, &big).is_err());
    }
}