package app.gipny

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.enableEdgeToEdge
import androidx.core.content.ContextCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    super.onCreate(savedInstanceState)
    val svc = Intent(this, GipnyService::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      ContextCompat.startForegroundService(this, svc)
    } else {
      startService(svc)
    }
    maybeRequestBatteryWhitelist()
  }

  /** "Back" at the first screen hides the app; it does not end the process.
   *
   * Finishing the activity tears the process down gracefully, and a graceful
   * teardown is exactly what the embedded router cannot survive: static
   * destructors free the mutexes while i2pd's own threads are still waiting on
   * them, and the next `pthread_mutex_lock` aborts —
   *
   *     FORTIFY: pthread_mutex_lock called on a destroyed mutex
   *     Fatal signal 6 (SIGABRT) in tid …, name: Tunnels
   *
   * — which the owner saw as the app "collapsing" on its way out. Upstream's
   * Android shutdown path is racy and not ours to fix from here.
   *
   * Backgrounding is also what this app should do regardless. The router and
   * the relay connection are held by a foreground service, so going back means
   * putting the window away, not disconnecting. wry's own back callback still
   * walks the webview's history first (see WryActivity.setWebView) and only
   * calls this at the very first screen. To actually stop the app, swipe it out
   * of recents: the system kills the process outright, and a kill runs no
   * destructors and hits no destroyed mutex. */
  // OVERRIDE_DEPRECATION: overriding a deprecated member is its own warning
  // (KT-47902); DEPRECATION covers only calls to one.
  @Suppress("DEPRECATION", "OVERRIDE_DEPRECATION", "MissingSuperCall")
  override fun onBackPressed() {
    moveTaskToBack(true)
  }

  /**
   * Ask the user to exempt gipny from battery optimizations (Doze).
   *
   * Without this exemption Android pauses network access roughly one minute
   * after the screen goes off, killing all i2p tunnels.  The previous version
   * set a "never ask again" flag on the first attempt regardless of the
   * user's answer; if they tapped "Deny" once, the app would never recover.
   *
   * Now we re-check on every launch: if already whitelisted we do nothing;
   * otherwise we ask at most once per 24 hours (Google Play policy for
   * ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).
   */
  private fun maybeRequestBatteryWhitelist() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
    val pm = getSystemService(POWER_SERVICE) as PowerManager
    if (pm.isIgnoringBatteryOptimizations(packageName)) return

    val prefs = getSharedPreferences("gipny", Context.MODE_PRIVATE)
    val lastAsked = prefs.getLong("battery_whitelist_last_asked", 0)
    if (System.currentTimeMillis() - lastAsked < 24 * 60 * 60 * 1000) return

    try {
      startActivity(Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
        data = Uri.parse("package:$packageName")
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      })
    } catch (_: Exception) {
      try {
        startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS)
          .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      } catch (_: Exception) {}
    }
    prefs.edit().putLong("battery_whitelist_last_asked", System.currentTimeMillis()).apply()
  }
}
