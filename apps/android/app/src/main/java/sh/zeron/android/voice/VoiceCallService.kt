package sh.zeron.android.voice

import android.app.Notification
import android.app.NotificationChannel
import android.app.NotificationManager
import android.app.PendingIntent
import android.app.Service
import android.content.Context
import android.content.Intent
import android.content.pm.ServiceInfo
import android.os.Build
import android.os.IBinder
import android.util.Log
import androidx.core.app.NotificationCompat
import androidx.core.app.NotificationManagerCompat
import androidx.core.app.Person
import androidx.core.app.ServiceCompat
import androidx.core.content.ContextCompat
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.launch
import sh.zeron.android.MainActivity
import sh.zeron.android.R

/**
 * The ongoing call (Android's stand-in for the iOS Live Activity): a
 * microphone foreground service, so the call keeps its microphone with the
 * screen locked or Zeron behind, and its call notification — the host, the
 * call clock, Mute and End. Tapping it returns to the stage.
 *
 * Started by the peer once the microphone is granted (a microphone service
 * needs it) and stopped when the call ends.
 */
class VoiceCallService : Service() {
    companion object {
        private const val TAG = "ZeronVoice"
        private const val CHANNEL = "voice-call"
        private const val NOTIFICATION = 0x766f
        private const val ACTION_MUTE = "sh.zeron.android.voice.MUTE"
        private const val ACTION_END = "sh.zeron.android.voice.END"

        private var instance: VoiceCallService? = null
        private var starting = false
        private var stopRequested = false

        fun start(context: Context) {
            if (instance != null || starting) return
            starting = true
            stopRequested = false
            try {
                ContextCompat.startForegroundService(context, Intent(context, VoiceCallService::class.java))
            } catch (e: Exception) {
                // Backgrounded before the microphone opened: the call runs while Zeron is in front.
                Log.w(TAG, "call service not started", e)
                starting = false
            }
        }

        fun stop(context: Context) {
            val service = instance
            if (service != null) service.finish() else if (starting) stopRequested = true
        }

        /** Mute and the call going live answer at once (the flow catches the rest). */
        fun refresh(context: Context) {
            instance?.post()
        }

        private val controller: VoiceController? get() = VoiceController.current?.get()
    }

    private val scope = MainScope()
    private var lastKey: Any? = null

    override fun onBind(intent: Intent?): IBinder? = null

    override fun onCreate() {
        super.onCreate()
        instance = this
        starting = false
        ensureChannel()
    }

    override fun onStartCommand(intent: Intent?, flags: Int, startId: Int): Int {
        when (intent?.action) {
            ACTION_MUTE -> controller?.toggleMute()
            ACTION_END -> controller?.stop()
        }
        if (lastKey == null) {
            // Within five seconds of startForegroundService, always — even when stopping right away.
            try {
                ServiceCompat.startForeground(
                    this,
                    NOTIFICATION,
                    build(controller?.snapshot?.value ?: VoiceSnapshot(live = true)),
                    if (Build.VERSION.SDK_INT >= 30) ServiceInfo.FOREGROUND_SERVICE_TYPE_MICROPHONE else 0,
                )
                lastKey = Unit
            } catch (e: Exception) {
                Log.w(TAG, "startForeground failed", e)
                finish()
                return START_NOT_STICKY
            }
            observe()
        }
        if (stopRequested || controller?.live != true) finish()
        return START_NOT_STICKY
    }

    private fun observe() {
        val controller = controller ?: return
        scope.launch {
            // Orb states flip with every sentence; the shade needs at most one update a second.
            controller.snapshot
                .map { Key(it.hostName, it.statusText, it.muted, it.activeSinceWall, it.awaitingInput) }
                .distinctUntilChanged()
                .collect { key ->
                    if (key != lastKey) post()
                    delay(1000)
                }
        }
    }

    private fun post() {
        val snapshot = controller?.snapshot?.value ?: return
        if (!snapshot.live) return
        lastKey = Key(snapshot.hostName, snapshot.statusText, snapshot.muted, snapshot.activeSinceWall, snapshot.awaitingInput)
        runCatching { NotificationManagerCompat.from(this).notify(NOTIFICATION, build(snapshot)) }
    }

    private data class Key(val host: String, val status: String, val muted: Boolean, val since: Long?, val awaiting: Boolean)

    private fun finish() {
        if (instance === this) instance = null
        starting = false
        stopRequested = false
        scope.cancel()
        ServiceCompat.stopForeground(this, ServiceCompat.STOP_FOREGROUND_REMOVE)
        stopSelf()
    }

    override fun onDestroy() {
        if (instance === this) instance = null
        scope.cancel()
        super.onDestroy()
    }

    private fun ensureChannel() {
        val manager = getSystemService(NotificationManager::class.java) ?: return
        if (manager.getNotificationChannel(CHANNEL) != null) return
        manager.createNotificationChannel(
            NotificationChannel(CHANNEL, "Voice calls", NotificationManager.IMPORTANCE_DEFAULT).apply {
                description = "The ongoing Codex voice call"
                setSound(null, null)
                enableVibration(false)
                setShowBadge(false)
            },
        )
    }

    private fun pending(action: String, code: Int): PendingIntent = PendingIntent.getService(
        this,
        code,
        Intent(this, VoiceCallService::class.java).setAction(action),
        PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
    )

    private fun build(snapshot: VoiceSnapshot): Notification {
        val open = PendingIntent.getActivity(
            this,
            1,
            Intent(this, MainActivity::class.java)
                .setAction(VoiceController.ACTION_OPEN_STAGE)
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK or Intent.FLAG_ACTIVITY_SINGLE_TOP),
            PendingIntent.FLAG_IMMUTABLE or PendingIntent.FLAG_UPDATE_CURRENT,
        )
        val host = snapshot.hostName.ifEmpty { "your computer" }
        val caller = Person.Builder().setName("Codex on $host").setImportant(true).build()
        val status = when {
            !snapshot.active -> snapshot.statusText
            snapshot.awaitingInput -> "Codex needs your answer"
            snapshot.muted -> "Muted"
            else -> snapshot.statusText
        }
        val builder = NotificationCompat.Builder(this, CHANNEL)
            .setSmallIcon(R.drawable.zi_microphone)
            .setContentTitle("Codex on $host")
            .setContentText(status)
            .setContentIntent(open)
            .setOngoing(true)
            .setSilent(true)
            .setOnlyAlertOnce(true)
            .setCategory(NotificationCompat.CATEGORY_CALL)
            .setVisibility(NotificationCompat.VISIBILITY_PUBLIC)
            .setForegroundServiceBehavior(NotificationCompat.FOREGROUND_SERVICE_IMMEDIATE)
            .setStyle(NotificationCompat.CallStyle.forOngoingCall(caller, pending(ACTION_END, 3)))
            .addAction(
                if (snapshot.muted) R.drawable.zi_microphone else R.drawable.zi_microphone_off,
                if (snapshot.muted) "Unmute" else "Mute",
                pending(ACTION_MUTE, 2),
            )
        val since = snapshot.activeSinceWall
        if (since != null) {
            builder.setWhen(since).setShowWhen(true).setUsesChronometer(true)
        } else {
            builder.setShowWhen(false)
        }
        return builder.build()
    }
}
