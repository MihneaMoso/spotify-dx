//! Android-only platform WebView for the Spotify sign-in/session page.
//!
//! Why this exists instead of a second wry WebView: wry 0.53.5's Android
//! backend keeps its custom-protocol/IPC/navigation handlers in process-global
//! locks, and EVERY `WebViewBuilder::build()` unconditionally replaces them
//! with that builder's own registrations. The dioxus base view serves
//! `https://dioxus.index.html/` assets + `//__events` through its `dioxus`
//! protocol; building our login overlay with wry wipes that handler
//! process-wide, so from that moment the base view's CSS and event XHRs go to
//! the real network and die (unstyled UI, dead taps, frozen Router). A
//! platform `android.webkit.WebView` created directly over JNI never touches
//! wry's globals, so the base view keeps working untouched underneath it.
//!
//! Session cookies interoperate through the process-wide
//! `android.webkit.CookieManager` singleton, and Rust<->page IPC goes over
//! `document.title`: `addJavascriptInterface` would need a Java class this
//! crate cannot define under `#![forbid(unsafe_code)]`. The page-side shim
//! (see `crate::auth::webview_login::NATIVE_IPC_SHIM`) collects outbound
//! messages in `window.__spotifyDxOutbox`; Rust ferries them out with an
//! `evaluateJavascript` copy into `document.title` + a synchronous
//! `getTitle()` read. The outbox variable is the source of truth, so a
//! clobbered ferry is simply retried on the next poll — no message loss.
//!
//! Every JNI call runs on the activity's UI thread through wry's `dispatch`.
//! Fire-and-forget posts (`load_url`, `eval_js`, `remove`) are synchronous
//! one-way sends; queries (`get_progress`, `get_title`) round-trip through a
//! oneshot and must only be `.await`ed from async driver tasks, never blocked
//! on from inside a dispatch callback.

use jni::objects::{GlobalRef, JObject, JValue};
use jni::JNIEnv;

/// `android.R.id.content` — the frame `Activity.setContentView` fills.
const CONTENT_FRAME_ID: i32 = 0x0102_0002;

/// `View.VISIBLE` / `View.GONE`.
const VIEW_VISIBLE: i32 = 0;
const VIEW_GONE: i32 = 8;

/// Run `f` on the activity's UI thread and await its result. Callers must be
/// async driver tasks (`.await` yields); never block on the returned future
/// from inside a dispatch callback.
pub async fn dispatch_call<T, F>(f: F) -> anyhow::Result<T>
where
    T: Send + 'static,
    F: FnOnce(&mut JNIEnv, &JObject, &JObject) -> anyhow::Result<T> + Send + 'static,
{
    let (tx, rx) = tokio::sync::oneshot::channel();
    wry::prelude::dispatch(move |env, activity, webview| {
        let result = f(env, activity, webview);
        // Never leave a Java exception pending: the next `FindClass` on this
        // thread would otherwise abort the process (SIGABRT). This fired
        // once from an app-class lookup via the boot loader — see below.
        clear_pending(env);
        let _ = tx.send(result);
    });
    rx.await
        .map_err(|_| anyhow::anyhow!("android webview dispatch dropped"))?
}

/// Clear a pending Java exception, if any. Best-effort hardening: an
/// uncleared exception makes the next `FindClass` on the thread fatal.
fn clear_pending(env: &JNIEnv) {
    let _ = env.exception_clear();
}

/// Load an app/framework class through the activity's own class loader.
/// Plain `find_class` uses the boot loader, which cannot see app classes
/// (`androidx.*`): the resulting `NoClassDefFoundError` stays pending and
/// SIGABRTs the process at the next `FindClass`. This mirrors wry's own
/// `getAppClass` helper.
fn app_class<'a>(
    env: &mut JNIEnv<'a>,
    activity: &JObject,
    dotted_name: &str,
) -> jni::errors::Result<jni::objects::JClass<'a>> {
    let name = env.new_string(dotted_name.replace('/', "."))?;
    let name_obj = JObject::from(name);
    let cls = env
        .call_method(
            activity,
            "getAppClass",
            "(Ljava/lang/String;)Ljava/lang/Class;",
            &[JValue::Object(&name_obj)],
        )?
        .l()?;
    Ok(jni::objects::JClass::from(cls))
}

/// Create a platform WebView showing `url`, layered fullscreen above the
/// current content when `visible` (GONE otherwise). `start_scripts` run at
/// document-start on every navigation (before any page script); pass the
/// neutralizer + IPC shim + capture bundle. Returns a global ref the caller
/// keeps so the view stays addressable (hide / refresh / teardown).
pub async fn create(
    url: &str,
    visible: bool,
    start_scripts: Vec<String>,
) -> anyhow::Result<GlobalRef> {
    let url = url.to_owned();
    dispatch_call(move |env, activity, _webview| {
        let view = env.new_object(
            "android/webkit/WebView",
            "(Landroid/content/Context;)V",
            &[JValue::Object(activity)],
        )?;
        let settings = env
            .call_method(&view, "getSettings", "()Landroid/webkit/WebSettings;", &[])?
            .l()?;
        env.call_method(&settings, "setJavaScriptEnabled", "(Z)V", &[true.into()])?;
        env.call_method(&settings, "setDomStorageEnabled", "(Z)V", &[true.into()])?;
        // NOTE: no custom user agent — the default mobile UA keeps Spotify's
        // mobile layout (a desktop UA was tried: it renders awkwardly small on
        // phones and did not stop the app-handoff navigation anyway).
        //
        // Explicit view setup: a programmatically created + attached WebView
        // must be focusable/clickable/enabled with real layout params, or it
        // renders yet silently drops all touch input (verified on-device:
        // taps/scrolls had zero effect until this was set).
        let match_parent = -1i32;
        let layout_params = env.new_object(
            "android/widget/FrameLayout$LayoutParams",
            "(II)V",
            &[JValue::Int(match_parent), JValue::Int(match_parent)],
        )?;
        env.call_method(
            &view,
            "setLayoutParams",
            "(Landroid/view/ViewGroup$LayoutParams;)V",
            &[JValue::Object(&layout_params)],
        )?;
        env.call_method(&view, "setFocusable", "(Z)V", &[true.into()])?;
        env.call_method(&view, "setFocusableInTouchMode", "(Z)V", &[true.into()])?;
        env.call_method(&view, "setClickable", "(Z)V", &[true.into()])?;
        env.call_method(&view, "setEnabled", "(Z)V", &[true.into()])?;
        env.call_method(&view, "requestFocus", "()Z", &[])?;
        // Document-start scripts: page scripts (popup openers, app-handoff
        // probes) run during parse, long before any post-load
        // `evaluateJavascript` could install our shims — always too late.
        // AndroidX runs these before the first page script on every
        // navigation, so the neutralizer + IPC + capture logic is already in
        // place when Spotify's own code starts. Best-effort (the packaged
        // AndroidX may predate the API): per-pass re-injection remains.
        // The exception is cleared IMMEDIATELY on failure — any further JNI
        // call (even an internal `FindClass`) with it pending aborts the
        // process, which is exactly how the first version of this crashed.
        for script in &start_scripts {
            match add_document_start_script(env, activity, &view, script) {
                Ok(()) => {}
                Err(err) => {
                    clear_pending(env);
                    tracing::warn!("android webview: document-start script failed: {err}");
                    break;
                }
            }
        }
        // Fail-closed popups: a default `WebChromeClient` (no subclass needed)
        // swallows uncaptured new-window requests in-view instead of letting
        // them escape to the system browser (the JS neutralizer converts the
        // legitimate ones to same-window navigations first).
        let chrome = env.new_object("android/webkit/WebChromeClient", "()V", &[])?;
        env.call_method(
            &view,
            "setWebChromeClient",
            "(Landroid/webkit/WebChromeClient;)V",
            &[JValue::Object(&chrome)],
        )?;
        // The process-wide cookie jar: this is what makes the captured web
        // session visible to later token refreshes (and vice versa).
        let cookies = env
            .call_static_method(
                "android/webkit/CookieManager",
                "getInstance",
                "()Landroid/webkit/CookieManager;",
                &[],
            )?
            .l()?;
        env.call_method(&cookies, "setAcceptCookie", "(Z)V", &[true.into()])?;
        env.call_method(
            &cookies,
            "setAcceptThirdPartyCookies",
            "(Landroid/webkit/WebView;Z)V",
            &[JValue::Object(&view), true.into()],
        )?;
        // Layer above the current content (`addView` uses MATCH_PARENT params).
        // The dioxus base view is never touched: no `setContentView`, so no
        // reparent and no wry-global clobbering.
        let frame = env
            .call_method(
                activity,
                "findViewById",
                "(I)Landroid/view/View;",
                &[JValue::Int(CONTENT_FRAME_ID)],
            )?
            .l()?;
        env.call_method(
            &frame,
            "addView",
            "(Landroid/view/View;)V",
            &[JValue::Object(&view)],
        )?;
        env.call_method(
            &view,
            "setVisibility",
            "(I)V",
            &[JValue::Int(if visible { VIEW_VISIBLE } else { VIEW_GONE })],
        )?;
        let target = env.new_string(url)?;
        let target_obj = JObject::from(target);
        env.call_method(
            &view,
            "loadUrl",
            "(Ljava/lang/String;)V",
            &[JValue::Object(&target_obj)],
        )?;
        Ok(env.new_global_ref(&view)?)
    })
    .await
}

/// Register `script` to run at document-start on every navigation in `view`
/// (AndroidX `WebViewCompat`). Best-effort: old WebView versions (or a
/// missing AndroidX class) error out and the caller falls back to post-load
/// injection. App classes load through the activity loader — never the boot
/// loader (see [`app_class`]).
fn add_document_start_script(
    env: &mut JNIEnv,
    activity: &JObject,
    view: &JObject,
    script: &str,
) -> anyhow::Result<()> {
    let compat = app_class(env, activity, "androidx/webkit/WebViewCompat")?;
    let set = env.new_object("java/util/HashSet", "()V", &[])?;
    let star = env.new_string("*")?;
    let star_obj = JObject::from(star);
    env.call_method(
        &set,
        "add",
        "(Ljava/lang/Object;)Z",
        &[JValue::Object(&star_obj)],
    )?;
    let code = env.new_string(script)?;
    let code_obj = JObject::from(code);
    env.call_static_method(
        &compat,
        "addDocumentStartJavaScript",
        "(Landroid/webkit/WebView;Ljava/lang/String;Ljava/util/Set;)V",
        &[
            JValue::Object(view),
            JValue::Object(&code_obj),
            JValue::Object(&set),
        ],
    )?;
    Ok(())
}

/// Navigate an existing view. Fire-and-forget (no round-trip).
pub fn post_load_url(view: GlobalRef, url: String) {
    wry::prelude::dispatch(move |env, _activity, _webview| {
        if let Err(err) = (|| -> jni::errors::Result<()> {
            let target = env.new_string(url)?;
            let target_obj = JObject::from(target);
            env.call_method(
                view.as_obj(),
                "loadUrl",
                "(Ljava/lang/String;)V",
                &[JValue::Object(&target_obj)],
            )?;
            Ok(())
        })() {
            clear_pending(env);
            tracing::warn!("android webview: load url failed: {err}");
        }
    });
}

/// Evaluate `js` in the page without a result callback. Fire-and-forget.
pub fn post_eval_js(view: GlobalRef, js: String) {
    wry::prelude::dispatch(move |env, _activity, _webview| {
        if let Err(err) = eval_js_now(env, view.as_obj(), &js) {
            clear_pending(env);
            tracing::warn!("android webview: eval failed: {err}");
        }
    });
}

fn eval_js_now(env: &mut JNIEnv, view: &JObject, js: &str) -> jni::errors::Result<()> {
    let script = env.new_string(js)?;
    let script_obj = JObject::from(script);
    let null_obj = JObject::null();
    env.call_method(
        view,
        "evaluateJavascript",
        "(Ljava/lang/String;Landroid/webkit/ValueCallback;)V",
        &[JValue::Object(&script_obj), JValue::Object(&null_obj)],
    )?;
    Ok(())
}

/// Synchronous `getProgress()` round-trip (0–100).
pub async fn get_progress(view: &GlobalRef) -> anyhow::Result<i32> {
    let view = view.clone();
    dispatch_call(move |env, _activity, _webview| {
        let progress = env
            .call_method(view.as_obj(), "getProgress", "()I", &[])?
            .i()?;
        Ok(progress)
    })
    .await
}

/// Synchronous `getTitle()` round-trip ("" when unset).
pub async fn get_title(view: &GlobalRef) -> anyhow::Result<String> {
    let view = view.clone();
    dispatch_call(move |env, _activity, _webview| {
        let title = env
            .call_method(view.as_obj(), "getTitle", "()Ljava/lang/String;", &[])?
            .l()?;
        if title.as_raw().is_null() {
            return Ok(String::new());
        }
        let title = jni::objects::JString::from(title);
        let title = env.get_string(&title)?;
        Ok(title.to_string_lossy().into_owned())
    })
    .await
}

/// Synchronous `getUrl()` round-trip ("" when unset).
pub async fn get_url(view: &GlobalRef) -> anyhow::Result<String> {
    let view = view.clone();
    dispatch_call(move |env, _activity, _webview| {
        let url = env
            .call_method(view.as_obj(), "getUrl", "()Ljava/lang/String;", &[])?
            .l()?;
        if url.as_raw().is_null() {
            return Ok(String::new());
        }
        let url = jni::objects::JString::from(url);
        let url = env.get_string(&url)?;
        Ok(url.to_string_lossy().into_owned())
    })
    .await
}

/// Best-effort cookie wipe for logout (the keychain/file token is cleared
/// separately by the caller).
pub fn post_clear_cookies() {
    wry::prelude::dispatch(|env, _activity, _webview| {
        if let Err(err) = (|| -> jni::errors::Result<()> {
            let cookies = env
                .call_static_method(
                    "android/webkit/CookieManager",
                    "getInstance",
                    "()Landroid/webkit/CookieManager;",
                    &[],
                )?
                .l()?;
            let null_obj = JObject::null();
            env.call_method(
                &cookies,
                "removeAllCookies",
                "(Landroid/webkit/ValueCallback;)V",
                &[JValue::Object(&null_obj)],
            )?;
            env.call_method(&cookies, "flush", "()V", &[])?;
            Ok(())
        })() {
            clear_pending(env);
            tracing::warn!("android webview: clear cookies failed: {err}");
        }
    });
}
