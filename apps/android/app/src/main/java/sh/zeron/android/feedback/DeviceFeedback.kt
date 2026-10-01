package sh.zeron.android.feedback

/** The phone engine's lifecycle as feedback sees it (the runtime's `RuntimeState`, minus the details). */
enum class EngineStage { Idle, Setup, Starting, Running, Stopped, Failed }

enum class EngineEvent {
    /** First-time setup (or setup after a reset) finished. */
    SetupDone,

    /** The engine stopped with an error. */
    Failed,
}

/**
 * One-shot engine events from successive states. The first state is a
 * baseline (opening the app on a failed engine does not chime for what
 * already happened); setup progress is silent, setup completing is an event
 * once, and so is a fresh failure.
 */
class EngineTransitions {
    private var previous: EngineStage? = null

    fun reset() {
        previous = null
    }

    fun observe(next: EngineStage): EngineEvent? {
        val before = previous
        previous = next
        if (before == null || before == next) return null
        return when {
            next == EngineStage.Failed -> EngineEvent.Failed
            before == EngineStage.Setup && (next == EngineStage.Starting || next == EngineStage.Running) -> EngineEvent.SetupDone
            else -> null
        }
    }
}

enum class TransferEvent {
    /** Another device asks to send this phone files and waits for an answer. */
    Asked,

    /** Files from another device arrived. */
    Received,

    /** This phone's outgoing transfer completed. */
    Sent,

    /** A transfer failed, or the other side declined or cancelled it. */
    Failed,
}

/**
 * Feedback for device-level events that are not a response to a tap: engine
 * lifecycle and file transfers. In front they play the in-app cue and haptic
 * (the notifications that go with transfers are posted silently there); behind,
 * the transfer notification's channel sounds the same cue (`Notifier`), and an
 * engine event is not announced at all (nobody is there to hear it).
 * The same event for the same subject within [DEDUPE_MS] is dropped.
 */
class DeviceFeedbackPolicy(
    private val feedback: Feedback,
    private val foreground: () -> Boolean,
    private val clock: () -> Long,
) {
    private val last = HashMap<Pair<String, Any>, Long>()

    private fun fresh(subject: String, event: Any): Boolean {
        val now = clock()
        val key = subject to event
        if (now - (last[key] ?: Long.MIN_VALUE / 2) < DEDUPE_MS) return false
        last[key] = now
        return true
    }

    fun transfer(id: String, event: TransferEvent) {
        if (!fresh(id, event) || !foreground()) return
        when (event) {
            TransferEvent.Asked -> feedback.both(Haptic.Attention, Cue.Request)
            TransferEvent.Received, TransferEvent.Sent -> feedback.both(Haptic.Success, Cue.UploadReady)
            TransferEvent.Failed -> feedback.both(Haptic.Error, Cue.Attention)
        }
    }

    fun engine(event: EngineEvent) {
        if (!fresh("engine", event) || !foreground()) return
        when (event) {
            EngineEvent.SetupDone -> feedback.both(Haptic.Success, Cue.UploadReady)
            EngineEvent.Failed -> feedback.both(Haptic.Error, Cue.Error)
        }
    }

    companion object {
        const val DEDUPE_MS = 2_000L
    }
}
