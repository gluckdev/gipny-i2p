package app.gipny

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.os.PowerManager
import android.provider.Settings
import androidx.activity.enableEdgeToEdge
import androidx.biometric.BiometricManager
import androidx.biometric.BiometricPrompt
import androidx.core.content.ContextCompat

import androidx.core.view.WindowCompat
import androidx.core.view.WindowInsetsCompat
import androidx.core.view.WindowInsetsControllerCompat

class MainActivity : TauriActivity() {
  override fun onCreate(savedInstanceState: Bundle?) {
    enableEdgeToEdge()
    window.setFlags(
      android.view.WindowManager.LayoutParams.FLAG_SECURE,
      android.view.WindowManager.LayoutParams.FLAG_SECURE
    )
    val windowInsetsController = WindowCompat.getInsetsController(window, window.decorView)
    windowInsetsController.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
    windowInsetsController.hide(WindowInsetsCompat.Type.navigationBars())
    super.onCreate(savedInstanceState)
    val svc = Intent(this, GipnyService::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      ContextCompat.startForegroundService(this, svc)
    } else {
      startService(svc)
    }
    maybeRequestBatteryWhitelist()
    // A share that cold-started the process arrives here; a share to an
    // already-running instance comes through onNewIntent (singleTask).
    if (savedInstanceState == null) stageShareIntent(intent)
  }

  /** launchMode=singleTask: a second "share into Gipny" reuses this instance. */
  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    stageShareIntent(intent)
  }

  /**
   * Show a biometric (or device credential) prompt and report the outcome.
   *
   * Allowed authenticators are weak/strong biometrics OR the device
   * PIN/pattern/password. That combination is the only one androidx.biometric
   * accepts on every API level from minSdk 24 up: `DEVICE_CREDENTIAL` alone is
   * rejected below API 30, and `BIOMETRIC_STRONG | DEVICE_CREDENTIAL` is
   * rejected on API 28–29 (see AuthenticatorUtils.isSupportedCombination).
   * Allowing the credential half also forbids a negative-button label, so none
   * is set. `callback` runs on the main thread; `false` covers user cancel,
   * error, and a missing/unsupported authenticator.
   */
  fun authenticateWithBiometrics(title: String, subtitle: String, callback: (Boolean) -> Unit) {
    val prompt = BiometricPrompt(
      this,
      ContextCompat.getMainExecutor(this),
      object : BiometricPrompt.AuthenticationCallback() {
        override fun onAuthenticationSucceeded(result: BiometricPrompt.AuthenticationResult) {
          callback(true)
        }

        override fun onAuthenticationError(errorCode: Int, errString: CharSequence) {
          callback(false)
        }

        // Non-terminal: the prompt stays up for another attempt, so no callback.
        override fun onAuthenticationFailed() {}
      },
    )
    val info = BiometricPrompt.PromptInfo.Builder()
      .setTitle(title)
      .setSubtitle(subtitle)
      .setAllowedAuthenticators(
        BiometricManager.Authenticators.BIOMETRIC_WEAK or
          BiometricManager.Authenticators.DEVICE_CREDENTIAL
      )
      .build()
    try {
      prompt.authenticate(info)
    } catch (_: IllegalArgumentException) {
      // No enrolled authenticator / no hardware on this device.
      callback(false)
    }
  }

  /** One share-sheet submission, parked until the UI drains it. */
  data class StagedShare(
    val action: String,
    val text: String?,
    val uris: List<Uri>,
    val mimeType: String?,
  )

  /**
   * Parked shares live on the companion so they survive activity recreation
   * (the process, and therefore the router, keeps running).
   */
  companion object {
    private val stagedShares = mutableListOf<StagedShare>()

    /** Everything staged so far; the caller takes the items and clears the queue. */
    @Synchronized
    fun drainStagedShares(): List<StagedShare> = stagedShares.toList().also { stagedShares.clear() }

    @Synchronized
    private fun stageShare(share: StagedShare) {
      stagedShares.add(share)
    }
  }

  /**
   * Pull EXTRA_TEXT / EXTRA_STREAM out of an ACTION_SEND(_MULTIPLE) intent and
   * queue it. The share sheet grants read access only for the receiver's
   * current lifetime, so a persistable grant is taken when the sender offered
   * one; otherwise the consumer must copy the content out promptly.
   */
  private fun stageShareIntent(intent: Intent?) {
    val action = intent?.action ?: return
    if (action != Intent.ACTION_SEND && action != Intent.ACTION_SEND_MULTIPLE) return

    @Suppress("DEPRECATION") // getParcelable*(String, Class) is API 33+ only
    val uris: List<Uri> = if (action == Intent.ACTION_SEND_MULTIPLE) {
      intent.getParcelableArrayListExtra<Uri>(Intent.EXTRA_STREAM)
        ?.filterNotNull().orEmpty()
    } else {
      listOfNotNull(intent.getParcelableExtra<Uri>(Intent.EXTRA_STREAM))
    }
    for (uri in uris) {
      try {
        contentResolver.takePersistableUriPermission(uri, Intent.FLAG_GRANT_READ_URI_PERMISSION)
      } catch (_: SecurityException) {
        // Transient grant only: still stage it, but read it before the task dies.
      }
    }

    val text = intent.getStringExtra(Intent.EXTRA_TEXT)
    if (text.isNullOrEmpty() && uris.isEmpty()) return
    stageShare(StagedShare(action, text, uris, intent.type))
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
