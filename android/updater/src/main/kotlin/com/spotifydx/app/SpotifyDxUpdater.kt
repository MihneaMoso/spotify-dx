package com.spotifydx.app

import android.content.Context
import android.content.Intent
import android.net.Uri

/**
 * Kicks off an APK self-update by firing the system PackageInstaller intent
 * for the staged APK, served over the SpotifyDxFileProvider content URI. Name
 * and signature are fixed: native code calls `SpotifyDxUpdater.installApk(
 * Context, String)` through JNI (see updater::fire_install_intent in
 * src/updater.rs).
 */
object SpotifyDxUpdater {

    @JvmStatic
    fun installApk(context: Context, path: String) {
        // `path` is the native-side staged APK path; we serve the canonical
        // staged file over the provider regardless.
        val uri: Uri = Uri.parse("content://${SpotifyDxFileProvider.AUTHORITY}/apk")
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        context.startActivity(intent)
    }
}