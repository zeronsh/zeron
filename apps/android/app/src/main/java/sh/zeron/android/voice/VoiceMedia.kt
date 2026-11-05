package sh.zeron.android.voice

import android.content.Context
import android.os.Handler
import android.os.Looper
import kotlinx.coroutines.CancellationException
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.cancel
import kotlinx.coroutines.isActive
import kotlinx.coroutines.launch
import uniffi.zeron_core.VoiceCall
import uniffi.zeron_core.VoiceMediaFailure
import uniffi.zeron_core.VoiceMediaListener
import uniffi.zeron_core.VoiceMediaOperation
import uniffi.zeron_core.VoiceMediaRequest
import java.lang.ref.WeakReference

/**
 * A single-generation platform bridge (iOS `NativeVoiceMedia`). The Rust call
 * owns this callback object; its back-reference is weak so releasing the call
 * always closes native audio. Requests arrive on core threads and run on the
 * main thread; each completes exactly once through [VoiceCall.completeMedia].
 */
internal class VoiceMedia(
    context: Context,
    controller: VoiceController,
    speakerOn: Boolean,
) : VoiceMediaListener {
    private val main = Handler(Looper.getMainLooper())
    private val scope = MainScope()
    private val peer = VoicePeer(context, controller, speakerOn)
    private var call: WeakReference<VoiceCall>? = null
    private val operations = HashMap<ULong, Job>()
    private var closed = false
    private var activated = false
    var onFailure: (() -> Unit)? = null
    var onRoute: ((VoiceRoute) -> Unit)? = null

    init {
        peer.onFailure = {
            close()
            call?.get()?.let { runCatching { it.stop() } }
            onFailure?.invoke()
        }
        peer.onRoute = { onRoute?.invoke(it) }
    }

    fun attach(call: VoiceCall) {
        this.call = WeakReference(call)
    }

    override fun onRequest(request: VoiceMediaRequest) {
        main.post { perform(request) }
    }

    private fun perform(request: VoiceMediaRequest) {
        if (request.operation == VoiceMediaOperation.CLOSE) {
            close()
            return
        }
        if (closed) {
            complete(request.requestId, VoiceMediaFailure.UNAVAILABLE)
            return
        }
        if (operations.size >= 4) {
            close()
            call?.get()?.let { runCatching { it.stop() } }
            return
        }
        val id = request.requestId
        operations[id] = scope.launch {
            try {
                var sdp: String? = null
                var levels = 0 to 0
                when (request.operation) {
                    VoiceMediaOperation.PREPARE -> peer.prepare()
                    VoiceMediaOperation.OFFER -> sdp = peer.offer()
                    VoiceMediaOperation.APPLY_ANSWER -> peer.apply(request.sdp ?: throw VoicePeer.Failure(VoicePeer.Failure.Kind.Unavailable))
                    VoiceMediaOperation.SET_MUTED -> {
                        peer.setMuted(request.muted)
                        activated = true
                    }
                    VoiceMediaOperation.LEVELS -> levels = peer.levels()
                    VoiceMediaOperation.CLOSE -> close()
                }
                if (closed || !isActive) return@launch
                complete(id, null, sdp, levels.first, levels.second)
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (closed) return@launch
                val failure = if ((e as? VoicePeer.Failure)?.kind == VoicePeer.Failure.Kind.PermissionDenied) {
                    VoiceMediaFailure.PERMISSION_DENIED
                } else {
                    VoiceMediaFailure.UNAVAILABLE
                }
                complete(id, failure)
            } finally {
                operations.remove(id)
            }
        }
    }

    private fun complete(id: ULong, failure: VoiceMediaFailure?, sdp: String? = null, microphone: Int = 0, speaker: Int = 0) {
        val call = call?.get() ?: return
        // A late completion after the call was released simply misses.
        runCatching { call.completeMedia(id, failure, sdp, microphone.toUShort(), speaker.toUShort()) }
    }

    /** UI mute never waits for the Rust control queue or the relay. */
    fun muteLocally(muted: Boolean) {
        if (closed || !activated) return
        runCatching { peer.setMuted(muted) }
    }

    /** Proximity sensor state: the earpiece while the phone is at the ear. */
    fun setNearEar(near: Boolean) {
        if (!closed) peer.setNearEar(near)
    }

    fun setSpeaker(on: Boolean) {
        if (!closed) peer.setSpeaker(on)
    }

    fun close() {
        if (closed) return
        closed = true
        peer.close()
        for (operation in operations.values) operation.cancel()
        operations.clear()
        scope.cancel()
    }
}
