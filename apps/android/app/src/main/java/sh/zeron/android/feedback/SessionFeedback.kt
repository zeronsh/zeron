package sh.zeron.android.feedback

/** The phases of a session that matter to feedback (the core's `ChatIndicator`, minus presentation). */
enum class Phase { Working, AwaitingInput, Errored, Completed, Idle }

enum class SessionEvent { Done, NeedsInput, Failed }

enum class LinkEvent { Lost, Restored }

/**
 * Turns successive snapshots into events, once each. The first snapshot is a
 * baseline (opening the app never chimes for what already happened), and
 * [reset] starts over on sign-in / sign-out.
 */
class SessionTransitions {
    private var previous: Map<String, Phase>? = null

    fun reset() {
        previous = null
    }

    fun observe(next: Map<String, Phase>): List<Pair<String, SessionEvent>> {
        val prev = previous
        previous = next
        if (prev == null) return emptyList()
        val events = ArrayList<Pair<String, SessionEvent>>()
        for ((id, now) in next) {
            val before = prev[id] ?: continue // new rows are a baseline too
            if (before == now) continue
            when {
                now == Phase.Errored -> events += id to SessionEvent.Failed
                now == Phase.AwaitingInput -> events += id to SessionEvent.NeedsInput
                before == Phase.Working && (now == Phase.Completed || now == Phase.Idle) -> events += id to SessionEvent.Done
            }
        }
        return events
    }
}

/**
 * The connection announces itself only when it matters: a loss while a turn
 * is running is an event (the phone cannot tell you the answer arrived); the
 * restore is announced only if the loss was.
 */
class LinkTransitions {
    private var degraded: Boolean? = null
    private var announced = false

    fun reset() {
        degraded = null
        announced = false
    }

    /** [degraded]: offline or reconnecting; [turnRunning]: any session working. */
    fun observe(degraded: Boolean, turnRunning: Boolean): LinkEvent? {
        val was = this.degraded
        this.degraded = degraded
        if (was == null) return null // booting into an outage is seeded silently
        return when {
            degraded && !was && turnRunning -> LinkEvent.Lost.also { announced = true }
            !degraded && was && announced -> LinkEvent.Restored.also { announced = false }
            !degraded -> null.also { announced = false }
            else -> null
        }
    }
}

/** Where an event goes when the app is not in front. */
fun interface SessionAlerts {
    fun alert(chatId: String, event: SessionEvent)
}

/**
 * The policy that maps events to feedback.
 *
 *  - App in the foreground: the in-app cue and haptic (Done + Success, Request +
 *    Attention, Attention + Error; link lost / restored).
 *  - Backgrounded: a notification whose channel sound and vibration are the
 *    same cues; nothing plays in-app.
 *  - Never twice: the same event for the same session inside [DEDUPE_MS] is
 *    dropped, and a Done right after the user stopped that session is not a
 *    completion.
 */
class SessionFeedbackPolicy(
    private val feedback: Feedback,
    private val alerts: SessionAlerts,
    private val foreground: () -> Boolean,
    private val clock: () -> Long,
) {
    private val last = HashMap<Pair<String, SessionEvent>, Long>()
    private val interrupted = HashMap<String, Long>()

    /** The user stopped [chatId]: the idle that follows is not a completion. */
    fun interrupted(chatId: String) {
        interrupted[chatId] = clock()
    }

    fun session(chatId: String, event: SessionEvent) {
        val now = clock()
        if (event == SessionEvent.Done && now - (interrupted[chatId] ?: Long.MIN_VALUE / 2) < INTERRUPT_MS) return
        val key = chatId to event
        if (now - (last[key] ?: Long.MIN_VALUE / 2) < DEDUPE_MS) return
        last[key] = now
        if (!foreground()) {
            alerts.alert(chatId, event)
            return
        }
        when (event) {
            SessionEvent.Done -> feedback.both(Haptic.Success, Cue.Done)
            SessionEvent.NeedsInput -> feedback.both(Haptic.Attention, Cue.Request)
            SessionEvent.Failed -> feedback.both(Haptic.Error, Cue.Attention)
        }
    }

    fun link(event: LinkEvent) {
        if (!foreground()) return
        when (event) {
            LinkEvent.Lost -> feedback.both(Haptic.Attention, Cue.Attention)
            LinkEvent.Restored -> feedback.both(Haptic.Confirm, Cue.Reconnected)
        }
    }

    companion object {
        const val DEDUPE_MS = 2_000L
        const val INTERRUPT_MS = 6_000L
    }
}
