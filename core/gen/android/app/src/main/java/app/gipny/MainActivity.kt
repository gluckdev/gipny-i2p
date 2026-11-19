package app.gipny

import android.content.Intent
import android.net.Uri
import android.os.Build
import android.os.Bundle
import android.webkit.WebView
import androidx.activity.enableEdgeToEdge
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
    // On a phone the app draws its own keyboard (owner decision 2026-11-07):
    // the system IME must never see what is typed. inputmode="none" on the
    // fields is not enough everywhere — Huawei WebView 11 (Chromium 83) still
    // bound HeliBoard to a touched field — so the window itself declares that
    // it does not interact with the input method: the IME is not shown for it
    // at all, while a hardware or Bluetooth keyboard keeps delivering keys.
    window.setFlags(
      android.view.WindowManager.LayoutParams.FLAG_ALT_FOCUSABLE_IM,
      android.view.WindowManager.LayoutParams.FLAG_ALT_FOCUSABLE_IM
    )
    window.setSoftInputMode(android.view.WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_HIDDEN)
    val windowInsetsController = WindowCompat.getInsetsController(window, window.decorView)
    windowInsetsController.systemBarsBehavior = WindowInsetsControllerCompat.BEHAVIOR_SHOW_TRANSIENT_BARS_BY_SWIPE
    windowInsetsController.hide(WindowInsetsCompat.Type.navigationBars())
    // Prerelease builds (version with a `-`, built with the full developer log)
    // expose the webview to chrome://inspect over USB debugging, so a page that
    // fails before it can draw is readable from a computer instead of being a
    // blank screen. wry only logs console messages in debug builds, and a stable
    // build never opens this door. See docs/release-diagnostics.md.
    if (BuildConfig.DEBUG || BuildConfig.VERSION_NAME.contains('-')) {
      WebView.setWebContentsDebuggingEnabled(true)
    }
    super.onCreate(savedInstanceState)
    ContextCompat.registerReceiver(this, screenReceiver, android.content.IntentFilter(Intent.ACTION_SCREEN_OFF),
      ContextCompat.RECEIVER_NOT_EXPORTED)
    val svc = Intent(this, GipnyService::class.java)
    if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
      ContextCompat.startForegroundService(this, svc)
    } else {
      startService(svc)
    }
    // A share that cold-started the process arrives here; a share to an
    // already-running instance comes through onNewIntent (singleTask).
    if (savedInstanceState == null) stageShareIntent(intent)
  }

  private val screenReceiver = object : android.content.BroadcastReceiver() {
    override fun onReceive(context: android.content.Context, intent: Intent) {
      if (intent.action == Intent.ACTION_SCREEN_OFF) screenHidden()
    }
  }
  @androidx.annotation.Keep
  private external fun screenHidden()

  private var credentialRequest = 0L
  private val deviceCredentialCode = 4931

  @androidx.annotation.Keep
  fun deviceAuthAvailable(): Boolean =
    (getSystemService(KEYGUARD_SERVICE) as android.app.KeyguardManager).isDeviceSecure

  @androidx.annotation.Keep
  @Suppress("DEPRECATION")
  fun verifyDeviceAuth(id: Long, reason: String): Boolean {
    if (credentialRequest != 0L || !deviceAuthAvailable()) return false
    val manager = getSystemService(KEYGUARD_SERVICE) as android.app.KeyguardManager
    val intent = manager.createConfirmDeviceCredentialIntent("Gipny", reason) ?: return false
    credentialRequest = id
    try { startActivityForResult(intent, deviceCredentialCode) }
    catch (_: android.content.ActivityNotFoundException) { credentialRequest = 0; return false }
    return true
  }

  @androidx.annotation.Keep
  private external fun deviceAuthFinished(id: Long, success: Boolean)

  @Suppress("DEPRECATION", "OVERRIDE_DEPRECATION")
  override fun onActivityResult(requestCode: Int, resultCode: Int, data: Intent?) {
    super.onActivityResult(requestCode, resultCode, data)
    if (requestCode == deviceCredentialCode && credentialRequest != 0L) {
      val id = credentialRequest
      credentialRequest = 0
      deviceAuthFinished(id, resultCode == RESULT_OK)
    }
  }

  override fun onDestroy() {
    unregisterReceiver(screenReceiver)
    if (credentialRequest != 0L) { deviceAuthFinished(credentialRequest, false); credentialRequest = 0 }
    super.onDestroy()
  }

  @androidx.annotation.Keep
  fun closeLockedApp() {
    stopService(Intent(this, GipnyService::class.java))
    finishAndRemoveTask()
  }

  /** launchMode=singleTask: a second "share into Gipny" reuses this instance. */
  override fun onNewIntent(intent: Intent) {
    super.onNewIntent(intent)
    setIntent(intent)
    stageShareIntent(intent)
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

}
