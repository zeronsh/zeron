package sh.zeron.android.voice

import android.Manifest
import android.content.Context
import android.content.Intent
import android.content.pm.ApplicationInfo
import android.content.pm.PackageManager
import android.hardware.Sensor
import android.hardware.SensorEvent
import android.hardware.SensorEventListener
import android.hardware.SensorManager
import android.media.AudioAttributes
import android.media.AudioManager
import android.media.SoundPool
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.PowerManager
import android.os.SystemClock
import android.os.VibrationEffect
import android.os.Vibrator
import android.os.VibratorManager
import android.util.Log
import androidx.core.content.ContextCompat
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.Flow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharingStarted
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.map
import kotlinx.coroutines.flow.receiveAsFlow
import kotlinx.coroutines.flow.stateIn
import kotlinx.coroutines.flow.update
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import sh.zeron.android.R
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.VoiceCall
import uniffi.zeron_core.VoiceCallPhase
import uniffi.zeron_core.VoiceCallState
import uniffi.zeron_core.VoiceCallWork
import uniffi.zeron_core.VoiceEndReason
import uniffi.zeron_core.VoiceOrb
import uniffi.zeron_core.VoiceSessionListener
import uniffi.zeron_core.VoiceSpeaker
import uniffi.zeron_core.WorkspaceSnapshot
import uniffi.zeron_core.defaultVoiceStyles
import uniffi.zeron_core.voiceHostCapability
import java.lang.ref.WeakReference
import java.util.Locale
import kotlin.math.abs
import kotlin.math.max
import kotlin.math.sin

/** Where the call's audio plays right now. */
enum class VoiceRoute { Speaker, Earpiece, External }

/** One immutable picture of the voice call, for Compose and the notification. */
data class VoiceSnapshot(
    /** A call is placed (or the scripted preview runs). */
    val live: Boolean = false,
    val state: VoiceCallState? = null,
    /** Why the last call ended on its own; cleared by the next start. */
    val endReason: VoiceEndReason? = null,
    val muted: Boolean = false,
    val hostName: String = "",
    /** `SystemClock.elapsedRealtime()` when the call went live. */
    val activeSince: Long? = null,
    /** Wall clock of the same moment (the notification's chronometer). */
    val activeSinceWall: Long? = null,
    /** Styles the last host offered. */
    val styles: List<String> = emptyList(),
    /** Loudspeaker while in hand (the default); off plays the earpiece. */
    val speakerOn: Boolean = true,
    val route: VoiceRoute = VoiceRoute.Speaker,
    val previewing: Boolean = false,
) {
    val active: Boolean get() = state?.phase == VoiceCallPhase.ACTIVE
    val orb: VoiceOrb get() = state?.orb ?: if (live) VoiceOrb.CONNECTING else VoiceOrb.IDLE
    val awaitingInput: Boolean get() = state?.work == VoiceCallWork.AWAITING_INPUT
    val chatId: String? get() = state?.chatId

    /** `m:ss`, or `h:mm:ss` past the hour (desktop `format_elapsed`). */
    fun elapsedText(now: Long = SystemClock.elapsedRealtime()): String? {
        val since = activeSince ?: return null
        val seconds = max(0L, (now - since) / 1000)
        val h = seconds / 3600
        val m = seconds / 60 % 60
        val s = seconds % 60
        return if (h > 0) String.format(Locale.US, "%d:%02d:%02d", h, m, s) else String.format(Locale.US, "%d:%02d", m, s)
    }

    /** One line for what the call is doing now. */
    val statusText: String
        get() {
            val state = state ?: return if (live) "Calling $hostName…" else "Codex voice"
            if (state.phase == VoiceCallPhase.CONNECTING) return "Connecting to $hostName…"
            if (state.phase == VoiceCallPhase.ENDING) return "Ending…"
            return when (state.orb) {
                VoiceOrb.SPEAKING -> "Speaking"
                VoiceOrb.AWAITING_INPUT -> "Codex needs your answer"
                VoiceOrb.WORKING -> "Working on it…"
                VoiceOrb.MUTED -> "Muted"
                else -> "Listening"
            }
        }

    fun message(reason: VoiceEndReason): String =
        VoiceController.message(reason, hostName.ifEmpty { "the selected device" })
}

/** A call that failed while the stage wasn't showing: a snackbar explains it. */
data class VoiceNotice(val reason: VoiceEndReason, val message: String) {
    val action: String get() = if (reason == VoiceEndReason.MICROPHONE_DENIED) "Settings" else "Retry"
}

/**
 * The app-wide voice orchestrator call (iOS `RemoteVoiceController`). Audio
 * runs on this phone; Codex, its credentials and the orchestrator chat run on
 * a registered execution host (any device advertising [capability]). One call
 * at a time, no automatic reconnect: a dropped call explains itself and waits
 * for a tap.
 *
 * Main-thread only. Owned by `AppModel` (`val voice by lazy { VoiceController(app, client, workspace) }`).
 */
class VoiceController(
    context: Context,
    private val client: StateFlow<CoreClient?>,
    workspace: StateFlow<WorkspaceSnapshot?>,
) {
    companion object {
        private const val TAG = "ZeronVoice"
        val capability: String by lazy { voiceHostCapability() }

        /** Intent action (the call notification's tap) that opens the stage. */
        const val ACTION_OPEN_STAGE = "sh.zeron.android.voice.OPEN_STAGE"

        /** The live controller, for the call service and its notification actions. */
        @Volatile
        var current: WeakReference<VoiceController>? = null
            private set

        fun compatible(device: DeviceView): Boolean = device.isExecutionHost && capability in device.capabilities

        /** Whether [intent] asks to return to the call (notification tap, `zeron://voice`). */
        fun opensStage(intent: Intent?): Boolean {
            intent ?: return false
            if (intent.action == ACTION_OPEN_STAGE) return true
            val data = intent.data ?: return false
            return data.scheme == "zeron" && data.host == "voice"
        }

        fun message(reason: VoiceEndReason, host: String): String = when (reason) {
            VoiceEndReason.MICROPHONE_DENIED -> "Zeron needs microphone access to talk with Codex."
            VoiceEndReason.AUDIO_UNAVAILABLE -> "The audio connection closed. Start a new call to reconnect."
            VoiceEndReason.SIGN_IN_REQUIRED -> "Sign in to Codex with ChatGPT on $host."
            VoiceEndReason.USAGE_UNAVAILABLE -> "Codex usage is currently unavailable on $host."
            VoiceEndReason.BUSY -> "$host is already on a voice call."
            VoiceEndReason.HOST_UNAVAILABLE -> "$host isn't reachable right now."
            VoiceEndReason.HOST_INCOMPATIBLE -> "Update Zeron on $host and enable remote voice there."
            VoiceEndReason.CONNECTION_LOST -> "The call dropped. Start a new one to reconnect."
        }

        /** "alloy" → "Alloy"; null is the host's default. */
        fun styleLabel(style: String?): String =
            style?.replaceFirstChar { it.titlecase(Locale.getDefault()) } ?: "Codex default"
    }

    private val context = context.applicationContext
    private val main = Handler(Looper.getMainLooper())
    private val scope = MainScope()
    private val prefs = this.context.getSharedPreferences("voice", 0)
    private val debuggable = this.context.applicationInfo.flags and ApplicationInfo.FLAG_DEBUGGABLE != 0
    private val sounds = CallSounds(this.context)

    private var call: VoiceCall? = null
    private var media: VoiceMedia? = null
    private var generation = 0L
    private var stageViews = 0
    private var proximity: Proximity? = null

    /** Development override (`--ez remote-voice true`): offer voice before any host advertises it. */
    var forced: Boolean = false
        set(value) {
            field = value
            _forcedFlow.value = value
        }
    private val _forcedFlow = MutableStateFlow(false)

    private val _snapshot = MutableStateFlow(VoiceSnapshot(styles = runCatching { defaultVoiceStyles() }.getOrDefault(emptyList())))
    val snapshot: StateFlow<VoiceSnapshot> = _snapshot.asStateFlow()
    val live: Boolean get() = _snapshot.value.live

    /** Registered devices, refreshed with the workspace. */
    val devices: StateFlow<List<DeviceView>> = workspace
        .map { it?.devices.orEmpty() }
        .stateIn(scope, SharingStarted.Eagerly, workspace.value?.devices.orEmpty())

    private val _selectedHost = MutableStateFlow<String?>(null)
    /** The remembered host (per user and organization). */
    val selectedHost: StateFlow<String?> = _selectedHost.asStateFlow()
    private val _selectedStyle = MutableStateFlow<String?>(null)
    /** The remembered voice style; null is the host's default. */
    val selectedStyle: StateFlow<String?> = _selectedStyle.asStateFlow()

    private val _notice = MutableStateFlow<VoiceNotice?>(null)
    val notice: StateFlow<VoiceNotice?> = _notice.asStateFlow()

    private val _stageRequests = Channel<Unit>(Channel.CONFLATED)
    /**
     * Navigate to the call stage (a notification tap, a snackbar Retry, the
     * `voice-stage` launch flag). Held until the navigation host collects it.
     */
    val stageRequests: Flow<Unit> = _stageRequests.receiveAsFlow()

    private val _permissionRequest = MutableStateFlow<CompletableDeferred<Boolean>?>(null)
    /** A pending microphone prompt for [VoiceHost] to show. */
    val permissionRequest: StateFlow<CompletableDeferred<Boolean>?> = _permissionRequest.asStateFlow()

    init {
        current = WeakReference(this)
        // Remembered host and style belong to one user in one organization.
        scope.launch { client.collect { loadPreferences() } }
    }

    // ── Hosts and preferences ──────────────────────────────────────────────

    private fun allDevices(): List<DeviceView> =
        runCatching { client.value?.devices() }.getOrNull() ?: devices.value

    /**
     * Registered hosts that can take a call right now. The [forced] dev
     * override also offers execution hosts that don't advertise voice yet.
     */
    fun hosts(): List<DeviceView> = allDevices().filter { it.online && callable(it) }

    /** [compatible], or any execution host under the [forced] dev override. */
    fun callable(device: DeviceView): Boolean = compatible(device) || (forced && device.isExecutionHost)

    /** Voice controls appear once any registered host can run them. */
    val available: Boolean get() = forced || _snapshot.value.previewing || allDevices().any(::compatible)

    /** [available] against a collected device list, so composables recompose with it. */
    fun availableAmong(devices: List<DeviceView>): Boolean = forced || _snapshot.value.previewing || devices.any(::compatible)

    /** The host a plain tap calls: the chosen one, or the only one online. */
    fun startHost(): DeviceView? {
        val hosts = hosts()
        _selectedHost.value?.let { id -> hosts.firstOrNull { it.id == id }?.let { return it } }
        return hosts.singleOrNull()
    }

    private val preferenceKey: String
        get() {
            val c = client.value
            val user = runCatching { c?.userId() }.getOrNull() ?: "none"
            val org = runCatching { c?.orgId() }.getOrNull() ?: "none"
            return "remoteVoice.$user.$org"
        }

    private fun loadPreferences() {
        val key = preferenceKey
        _selectedHost.value = prefs.getString("$key.host", null)
        _selectedStyle.value = prefs.getString("$key.style", null)
    }

    /** Remember the host for the next call (not while one runs). */
    fun selectHost(id: String?) {
        if (live) return
        prefs.edit().putString("$preferenceKey.host", id).apply()
        _selectedHost.value = id
    }

    fun selectStyle(style: String?) {
        prefs.edit().putString("$preferenceKey.style", style).apply()
        _selectedStyle.value = style
    }

    // ── Call ───────────────────────────────────────────────────────────────

    /**
     * Opens the live call, or starts one on the default host. False when there
     * is no host to call; the caller offers the picker (iOS `openVoice`).
     */
    fun open(): Boolean = live || start()

    /**
     * Calls [host] (remembered for next time) or the default host. False when
     * there is no host to call; the caller offers the picker.
     */
    fun start(host: DeviceView? = null): Boolean {
        if (live) return false
        val client = client.value ?: return false
        val device = host ?: startHost() ?: return false
        if (host != null) selectHost(device.id)
        generation++
        val generation = generation
        _notice.value = null
        val previous = _snapshot.value
        val media = VoiceMedia(context, this, speakerOn = previous.speakerOn)
        media.onFailure = { if (this.generation == generation) finish(VoiceEndReason.AUDIO_UNAVAILABLE) }
        media.onRoute = { route -> if (this.generation == generation) _snapshot.update { it.copy(route = route) } }
        val call = try {
            client.startVoice(device.id, _selectedStyle.value, media, Bridge(this, generation))
        } catch (e: Exception) {
            Log.w(TAG, "startVoice failed", e)
            media.close()
            return false
        }
        media.attach(call)
        this.call = call
        this.media = media
        _snapshot.value = VoiceSnapshot(
            live = true,
            hostName = device.name,
            styles = previous.styles,
            speakerOn = previous.speakerOn,
            route = if (previous.speakerOn) VoiceRoute.Speaker else VoiceRoute.Earpiece,
        )
        haptic(VibrationEffect.EFFECT_CLICK)
        // The call sounds as it is placed, well before the microphone opens.
        sounds.play(CallSounds.Sound.Start)
        return true
    }

    fun toggleMute() {
        val s = _snapshot.value
        if (!s.live) return
        val muted = !s.muted
        media?.muteLocally(muted)
        call?.let { c -> runCatching { c.setMuted(muted) } }
        _snapshot.update { it.copy(muted = muted, state = if (it.previewing) it.state?.copy(muted = muted) else it.state) }
        VoiceCallService.refresh(context)
    }

    /** Loudspeaker ↔ earpiece. A headset or car always wins while connected. */
    fun toggleSpeaker() {
        val on = !_snapshot.value.speakerOn
        media?.setSpeaker(on)
        _snapshot.update {
            it.copy(speakerOn = on, route = if (it.route == VoiceRoute.External && media != null) it.route else if (on) VoiceRoute.Speaker else VoiceRoute.Earpiece)
        }
    }

    /** The user hangs up. Local capture stops before any remote control. */
    fun stop() {
        if (!live) return
        haptic(VibrationEffect.EFFECT_HEAVY_CLICK)
        finish(null)
    }

    fun dismissEndReason() {
        if (_snapshot.value.endReason == null) return
        _snapshot.update { it.copy(endReason = null) }
    }

    fun dismissNotice() {
        _notice.value = null
    }

    /** The snackbar's action: open Settings for the microphone, or call again. */
    fun noticeAction(openSettings: () -> Unit) {
        val notice = _notice.value ?: return
        _notice.value = null
        if (notice.reason == VoiceEndReason.MICROPHONE_DENIED) openSettings() else if (start()) requestStage()
    }

    fun requestStage() {
        _stageRequests.trySend(Unit)
    }

    /** The stage reports itself so failures explain themselves there, not in a snackbar. */
    internal fun stageShown(shown: Boolean) {
        stageViews = max(0, stageViews + if (shown) 1 else -1)
        // A call that failed while the stage was still on its way: the stage explains it.
        val notice = _notice.value
        if (shown && notice != null && !live) {
            _notice.value = null
            _snapshot.update { it.copy(endReason = notice.reason) }
        }
    }

    private fun finish(reason: VoiceEndReason?) {
        stopPreview()
        generation++
        val s = _snapshot.value
        // A hang-up or a connected call ending sounds; a failure to connect only explains itself.
        val sounds = call != null && (s.activeSince != null || reason == null)
        media?.close()
        call?.let { c ->
            runCatching { c.stop() }
            runCatching { c.close() }
        }
        // After close: the call's audio no longer holds the communication mode.
        if (sounds) this.sounds.play(CallSounds.Sound.End)
        call = null
        media = null
        setProximityMonitoring(false)
        VoiceCallService.stop(context)
        var shown = reason
        if (reason != null) {
            haptic(VibrationEffect.EFFECT_DOUBLE_CLICK)
            // The stage explains a failure itself; elsewhere a snackbar does.
            if (stageViews == 0) {
                _notice.value = VoiceNotice(reason, s.message(reason))
                shown = null
            }
        }
        _snapshot.update {
            it.copy(live = false, state = null, activeSince = null, activeSinceWall = null, endReason = shown, previewing = false)
        }
    }

    internal fun receive(state: VoiceCallState, generation: Long) {
        if (generation != this.generation || !live) return
        val s = _snapshot.value
        val becameActive = state.phase == VoiceCallPhase.ACTIVE && s.state?.phase != VoiceCallPhase.ACTIVE
        _snapshot.update {
            it.copy(
                state = state,
                styles = state.voices.ifEmpty { it.styles },
                activeSince = if (becameActive) SystemClock.elapsedRealtime() else it.activeSince,
                activeSinceWall = if (becameActive) System.currentTimeMillis() else it.activeSinceWall,
            )
        }
        if (becameActive) {
            setProximityMonitoring(true)
            haptic(VibrationEffect.EFFECT_TICK)
        }
        VoiceCallService.refresh(context)
    }

    internal fun closed(reason: VoiceEndReason?, generation: Long) {
        if (generation != this.generation || !live) return
        finish(reason)
    }

    // ── Microphone permission ──────────────────────────────────────────────

    fun hasMicrophone(): Boolean =
        ContextCompat.checkSelfPermission(context, Manifest.permission.RECORD_AUDIO) == PackageManager.PERMISSION_GRANTED

    /**
     * Asks for the microphone through [VoiceHost] (the call's Prepare step,
     * like iOS asking inside the call). False when denied or nobody can ask.
     */
    internal suspend fun requestMicrophone(): Boolean {
        if (hasMicrophone()) return true
        val request = CompletableDeferred<Boolean>()
        _permissionRequest.value?.complete(false)
        _permissionRequest.value = request
        // The core gives a media step 30 s; answer before it gives up.
        val granted = withTimeoutOrNull(28_000) { request.await() } ?: false
        if (_permissionRequest.value === request) _permissionRequest.value = null
        return granted && hasMicrophone()
    }

    internal fun completePermission(granted: Boolean) {
        _permissionRequest.value?.complete(granted)
        _permissionRequest.value = null
    }

    // ── Proximity ──────────────────────────────────────────────────────────

    /**
     * Held to the ear the screen goes dark and audio moves to the earpiece; in
     * hand it plays on the loudspeaker. No-op on devices without a sensor.
     */
    private fun setProximityMonitoring(on: Boolean) {
        if (on && proximity == null && !_snapshot.value.previewing) {
            proximity = Proximity(context) { near -> media?.setNearEar(near) }.also { it.start() }
        } else if (!on) {
            proximity?.stop()
            proximity = null
        }
    }

    private fun haptic(effect: Int) {
        runCatching {
            val vibrator = if (Build.VERSION.SDK_INT >= 31) {
                context.getSystemService(VibratorManager::class.java)?.defaultVibrator
            } else {
                @Suppress("DEPRECATION")
                context.getSystemService(Vibrator::class.java)
            }
            vibrator?.vibrate(VibrationEffect.createPredefined(effect))
        }
    }

    // ── Preview ────────────────────────────────────────────────────────────

    private var previewTick: Runnable? = null

    /**
     * `--ez voice-preview true`: a scripted call for screenshots and UI work —
     * the orb walks through listening, speaking, working and awaiting input
     * with synthetic levels. No audio, host or relay is involved. Debuggable
     * builds only.
     */
    fun preview(): Boolean {
        if (!debuggable || live) return false
        val script = listOf(
            PreviewTurn(VoiceOrb.LISTENING, VoiceCallWork.IDLE, "Open a chat on the Mac and ask Claude to fix the failing sync test", VoiceSpeaker.USER),
            PreviewTurn(VoiceOrb.SPEAKING, VoiceCallWork.IDLE, "On it. I'm starting a Claude session in the comet project to look at the sync test.", null),
            PreviewTurn(VoiceOrb.WORKING, VoiceCallWork.WORKING, "On it. I'm starting a Claude session in the comet project to look at the sync test.", VoiceSpeaker.ASSISTANT),
            PreviewTurn(VoiceOrb.AWAITING_INPUT, VoiceCallWork.AWAITING_INPUT, "Claude wants to know whether to update the fixture or the parser.", VoiceSpeaker.ASSISTANT),
        )
        generation++
        _notice.value = null
        _snapshot.update {
            it.copy(
                live = true,
                previewing = true,
                hostName = "Fedora",
                endReason = null,
                muted = false,
                activeSince = SystemClock.elapsedRealtime() - 83_000,
                activeSinceWall = System.currentTimeMillis() - 83_000,
            )
        }
        var step = 0
        val tick = object : Runnable {
            override fun run() {
                val turn = (step / 40) % script.size
                val t = script[turn]
                val wave = abs(sin(step / 2.3)).toFloat()
                // Words stream in, about three per second, like a live partial.
                val words = t.caption.split(" ")
                val streamed = words.take(1 + (step % 40) / 3).joinToString(" ")
                val muted = _snapshot.value.muted
                val state = VoiceCallState(
                    phase = VoiceCallPhase.ACTIVE,
                    orb = if (muted && t.orb == VoiceOrb.LISTENING) VoiceOrb.MUTED else t.orb,
                    chatId = "voice-orchestrator-preview",
                    work = t.work,
                    muted = muted,
                    speaking = t.orb == VoiceOrb.SPEAKING,
                    caption = streamed,
                    captionSpeaker = t.speaker,
                    captionItem = "preview-$turn",
                    microphone = if (t.orb == VoiceOrb.LISTENING && !muted) wave else 0f,
                    speaker = if (t.orb == VoiceOrb.SPEAKING) wave else 0f,
                    voices = emptyList(),
                )
                _snapshot.update { it.copy(state = state) }
                step++
                main.postDelayed(this, 100)
            }
        }
        previewTick = tick
        tick.run()
        // The call notification too, once the microphone is granted (a microphone service needs it).
        if (hasMicrophone()) VoiceCallService.start(context)
        return true
    }

    /**
     * Debug (`--ez voice-selftest true`): runs this phone's half of a call
     * without a host — the microphone prompt, the WebRTC factory, the peer,
     * its audio track and `oai-events` channel, and a gathered offer — then
     * closes it and logs the outcome under the `ZeronVoice` tag.
     */
    fun selfTest() {
        if (!debuggable || live) return
        scope.launch {
            val peer = VoicePeer(context, this@VoiceController, speakerOn = true)
            val started = SystemClock.elapsedRealtime()
            val result = try {
                peer.prepare()
                Log.i(TAG, "self-test: prepared (factory, peer connection, audio track, data channel)")
                val sdp = peer.offer()
                "ok: offer ${sdp.length} bytes in ${SystemClock.elapsedRealtime() - started} ms, " +
                    "audio=${"m=audio" in sdp}, datachannel=${"webrtc-datachannel" in sdp}, " +
                    "candidates=${sdp.lines().count { it.startsWith("a=candidate") }}"
            } catch (e: Exception) {
                "failed: $e"
            } finally {
                peer.close()
                VoiceCallService.stop(context)
            }
            Log.i(TAG, "self-test $result")
        }
    }

    private fun stopPreview() {
        previewTick?.let(main::removeCallbacks)
        previewTick = null
    }

    private data class PreviewTurn(val orb: VoiceOrb, val work: VoiceCallWork, val caption: String, val speaker: VoiceSpeaker?)

    /** Core callbacks arrive on Rust threads; hop to the main thread. */
    private class Bridge(controller: VoiceController, private val generation: Long) : VoiceSessionListener {
        private val controller = WeakReference(controller)
        private val main = Handler(Looper.getMainLooper())
        override fun onVoiceState(state: VoiceCallState) {
            main.post { controller.get()?.receive(state, generation) }
        }

        override fun onVoiceClosed(reason: VoiceEndReason?) {
            main.post { controller.get()?.closed(reason, generation) }
        }
    }
}

/**
 * The call's two sounds. They play as call signalling beside the call's own
 * audio and stay quiet when the ringer is silenced, like the phone's.
 */
internal class CallSounds(private val context: Context) {
    enum class Sound { Start, End }

    private val pool = SoundPool.Builder()
        .setMaxStreams(2)
        .setAudioAttributes(
            AudioAttributes.Builder()
                .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION_SIGNALLING)
                .setContentType(AudioAttributes.CONTENT_TYPE_SONIFICATION)
                .build(),
        )
        .build()
    private val ids = mapOf(
        Sound.Start to pool.load(context, R.raw.voice_start, 1),
        Sound.End to pool.load(context, R.raw.voice_end, 1),
    )
    private val loaded = mutableSetOf<Int>()
    private var pending: Int? = null

    init {
        pool.setOnLoadCompleteListener { _, id, status ->
            if (status != 0) return@setOnLoadCompleteListener
            loaded += id
            if (pending == id) {
                pending = null
                pool.play(id, 1f, 1f, 1, 0, 1f)
            }
        }
    }

    fun play(sound: Sound) {
        val audio = context.getSystemService(AudioManager::class.java)
        if (audio != null && audio.ringerMode != AudioManager.RINGER_MODE_NORMAL) return
        val id = ids[sound] ?: return
        if (id in loaded) pool.play(id, 1f, 1f, 1, 0, 1f) else pending = id
    }
}

/**
 * The proximity sensor and its screen-off wake lock, like a phone call: at
 * the ear the display sleeps and [onChange] moves audio to the earpiece.
 */
private class Proximity(context: Context, private val onChange: (Boolean) -> Unit) : SensorEventListener {
    private val sensors = context.getSystemService(SensorManager::class.java)
    private val sensor = sensors?.getDefaultSensor(Sensor.TYPE_PROXIMITY)
    private val lock: PowerManager.WakeLock? = context.getSystemService(PowerManager::class.java)?.let { power ->
        if (power.isWakeLockLevelSupported(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK)) {
            power.newWakeLock(PowerManager.PROXIMITY_SCREEN_OFF_WAKE_LOCK, "zeron:voice-proximity")
        } else {
            null
        }
    }
    private var near = false

    fun start() {
        sensor?.let { sensors?.registerListener(this, it, SensorManager.SENSOR_DELAY_NORMAL) }
        runCatching { if (lock?.isHeld == false) lock.acquire(4 * 60 * 60 * 1000L) }
    }

    fun stop() {
        sensors?.unregisterListener(this)
        runCatching { if (lock?.isHeld == true) lock.release(PowerManager.RELEASE_FLAG_WAIT_FOR_NO_PROXIMITY) }
        if (near) onChange(false)
        near = false
    }

    override fun onSensorChanged(event: SensorEvent) {
        val range = sensor?.maximumRange ?: return
        val value = event.values.firstOrNull() ?: return
        val now = value < range && value < 5f
        if (now != near) {
            near = now
            onChange(now)
        }
    }

    override fun onAccuracyChanged(sensor: Sensor?, accuracy: Int) = Unit
}
