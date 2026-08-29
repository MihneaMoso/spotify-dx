//! Persistent user settings (theme, volume, playback engine, cosmetic toggle).
//!
//! Stored as human-readable JSON under `{data_dir}/settings.json` so users can
//! hand-edit them too. Everything here must stay cheap to load at bootstrap:
//! `main.rs::bootstrap()` reads this file before the window exists to decide
//! the theme attribute and playback defaults.

use serde::{Deserialize, Serialize};

/// Canonical storage key for the settings blob. Same value lands in
/// `{data_dir}/settings.json` on native and a `localStorage` key on wasm.
const SETTINGS_KEY: &str = "settings.json";

/// Theme selection. Must stay in sync with `[data-theme="…"]` blocks in
/// `assets/main.css`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum ThemeName {
    /// Default dark/deep-blue palette.
    #[default]
    DeepBlue,
    /// Near-black alternative.
    Onyx,
}

impl ThemeName {
    /// The value set as `data-theme` on `<html>`.
    pub fn attr_value(self) -> &'static str {
        match self {
            ThemeName::DeepBlue => "deep-blue",
            ThemeName::Onyx => "onyx",
        }
    }
}

/// Which playback engine should drive playback when both are available
/// (`SYSTEM_DESIGN.md` §6.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum EnginePreference {
    /// SDK when the account/device allows it, open engine otherwise.
    #[default]
    Auto,
    /// Force the Web Playback SDK (Premium accounts only).
    SpotifySdk,
    /// Force the open multi-source engine.
    Open,
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    pub theme: ThemeName,
    /// Master volume, 0.0–1.0 (clamped on load).
    pub volume: f32,
    pub engine: EnginePreference,
    /// Inject element-hiding CSS into the login/session WebView to suppress
    /// premium upgrade buttons, HPTO banners, and sponsored items.
    /// Disabled by default (ToS-sensitive; see RESEARCH §3.3).
    pub hide_upsell: bool,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            theme: ThemeName::DeepBlue,
            volume: 0.8,
            engine: EnginePreference::Auto,
            hide_upsell: false,
        }
    }
}

impl Settings {
    /// Canonical native location: a durable-state file, not a cache entry.
    #[cfg(not(target_arch = "wasm32"))]
    pub fn path() -> std::path::PathBuf {
        crate::util::data_dir().join(SETTINGS_KEY)
    }

    /// Load persisted settings. ANY failure (missing, bad JSON, unknown fields)
    /// falls back to defaults — settings must never block boot.
    pub fn load() -> Self {
        #[cfg(not(target_arch = "wasm32"))]
        {
            Self::load_from(&Self::path()).unwrap_or_default()
        }
        #[cfg(target_arch = "wasm32")]
        {
            crate::platform::storage::get_bytes(SETTINGS_KEY)
                .and_then(|raw| String::from_utf8(raw).ok())
                .and_then(|raw| serde_json::from_str::<Self>(&raw).ok())
                .map(|mut parsed| {
                    parsed.normalize();
                    parsed
                })
                .unwrap_or_default()
        }
    }

    /// Load from a filesystem path (native tests/tools only).
    #[cfg(not(target_arch = "wasm32"))]
    pub fn load_from(path: &std::path::Path) -> Option<Self> {
        let raw = std::fs::read_to_string(path).ok()?;
        let mut parsed: Self = serde_json::from_str(&raw).ok()?;
        parsed.normalize();
        Some(parsed)
    }

    /// Persist settings. Best-effort by callers; returns the error for tests.
    pub fn save(&self) -> std::io::Result<()> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.save_to(&Self::path())
        }
        #[cfg(target_arch = "wasm32")]
        {
            let json = serde_json::to_string(self)
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
            crate::platform::storage::set_bytes(SETTINGS_KEY, json.as_bytes());
            Ok(())
        }
    }

    #[cfg(not(target_arch = "wasm32"))]
    pub fn save_to(&self, path: &std::path::Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let json = serde_json::to_string_pretty(self)
            .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))?;
        std::fs::write(path, json)
    }

    /// Clamp/repair values that could have been hand-edited into nonsense.
    pub fn normalize(&mut self) {
        if !self.volume.is_finite() {
            self.volume = Settings::default().volume;
        }
        self.volume = self.volume.clamp(0.0, 1.0);
    }
}

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "spotify-dx-settings-{tag}-{}.json",
            std::process::id()
        ))
    }

    #[test]
    fn roundtrips_through_disk() {
        let path = temp_path("roundtrip");
        let mut s = Settings::default();
        s.theme = ThemeName::Onyx;
        s.volume = 0.42;
        s.engine = EnginePreference::Open;
        s.save_to(&path).expect("save");

        let loaded = Settings::load_from(&path).expect("load");
        assert_eq!(loaded, s);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn missing_file_means_defaults() {
        let path = temp_path("missing");
        assert_eq!(Settings::load_from(&path), None);
        assert_eq!(Settings::default(), Settings::load_from(&path).unwrap_or_default());
    }

    #[test]
    fn garbage_file_means_defaults() {
        let path = temp_path("garbage");
        std::fs::write(&path, "{ not json").unwrap();
        assert_eq!(Settings::load_from(&path), None);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn volume_is_clamped_on_load() {
        let path = temp_path("clamp");
        std::fs::write(&path, r#"{ "theme": "onyx", "volume": 7.5 }"#).unwrap();
        let s = Settings::load_from(&path).expect("parses");
        assert_eq!(s.theme, ThemeName::Onyx);
        assert!((s.volume - 1.0).abs() < f32::EPSILON);
        std::fs::remove_file(&path).ok();
    }

    #[test]
    fn theme_attr_values_match_css() {
        assert_eq!(ThemeName::DeepBlue.attr_value(), "deep-blue");
        assert_eq!(ThemeName::Onyx.attr_value(), "onyx");
    }
}
