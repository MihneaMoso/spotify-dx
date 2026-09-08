# Keep the JNI bridge + self-update installer even if minification is enabled
# one day (docs/KOTLIN_MIGRATION.md §16: ProGuard stripping bridge symbols is
# fatal and is verified by the bridge-compat check).
-keep class com.spotifydx.app.CoreBridge { *; }
-keep class com.spotifydx.app.SpotifyDxUpdater { *; }
-keep class com.spotifydx.app.SpotifyDxFileProvider { *; }
