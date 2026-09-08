package com.spotifydx.app

/**
 * Injected page scripts for the login/session WebView (§8 migration).
 *
 * [POLL_JS] is the battle-tested capture bundle from the native core
 * (`src/auth/webview_login.rs`), ported verbatim — same four layered methods
 * in priority order (player token endpoint with locally computed TOTP →
 * page-traffic observation → legacy polling → logged-in-DOM fallback),
 * anonymous tokens ignored, first-wins delivery, timers destroyed at capture,
 * `window._relay.refreshToken()` for on-demand refresh. The TOTP key,
 * SHA-1 constants, endpoint shape, and message vocabulary are identical; only
 * the transport differs:
 *
 * - Native dioxus build: `window.ipc` is wry's IPC channel (desktop) or the
 *   `document.title` ferry outbox (legacy Android renderer).
 * - Kotlin app: [IPC_SHIM] defines `window.ipc.postMessage` over the
 *   `SpotifyDx` JavascriptInterface — strictly more reliable, same message
 *   vocabulary (`logged_in`, `token_refresh_result`, `token_error`,
 *   `token_debug`).
 */
object CaptureJs {
    /** Defines `window.ipc` over the `SpotifyDx` script interface. */
    const val IPC_SHIM = """(function(){
if(window.__spotifyDxIpc){return;} window.__spotifyDxIpc=true;
window.ipc={postMessage:function(m){ try{ SpotifyDx.postMessage(String(m)); }catch(e){} }};
})();"""

    /** Evaluate to trigger an on-demand token refresh through the live page. */
    const val REFRESH = """try{ (window._relay && window._relay.refreshToken && window._relay.refreshToken()); 'ok'; }catch(e){ 'missing:'+e; }"""

    /** True when the page is a live Spotify web document (not parked blank). */
    const val IS_LIVE = """(window.location.protocol==='http:'||window.location.protocol==='https:')?'1':'0'"""

    const val POLL_JS = """
(function () {
  if (window.__spotifyDxLogin) { return; }
  window.__spotifyDxLogin = true;
  if (window.location.protocol !== 'http:' && window.location.protocol !== 'https:') { return; }

  var reported = false;
  function post(obj) {
    if (reported) { return; }
    reported = true;
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
    tryApiToken();
    origFetch('https://open.spotify.com/get_access_token?reason=transport&productType=web_player', { credentials: 'include' })
      .then(function (r) { return r.json(); })
      .then(function (d) { if (d && d.accessToken) { store(d); } })
      .catch(function () {});
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
"""
}
