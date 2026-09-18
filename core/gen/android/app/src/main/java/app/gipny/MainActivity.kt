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
  @Suppress("DEPRECATION", "MissingSuperCall")
  override fun onBackPressed() {
    moveTaskToBack(true)
  }

  private fun maybeRequestBatteryWhitelist() {
    if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
    val prefs = getSharedPreferences("gipny", Context.MODE_PRIVATE)
    if (prefs.getBoolean("battery_optimization_asked", false)) return
    val pm = getSystemService(POWER_SERVICE) as PowerManager
    if (pm.isIgnoringBatteryOptimizations(packageName)) {
      prefs.edit().putBoolean("battery_optimization_asked", true).apply()
      return
    }
    try {
      val intent = Intent(Settings.ACTION_REQUEST_IGNORE_BATTERY_OPTIMIZATIONS).apply {
        data = Uri.parse("package:$packageName")
        addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
      }
      startActivity(intent)
    } catch (_: Exception) {
      try {
        startActivity(Intent(Settings.ACTION_IGNORE_BATTERY_OPTIMIZATION_SETTINGS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK))
      } catch (_: Exception) {
      }
    }
    prefs.edit().putBoolean("battery_optimization_asked", true).apply()
  }
}
