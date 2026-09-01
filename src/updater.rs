//! In-app update checking.
//!
//! Queries the latest GitHub release for Spotify DX the same way `install.sh`
//! does, then stages the new binary on desktop or hands the APK to the system
//! package installer on Android. The web build ships via the GH Pages app
//! itself and is not auto-updatable this way, so it is compiled out entirely
//! on `target_arch = "wasm32"`.
//!
//! Unlike kal (which uses blocking `ureq` + raw JNI), this port uses `reqwest`
//! (already the app's HTTP stack) and drives the Android package installer
//! through wry's safe `dispatch` + the `jni` crate — the crate is
//! `#![forbid(unsafe_code)]`, so `ndk_context`'s raw-pointer casts are not an
//! option here.

use dioxus::prelude::*;

#[cfg(target_os = "android")]
use std::sync::OnceLock;
#[cfg(target_os = "android")]
use jni::objects::JObject;
#[cfg(target_os = "android")]
use jni::JNIEnv;

/// Human-readable outcome of the last update check, shown in settings.
pub static UPDATE_STATUS: GlobalSignal<Option<String>> = Signal::global(|| None);
/// True once a newer release has been downloaded and is ready to apply.
pub static UPDATE_READY: GlobalSignal<bool> = Signal::global(|| false);

/// The version this binary reports for comparison against release tags.
///
/// Injected at build time by `build.rs` (from the nearest git tag, or the
/// `SPOTIFY_DX_RELEASE_VERSION` env override in CI), so it always matches the
/// release and never needs hand-editing. Falls back to the manifest version.
pub const CURRENT_VERSION: &str = env!("SPOTIFY_DX_VERSION");

/// GitHub repo owning the releases, as "owner/name".
#[cfg(not(target_arch = "wasm32"))]
const REPO: &str = "MihneaMoso/spotify-dx";

/// Asset name token matched against published asset names. The published
/// basenames embed the version (e.g. `spotify-dx-v0.1.8-x86_64-unknown-linux-gnu.tar.gz`)
/// and an unversioned alias is also uploaded, so we match by the stable
/// platform-tail substring that both share.
#[cfg(not(target_arch = "wasm32"))]
const LINUX_TOKEN: &str = "x86_64-unknown-linux-gnu.tar.gz";
#[cfg(not(target_arch = "wasm32"))]
const MACOS_ARM_TOKEN: &str = "aarch64-apple-darwin.tar.gz";
#[cfg(not(target_arch = "wasm32"))]
const MACOS_X86_TOKEN: &str = "x86_64-apple-darwin.tar.gz";
#[cfg(not(target_arch = "wasm32"))]
const WINDOWS_TOKEN: &str = "x86_64-pc-windows-msvc.zip";
#[cfg(not(target_arch = "wasm32"))]
const ANDROID_TOKEN: &str = "app-release-unsigned-signed.apk";

/// Filenames inside the local updates directory.
#[cfg(not(target_arch = "wasm32"))]
#[allow(dead_code)] // APK_NAME is consumed by the Android staging path only.
const APK_NAME: &str = "spotify-dx-update.apk";
#[cfg(all(not(target_os = "android"), not(target_arch = "wasm32")))]
#[allow(dead_code)] // used by the desktop staging path
const ARCHIVE_NAME: &str = "spotify-dx-update.tar.gz";
#[cfg(all(not(target_os = "android"), not(target_arch = "wasm32")))]
#[allow(dead_code)] // used by the desktop staging path
const BIN_STAGED: &str = "spotify-dx.new";

/// A discovered candidate release for the current platform.
#[derive(Debug, Clone)]
pub struct ReleaseInfo {
    pub version: String,
    #[allow(dead_code)] // consumed by the native fetch_update staging path
    pub asset_url: String,
    #[allow(dead_code)] // consumed by the native fetch_update staging path
    pub sha256: Option<String>,
}

/// Parse a leading `v`-optional, dot-separated numeric version into
/// comparable integers. Ignores non-numeric tail segments (`-rc.1` → `1`).
pub fn parse_version(v: &str) -> Vec<u64> {
    v.trim()
        .trim_start_matches('v')
        .split(['.', '-'])
        .filter_map(|p| p.parse::<u64>().ok())
        .collect()
}

/// True when `latest` is strictly newer than `current`.
pub fn is_newer(latest: &str, current: &str) -> bool {
    if latest == current {
        return false;
    }
    let a = parse_version(latest);
    let b = parse_version(current);
    let max = a.len().max(b.len());
    (0..max).any(|i| a.get(i).copied().unwrap_or(0) > b.get(i).copied().unwrap_or(0))
}

/// A newer release that has been downloaded and verified, ready to apply.
#[derive(Debug, Clone)]
pub struct ReadyUpdate {
    pub version: String,
}

/// Query the GitHub latest release and resolve the current platform's asset.
#[cfg(not(target_arch = "wasm32"))]
pub async fn latest_release() -> Result<ReleaseInfo, String> {
    let url = format!("https://api.github.com/repos/{REPO}/releases/latest");
    let resp = reqwest::Client::new()
        .get(&url)
        .header("Accept", "application/vnd.github+json")
        .header("User-Agent", "spotify-dx-updater")
        .send()
        .await
        .map_err(|e| format!("update check failed: {e}"))?;
    let body = resp
        .error_for_status()
        .map_err(|e| format!("update check failed: {e}"))?
        .text()
        .await
        .map_err(|e| format!("read failed: {e}"))?;
    pick_asset(&body, platform_token()?)
}

/// Find the asset matching `token` from a release JSON body.
#[cfg(not(target_arch = "wasm32"))]
fn pick_asset(body: &str, token: &str) -> Result<ReleaseInfo, String> {
    let v: serde_json::Value =
        serde_json::from_str(body).map_err(|e| format!("bad release payload: {e}"))?;
    let tag = v["tag_name"].as_str().unwrap_or("").to_string();
    let version = tag.trim_start_matches('v').to_string();
    if version.is_empty() {
        return Err("release has no version".into());
    }
    let assets = v["assets"].as_array().ok_or("release has no assets")?;
    let asset = assets
        .iter()
        .find(|a| {
            a["name"]
                .as_str()
                .map(|n| n.contains(token))
                .unwrap_or(false)
        })
        .ok_or_else(|| format!("no asset matching '{token}'"))?;
    let asset_url = asset["browser_download_url"]
        .as_str()
        .unwrap_or("")
        .to_string();
    if asset_url.is_empty() {
        return Err("asset has no download url".into());
    }
    let sha256 = asset["digest"]
        .as_str()
        .and_then(|d| d.strip_prefix("sha256:"))
        .map(|s| s.to_string());
    Ok(ReleaseInfo {
        version,
        asset_url,
        sha256,
    })
}

#[cfg(not(target_arch = "wasm32"))]
fn platform_token() -> Result<&'static str, String> {
    let os = std::env::consts::OS;
    let arch = std::env::consts::ARCH;
    match (os, arch) {
        ("linux", "x86_64") => Ok(LINUX_TOKEN),
        ("macos", "aarch64") => Ok(MACOS_ARM_TOKEN),
        ("macos", "x86_64") => Ok(MACOS_X86_TOKEN),
        ("windows", "x86_64") => Ok(WINDOWS_TOKEN),
        ("android", _) => Ok(ANDROID_TOKEN),
        _ => Err(format!("unsupported platform for auto-update ({os}/{arch})")),
    }
}

/// Download `url` into `dest`, hashing as it streams, and verify the (optional)
/// SHA-256 digest. The archive is written through `{dest}.part` and renamed so
/// a failed/cancelled download never leaves a half-written file look staged.
#[cfg(not(target_arch = "wasm32"))]
async fn download_to(url: &str, dest: &std::path::Path, sha256: Option<&str>) -> Result<(), String> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    use tokio::io::BufWriter;

    let resp = reqwest::Client::new()
        .get(url)
        .header("User-Agent", "spotify-dx-updater")
        .send()
        .await
        .map_err(|e| format!("download failed: {e}"))?;
    let resp = resp.error_for_status().map_err(|e| e.to_string())?;

    if let Some(parent) = dest.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("mkdir failed: {e}"))?;
    }
    let part = dest.with_extension("part");
    let file = tokio::fs::File::create(&part)
        .await
        .map_err(|e| format!("create file failed: {e}"))?;
    let mut file = BufWriter::new(file);

    let mut hasher = sha2::Sha256::new();
    let mut stream = resp.bytes_stream();
    use futures::StreamExt;
    while let Some(chunk) = stream
        .next()
        .await
        .transpose()
        .map_err(|e| format!("read download failed: {e}"))?
    {
        hasher.update(&chunk);
        file.write_all(&chunk)
            .await
            .map_err(|e| format!("write failed: {e}"))?;
    }
    file.flush().await.map_err(|e| format!("flush failed: {e}"))?;
    drop(file);

    if let Some(hex) = sha256 {
        let actual = finalize_hex(hasher);
        if !actual.eq_ignore_ascii_case(hex) {
            let _ = std::fs::remove_file(&part);
            return Err(format!("SHA-256 mismatch (expected {hex}, got {actual})"));
        }
    }

    std::fs::rename(&part, dest).map_err(|e| format!("finalize failed: {e}"))?;
    Ok(())
}

#[cfg(not(target_arch = "wasm32"))]
fn finalize_hex(hasher: sha2::Sha256) -> String {
    use sha2::Digest;
    hasher
        .finalize()
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

/// Download the release's platform asset into the local updates dir and report
/// the transitioned status. On desktop this stages a swap-on-next-launch; on
/// Android it just downloads the APK (the user then confirms the system
/// PackageInstaller prompt).
#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_update(info: &ReleaseInfo) -> Result<ReadyUpdate, String> {
    #[cfg(target_os = "android")]
    {
        let Some(dir) = android_files_dir().await else {
            return Err("no app files dir".into());
        };
        let dir = dir.join("updates");
        download_to(&info.asset_url, &dir.join(APK_NAME), info.sha256.as_deref())
            .await?;
        Ok(ReadyUpdate {
            version: info.version.clone(),
        })
    }

    #[cfg(not(target_os = "android"))]
    {
        let Some(dir) = updates_dir() else {
            return Err("no writable data dir".into());
        };
        let archive = dir.join(ARCHIVE_NAME);
        download_to(&info.asset_url, &archive, info.sha256.as_deref()).await?;
        stage_desktop_binary(&dir, &archive)?;
        Ok(ReadyUpdate {
            version: info.version.clone(),
        })
    }
}

// ---------------------------------------------------------------------------
// Update staging directory
// ---------------------------------------------------------------------------

/// Durable directory that holds the staged update payload.
#[cfg(not(target_arch = "wasm32"))]
fn updates_dir() -> Option<std::path::PathBuf> {
    #[cfg(target_os = "android")]
    {
        cached_files_dir().map(|p| p.join("updates"))
    }
    #[cfg(not(target_os = "android"))]
    {
        Some(crate::util::data_dir().join("updates"))
    }
}

/// App-private files directory, resolved through wry's safe `dispatch` (which
/// hands us a live `JNIEnv` + activity on the Android main thread) and cached
/// for the lifetime of the process. Returns `None` until the async resolution
/// has completed (see [`fetch_update`]).
#[cfg(target_os = "android")]
static FILES_DIR: OnceLock<Option<std::path::PathBuf>> = OnceLock::new();

#[cfg(target_os = "android")]
fn cached_files_dir() -> Option<std::path::PathBuf> {
    FILES_DIR.get().and_then(|p| p.clone())
}

#[cfg(target_os = "android")]
async fn android_files_dir() -> Option<std::path::PathBuf> {
    if let Some(p) = cached_files_dir() {
        return Some(p);
    }
    let (tx, rx) = tokio::sync::oneshot::channel();
    wry::prelude::dispatch(move |env, activity, _webview| {
        let path = resolve_files_dir(env, activity).ok();
        let _ = tx.send(path);
    });
    if let Ok(Some(p)) = rx.await {
        let _ = FILES_DIR.set(Some(p.clone()));
        Some(p)
    } else {
        None
    }
}

/// Resolve the `Context.getFilesDir()` path synchronously given the safe JNI
/// context wry `dispatch` provides.
#[cfg(target_os = "android")]
fn resolve_files_dir(env: &mut JNIEnv, activity: &JObject) -> jni::errors::Result<std::path::PathBuf> {
    let file = env
        .call_method(activity, "getFilesDir", "()Ljava/io/File;", &[])?
        .l()?;
    let abs = env
        .call_method(&file, "getAbsolutePath", "()Ljava/lang/String;", &[])?
        .l()?;
    let jstring = jni::objects::JString::from(abs);
    let s = env.get_string(&jstring)?;
    Ok(std::path::PathBuf::from(s.to_string_lossy().into_owned()))
}

// ---------------------------------------------------------------------------
// Desktop: stage + restart-to-apply
// ---------------------------------------------------------------------------

/// Extract the single root binary from the gzip'd tarball to `dir/spotify-dx.new`
/// and write a marker naming the executable to swap on the next launch.
#[cfg(all(not(target_os = "android"), not(target_arch = "wasm32")))]
fn stage_desktop_binary(dir: &std::path::Path, archive: &std::path::Path) -> Result<(), String> {
    use std::io::Read;

    let gz = std::fs::File::open(archive).map_err(|e| format!("open archive: {e}"))?;
    let tar = flate2::read::GzDecoder::new(gz);
    let mut ar = tar::Archive::new(tar);
    let staged = dir.join(BIN_STAGED);
    let mut found = false;
    for entry in ar.entries().map_err(|e| format!("read archive: {e}"))? {
        let mut entry = entry.map_err(|e| format!("entry: {e}"))?;
        if !entry
            .path()
            .map(|p| p.components().count() == 1)
            .unwrap_or(false)
        {
            continue;
        }
        let mut buf = Vec::new();
        entry
            .read_to_end(&mut buf)
            .map_err(|e| format!("read entry: {e}"))?;
        std::fs::write(&staged, &buf).map_err(|e| format!("write staged: {e}"))?;
        found = true;
        break;
    }
    if !found {
        return Err("archive contained no binary".into());
    }
    let target = std::env::current_exe().map_err(|e| format!("current exe: {e}"))?;
    std::fs::write(dir.join("swap.marker"), target.to_string_lossy().as_bytes())
        .map_err(|e| format!("write marker: {e}"))?;
    let _ = std::fs::remove_file(archive);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let _ = std::fs::set_permissions(&staged, std::fs::Permissions::from_mode(0o755));
    }
    Ok(())
}

/// Swap the running executable for the staged one and relaunch it, then exit
/// this process. Returns false (without exiting) if nothing is staged.
#[cfg(all(not(target_os = "android"), not(target_arch = "wasm32")))]
pub fn apply_staged_update() -> bool {
    let Some(dir) = updates_dir() else {
        return false;
    };
    let staged = dir.join(BIN_STAGED);
    let marker = dir.join("swap.marker");
    let Ok(raw) = std::fs::read_to_string(&marker) else {
        return false;
    };
    let target = std::path::PathBuf::from(raw.trim().to_string());
    if !staged.exists() || !target.exists() {
        let _ = std::fs::remove_file(&marker);
        return false;
    }

    // Windows cannot overwrite a running .exe but may rename it into place
    // after the canonical name is freed; Unix may overwrite in place.
    #[cfg(windows)]
    {
        let old = target.with_extension("old~");
        let _ = std::fs::remove_file(&old);
        if std::fs::rename(&target, &old).is_err() {
            return false;
        }
        if std::fs::copy(&staged, &target).is_err() {
            return false;
        }
    }
    #[cfg(not(windows))]
    {
        if std::fs::copy(&staged, &target).is_err() {
            return false;
        }
    }
    let _ = std::fs::remove_file(&marker);
    let _ = std::fs::remove_file(&staged);

    // Relaunch the newly swapped binary, then exit the old process.
    if std::process::Command::new(&target).spawn().is_ok() {
        std::process::exit(0);
    }
    false
}

// ---------------------------------------------------------------------------
// Android: confirm the staged APK through the system package installer.
// ---------------------------------------------------------------------------

/// Build and fire the ACTION_VIEW install intent for the staged APK using a
/// FileProvider-backed content URI (wired up in `SpotifyDxUpdater.kt`/manifest
/// by `scripts/stage-updater.sh`). Runs through wry's safe `dispatch`, so no
/// `unsafe` is needed to reach the Android context. Returns Ok once the intent
/// is handed to the system.
#[cfg(target_os = "android")]
pub fn request_android_install() -> Result<(), String> {
    let Some(dir) = updates_dir() else {
        return Err("no app files dir".into());
    };
    let apk = dir.join(APK_NAME);
    if !apk.exists() {
        return Err("no staged apk".into());
    }
    let path = apk.to_string_lossy().into_owned();
    wry::prelude::dispatch(move |env, activity, _webview| {
        if let Err(err) = fire_install_intent(env, activity, &path) {
            tracing::warn!("updater: could not launch package installer: {err}");
        }
    });
    Ok(())
}

/// `SpotifyDxUpdater.installApk(Context, String): V` — fires the system
/// package-installer intent for the staged APK.
#[cfg(target_os = "android")]
fn fire_install_intent(
    env: &mut JNIEnv,
    activity: &JObject,
    path: &str,
) -> jni::errors::Result<()> {
    let class = env.find_class("com/spotifydx/app/SpotifyDxUpdater")?;
    let jpath = env.new_string(path)?;
    env.call_static_method(
        class,
        "installApk",
        "(Landroid/content/Context;Ljava/lang/String;)V",
        &[
            jni::objects::JValue::Object(activity),
            jni::objects::JValue::Object(&jpath),
        ],
    )?;
    Ok(())
}

// ---------------------------------------------------------------------------
// Web stubs (compiled out).
// ---------------------------------------------------------------------------

#[cfg(target_arch = "wasm32")]
pub async fn latest_release() -> Result<ReleaseInfo, String> {
    Err("auto-update is not applicable on web".into())
}

#[cfg(target_arch = "wasm32")]
pub async fn fetch_update(_info: &ReleaseInfo) -> Result<ReadyUpdate, String> {
    Err("auto-update is not applicable on web".into())
}

#[cfg(target_arch = "wasm32")]
pub fn apply_staged_update() -> bool {
    false
}

#[cfg(target_arch = "wasm32")]
#[allow(dead_code)]
pub fn request_android_install() -> Result<(), String> {
    Err("not applicable".into())
}

// ---------------------------------------------------------------------------
// UI wiring
// ---------------------------------------------------------------------------

/// Run a background update check that downloads the latest release when a
/// newer version exists, updating the shared status globals. Safe to call from
/// any platform; on web it reports "not applicable".
pub fn run_check() {
    spawn(async move {
        let outcome: Result<String, String> = async {
            let latest = latest_release().await?;
            if is_newer(&latest.version, CURRENT_VERSION) {
                let ready = fetch_update(&latest).await?;
                *UPDATE_READY.write() = true;
                Ok(format!(
                    "Update ready: v{} — apply to restart",
                    ready.version
                ))
            } else {
                Ok(format!("You're up to date (v{CURRENT_VERSION})"))
            }
        }
        .await;
        *UPDATE_STATUS.write() = Some(match outcome {
            Ok(msg) => msg,
            Err(e) => format!("Update check failed: {e}"),
        });
    });
}

/// User tapped "apply/install now": on desktop swap+relaunch, on Android fire
/// the PackageInstaller intent.
pub fn apply_now() -> bool {
    // Android completes through the system installer (this returns after the
    // intent is handed off); desktop does an in-process swap + relaunch.
    #[cfg(target_os = "android")]
    {
        match request_android_install() {
            Ok(()) => {
                *UPDATE_STATUS.write() =
                    Some("Package installer opened — confirm to finish".into());
                true
            }
            Err(e) => {
                *UPDATE_STATUS.write() = Some(format!("Install failed: {e}"));
                false
            }
        }
    }
    #[cfg(not(target_os = "android"))]
    {
        if apply_staged_update() {
            true // process exits here on success
        } else {
            *UPDATE_STATUS.write() = Some("No staged update to apply".into());
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn version_ordering() {
        assert_eq!(parse_version("0.1.7"), vec![0, 1, 7]);
        assert_eq!(parse_version("v1.2.3"), vec![1, 2, 3]);
        assert_eq!(parse_version("1.10.0"), vec![1, 10, 0]);
        assert!(is_newer("0.1.8", "0.1.7"));
        assert!(is_newer("1.0.0", "0.1.7"));
        assert!(!is_newer("0.1.7", "0.1.7"));
        assert!(!is_newer("0.1.6", "0.1.7"));
    }

    #[test]
    fn embedded_version_is_real() {
        // CURRENT_VERSION comes from build.rs (git tag / SPOTIFY_DX_RELEASE_VERSION).
        // It must be non-empty and parseable, otherwise the updater can never
        // detect anything (never mind the stale-manual-constant case).
        assert!(!CURRENT_VERSION.is_empty());
        assert!(!CURRENT_VERSION.contains('v'));
        assert!(!parse_version(CURRENT_VERSION).is_empty());
        // Where git is available, the embedded version must match the nearest
        // tag (build.rs's primary source) rather than the "0.1.0" fallback.
        if let Ok(out) = std::process::Command::new("git")
            .args(["describe", "--tags", "--abbrev=0"])
            .output()
        {
            if out.status.success() {
                let tag = String::from_utf8_lossy(&out.stdout).trim().to_string();
                if !tag.is_empty() {
                    assert_eq!(CURRENT_VERSION, tag.trim_start_matches('v'));
                }
            }
        }
    }

    #[test]
    fn pick_asset_matches_platform() {
        let body = r#"{
            "tag_name": "v0.1.8",
            "assets": [
                {"name": "spotify-dx-v0.1.8-x86_64-unknown-linux-gnu.tar.gz",
                 "browser_download_url": "https://example/linux.tar.gz",
                 "digest": "sha256:deadbeef"},
                {"name": "app-release-unsigned-signed.apk",
                 "browser_download_url": "https://example/android.apk",
                 "digest": "sha256:f00d"},
                {"name": "spotify-dx-v0.1.8-x86_64-pc-windows-msvc.zip",
                 "browser_download_url": "https://example/win.zip",
                 "digest": "sha256:beef"}
            ]
        }"#;

        let linux = pick_asset(body, LINUX_TOKEN).unwrap();
        assert_eq!(linux.version, "0.1.8");
        assert_eq!(linux.asset_url, "https://example/linux.tar.gz");
        assert_eq!(linux.sha256.as_deref(), Some("deadbeef"));

        let android = pick_asset(body, ANDROID_TOKEN).unwrap();
        assert_eq!(android.asset_url, "https://example/android.apk");

        let win = pick_asset(body, WINDOWS_TOKEN).unwrap();
        assert_eq!(win.asset_url, "https://example/win.zip");

        assert!(pick_asset(body, "no-such-token").is_err());
    }

    #[test]
    fn parse_generic_asset_prefix() {
        // The matcher uses substring containment, so both the version-embedded
        // published names and the unversioned aliases resolve.
        assert!("spotify-dx-v0.1.8-x86_64-unknown-linux-gnu.tar.gz".contains(LINUX_TOKEN));
        assert!("spotify-dx-x86_64-unknown-linux-gnu.tar.gz".contains(LINUX_TOKEN));
        assert!("spotify-dx-v0.1.8-aarch64-apple-darwin.tar.gz".contains(MACOS_ARM_TOKEN));
        assert!("spotify-dx-x86_64-apple-darwin.tar.gz".contains(MACOS_X86_TOKEN));
        assert!("app-release-unsigned-signed.apk".contains(ANDROID_TOKEN));
        assert!("spotify-dx-x86_64-pc-windows-msvc.zip".contains(WINDOWS_TOKEN));
    }
}