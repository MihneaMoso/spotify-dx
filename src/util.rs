#[cfg(not(target_arch = "wasm32"))]
use std::sync::OnceLock;

/// Host-provided base-directory overrides (Kotlin bridge `initCore` sets these
/// to the app's `filesDir`/`cacheDir`: `dirs` has no Android mapping, so
/// without an override durable state would land in a temp directory).
#[cfg(not(target_arch = "wasm32"))]
static DATA_DIR_OVERRIDE: OnceLock<std::path::PathBuf> = OnceLock::new();
#[cfg(not(target_arch = "wasm32"))]
static CACHE_DIR_OVERRIDE: OnceLock<std::path::PathBuf> = OnceLock::new();

/// Pin the base directories for this process. First call wins; later calls are
/// ignored so the bridge init cannot reroute live paths.
#[cfg(not(target_arch = "wasm32"))]
pub fn set_dir_overrides(data_dir: std::path::PathBuf, cache_dir: std::path::PathBuf) {
    let _ = DATA_DIR_OVERRIDE.set(data_dir);
    let _ = CACHE_DIR_OVERRIDE.set(cache_dir);
}

/// Base directory for runtime-cached data (`~/Library/Caches`, `$XDG_CACHE_HOME`,
/// `%LOCALAPPDATA%`, …). Native-only: filesystem routing is replaced by
/// `crate::platform::storage` (localStorage) on wasm.
#[cfg(not(target_arch = "wasm32"))]
pub fn cache_dir() -> std::path::PathBuf {
    if let Some(dir) = CACHE_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    dirs::cache_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}

/// Base directory for durable local state (tokens, preferences). Native-only.
#[cfg(not(target_arch = "wasm32"))]
pub fn data_dir() -> std::path::PathBuf {
    if let Some(dir) = DATA_DIR_OVERRIDE.get() {
        return dir.clone();
    }
    dirs::data_local_dir()
        .unwrap_or_else(std::env::temp_dir)
        .join("spotify-dx")
}

/// Bridge/query JSON arg parsers (shared: the Android bridge and any future
/// caller). Pure — unit-tested below. Errors are plain messages; the bridge
/// wraps them in its envelope (`INVALID_ARGS`).
pub fn arg_id(raw: &str) -> Result<String, String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("bad arg JSON: {e}"))?;
    let id = v.get("id").and_then(|i| i.as_str()).unwrap_or_default();
    if id.is_empty() {
        return Err("arg.id must be non-empty".into());
    }
    Ok(id.to_string())
}

pub fn arg_page(raw: &str) -> Result<(u32, u32), String> {
    let v: serde_json::Value =
        serde_json::from_str(raw).map_err(|e| format!("bad arg JSON: {e}"))?;
    let limit = v
        .get("limit")
        .and_then(|l| l.as_u64())
        .unwrap_or(20)
        .clamp(1, 50) as u32;
    // Reject, don't wrap: `as u32` on an absurd offset silently fetched a
    // small page with no error (the wrong data served as correct).
    let offset_raw = v.get("offset").and_then(|o| o.as_u64()).unwrap_or(0);
    let offset = u32::try_from(offset_raw)
        .map_err(|_| "arg.offset exceeds u32 range".to_string())?;
    Ok((limit, offset))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: absurd page offsets wrapped (`as u32`) into small pages
    /// and served the wrong data as correct. Now rejected.
    #[test]
    fn arg_page_rejects_absurd_offset() {
        assert!(arg_page(r#"{"limit": 20, "offset": 5000000000}"#).is_err());
        assert_eq!(arg_page(r#"{"limit": 20, "offset": 40}"#).unwrap(), (20, 40));
    }

    #[test]
    fn arg_page_defaults_and_clamps_limit() {
        assert_eq!(arg_page(r#"{}"#).unwrap(), (20, 0));
        assert_eq!(arg_page(r#"{"limit": 5000}"#).unwrap(), (50, 0));
    }

    #[test]
    fn arg_id_rejects_missing_and_empty() {
        assert!(arg_id(r#"{}"#).is_err());
        assert!(arg_id(r#"{"id": ""}"#).is_err());
        assert_eq!(arg_id(r#"{"id": "abc"}"#).unwrap(), "abc");
    }
}

/// Bridge event envelope (`{"kind":…,"payload":…}`). The kind goes through
/// serde, never raw interpolation — a future dynamic kind containing a
/// quote would otherwise break the envelope and the `pollEvents` parse.
/// Pure, unit-tested.
pub fn json_event(kind: &str, payload_json: &str) -> String {
    let kind = serde_json::Value::String(kind.to_string());
    format!(
        "{{\"kind\":{},\"payload\":{payload_json}}}",
        serde_json::to_string(&kind).unwrap_or_default()
    )
}

    /// Regression: raw kind interpolation is a latent envelope injection —
    /// any future dynamic kind breaks parsing. Serde escaping must hold.
    #[test]
    fn event_kind_is_json_escaped() {
        assert_eq!(
            json_event("session", "{\"authenticated\":true}"),
            "{\"kind\":\"session\",\"payload\":{\"authenticated\":true}}"
        );
        assert_eq!(
            json_event("we\"ird", "{}"),
            "{\"kind\":\"we\\\"ird\",\"payload\":{}}"
        );
}
