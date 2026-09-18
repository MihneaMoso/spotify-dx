package com.spotifydx.app

import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.pm.PackageInstaller
import android.os.Build
import android.util.Log
import android.widget.Toast

/**
 * Commit callback for [SpotifyDxUpdater] sessions. The system delivers the
 * install result here — critically also `STATUS_PENDING_USER_ACTION`,
 * whose confirmation intent *we* must start, or no install window ever
 * appears (the silent-install symptom). Manifest-registered, non-exported.
 */
class InstallResultReceiver : BroadcastReceiver() {
    override fun onReceive(context: Context, intent: Intent) {
        if (intent.action != SpotifyDxUpdater.STATUS_ACTION) return
        val status = intent.getIntExtra(PackageInstaller.EXTRA_STATUS, -999)
        val msg = intent.getStringExtra(PackageInstaller.EXTRA_STATUS_MESSAGE)
        when (status) {
            PackageInstaller.STATUS_PENDING_USER_ACTION -> {
                @Suppress("DEPRECATION")
                val confirm: Intent? = if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT, Intent::class.java)
                } else {
                    intent.getParcelableExtra(Intent.EXTRA_INTENT)
                }
                confirm?.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
                try {
                    context.startActivity(confirm)
                } catch (e: Exception) {
                    Log.e(TAG, "confirm intent failed", e)
                    Toast.makeText(context, "Could not open installer", Toast.LENGTH_LONG).show()
                }
            }
            PackageInstaller.STATUS_SUCCESS ->
                Toast.makeText(context, "Update installed", Toast.LENGTH_LONG).show()
            else -> {
                Log.w(TAG, "install status=$status msg=$msg")
                Toast.makeText(
                    context,
                    "Install failed${if (!msg.isNullOrEmpty()) ": $msg" else ""}",
                    Toast.LENGTH_LONG,
                ).show()
            }
        }
    }

    companion object {
        private const val TAG = "SpotifyDxInstall"
    }
}
