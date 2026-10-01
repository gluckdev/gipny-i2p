package app.gipny

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.BroadcastReceiver
import android.content.Context
import android.content.Intent
import android.content.IntentFilter
import android.content.pm.ServiceInfo
import android.net.ConnectivityManager
import android.net.Network
import android.net.wifi.WifiManager
import android.os.Build
import android.os.IBinder
import android.os.PowerManager
import androidx.core.app.NotificationCompat

class GipnyService : Service() {
    // The i2p router runs inside this process, started and used by the Rust
    // side through libi2pd.so (android-router/jni: libi2pd plus gipny's shim
    // over its api). No daemon, no SAM, no proxy, no port. This service keeps
    // the process in the foreground and tells the router when the network
    // changes; nothing else.
    private external fun nativeNetworkChanged(online: Boolean)

    private var networkCallback: ConnectivityManager.NetworkCallback? = null
    private var wakeLock: PowerManager.WakeLock? = null
    private var wifiLock: WifiManager.WifiLock? = null
    private var idleReceiver: BroadcastReceiver? = null
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
        acquireLocks()
        if (libLoaded && !destroyed) {
            watchNetwork()
            watchIdleMode()
        }
        return START_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        destroyed = true
        unwatchIdleMode()
        unwatchNetwork()
        releaseLocks()
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

    /**
     * Keep the CPU and Wi-Fi radio alive while this service runs.
     *
     * A foreground service prevents the process from being killed, but it does
     * NOT prevent the CPU from suspending or Wi-Fi from sleeping when the
     * screen is off.  Without these locks i2pd's threads freeze, open TCP
     * connections time out on the peer side, tunnels expire, and we appear
     * disconnected until the user unlocks the screen and waits for a full
     * re-bootstrap (2–5 minutes).  Briar, Orbot, and InviZible Pro all hold
     * the same locks for the same reason.
     */
    private fun acquireLocks() {
        if (wakeLock == null) {
            val pm = getSystemService(POWER_SERVICE) as PowerManager
            wakeLock = pm.newWakeLock(PowerManager.PARTIAL_WAKE_LOCK, "gipny:i2p-router")
                .apply { acquire() }
        }
        if (wifiLock == null) {
            val wm = applicationContext.getSystemService(WIFI_SERVICE) as? WifiManager
            wifiLock = wm?.createWifiLock(
                if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.Q)
                    WifiManager.WIFI_MODE_FULL_LOW_LATENCY
                else
                    @Suppress("DEPRECATION")
                    WifiManager.WIFI_MODE_FULL_HIGH_PERF,
                "gipny:i2p-wifi"
            )?.apply { acquire() }
        }
    }

    private fun releaseLocks() {
        wifiLock?.let { if (it.isHeld) it.release() }
        wifiLock = null
        wakeLock?.let { if (it.isHeld) it.release() }
        wakeLock = null
    }

    /**
     * Re-test the network when the device exits Doze (idle) mode.
     *
     * ConnectivityManager.NetworkCallback does not always fire when the device
     * wakes from Doze — the network object stays the same, it was just paused.
     * Listening to ACTION_DEVICE_IDLE_MODE_CHANGED lets us poke the router as
     * soon as the maintenance window opens, so it re-tests reachability
     * immediately instead of sitting with stale measurements.
     */
    private fun watchIdleMode() {
        if (idleReceiver != null) return
        if (Build.VERSION.SDK_INT < Build.VERSION_CODES.M) return
        val receiver = object : BroadcastReceiver() {
            override fun onReceive(context: Context, intent: Intent) {
                if (destroyed) return
                val pm = getSystemService(POWER_SERVICE) as PowerManager
                if (!pm.isDeviceIdleMode) {
                    android.util.Log.i(TAG, "exited Doze; router re-tests reachability")
                    nativeNetworkChanged(false)
                    nativeNetworkChanged(true)
                }
            }
        }
        if (Build.VERSION.SDK_INT >= Build.VERSION_CODES.TIRAMISU) {
            registerReceiver(
                receiver,
                IntentFilter(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED),
                Context.RECEIVER_NOT_EXPORTED
            )
        } else {
            registerReceiver(
                receiver,
                IntentFilter(PowerManager.ACTION_DEVICE_IDLE_MODE_CHANGED)
            )
        }
        idleReceiver = receiver
    }

    private fun unwatchIdleMode() {
        val receiver = idleReceiver ?: return
        idleReceiver = null
        try {
            unregisterReceiver(receiver)
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
