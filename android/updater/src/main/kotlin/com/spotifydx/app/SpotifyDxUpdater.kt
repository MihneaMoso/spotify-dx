package com.spotifydx.app

import android.app.PendingIntent
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.content.pm.PackageManager
import android.net.Uri
import android.os.Build
import android.provider.Settings
import java.io.File

/**
 * Self-update installer (§11 migration; fixed 2026-09: the old
 * `ACTION_VIEW`-only path silently did nothing on Android 8+ when the
 * per-app "Install unknown apps" grant was missing, and the provider's
 * null `query()` aborted the system installer that did open).
 *
 * Primary path is now the `PackageInstaller` Session API (the documented
 * replacement for the deprecated `ACTION_INSTALL_PACKAGE`; `ACTION_VIEW`
 * is kept as a fallback only): the APK streams into a session and
 * `commit()` always surfaces the system install UI via
 * [InstallResultReceiver] (which also fires the `PENDING_USER_ACTION`
 * confirmation intent — without that handler no window appears).
 *
 * Conflict/data guarantees (the "package conflicts with the older
 * version" class of failure):
 * - Same `applicationId`: the staged APK's package must equal ours, or
 *   we abort with a clear message instead of letting the system show
 *   a cryptic conflict dialog.
 * - Same signing key: staged signers must match ours (release-over-debug
 *   or a re-keyed APK installs as a *conflicting* package, not an update).
 * - Monotonic version: staged `versionCode` must be >= installed, or the
 *   system rejects it as a downgrade.
 * - Data preservation: a session commit is an *update*, never an
 *   uninstall — app data/cache survive by default. Nothing here deletes.
 *
 * Native code calls `SpotifyDxUpdater.installApk(Context, String)` through
 * JNI (see updater::fire_install_intent in src/updater.rs); the `path`
 * argument is informational — the canonical staged file is resolved here.
 */
object SpotifyDxUpdater {
    const val STATUS_ACTION = "com.spotifydx.app.INSTALL_STATUS"
    private const val STAGED_APK = "updates/spotify-dx-update.apk"

    @JvmStatic
    fun installApk(context: Context, path: String) {
        val apk = File(context.filesDir, STAGED_APK)
        if (!apk.exists() || apk.length() == 0L) {
            throw IllegalStateException("staged update missing (${apk.absolutePath}) — download it first")
        }
        // Fail fast with a readable message instead of the system's
        // "package conflicts" dialog: wrong package, foreign signature, or
        // older versionCode can never install as an update.
        verifyUpdateCompatible(context, apk)
        // Android 8+: without the per-app unknown-sources grant the
        // installer UI never appears. Route the user to the toggle first.
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O &&
            !context.packageManager.canRequestPackageInstalls()
        ) {
            context.startActivity(
                Intent(
                    Settings.ACTION_MANAGE_UNKNOWN_APP_SOURCES,
                    Uri.parse("package:${context.packageName}"),
                ).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
            throw SecurityException(
                "unknown-sources grant required — enable \"Install unknown apps\", then tap Install again",
            )
        }
        try {
            installViaSession(context, apk)
        } catch (e: SecurityException) {
            throw e
        } catch (e: IllegalStateException) {
            throw e
        } catch (e: IllegalArgumentException) {
            // Programming/config errors (bad session params on an OEM ROM):
            // surface them instead of degrading into the legacy path, whose
            // silent failure is exactly what the session path replaced.
            throw e
        } catch (e: Exception) {
            // Session machinery failed (OEM quirk, etc.) — fall back to the
            // legacy VIEW intent over the provider before giving up.
            installViaView(context)
        }
    }

    /** Prechecks that turn system "conflict" rejections into clear errors. */
    private fun verifyUpdateCompatible(context: Context, apk: File) {
        val pm = context.packageManager
        val staged = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageArchiveInfo(
                apk.absolutePath,
                PackageManager.PackageInfoFlags.of(
                    PackageManager.GET_SIGNING_CERTIFICATES.toLong(),
                ),
            )
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageArchiveInfo(
                apk.absolutePath,
                PackageManager.GET_SIGNING_CERTIFICATES,
            )
        } ?: throw IllegalStateException("staged file is not a valid APK — re-download the update")
        if (staged.packageName != context.packageName) {
            throw IllegalStateException(
                "staged package ${staged.packageName} != ${context.packageName} — refusing to install",
            )
        }
        val installed = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            pm.getPackageInfo(
                context.packageName,
                PackageManager.PackageInfoFlags.of(
                    PackageManager.GET_SIGNING_CERTIFICATES.toLong(),
                ),
            )
        } else {
            @Suppress("DEPRECATION")
            pm.getPackageInfo(
                context.packageName,
                PackageManager.GET_SIGNING_CERTIFICATES,
            )
        }
        @Suppress("DEPRECATION")
        val stagedCode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            staged.longVersionCode
        } else {
            staged.versionCode.toLong()
        }
        @Suppress("DEPRECATION")
        val installedCode = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.P) {
            installed.longVersionCode
        } else {
            installed.versionCode.toLong()
        }
        if (stagedCode < installedCode) {
            throw IllegalStateException(
                "staged v$stagedCode is older than installed v$installedCode — check for a newer update",
            )
        }
        // Same-key rule: an APK signed with any other key (debug vs
        // release, re-keyed CI) is a conflicting package, not an update.
        val stagedSigners = staged.signingInfo
            ?.apkContentsSigners
            ?.map { it.toByteArray().contentHashCode() }
            ?.toSet()
        val installedSigners = installed.signingInfo
            ?.apkContentsSigners
            ?.map { it.toByteArray().contentHashCode() }
            ?.toSet()
        if (stagedSigners != null && installedSigners != null && stagedSigners != installedSigners) {
            throw IllegalStateException(
                "staged APK is signed with a different key — updates must use the same signing key",
            )
        }
    }

    private fun installViaSession(context: Context, apk: File) {
        val installer = context.packageManager.packageInstaller
        val params = PackageInstaller.SessionParams(PackageInstaller.SessionParams.MODE_FULL_INSTALL)
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) {
            params.setAppPackageName(context.packageName)
            params.setRequireUserAction(PackageInstaller.SessionParams.USER_ACTION_REQUIRED)
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            params.setPackageSource(PackageInstaller.PACKAGE_SOURCE_LOCAL_FILE)
        }
        val sessionId = installer.createSession(params)
        try {
            val session = installer.openSession(sessionId)
            try {
                session.openWrite("update", 0, apk.length()).use { out ->
                    apk.inputStream().use { input -> input.copyTo(out) }
                    session.fsync(out)
                }
                session.commit(pendingStatusIntent(context))
            } finally {
                // Post-commit close releases our handle; the system owns the
                // session from here (data preserved — update, not reinstall).
                runCatching { session.close() }
            }
        } catch (e: Exception) {
            runCatching { installer.abandonSession(sessionId) }
            throw e
        }
    }

    private fun pendingStatusIntent(context: Context): android.content.IntentSender {
        val intent = Intent(STATUS_ACTION).setPackage(context.packageName)
        val flags = PendingIntent.FLAG_UPDATE_CURRENT or
            (if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.S) PendingIntent.FLAG_MUTABLE else 0)
        return PendingIntent.getBroadcast(context, 0, intent, flags).intentSender
    }

    /** Legacy fallback: VIEW intent over our provider (query() now serves metadata). */
    private fun installViaView(context: Context) {
        val uri: Uri = Uri.parse("content://${SpotifyDxFileProvider.AUTHORITY}/apk")
        val intent = Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, "application/vnd.android.package-archive")
            .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
        context.startActivity(intent)
    }
}
