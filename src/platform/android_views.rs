//! Android-only view-layering manager for the in-window WebViews.
//!
//! wry 0.53.5's Android backend supports exactly ONE visible WebView per
//! activity: every `WebViewBuilder::build(&window)` calls
//! `Activity.setContentView(webview)`, silently detaching whatever was on
//! screen, and `set_visible`/`set_bounds` are documented no-ops (see
//! `wry::android`). That breaks the desktop "stack several WebViews in the
//! window" model: the sign-in / hidden-SDK WebViews *replace* the dioxus UI
//! instead of overlaying it, and once the sign-in page is parked at
//! `about:blank` the app is stuck on a full-screen white page that swallows
//! every touch.
//!
//! This module restores the window so the dioxus UI stays at the bottom
//! permanently and every extra WebView is a normal Android View layered above
//! it:
//!
//! * [`capture_base`] grabs the view that exists before the first of our
//!   WebViews is created (the dioxus UI) and keeps a JNI global reference to it.
//! * [`install_overlay`] (invoked from each WebView's `on_webview_created`
//!   hook, which runs *after* wry's `setContentView`) re-attaches the dioxus UI
//!   as the activity content and adds the just-created WebView as a full-screen
//!   `FrameLayout` child on top of it.
//! * [`set_visible`] / [`remove`] then simply hide or detach an overlay the
//!   normal Android way — no `set_visible`/`set_bounds` no-ops involved.
//!
//! All of this happens through wry's `dispatch` / `on_webview_created`
//! callbacks and the safe `jni`-crate API, so no `unsafe` is introduced (the
//! crate is `#![forbid(unsafe_code)]`).

use std::sync::OnceLock;

use jni::errors::{self, Error};
use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

/// `android.R.id.content` — the frame `Activity.setContentView` fills. Used via
/// `findViewById` because the decor view is not the content parent, and its id
/// is stable across `setContentView` calls.
const CONTENT_FRAME_ID: i32 = 0x0102_0002;

/// `View.VISIBLE` / `View.GONE` (keeping the view in the layout but skipping
/// draw/touch is all we need; `GONE` also releases its layout slot).
const VIEW_VISIBLE: i32 = 0;
const VIEW_GONE: i32 = 8;

/// The view that existed before any of our WebViews: the dioxus UI. Captured
/// once (the first WebView build wins), reused on every overlay install.
static BASE_VIEW: OnceLock<GlobalRef> = OnceLock::new();

/// Locate the window content frame (`android.R.id.content`) inside `activity`.
/// `call_method` is flexible about reference lifetimes, but the returned
/// `JObject` borrows the frame's own JNI local scope, so callers must not try
/// to thread it through helpers that need to unify outer lifetimes.
macro_rules! content_frame {
    ($env:expr, $activity:expr) => {
        $env.call_method(
            $activity,
            "findViewById",
            "(I)Landroid/view/View;",
            &[JValue::Int(CONTENT_FRAME_ID)],
        )?
        .l()?
    };
}

/// Remember the dioxus UI view. Must run BEFORE the first of our WebViews is
/// built (see [`install_overlay`]). Idempotent; safe to call repeatedly.
///
/// Runs through wry's `dispatch`, so it executes on the activity's UI thread
/// and in program order with the `CreateWebView` messages that follow it.
pub fn capture_base() {
    wry::prelude::dispatch(|env, activity, webview| {
        if BASE_VIEW.get().is_some() {
            return;
        }
        let result = (|| -> errors::Result<()> {
            // The dioxus webview is the first child of the content frame when
            // nothing of ours has been created yet.
            let frame = content_frame!(env, activity);
            let child = env
                .call_method(&frame, "getChildAt", "(I)Landroid/view/View;", &[JValue::Int(0)])?
                .l()?;
            // Fall back to whatever wry passed as its current webview (the
            // dioxus UI) when the content frame is still empty.
            let base: &JObject = if child.as_raw().is_null() {
                webview
            } else {
                &child
            };
            let base = env.new_global_ref(base)?;
            let _ = BASE_VIEW.set(base);
            Ok(())
        })();
        if let Err(err) = result {
            tracing::warn!("android views: could not capture the app UI view: {err}");
        }
    });
}

/// Re-attach the dioxus UI as the activity content and layer `webview` above
/// it, full-screen. Returns a global reference to `webview` the caller should
/// keep so the overlay stays addressable (hide / remove).
///
/// Called from a WebView's `on_webview_created` hook — i.e. *after* wry has
/// already `setContentView`-ed the new WebView over the dioxus UI. The
/// `Context` that wry hands to that hook shares one JNI frame lifetime, so all
/// three parameters unify.
pub fn install_overlay<'local>(
    env: &mut JNIEnv<'local>,
    activity: &JObject<'local>,
    webview: &JObject<'local>,
) -> errors::Result<GlobalRef> {
    let base = BASE_VIEW
        .get()
        .ok_or(Error::NullPtr(
            "no base view captured — call capture_base() before building overlay webviews",
        ))?;

    // 1. Put the dioxus UI back as the content view.
    env.call_method(
        activity,
        "setContentView",
        "(Landroid/view/View;)V",
        &[JValue::Object(base.as_obj())],
    )?;

    // 2. Add our WebView on top of it. `addView(View)` uses the parent
    //    FrameLayout's default layout params (MATCH_PARENT × MATCH_PARENT), so
    //    no LayoutParams need to be constructed.
    let frame = content_frame!(env, activity);
    env.call_method(&frame, "addView", "(Landroid/view/View;)V", &[JValue::Object(webview)])?;

    env.new_global_ref(webview)
}

/// Show or hide an overlay synchronously. Must be called with a live `JNIEnv`
/// on the UI thread (i.e. inside a wry `on_webview_created` hook or dispatch).
pub fn set_visible_now(
    env: &mut JNIEnv,
    webview: &JObject,
    visible: bool,
) -> errors::Result<()> {
    env.call_method(
        webview,
        "setVisibility",
        "(I)V",
        &[JValue::Int(if visible { VIEW_VISIBLE } else { VIEW_GONE })],
    )?;
    Ok(())
}

/// Show or hide an overlay asynchronously (queued on the activity's UI thread).
pub fn set_visible(overlay: GlobalRef, visible: bool) {
    wry::prelude::dispatch(move |env, _activity, _webview| {
        if let Err(err) = set_visible_now(env, overlay.as_obj(), visible) {
            tracing::warn!("android views: set overlay visibility failed: {err}");
        }
    });
}

/// Detach an overlay from the view tree (and hide it) asynchronously.
pub fn remove(overlay: GlobalRef) {
    wry::prelude::dispatch(move |env, activity, _webview| {
        if let Err(err) = detach(env, activity, overlay.as_obj()) {
            tracing::warn!("android views: remove overlay failed: {err}");
        }
    });
}

/// Hide and detach an overlay synchronously.
fn detach(env: &mut JNIEnv, activity: &JObject, webview: &JObject) -> errors::Result<()> {
    let frame = content_frame!(env, activity);
    env.call_method(&frame, "removeView", "(Landroid/view/View;)V", &[JValue::Object(webview)])?;
    set_visible_now(env, webview, false)
}