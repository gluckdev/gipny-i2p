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
import java.util.concurrent.Executors

class GipnyService : Service() {
    // Embedded i2pd (see android-router/jni/gipny_i2pd_jni.cpp), built per-ABI
    // in CI and packaged as libi2pd.so. `RouterHandle::attach` on the Rust side
    // expects the SAM bridge to already be listening on `SAM_PORT` by the time
    // `I2pNode::start()` runs.
    //
    // i2pd takes no argv here, so everything about how it runs comes from the
    // i2pd.conf written by `prepareDataDir` before the daemon starts.
    private external fun nativeStartSam(dataDir: String, samListen: String): String?
    private external fun nativeStopSam()
    private external fun nativeNetworkChanged(online: Boolean)

    private val routerExecutor = Executors.newSingleThreadExecutor()
    private var samStarted = false
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
        startEmbeddedRouter()
        return START_STICKY
    }

    override fun onTaskRemoved(rootIntent: Intent?) {
        super.onTaskRemoved(rootIntent)
    }

    override fun onDestroy() {
        destroyed = true
        unwatchNetwork()
        routerExecutor.execute { stopEmbeddedRouter() }
        routerExecutor.shutdown()
        super.onDestroy()
    }

    private fun startEmbeddedRouter() {
        if (!libLoaded || destroyed) return
        routerExecutor.execute {
            if (samStarted || destroyed) return@execute
            val routerDir = java.io.File(filesDir, "gipny/i2p/router")
            try {
                prepareDataDir(routerDir)
            } catch (t: Throwable) {
                android.util.Log.e(TAG, "failed to prepare i2pd data dir", t)
                return@execute
            }
            val err = try {
                nativeStartSam(routerDir.absolutePath, "127.0.0.1:$SAM_PORT")
            } catch (t: UnsatisfiedLinkError) {
                android.util.Log.e(TAG, "libi2pd not loaded", t)
                return@execute
            }
            if (err != null) {
                android.util.Log.e(TAG, "failed to start embedded i2p router: $err")
                return@execute
            }
            samStarted = true
            android.util.Log.i(TAG, "SAM bridge ready on port $SAM_PORT")
            if (destroyed) stopEmbeddedRouter() else watchNetwork()
        }
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
                if (!samStarted || destroyed) return
                android.util.Log.i(TAG, "default network changed; router re-tests reachability")
                nativeNetworkChanged(false)
                nativeNetworkChanged(true)
            }

            override fun onLost(network: Network) {
                if (!samStarted || destroyed) return
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
     * Write i2pd's config and unpack the reseed certificates it needs to
     * bootstrap.
     *
     * Two things have to be on disk before the daemon starts: i2pd.conf, which
     * is the only channel for configuration because DaemonAndroid passes no
     * argv, and certificates/, without which reseeding cannot verify anything
     * and a fresh router never joins the network. i2pd's own start() blocks for
     * up to 10 s waiting for the assets.ready marker, so it is written last.
     */
    private fun prepareDataDir(routerDir: java.io.File) {
        routerDir.mkdirs()

        // Rewritten every start: settings should follow the app, not whatever an
        // older version happened to leave behind.
        java.io.File(routerDir, "i2pd.conf").writeText(
            """
            # Generated by gipny — edits are overwritten on every start.
            # Everything but SAM and the HTTP proxy is off: gipny speaks SAMv3
            # over loopback, and the proxy exists for one caller only — the
            # update checker, which reaches GitHub through an i2p outproxy so the
            # phone's real address never appears there. Mirrors the flags the
            # desktop build passes in libcore/src/router.rs.
            loglevel = warn
            log = file
            logfile = ${java.io.File(routerDir, "i2pd.log").absolutePath}
            ipv4 = true
            ipv6 = false

            # Transit traffic is cover traffic other people generate and pay for.
            # A router that carries nothing but its own messages gives an observer
            # a clean signal; one that relays for strangers is indistinguishable
            # from one that is merely passing something along. So carrying some
            # transit buys anonymity that costs us only bandwidth.
            #
            # `share` is the fraction of the line offered to it, and the numbers
            # below are the cautious end of the curve, not the free end: a phone
            # should contribute, not volunteer as infrastructure. i2pd's own
            # defaults — 100% share, 25000 transit tunnels — are sized for a
            # server and would flatten a battery.
            bandwidth = L
            share = 25

            [ntcp2]
            enabled = true

            [ssu2]
            enabled = true

            [sam]
            enabled = true
            address = 127.0.0.1
            port = $SAM_PORT

            [http]
            enabled = false

            [httpproxy]
            enabled = true
            address = 127.0.0.1
            port = $HTTP_PROXY_PORT
            outproxy = $OUTPROXY

            [socksproxy]
            enabled = false

            [bob]
            enabled = false

            [i2cp]
            enabled = false

            [i2pcontrol]
            enabled = false

            [upnp]
            enabled = false

            [reseed]
            verify = true

            [limits]
            # Enough to blend into, few enough that the phone stays a phone.
            # This was 0, which was a silent trade of anonymity for battery.
            transittunnels = 32
            """.trimIndent()
        )

        // Re-unpack when the app version changes: upstream rotates reseed certs
        // and a stale set silently breaks bootstrap on a fresh profile.
        val stamp = java.io.File(routerDir, "assets.version")
        val version = try {
            packageManager.getPackageInfo(packageName, 0).versionName ?: "dev"
        } catch (t: Throwable) {
            "dev"
        }
        val certs = java.io.File(routerDir, "certificates")
        if (!certs.isDirectory || stamp.takeIf { it.isFile }?.readText() != version) {
            certs.deleteRecursively()
            copyAssetDir("certificates", certs)
            stamp.writeText(version)
        }

        java.io.File(routerDir, "assets.ready").writeText(version)
    }

    private fun copyAssetDir(assetPath: String, dest: java.io.File) {
        val children = assets.list(assetPath) ?: emptyArray()
        if (children.isEmpty()) {
            // A leaf: assets.list() returns nothing for files.
            dest.parentFile?.mkdirs()
            assets.open(assetPath).use { input ->
                dest.outputStream().use { output -> input.copyTo(output) }
            }
            return
        }
        dest.mkdirs()
        for (child in children) {
            copyAssetDir("$assetPath/$child", java.io.File(dest, child))
        }
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

        // Must match libcore::router::DEFAULT_HTTP_PROXY_PORT and
        // DEFAULT_OUTPROXY: the Rust side attaches to this proxy by that
        // constant and gives up quietly if nothing answers there.
        private const val HTTP_PROXY_PORT = 4444
        private const val OUTPROXY = "http://exit.stormycloud.i2p"

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
