package sh.zeron.android.feedback

/**
 * What the device is doing right now. Read per event, so implementations
 * cache (the real one answers from a snapshot refreshed by the screen
 * broadcasts and a short TTL, see `SystemEnvironment`): the gate sits on the
 * tap-to-sound path and must not make a binder call per event. It deliberately
 * knows nothing of the phone's own touch-sound, touch-feedback, ringer or Do
 * Not Disturb settings: the in-app switches are the single source of truth.
 */
interface FeedbackEnvironment {
    /** The app is in the foreground and its screen is on and interactive. */
    val active: Boolean

    val hasVibrator: Boolean
}

/** Why a haptic or cue did not play. Logged by debug builds. */
enum class Skipped(val label: String) {
    MasterOff("sounds & haptics master off"),
    HapticsOff("haptics switch off"),
    NoVibrator("no vibrator"),
    Inactive("app not active (background or screen off)"),
    SoundsOff("sounds switch off"),
    CategoryOff("category switch off"),
    Rate("rate limited"),
    Streams("too many streams"),
    NotLoaded("sound not loaded"),
}

sealed interface Decision {
    data object Play : Decision
    data class Skip(val why: Skipped) : Decision
}

/**
 * Decides whether a haptic or cue plays: the in-app switches, whether the app
 * is active, and rate limiting. Pure and clock-injected so its rules are unit
 * tested; the engine acts on the result.
 *
 * Rules, in order:
 *  - The **Sounds & haptics master** ([FeedbackSettings.master]) is absolute:
 *    off, nothing plays; on, only the rules below apply.
 *  - Haptics: the Haptics switch, a vibrator, an active app.
 *  - Sounds: the Sounds switch and the category switch, an active app.
 *  - Never consulted, on purpose: the phone's "Touch sounds" and "Touch
 *    feedback" settings, the ringer mode, Do Not Disturb. When the master is on
 *    the user has said "play", and the app does not second-guess it. (Android
 *    itself may still mute the audio stream: silent mode or a zero system
 *    volume are properties of the output, not decisions made here.)
 *  - Rate: the same haptic / cue never repeats inside its minimum gap, any
 *    two light events keep a short global gap, a burst of light events is
 *    capped per second, and sound streams are capped.
 */
class FeedbackGate(
    private val settings: () -> FeedbackSettings,
    private val env: FeedbackEnvironment,
    private val clock: () -> Long,
) {
    private val lastHaptic = HashMap<Haptic, Long>()
    private var lastAnyHaptic = Long.MIN_VALUE / 2
    private var lastAnyHapticPriority = 0
    private val lightHaptics = ArrayDeque<Long>()

    private val lastCue = HashMap<Cue, Long>()
    private var lastAnyCue = Long.MIN_VALUE / 2
    private var lastAnyCuePriority = 0
    private val streams = ArrayDeque<Pair<Long, Int>>() // end time, priority

    /** [preview]: the Settings page auditioning a feedback: the rate limits do not apply (a tap on a row is intent). */
    @Synchronized
    fun haptic(haptic: Haptic, preview: Boolean = false): Decision {
        val s = settings()
        if (!s.master) return skip(Skipped.MasterOff)
        if (!s.haptics) return skip(Skipped.HapticsOff)
        if (!env.hasVibrator) return skip(Skipped.NoVibrator)
        if (!env.active) return skip(Skipped.Inactive)
        if (preview) return Decision.Play
        val spec = HapticTable.spec(haptic)
        val now = clock()
        if (now - (lastHaptic[haptic] ?: Long.MIN_VALUE / 2) < spec.minGapMs) return skip(Skipped.Rate)
        if (spec.priority <= lastAnyHapticPriority && now - lastAnyHaptic < GLOBAL_HAPTIC_GAP_MS) return skip(Skipped.Rate)
        if (spec.priority == 0) {
            while (lightHaptics.isNotEmpty() && now - lightHaptics.first() > WINDOW_MS) lightHaptics.removeFirst()
            if (lightHaptics.size >= LIGHT_HAPTICS_PER_WINDOW) return skip(Skipped.Rate)
            lightHaptics.addLast(now)
        }
        lastHaptic[haptic] = now
        lastAnyHaptic = now
        lastAnyHapticPriority = spec.priority
        return Decision.Play
    }

    /** [preview]: as for [haptic], and the category switches do not apply (hearing a muted category's sound is the point of the row). */
    @Synchronized
    fun cue(cue: Cue, preview: Boolean = false): Decision {
        val s = settings()
        val spec = CueTable.spec(cue)
        if (!s.master) return skip(Skipped.MasterOff)
        if (!s.sounds) return skip(Skipped.SoundsOff)
        if (!preview && !s.allows(spec.category)) return skip(Skipped.CategoryOff)
        if (!env.active) return skip(Skipped.Inactive)
        if (preview) return Decision.Play
        val now = clock()
        if (now - (lastCue[cue] ?: Long.MIN_VALUE / 2) < spec.minGapMs) return skip(Skipped.Rate)
        if (spec.priority <= lastAnyCuePriority && now - lastAnyCue < GLOBAL_CUE_GAP_MS) return skip(Skipped.Rate)
        while (streams.isNotEmpty() && streams.first().first <= now) streams.removeFirst()
        if (streams.size >= MAX_STREAMS && streams.minOf { it.second } >= spec.priority) return skip(Skipped.Streams)
        streams.addLast(now + spec.approxMs to spec.priority)
        lastCue[cue] = now
        lastAnyCue = now
        lastAnyCuePriority = spec.priority
        return Decision.Play
    }

    private fun skip(why: Skipped): Decision = Decision.Skip(why)

    companion object {
        const val GLOBAL_HAPTIC_GAP_MS = 25L
        const val GLOBAL_CUE_GAP_MS = 20L
        const val WINDOW_MS = 1000L
        const val LIGHT_HAPTICS_PER_WINDOW = 12
        const val MAX_STREAMS = 4
    }
}
