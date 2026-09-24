package app.gipny

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Intent
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.os.Build
import android.os.IBinder
import androidx.core.app.NotificationCompat

class GipnyService : Service() {
    // The i2p router runs inside this process, started and used by the Rust
    // side through libi2pd.so (android-router/jni: libi2pd plus gipny's shim
    // over its api). No daemon, no SAM, no proxy, no port. This service keeps
    // the process in the foreground and tells the router when the network
    // changes; nothing else.
    private external fun nativeNetworkChanged(online: Boolean)

    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    @Volatile private var destroyed = false

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        android.util.Log.i(TAG, "GipnyService started")
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
            startForeground(NOTIFICATION_ID, notification, ServiceInfo.FOREGROUND_SERVICE_TYPE_REMOTE_MESSAGING)
        } else {
            startForeground(NOTIFICATION_ID, notification)
        }
        if (libLoaded && !destroyed) watchNetwork()
        return START_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        destroyed = true
        unwatchNetwork()
        super.onDestroy()
    }

    /**
     * Tell the router when the phone's default network changes.
     *
     * Without it i2pd keeps what it measured on the network it started on: after
     * a Wi-Fi/LTE switch or a long sleep it had no tunnels, could not publish
     * itself, and our own relay was "not found" for as long as the app ran.
     * A new default network is offline-then-online for the router, which makes
     * it test how that network sees it; losing the default network is offline.
     * Upstream i2pd-android does the same from its own NetworkCallback.
     */
    private fun watchNetwork() {
        if (networkCallback != null) return
        val cm = getSystemService(ConnectivityManager::class.java) ?: return
        val callback = object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                if (destroyed) return
                android.util.Log.i(TAG, "default network changed; router re-tests reachability")
                nativeNetworkChanged(false)
                nativeNetworkChanged(true)
            }

            override fun onLost(network: Network) {
                if (destroyed) return
                android.util.Log.i(TAG, "default network lost")
                nativeNetworkChanged(false)
            }
        }
        try {
            cm.registerDefaultNetworkCallback(callback)
            networkCallback = callback
        } catch (t: Throwable) {
            android.util.Log.e(TAG, "cannot watch the network", t)
        }
    }

    private fun unwatchNetwork() {
        val callback = networkCallback ?: return
        networkCallback = null
        try {
            getSystemService(ConnectivityManager::class.java)?.unregisterNetworkCallback(callback)
        } catch (_: Throwable) {
        }
    }

    companion object {
        private const val NOTIFICATION_ID = 0x9197
        private const val TAG = "GipnyService"

        private var libLoaded = false

        init {
            try {
                System.loadLibrary("i2pd")
                libLoaded = true
            } catch (t: UnsatisfiedLinkError) {
                android.util.Log.e("GipnyService", "libi2pd.so not available", t)
            }
        }
    }
}
