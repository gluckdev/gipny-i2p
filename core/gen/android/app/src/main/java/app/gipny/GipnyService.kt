package app.gipny

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

class GipnyService : Service() {
    // Embedded go-i2p SAM router (see i2p-router/android_export.go +
    // jni_shim_android.c), built per-ABI by the `buildGoRouterJniLibs`
    // Gradle task and packaged as a regular jniLib. `RouterHandle::attach`
    // on the Rust side expects the SAM bridge to already be listening on
    // `SAM_PORT` by the time `I2pNode::start()` runs.
    private external fun nativeStartSam(dataDir: String, samListen: String): String?
    private external fun nativeStopSam()

    private var samStarted = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        startEmbeddedRouter()
        val channelId = "gipny_runtime"
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.O) {
            val channel = NotificationChannel(channelId, "gipny runtime", NotificationManager.IMPORTANCE_MIN).apply {
                setShowBadge(false)
                setSound(null, null)
                enableVibration(false)
                lockscreenVisibility = Notification.VISIBILITY_SECRET
                description = "keeps i2p + relay connection alive in background"
            }
            getSystemService(NotificationManager::class.java).createNotificationChannel(channel)
        }
        val tap = PendingIntent.getActivity(
            this,
            0,
            Intent(this, MainActivity::class.java).addFlags(Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT
        )
        val notification: Notification = NotificationCompat.Builder(this, channelId)
            .setSmallIcon(R.drawable.ic_notification)
            .setContentTitle("gipny")
            .setContentText("connected via i2p")
            .setOngoing(true)
            .setSilent(true)
            .setShowWhen(false)
            .setPriority(NotificationCompat.PRIORITY_MIN)
            .setCategory(NotificationCompat.CATEGORY_SERVICE)
            .setVisibility(NotificationCompat.VISIBILITY_SECRET)
            .setContentIntent(tap)
            .build()
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.UPSIDE_DOWN_CAKE) {
            val type = ServiceInfo.FOREGROUND_SERVICE_TYPE_DATA_SYNC or ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING
            startForeground(NOTIFICATION_ID, notification, type)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        return START_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        stopEmbeddedRouter()
        super.onDestroy()
    }

    private fun startEmbeddedRouter() {
        if (samStarted || !libLoaded) return
        val routerDir = java.io.File(filesDir, "gipny/i2p").absolutePath
        val err = try {
            nativeStartSam(routerDir, "127.0.0.1:$SAM_PORT")
        } catch (t: UnsatisfiedLinkError) {
            android.util.Log.e(TAG, "libgipnyi2p not loaded", t)
            return
        }
        if (err != null) {
            android.util.Log.e(TAG, "failed to start embedded i2p router: $err")
            return
        }
        samStarted = true
    }

    private fun stopEmbeddedRouter() {
        if (!samStarted) return
        nativeStopSam()
        samStarted = false
    }

    companion object {
        private const val NOTIFICATION_ID = 0x9197
        private const val TAG = "GipnyService"
        private const val SAM_PORT = 7656

        private var libLoaded = false

        init {
            try {
                System.loadLibrary("gipnyi2p")
                libLoaded = true
            } catch (t: UnsatisfiedLinkError) {
                android.util.Log.e("GipnyService", "libgipnyi2p.so not available", t)
            }
        }
    }
}
