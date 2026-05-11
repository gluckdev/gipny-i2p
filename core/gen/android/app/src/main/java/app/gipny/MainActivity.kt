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
