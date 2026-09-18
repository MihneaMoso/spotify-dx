# Keep the JNI bridge + self-update installer (docs/KOTLIN_MIGRATION.md
# §16: ProGuard stripping bridge symbols is fatal and is verified by the
# bridge-compat check). Release builds minify (build.gradle).
-keep class com.spotifydx.app.CoreBridge { *; }
-keep class com.spotifydx.app.SpotifyDxUpdater { *; }
-keep class com.spotifydx.app.SpotifyDxFileProvider { *; }
-keep class com.spotifydx.app.InstallResultReceiver { *; }
-keep class com.spotifydx.app.ContextMenuSheet { *; }
# WebViews invoke @JavascriptInterface methods by name via reflection
# (LoginWebView/SdkWebView bridges) — shrinking them breaks login/SDK play.
-keepclassmembers class * {
    @android.webkit.JavascriptInterface <methods>;
}
