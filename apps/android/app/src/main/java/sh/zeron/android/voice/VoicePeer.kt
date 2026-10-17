package sh.zeron.android.voice

import android.content.Context
import android.media.AudioAttributes
import android.media.AudioDeviceCallback
import android.media.AudioDeviceInfo
import android.media.AudioFocusRequest
import android.media.AudioManager
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.util.Log
import androidx.lifecycle.Lifecycle
import androidx.lifecycle.ProcessLifecycleOwner
import kotlinx.coroutines.delay
import kotlinx.coroutines.suspendCancellableCoroutine
import org.webrtc.AudioSource
import org.webrtc.AudioTrack
import org.webrtc.DataChannel
import org.webrtc.IceCandidate
import org.webrtc.MediaConstraints
import org.webrtc.MediaStream
import org.webrtc.PeerConnection
import org.webrtc.PeerConnectionFactory
import org.webrtc.RTCStatsReport
import org.webrtc.SdpObserver
import org.webrtc.SessionDescription
import org.webrtc.audio.JavaAudioDeviceModule
import java.lang.ref.WeakReference
import java.util.concurrent.Executors
import kotlin.coroutines.resume
import kotlin.coroutines.resumeWithException

/**
 * Native audio endpoint (iOS `CodexVoicePeer`). Credentials and tools stay on
 * the execution host. The ordered `oai-events` channel and gathered offer
 * mirror the pinned helper.
 *
 * Audio behaves like a phone call: loudspeaker while the phone is in hand,
 * the earpiece while it is held to the ear, and whatever headset or car the
 * user connected otherwise. The call survives the screen locking (a
 * microphone foreground service); losing audio focus to a phone call ends it.
 *
 * Main-thread only; WebRTC callbacks hop back to the main thread.
 */
internal class VoicePeer(
    context: Context,
    private val controller: VoiceController,
    private var speakerOn: Boolean,
) {
    class Failure(val kind: Kind) : Exception(kind.name) {
        enum class Kind { Closed, Unavailable, PermissionDenied, Timeout }
    }

    companion object {
        private const val TAG = "ZeronVoice"
        private const val MAX_SDP = 65_536
        private val main = Handler(Looper.getMainLooper())

        /** Peer connections close and free their native state off the main thread. */
        private val disposer = Executors.newSingleThreadExecutor { r -> Thread(r, "voice-rtc-dispose") }
        private var factory: PeerConnectionFactory? = null

        /** The peer that owns the audio device right now (ADM errors fail it). */
        private var owner: WeakReference<VoicePeer>? = null

        private val external = buildSet {
            add(AudioDeviceInfo.TYPE_WIRED_HEADSET)
            add(AudioDeviceInfo.TYPE_WIRED_HEADPHONES)
            add(AudioDeviceInfo.TYPE_USB_HEADSET)
            add(AudioDeviceInfo.TYPE_BLUETOOTH_SCO)
            add(AudioDeviceInfo.TYPE_HEARING_AID)
            if (Build.VERSION.SDK_INT >= 31) add(AudioDeviceInfo.TYPE_BLE_HEADSET)
        }

        /** One factory and audio device module for the process, like iOS. */
        fun factory(context: Context): PeerConnectionFactory {
            factory?.let { return it }
            val app = context.applicationContext
            PeerConnectionFactory.initialize(PeerConnectionFactory.InitializationOptions.builder(app).createInitializationOptions())
            fun failOwner(what: String, detail: String?) {
                Log.w(TAG, "audio device: $what $detail")
                main.post { owner?.get()?.fail() }
            }
            val adm = JavaAudioDeviceModule.builder(app)
                .setUseHardwareAcousticEchoCanceler(JavaAudioDeviceModule.isBuiltInAcousticEchoCancelerSupported())
                .setUseHardwareNoiseSuppressor(JavaAudioDeviceModule.isBuiltInNoiseSuppressorSupported())
                .setAudioRecordErrorCallback(object : JavaAudioDeviceModule.AudioRecordErrorCallback {
                    override fun onWebRtcAudioRecordInitError(message: String?) = failOwner("record init", message)
                    override fun onWebRtcAudioRecordStartError(code: JavaAudioDeviceModule.AudioRecordStartErrorCode?, message: String?) = failOwner("record start", message)
                    override fun onWebRtcAudioRecordError(message: String?) = failOwner("record", message)
                })
                .setAudioTrackErrorCallback(object : JavaAudioDeviceModule.AudioTrackErrorCallback {
                    override fun onWebRtcAudioTrackInitError(message: String?) = failOwner("track init", message)
                    override fun onWebRtcAudioTrackStartError(code: JavaAudioDeviceModule.AudioTrackStartErrorCode?, message: String?) = failOwner("track start", message)
                    override fun onWebRtcAudioTrackError(message: String?) = failOwner("track", message)
                })
                .createAudioDeviceModule()
            return PeerConnectionFactory.builder()
                .setAudioDeviceModule(adm)
                .createPeerConnectionFactory()
                .also { factory = it }
        }

        private fun foreground(): Boolean =
            ProcessLifecycleOwner.get().lifecycle.currentState.isAtLeast(Lifecycle.State.STARTED)
    }

    private val context = context.applicationContext
    private val audio = this.context.getSystemService(AudioManager::class.java)
    private var peer: PeerConnection? = null
    private var channel: DataChannel? = null
    private var track: AudioTrack? = null
    private var source: AudioSource? = null
    private var closed = false
    private var active = false
    private var nearEar = false
    private var focus: AudioFocusRequest? = null
    private var devices: AudioDeviceCallback? = null
    private var savedMode = AudioManager.MODE_NORMAL
    var onFailure: (() -> Unit)? = null
    var onRoute: ((VoiceRoute) -> Unit)? = null

    suspend fun prepare() {
        if (closed) throw Failure(Failure.Kind.Closed)
        if (!controller.requestMicrophone()) throw Failure(Failure.Kind.PermissionDenied)
        if (closed || !foreground()) throw Failure(Failure.Kind.Closed)
        // The call keeps its microphone with the screen off or the app behind.
        VoiceCallService.start(context)
        val factory = factory(context)
        val config = PeerConnection.RTCConfiguration(emptyList()).apply {
            sdpSemantics = PeerConnection.SdpSemantics.UNIFIED_PLAN
            bundlePolicy = PeerConnection.BundlePolicy.MAXBUNDLE
        }
        val connection = factory.createPeerConnection(config, observer) ?: throw Failure(Failure.Kind.Unavailable)
        peer = connection
        owner = WeakReference(this)
        // Manual audio (iOS `useManualAudio`): nothing records or plays until
        // the host confirms the current generation with the first SetMuted.
        connection.setAudioPlayout(false)
        connection.setAudioRecording(false)
        val source = factory.createAudioSource(MediaConstraints())
        this.source = source
        val track = factory.createAudioTrack("voice-audio", source)
        track.setEnabled(false)
        this.track = track
        connection.addTrack(track, listOf("voice"))
        val channel = connection.createDataChannel("oai-events", DataChannel.Init().apply { ordered = true })
            ?: throw Failure(Failure.Kind.Unavailable)
        channel.registerObserver(channelObserver)
        this.channel = channel
    }

    suspend fun offer(): String {
        val peer = peer ?: throw Failure(Failure.Kind.Closed)
        if (closed) throw Failure(Failure.Kind.Closed)
        val constraints = MediaConstraints().apply {
            mandatory.add(MediaConstraints.KeyValuePair("OfferToReceiveAudio", "true"))
        }
        val offer = suspendCancellableCoroutine { cont ->
            peer.createOffer(object : SdpCallback() {
                override fun onCreateSuccess(description: SessionDescription) {
                    if (cont.isActive) cont.resume(description)
                }

                override fun onCreateFailure(error: String?) {
                    if (cont.isActive) cont.resumeWithException(Failure(Failure.Kind.Unavailable))
                }
            }, constraints)
        }
        setDescription { peer.setLocalDescription(it, offer) }
        waitUntil { peer.iceGatheringState() == PeerConnection.IceGatheringState.COMPLETE }
        val sdp = peer.localDescription?.description ?: throw Failure(Failure.Kind.Unavailable)
        if (sdp.toByteArray().size > MAX_SDP) throw Failure(Failure.Kind.Unavailable)
        return sdp
    }

    suspend fun apply(answer: String) {
        val peer = peer
        if (peer == null || closed || answer.isEmpty() || answer.toByteArray().size > MAX_SDP) throw Failure(Failure.Kind.Closed)
        setDescription { peer.setRemoteDescription(it, SessionDescription(SessionDescription.Type.ANSWER, answer)) }
        waitUntil {
            peer.connectionState() == PeerConnection.PeerConnectionState.CONNECTED &&
                channel?.state() == DataChannel.State.OPEN
        }
    }

    fun setMuted(muted: Boolean) {
        val peer = peer
        if (closed || peer == null || peer.connectionState() != PeerConnection.PeerConnectionState.CONNECTED) {
            throw Failure(Failure.Kind.Closed)
        }
        // Called only after the host confirms the current generation.
        if (!active) activate(peer)
        track?.setEnabled(!muted)
    }

    /** The proximity sensor: held to the ear, play through the earpiece. */
    fun setNearEar(near: Boolean) {
        if (nearEar == near) return
        nearEar = near
        applyRoute()
    }

    fun setSpeaker(on: Boolean) {
        if (speakerOn == on) return
        speakerOn = on
        applyRoute()
    }

    private fun activate(peer: PeerConnection) {
        val audio = audio ?: throw Failure(Failure.Kind.Unavailable)
        val request = AudioFocusRequest.Builder(AudioManager.AUDIOFOCUS_GAIN)
            .setAudioAttributes(
                AudioAttributes.Builder()
                    .setUsage(AudioAttributes.USAGE_VOICE_COMMUNICATION)
                    .setContentType(AudioAttributes.CONTENT_TYPE_SPEECH)
                    .build(),
            )
            .setOnAudioFocusChangeListener({ change ->
                // A phone call or an alarm takes the audio: end, never resume.
                if (change == AudioManager.AUDIOFOCUS_LOSS || change == AudioManager.AUDIOFOCUS_LOSS_TRANSIENT) fail()
            }, main)
            .build()
        if (audio.requestAudioFocus(request) != AudioManager.AUDIOFOCUS_REQUEST_GRANTED) {
            throw Failure(Failure.Kind.Unavailable)
        }
        focus = request
        savedMode = audio.mode
        audio.mode = AudioManager.MODE_IN_COMMUNICATION
        val callback = object : AudioDeviceCallback() {
            // A headset arriving or leaving re-routes the call.
            override fun onAudioDevicesAdded(added: Array<out AudioDeviceInfo>?) = applyRoute()
            override fun onAudioDevicesRemoved(removed: Array<out AudioDeviceInfo>?) = applyRoute()
        }
        audio.registerAudioDeviceCallback(callback, main)
        devices = callback
        active = true
        applyRoute()
        peer.setAudioPlayout(true)
        peer.setAudioRecording(true)
    }

    /**
     * Loudspeaker in hand (unless the user picked the earpiece), earpiece at
     * the ear; a headset, Bluetooth or car route always wins.
     */
    private fun applyRoute() {
        val audio = audio ?: return
        val wantsEarpiece = nearEar || !speakerOn
        if (!active || closed) {
            onRoute?.invoke(if (wantsEarpiece) VoiceRoute.Earpiece else VoiceRoute.Speaker)
            return
        }
        val route: VoiceRoute
        if (Build.VERSION.SDK_INT >= 31) {
            val available = audio.availableCommunicationDevices
            val headset = available.firstOrNull { it.type in external }
            val builtIn = available.firstOrNull {
                it.type == if (wantsEarpiece) AudioDeviceInfo.TYPE_BUILTIN_EARPIECE else AudioDeviceInfo.TYPE_BUILTIN_SPEAKER
            }
            val target = headset ?: builtIn
            if (target != null && audio.communicationDevice?.id != target.id) {
                runCatching { audio.setCommunicationDevice(target) }
            }
            route = when {
                headset != null -> VoiceRoute.External
                target?.type == AudioDeviceInfo.TYPE_BUILTIN_SPEAKER -> VoiceRoute.Speaker
                else -> VoiceRoute.Earpiece
            }
        } else {
            val headset = audio.getDevices(AudioManager.GET_DEVICES_OUTPUTS).any { it.type in external }
            @Suppress("DEPRECATION")
            audio.isSpeakerphoneOn = !headset && !wantsEarpiece
            route = if (headset) VoiceRoute.External else if (wantsEarpiece) VoiceRoute.Earpiece else VoiceRoute.Speaker
        }
        onRoute?.invoke(route)
    }

    /** Normalized microphone and speaker peaks, as 0…65535. */
    suspend fun levels(): Pair<Int, Int> {
        val peer = peer ?: throw Failure(Failure.Kind.Closed)
        val state = peer.connectionState()
        if (closed || state == PeerConnection.PeerConnectionState.FAILED || state == PeerConnection.PeerConnectionState.CLOSED) {
            throw Failure(Failure.Kind.Closed)
        }
        // Nobody sees the orb with the app behind; skip the statistics walk.
        if (!foreground()) return 0 to 0
        val report: RTCStatsReport = suspendCancellableCoroutine { cont ->
            peer.getStats { report -> if (cont.isActive) cont.resume(report) }
        }
        var microphone = 0.0
        var speaker = 0.0
        for (stat in report.statsMap.values) {
            val level = (stat.members["audioLevel"] as? Number)?.toDouble() ?: continue
            if (stat.type == "media-source") microphone = maxOf(microphone, level)
            if (stat.type == "inbound-rtp") speaker = maxOf(speaker, level)
        }
        return (microphone.coerceIn(0.0, 1.0) * 65535).toInt() to (speaker.coerceIn(0.0, 1.0) * 65535).toInt()
    }

    fun close() {
        if (closed) return
        closed = true
        if (owner?.get() === this) owner = null
        track?.setEnabled(false)
        val peer = peer
        val channel = channel
        val source = source
        this.peer = null
        this.channel = null
        this.track = null
        this.source = null
        channel?.unregisterObserver()
        peer?.setAudioRecording(false)
        peer?.setAudioPlayout(false)
        audio?.let { audio ->
            devices?.let(audio::unregisterAudioDeviceCallback)
            if (active) {
                if (Build.VERSION.SDK_INT >= 31) {
                    audio.clearCommunicationDevice()
                } else {
                    @Suppress("DEPRECATION")
                    audio.isSpeakerphoneOn = false
                }
                audio.mode = savedMode
            }
            focus?.let(audio::abandonAudioFocusRequest)
        }
        devices = null
        focus = null
        active = false
        disposer.execute {
            runCatching {
                channel?.close()
                peer?.close()
                channel?.dispose()
                peer?.dispose()
                source?.dispose()
            }.onFailure { Log.w(TAG, "peer dispose", it) }
        }
    }

    private fun fail() {
        if (closed) return
        close()
        onFailure?.invoke()
    }

    private suspend fun waitUntil(ready: () -> Boolean) {
        val deadline = System.nanoTime() + 25_000_000_000L
        while (true) {
            if (closed) throw Failure(Failure.Kind.Closed)
            if (ready()) break
            if (System.nanoTime() > deadline) throw Failure(Failure.Kind.Timeout)
            delay(20)
        }
        if (closed) throw Failure(Failure.Kind.Closed)
    }

    private suspend fun setDescription(start: (SdpObserver) -> Unit) = suspendCancellableCoroutine { cont ->
        start(object : SdpCallback() {
            override fun onSetSuccess() {
                if (cont.isActive) cont.resume(Unit)
            }

            override fun onSetFailure(error: String?) {
                if (cont.isActive) cont.resumeWithException(Failure(Failure.Kind.Unavailable))
            }
        })
    }

    private val observer = object : PeerConnection.Observer {
        override fun onSignalingChange(state: PeerConnection.SignalingState?) = Unit
        override fun onIceConnectionChange(state: PeerConnection.IceConnectionState?) {
            if (state == PeerConnection.IceConnectionState.FAILED ||
                state == PeerConnection.IceConnectionState.CLOSED ||
                state == PeerConnection.IceConnectionState.DISCONNECTED
            ) {
                main.post { fail() }
            }
        }

        override fun onIceConnectionReceivingChange(receiving: Boolean) = Unit
        override fun onIceGatheringChange(state: PeerConnection.IceGatheringState?) = Unit
        override fun onIceCandidate(candidate: IceCandidate?) = Unit
        override fun onIceCandidatesRemoved(candidates: Array<out IceCandidate>?) = Unit
        override fun onAddStream(stream: MediaStream?) = Unit
        override fun onRemoveStream(stream: MediaStream?) = Unit

        // Only our own ordered channel carries events.
        override fun onDataChannel(channel: DataChannel?) {
            channel?.close()
        }

        override fun onRenegotiationNeeded() = Unit
    }

    private val channelObserver = object : DataChannel.Observer {
        override fun onBufferedAmountChange(previous: Long) = Unit
        override fun onStateChange() {
            main.post {
                val state = channel?.state() ?: return@post
                if (state == DataChannel.State.CLOSING || state == DataChannel.State.CLOSED) fail()
            }
        }

        // The host owns canonical transcripts and handoffs. Do not persist a second copy.
        override fun onMessage(buffer: DataChannel.Buffer?) = Unit
    }

    private abstract class SdpCallback : SdpObserver {
        override fun onCreateSuccess(description: SessionDescription) = Unit
        override fun onSetSuccess() = Unit
        override fun onCreateFailure(error: String?) = Unit
        override fun onSetFailure(error: String?) = Unit
    }
}
