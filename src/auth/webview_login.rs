//! Native sign-in, hosted INSIDE the main window (desktop and mobile). While the
//! user isn't authenticated, a full-screen WebView showing `open.spotify.com`
//! fills the window; the dioxus UI is merely hidden underneath it. The moment
//! the web-player session is captured, the WebView is hidden and the native UI
//! is shown again — no separate window, and no reparenting of any WebView.
//!
//! The WebView stays alive as the process's **session WebView**: because its
//! page IS `open.spotify.com`, it is the only WebView whose
//! `get_access_token` fetch is not CORS-blocked (the hidden SDK WebView is a
//! null-origin page and fails). Token refreshes are routed through it. It is
//! torn down on logout (`shutdown`) and rebuilt on the next sign-in.
//!
//! The WebView shares the process-wide session `WebContext`
//! ([`crate::auth::with_session_context`]), so the session cookies (`sp_dc`,
//! `sp_key`, …) persist there — that is what keeps the user logged in across
//! app restarts.
//!
//! Two hosting strategies, selected by cfg:
//! - **Linux desktop** (`all(feature = "desktop", target_os = "linux")`): the
//!   WebView is packed into the window's GTK container and the dioxus UI is
//!   shown/hidden next to it (webkitgtk).
//! - **Mobile and non-Linux desktop** (`not(all(...))`): the WebView is built
//!   with wry's cross-platform `WebViewBuilder::build(&window)`, sized and
//!   positioned to fill the window on sign-in and moved off-screen + hidden
//!   once the session is captured. `dioxus::mobile` re-exports `dioxus::desktop`,
//!   so both share this path (iOS = WKWebView, Android = AndroidView).

use crate::auth::{with_session_context, WebSessionResult};
use std::cell::RefCell;
use wry::{WebView, WebViewBuilder};

const SPOTIFY_LOGIN_URL: &str = "https://open.spotify.com";

/// Runs on every page load inside the login/session WebView (it is injected on
/// each navigation). Login detection and token capture, in order of
/// reliability:
///
/// 1. **`open.spotify.com/api/token`** — the endpoint the web player itself now
///    uses (the old `get_access_token` serves a reCAPTCHA page to direct
///    callers since 2025). Its only challenge is a TOTP computed locally from
///    an obfuscated constant in the player bundle (version 61 as of Aug 2026),
///    plus the session cookies only this WebView can present — so we can obtain
///    a token ourselves without waiting for the page's player to boot behind
///    the invisible reCAPTCHA.
/// 2. **Fetch hook.** Watch the page's OWN network traffic anyway — whatever
///    token the real player receives is cached in `window.__spotifyDxToken`
///    and forwarded. Survives Spotify changing the endpoint contract.
/// 3. **Direct `get_access_token` poll.** Usually captcha-gated now.
/// 4. **DOM fallback.** The profile widget only renders once the user is logged
///    in. If no token arrives within a few cycles, wait a little longer for the
///    captures above, then report the login anyway — with a token if we got
///    one, empty otherwise.
///
/// `window._relay.refreshToken()` serves the most recently captured token,
/// polling for it and re-hitting `/api/token` before the captcha-gated legacy
/// endpoint. Any token captured after the initial login is also posted as a
/// `token_refresh_result` so `AUTH_STATE` stays current and pages that read it
/// (Home etc.) automatically retry.
const POLL_JS: &str = r#"
(function () {
  if (window.__spotifyDxLogin) { return; }
  window.__spotifyDxLogin = true;

  var reported = false;
  function post(obj) {
    if (reported) { return; }
    reported = true;
    // Session captured — the aggressive login detection no longer needs to run.
    // Kill the fixed-interval pollers so a hidden, fully-rendered live web app
    // isn't hammering /api/token (TOTP/HMAC + ~3 fetch()s per tick) forever.
    // On-demand token refresh still works via the explicit _relay.refreshToken()
    // path, and the fetch hook below keeps forwarding the page's own captures.
    try { clearInterval(window.__spotifyDxCheckTimer); } catch (e) {}
    try { clearInterval(window.__spotifyDxFlushTimer); } catch (e) {}
    try { window.ipc.postMessage(JSON.stringify(obj)); } catch (e) {}
  }
  function reportToken(d) {
    post({
      type: 'logged_in',
      token: d.accessToken || '',
      expiresMs: d.accessTokenExpirationTimestampMs || 0,
      isAnon: !!d.isAnonymous
    });
  }
  function notifyToken(d) {
    // Late captures (post-login) keep AUTH_STATE fresh so pages retry.
    try {
      window.ipc.postMessage(JSON.stringify({
        type: 'token_refresh_result',
        token: d.accessToken || '',
        expiresMs: d.accessTokenExpirationTimestampMs || 0,
        isAnon: !!d.isAnonymous
      }));
    } catch (e) {}
  }
  function debug(msg) {
    ipcQueue.push(String(msg));
    flushIpc();
  }
  var ipcQueue = [];
  function flushIpc() {
    if (!window.ipc) { return; }
    while (ipcQueue.length) {
      var m = ipcQueue.shift();
      try { window.ipc.postMessage(JSON.stringify({ type: 'token_debug', msg: m })); }
      catch (e) { ipcQueue.unshift(m); return; }
    }
  }
  function store(d) {
    if (!d || !d.accessToken) { return; }
    window.__spotifyDxToken = d;
    if (!d.isAnonymous) {
      reportToken(d);
      notifyToken(d);
    }
  }

  debug('poll js loaded, ipc=' + (typeof window.ipc !== 'undefined'));
  window.__spotifyDxFlushTimer = setInterval(flushIpc, 1000);

  var origFetch = window.fetch.bind(window);
  window.fetch = function (input, init) {
    try {
      return origFetch(input, init).then(function (resp) {
        try {
          var url = typeof input === 'string' ? input : ((input && input.url) || '');
          if (url.indexOf('access_token') !== -1 || url.indexOf('clientToken') !== -1 || url.indexOf('token') !== -1) {
            resp.clone().text().then(function (text) {
              try {
                var d = JSON.parse(text);
                if (d && d.accessToken) { store(d); }
              } catch (e) {}
            }).catch(function () {});
          }
        } catch (e) {}
        return resp;
      });
    } catch (e) {
      return origFetch(input, init);
    }
  };

  // TOTP for /api/token. Deobfuscated from the web-player bundle (version 61):
  // each char of the embedded string is XORed with (index % 33 + 9), the
  // results joined into a decimal string, and that string's bytes are the HMAC
  // key for a standard RFC 6238 HOTP (SHA-1, 6 digits, 30s period). Pure JS so
  // it also works where `crypto.subtle` is unavailable. Verified against the
  // same computation in Python / Node — the key below is the FULL decimal
  // string (a truncated version silently produced wrong TOTPs, which the
  // endpoint rejected as "Unauthorized request").
  var TOTP_KEY = '376136387538459893883312310911992847112448894410210511297108';
  function rotl(x, n) { return ((x << n) | (x >>> (32 - n))) >>> 0; }
  function sha1(bytes) {
    var h0 = 0x67452301, h1 = 0xEFCDAB89, h2 = 0x98BADCFE, h3 = 0x10325476, h4 = 0xC3D2E1F0;
    var len = bytes.length, ml = len * 8;
    var paddedLen = (((len + 8) >> 6) + 1) << 6;
    var p = new Array(paddedLen);
    for (var i = 0; i < len; i++) { p[i] = bytes[i]; }
    p[len] = 0x80;
    for (var j = len + 1; j < paddedLen; j++) { p[j] = 0; }
    for (var k = 0; k < 8; k++) { p[paddedLen - 1 - k] = Math.floor(ml / Math.pow(2, 8 * k)) & 0xff; }
    for (var m = 0; m < paddedLen; m += 64) {
      var w = [];
      for (var n = 0; n < 16; n++) {
        var o = m + n * 4;
        w[n] = ((p[o] << 24) | (p[o + 1] << 16) | (p[o + 2] << 8) | p[o + 3]) >>> 0;
      }
      for (var q = 16; q < 80; q++) { w[q] = rotl((w[q - 3] ^ w[q - 8] ^ w[q - 14] ^ w[q - 16]) >>> 0, 1); }
      var a = h0, b = h1, c = h2, d = h3, e = h4;
      for (var t = 0; t < 80; t++) {
        var f, k;
        if (t < 20) { f = (b & c) | (~b & d); k = 0x5A827999; }
        else if (t < 40) { f = b ^ c ^ d; k = 0x6ED9EBA1; }
        else if (t < 60) { f = (b & c) | (b & d) | (c & d); k = 0x8F1BBCDC; }
        else { f = b ^ c ^ d; k = 0xCA62C1D6; }
        var tmp = (rotl(a, 5) + f + e + k + w[t]) >>> 0;
        e = d; d = c; c = rotl(b, 30); b = a; a = tmp;
      }
      h0 = (h0 + a) >>> 0; h1 = (h1 + b) >>> 0; h2 = (h2 + c) >>> 0; h3 = (h3 + d) >>> 0; h4 = (h4 + e) >>> 0;
    }
    var out = [];
    [h0, h1, h2, h3, h4].forEach(function (x) {
      for (var s = 3; s >= 0; s--) { out.push((x >>> (s * 8)) & 0xff); }
    });
    return out;
  }
  function hmacSha1(keyBytes, msg) {
    var block = 64;
    if (keyBytes.length > block) { keyBytes = sha1(keyBytes); }
    var ipad = [], opad = [];
    for (var i = 0; i < block; i++) {
      var kb = keyBytes[i] || 0;
      ipad[i] = kb ^ 0x36;
      opad[i] = kb ^ 0x5c;
    }
    return sha1(opad.concat(sha1(ipad.concat(msg))));
  }
  function totpFor(ms) {
    var counter = Math.floor(ms / 30000);
    var msg = [];
    for (var i = 7; i >= 0; i--) { msg[i] = counter % 256; counter = Math.floor(counter / 256); }
    var key = [];
    for (var j = 0; j < TOTP_KEY.length; j++) { key.push(TOTP_KEY.charCodeAt(j)); }
    var h = hmacSha1(key, msg);
    var o = h[19] & 0x0f;
    var bin = ((h[o] & 0x7f) << 24) | (h[o + 1] << 16) | (h[o + 2] << 8) | h[o + 3];
    return ('000000' + (bin % 1000000)).slice(-6);
  }
  function serverTime() {
    var el = document.getElementById('appServerConfig');
    if (!el) { return null; }
    try { return JSON.parse(atob(el.textContent.trim())).serverTime || null; } catch (e) { return null; }
  }

  var apiInFlight = false;
  function tryApiToken() {
    if (apiInFlight) { return; }
    apiInFlight = true;
    try {
      var ms = Date.now();
      var st = serverTime();
      var tp0 = totpFor(ms);
      var tp1 = totpFor(st ? st * 1000 : ms);
      var reasons = ['transport', 'init'];
      var i = 0;
      function next() {
        if (i >= reasons.length) { apiInFlight = false; return; }
        var reason = reasons[i++];
        var url = 'https://open.spotify.com/api/token?reason=' + reason + '&productType=web_player'
          + '&totp=' + tp0 + '&totpServer=' + tp1 + '&totpVer=61';
        var ctrl = ('AbortController' in window) ? new AbortController() : null;
        var timer = ctrl ? setTimeout(function () { try { ctrl.abort(); } catch (e) {} }, 8000) : null;
        return origFetch(url, { credentials: 'include', signal: ctrl ? ctrl.signal : undefined }).then(function (resp) {
          return resp.text().then(function (text) {
            if (timer) { clearTimeout(timer); }
            var info = 'api/token(' + reason + ') status ' + resp.status;
            try {
              var d = JSON.parse(text);
              if (d && d.accessToken) {
                info += ' captured ' + d.accessToken.slice(0, 10) + '... anon=' + !!d.isAnonymous;
                store(d);
              } else {
                info += ' ' + text.slice(0, 220);
                debug(info);
                return next();
              }
            } catch (e) {
              info += ' non-json ' + text.slice(0, 140);
            }
            debug(info);
            apiInFlight = false;
          });
        }, function (err) {
          if (timer) { clearTimeout(timer); }
          debug('api/token(' + reason + ') error: ' + err.toString());
          apiInFlight = false;
        });
      }
      next();
    } catch (e) {
      debug('api/token threw: ' + e.toString());
      apiInFlight = false;
    }
  }

  var domTicks = 0;
  function check() {
    if (reported) { return; }
    // 1) The page's own TOTP-gated endpoint (primary token source).
    tryApiToken();

    // 2) Legacy direct poll as a backup (often captcha-gated now).
    origFetch('https://open.spotify.com/get_access_token?reason=transport&productType=web_player', { credentials: 'include' })
      .then(function (r) { return r.json(); })
      .then(function (d) { if (d && d.accessToken) { store(d); } })
      .catch(function () {});

    // 3) Last resort: logged-in pages render the profile widget. Check from
    //    the start (not after a fixed tick count) and settle ~5s after the
    //    widget appears, giving the captures above time to win first.
    if (++domTicks > 1 && !window.__spotifyDxDomSettled) {
      var userWidget = document.querySelector('[data-testid="user-widget-link"]');
      var loginBtn = document.querySelector('[data-testid="login-button"], [data-testid="signup-button"]');
      if (userWidget && !loginBtn) {
        window.__spotifyDxDomSettled = true;
        var waited = 0;
        var wait = setInterval(function () {
          waited++;
          var d = window.__spotifyDxToken;
          if (d && d.accessToken && !d.isAnonymous) {
            clearInterval(wait);
            reportToken(d);
          } else if (waited >= 10) {
            clearInterval(wait);
            post({ type: 'logged_in', token: '', expiresMs: 0, isAnon: false, via: 'dom' });
          }
        }, 500);
      }
    }
  }
  check();
  window.__spotifyDxCheckTimer = setInterval(check, 1500);

  // Token refresh for Rust. Same-origin (this page IS open.spotify.com), so
  // unlike the null-origin SDK WebView this path is not CORS-blocked. Serves
  // the latest captured token, re-hitting /api/token while polling, and only
  // as a last resort the captcha-gated legacy endpoint.
  window._relay = {
    refreshToken: function () {
      var deadline = Date.now() + 8000;
      function serve(d) {
        window.ipc.postMessage(JSON.stringify({
          type: 'token_refresh_result',
          token: d.accessToken,
          expiresMs: d.accessTokenExpirationTimestampMs,
          isAnon: !!d.isAnonymous
        }));
      }
      function attempt() {
        var d = window.__spotifyDxToken;
        var now = Date.now();
        if (d && d.accessToken && !d.isAnonymous && d.accessTokenExpirationTimestampMs > now) {
          serve(d);
          return;
        }
        if (Date.now() < deadline) {
          if (!apiInFlight) { tryApiToken(); }
          setTimeout(attempt, 500);
          return;
        }
        origFetch('https://open.spotify.com/get_access_token?reason=transport&productType=web_player', { credentials: 'include' })
          .then(function (r) { return r.json(); })
          .then(function (d2) {
            if (d2 && d2.accessToken && !d2.isAnonymous) {
              window.__spotifyDxToken = d2;
              serve(d2);
            }
          })
          .catch(function (e) {
            window.ipc.postMessage(JSON.stringify({ type: 'token_error', msg: e.toString() }));
          });
      }
      attempt();
    }
  };
})();
"#;

struct LoginWebView {
    /// Kept alive so the IPC handler and page stay alive until logout (wry
    /// WebViews are dropped along with their owner).
    #[allow(dead_code)]
    webview: WebView,
    /// Linux desktop: the sign-in widget packed into the window's vbox. Hidden
    /// on session capture, removed on shutdown. Absent on other native
    /// platforms, where show/hide is driven by `set_visible`/`set_bounds`.
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    widget: gtk::Widget,
    /// Linux desktop: the native UI children we hid under the sign-in page (the
    /// dioxus webview). Re-shown when the session is captured.
    #[cfg(all(feature = "desktop", target_os = "linux"))]
    hidden: Vec<gtk::Widget>,
    /// Set `true` by the page-load handler once `open.spotify.com` finishes
    /// loading (so `POLL_JS` ran and `window._relay` exists). Cleared on every
    /// navigation start.
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// Whether the page is currently parked at `about:blank` (see `park`).
    /// When `true`, `refresh_token` must revive it before refreshing.
    suspended: bool,
}

thread_local! {
    static LOGIN: RefCell<Option<LoginWebView>> = const { RefCell::new(None) };
}

/// Start the in-window Spotify sign-in and deliver the captured session over
/// `tx`. Must be called on the UI thread (webkitgtk / the wry window are not
/// thread-safe). Returns without doing anything if a sign-in is already in
/// progress.
///
/// Linux desktop: the sign-in WebView is packed straight into the window's
/// existing `vbox` (next to the dioxus UI), and the dioxus UI is hidden.
/// Nothing is reparented: moving a realized WebView between containers is
/// exactly what produced the blank screen, so we only ever show/hide widgets.
///
/// Other native platforms (mobile / non-Linux desktop): the WebView is built to
/// fill the whole window.
pub fn start(tx: tokio::sync::oneshot::Sender<WebSessionResult>) -> anyhow::Result<()> {
    if LOGIN.with(|cell| cell.borrow().is_some()) {
        return Ok(());
    }

    let tx = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    #[cfg(all(feature = "desktop", target_os = "linux"))]
    {
        use gtk::prelude::*;
        use wry::WebViewExtUnix as _;
        let vbox = main_vbox()?;
        let webview = build_webview_gtk(&vbox, true, tx, ready.clone())?;
        let widget: gtk::Widget = webview.webview().upcast();

        // The sign-in WebView was packed at the end of the vbox. Hide every
        // other child (the dioxus UI) so the sign-in page fills the window.
        let hidden: Vec<gtk::Widget> = vbox
            .children()
            .into_iter()
            .filter(|child| child != &widget)
            .collect();
        tracing::info!(
            "webview login: hiding {} native child(ren) under the sign-in page",
            hidden.len()
        );
        for child in &hidden {
            child.hide();
        }
        widget.show();
        widget.grab_focus();

        LOGIN.with(|cell| {
            *cell.borrow_mut() = Some(LoginWebView {
                webview,
                widget,
                hidden,
                ready,
                suspended: false,
            })
        });
    }

    #[cfg(not(all(feature = "desktop", target_os = "linux")))]
    {
        let webview = build_webview_cross(true, tx, ready.clone())?;
        LOGIN.with(|cell| {
            *cell.borrow_mut() = Some(LoginWebView {
                webview,
                ready,
                suspended: false,
            })
        });
    }
    Ok(())
}

/// Ensure a hidden session WebView exists so token refreshes work even on
/// sessions restored from the keychain: no sign-in ran, but the cookies are in
/// the shared data directory, so `open.spotify.com` auto-logs-in and the fetch
/// hook captures tokens in the background. Built like the sign-in WebView but
/// never shown; the native UI stays put. Idempotent.
pub fn ensure_session() -> anyhow::Result<()> {
    if LOGIN.with(|cell| cell.borrow().is_some()) {
        return Ok(());
    }
    let tx = std::sync::Arc::new(std::sync::Mutex::new(None));
    let ready = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));

    #[cfg(all(feature = "desktop", target_os = "linux"))]
    {
        use gtk::prelude::*;
        use wry::WebViewExtUnix as _;
        let vbox = main_vbox()?;
        let webview = build_webview_gtk(&vbox, false, tx, ready.clone())?;
        let widget: gtk::Widget = webview.webview().upcast();
        LOGIN.with(|cell| {
            *cell.borrow_mut() = Some(LoginWebView {
                webview,
                widget,
                hidden: Vec::new(),
                ready,
                suspended: false,
            })
        });
    }

    #[cfg(not(all(feature = "desktop", target_os = "linux")))]
    {
        let webview = build_webview_cross(false, tx, ready.clone())?;
        LOGIN.with(|cell| {
            *cell.borrow_mut() = Some(LoginWebView {
                webview,
                ready,
                suspended: false,
            })
        });
    }

    // Park the hidden page at `about:blank` so the offscreen Spotify SPA stops
    // rendering and burning CPU. Tokens are captured on demand via
    // `refresh_token` (which revives the page), so no background capture is
    // needed here.
    park_if_loaded();
    tracing::info!("webview login: hidden session webview ready for token refreshes");
    Ok(())
}

/// The window's `default_vbox()` — contains the dioxus UI; every WebView is
/// packed into it. Errors if the window isn't ready yet. Linux desktop only.
#[cfg(all(feature = "desktop", target_os = "linux"))]
fn main_vbox() -> anyhow::Result<gtk::Box> {
    use dioxus::desktop::tao::platform::unix::WindowExtUnix;
    let desktop = dioxus::desktop::window();
    Ok(desktop
        .window
        .default_vbox()
        .ok_or_else(|| anyhow::anyhow!("main window has no gtk container for the sign-in webview"))?
        .clone())
}

/// Build a sign-in WebView inside the process-wide session `WebContext`, with
/// the shared URL + `POLL_JS` + navigation/page-load/IPC handlers attached.
/// `build` receives the `WebViewBuilder` and must finish it with a backend
/// build (`.build_gtk(&vbox)` on Linux desktop, `.with_bounds(..).build(..)`
/// elsewhere). It runs inside `with_session_context` because the builder borrows
/// `&mut WebContext`, so the borrow cannot escape that closure.
fn build_in_context(
    tx: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<WebSessionResult>>>>,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
    build: impl FnOnce(WebViewBuilder<'_>) -> wry::Result<WebView>,
) -> anyhow::Result<WebView> {
    with_session_context(|context| {
        render(&mut *context, &tx, &ready, build)
    })
}

fn render(
    context: &mut wry::WebContext,
    tx: &std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<WebSessionResult>>>>,
    ready: &std::sync::Arc<std::sync::atomic::AtomicBool>,
    build: impl FnOnce(WebViewBuilder<'_>) -> wry::Result<WebView>,
) -> anyhow::Result<WebView> {
    let builder = WebViewBuilder::new_with_web_context(context)
        .with_url(SPOTIFY_LOGIN_URL)
        .with_initialization_script(POLL_JS)
        .with_navigation_handler(|url| {
            tracing::info!("webview login: navigating to {url}");
            true
        })
        .with_on_page_load_handler({
            let ready = ready.clone();
            move |event, url| match event {
                wry::PageLoadEvent::Started => {
                    ready.store(false, std::sync::atomic::Ordering::SeqCst);
                    tracing::info!("webview login: page load started: {url}");
                }
                wry::PageLoadEvent::Finished => {
                    ready.store(true, std::sync::atomic::Ordering::SeqCst);
                    tracing::info!("webview login: page load finished: {url}");
                }
            }
        })
        .with_ipc_handler({
            let tx = tx.clone();
            move |request| {
                let tx = tx.clone();
                handle_ipc(tx, request)
            }
        });
    build(builder).map_err(|e| anyhow::anyhow!("failed to build the sign-in webview: {e}"))
}

/// Build the sign-in WebView packed into `vbox` (Linux desktop).
#[cfg(all(feature = "desktop", target_os = "linux"))]
fn build_webview_gtk(
    vbox: &gtk::Box,
    visible: bool,
    tx: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<WebSessionResult>>>>,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<WebView> {
    use wry::WebViewBuilderExtUnix as _;
    build_in_context(tx, ready, |builder| {
        builder.with_visible(visible).build_gtk(vbox)
    })
}

/// Bounds filling the current window (mobile / non-Linux desktop sign-in).
#[cfg(not(all(feature = "desktop", target_os = "linux")))]
fn full_window_bounds() -> wry::Rect {
    let (w, h) = crate::platform::webview::window_logical_size();
    wry::Rect {
        position: wry::dpi::Position::Logical(wry::dpi::LogicalPosition::new(0.0, 0.0)),
        size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(w.max(1.0), h.max(1.0))),
    }
}

/// Bounds hiding the WebView once the session is captured / before sign-in on
/// the cross-platform path (off-screen, 1×1).
#[cfg(not(all(feature = "desktop", target_os = "linux")))]
fn hidden_bounds() -> wry::Rect {
    wry::Rect {
        position: wry::dpi::Position::Physical(wry::dpi::PhysicalPosition::new(0, -9999)),
        size: wry::dpi::Size::Logical(wry::dpi::LogicalSize::new(1.0, 1.0)),
    }
}

/// Build the sign-in WebView as a child of the wry window (mobile / non-Linux
/// desktop). When `visible` it fills the window, overlaying the dioxus UI while
/// the user signs in; otherwise it is parked off-screen, hidden.
#[cfg(not(all(feature = "desktop", target_os = "linux")))]
fn build_webview_cross(
    visible: bool,
    tx: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<WebSessionResult>>>>,
    ready: std::sync::Arc<std::sync::atomic::AtomicBool>,
) -> anyhow::Result<WebView> {
    let desktop = crate::platform::webview::window();
    let bounds = if visible { full_window_bounds() } else { hidden_bounds() };
    build_in_context(tx, ready, |builder| {
        builder
            .with_bounds(bounds)
            .with_visible(visible)
            .build(&desktop.window)
    })
}

/// Hide the sign-in WebView and reveal the native UI, but keep the WebView
/// alive as the session WebView: its page is same-origin with open.spotify.com,
/// so it remains the working path for token refreshes (see `refresh_token`).
///
/// As an idle-CPU optimization the page is then parked at `about:blank` (see
/// `park`), which stops the offscreen Spotify SPA from rendering/animating
/// forever. Token refreshes revive it on demand.
pub fn hide() {
    LOGIN.with(|cell| {
        if let Some(login) = cell.borrow_mut().as_mut() {
            #[cfg(all(feature = "desktop", target_os = "linux"))]
            {
                use gtk::prelude::*;
                login.widget.hide();
                for child in &login.hidden {
                    child.show();
                }
            }
            #[cfg(not(all(feature = "desktop", target_os = "linux")))]
            {
                let _ = login.webview.set_visible(false);
                if let Err(err) = login.webview.set_bounds(hidden_bounds()) {
                    tracing::warn!("webview login: hide (cross) failed: {err}");
                }
            }
        }
    });
    park_if_loaded();
}

/// Navigate the session WebView's page to `about:blank` to stop the offscreen
/// `open.spotify.com` SPA from burning CPU while nothing needs it. The widget,
/// cookies and WebContext all stay alive, so `refresh_token` can revive the
/// page at any time. Idempotent.
fn park_if_loaded() {
    LOGIN.with(|cell| {
        if let Some(login) = cell.borrow_mut().as_mut() {
            if login.suspended {
                return;
            }
            login.suspended = true;
            login.ready.store(false, std::sync::atomic::Ordering::SeqCst);
            if let Err(err) = login.webview.load_url("about:blank") {
                tracing::warn!("webview login: park failed: {err}");
            }
        }
    });
}

/// Tear the session WebView down entirely (logout / session expiry). The next
/// sign-in starts from a fresh WebView. Idempotent.
pub fn shutdown() {
    LOGIN.with(|cell| {
        if let Some(login) = cell.borrow_mut().take() {
            #[cfg(all(feature = "desktop", target_os = "linux"))]
            {
                use gtk::prelude::*;
                login.widget.unparent();
            }
            drop(login);
        }
    });
}

/// Ask the session WebView to fetch a fresh access token via the HttpOnly
/// session cookies. Same-origin, so unlike the SDK WebView's null-origin page
/// this `get_access_token` fetch is not CORS-blocked. Returns `false` when no
/// session WebView is alive (caller falls back to the SDK WebView).
///
/// If the page was parked at `about:blank` (idle-CPU optimization), this first
/// revives it by navigating back to `open.spotify.com` and waits for the page
/// to finish loading (so `POLL_JS` has run and `window._relay` exists) before
/// triggering the refresh. Revival happens on the UI thread; async so the wait
/// doesn't block it.
pub async fn refresh_token() -> bool {
    use std::sync::atomic::Ordering::*;

    let revived = LOGIN.with(|cell| {
        let mut guard = cell.borrow_mut();
        let Some(login) = guard.as_mut() else { return false };
        if login.suspended {
            login.suspended = false;
            login.ready.store(false, SeqCst);
            if let Err(err) = login.webview.load_url(SPOTIFY_LOGIN_URL) {
                tracing::warn!("webview login: revive failed: {err}");
                login.suspended = true;
                return false;
            }
        }
        true
    });
    if !revived {
        return false;
    }

    // Wait for the page to load so `window._relay` is defined before we eval.
    // The actual refresh then runs in JS (it polls /api/token itself), so a
    // short settle here is all we need.
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(6);
    loop {
        let ready = LOGIN.with(|cell| {
            cell.borrow()
                .as_ref()
                .map(|login| login.ready.load(SeqCst))
                .unwrap_or(false)
        });
        if ready {
            break;
        }
        if tokio::time::Instant::now() >= deadline {
            tracing::warn!("webview login: page did not finish loading for token refresh");
            return false;
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }

    LOGIN.with(|cell| {
        cell.borrow()
            .as_ref()
            .map(|login| {
                login
                    .webview
                    .evaluate_script(
                        "window._relay && window._relay.refreshToken && window._relay.refreshToken()",
                    )
                    .is_ok()
            })
            .unwrap_or(false)
    })
}

/// JS → Rust: the poller succeeded, so forward the web-player session. Runs on
/// the webkit thread, so it only sends over the channel — the awaiting task on
/// the UI thread performs the state writes and window teardown.
///
/// Only a non-anonymous token is accepted; an anonymous (guest) token means the
/// user still isn't signed in, so the message is dropped and the poller keeps
/// going. A `via: "dom"` message (empty token) is the JS-side fallback that
/// detected the logged-in page without capturing a token.
///
/// Any other message type (token refresh results, etc.) is forwarded verbatim
/// to the shared playback-bridge handler, because this WebView also serves as
/// the token-refresh channel for the whole app.
fn handle_ipc(
    tx: std::sync::Arc<std::sync::Mutex<Option<tokio::sync::oneshot::Sender<WebSessionResult>>>>,
    request: wry::http::Request<String>,
) {
    let body = request.body().clone();
    let Ok(msg) = serde_json::from_str::<serde_json::Value>(&body) else {
        return;
    };
    // Diagnostics are logged straight from the webkit thread — the bridge IPC
    // queue isn't drained until `webview_bridge::init()` runs (after the page
    // has loaded), so anything posted at document-start would otherwise be
    // silently dropped.
    if msg.get("type").and_then(|t| t.as_str()) == Some("token_debug") {
        let text = msg.get("msg").and_then(|m| m.as_str()).unwrap_or_default();
        tracing::info!("webview login: token debug: {text}");
        return;
    }
    if msg.get("type").and_then(|t| t.as_str()) != Some("logged_in") {
        crate::player::webview_bridge::handle_ipc(request);
        return;
    }
    let via = msg.get("via").and_then(|t| t.as_str()).unwrap_or("token");
    let is_anonymous = msg.get("isAnon").and_then(|t| t.as_bool()).unwrap_or(false);
    let token = msg
        .get("token")
        .and_then(|t| t.as_str())
        .unwrap_or_default()
        .to_owned();
    let expires_ms = msg.get("expiresMs").and_then(|t| t.as_u64()).unwrap_or_default();

    if via == "dom" {
        // No token captured, but the profile widget rendered — the user is
        // logged in. The hidden SDK WebView shares the session cookies and will
        // fetch a token itself, so an empty token is fine here.
        tracing::info!("webview login: logged-in page detected without a token");
        if let Some(tx) = tx.lock().unwrap().take() {
            let _ = tx.send(WebSessionResult {
                access_token: String::new(),
                expires_at_ms: 0,
                is_anonymous: false,
            });
        }
        return;
    }

    if token.is_empty() || is_anonymous {
        tracing::debug!(
            "webview login: token not usable yet (anon={is_anonymous}, len={})",
            token.len()
        );
        return;
    }

    tracing::info!("webview login: session captured ({via})");
    if let Some(tx) = tx.lock().unwrap().take() {
        let _ = tx.send(WebSessionResult {
            access_token: token,
            expires_at_ms: expires_ms,
            is_anonymous: false,
        });
    }
}
