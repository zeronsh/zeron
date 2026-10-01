package sh.zeron.android.feedback

/**
 * Anything pressable answers with a default tap, unless the moment already
 * has its own feedback. The default is deferred a few ms; if a haptic (or a
 * cue) was requested explicitly around the same press, that one is the
 * answer and the default stays out of its way. Haptic and cue are claimed
 * separately, so navigating (an explicit Open cue) still keeps the tap's
 * haptic.
 */
class ClaimTracker(private val clock: () -> Long) {
    private var hapticAt = Long.MIN_VALUE / 2
    private var cueAt = Long.MIN_VALUE / 2
    private var quietAt = Long.MIN_VALUE / 2

    fun claimHaptic() {
        hapticAt = clock()
    }

    fun claimCue() {
        cueAt = clock()
    }

    /** Was an explicit haptic requested from [windowMs] before [releasedAt] until now? */
    fun hapticClaimed(releasedAt: Long): Boolean = hapticAt >= releasedAt - WINDOW_BEFORE_MS

    fun cueClaimed(releasedAt: Long): Boolean = cueAt >= releasedAt - WINDOW_BEFORE_MS

    /** Was any cue requested in the last [windowMs]? */
    fun cueWithin(windowMs: Long): Boolean = clock() - maxOf(cueAt, quietAt) < windowMs

    /** A menu item was chosen: the menu closing right after is the item's doing, not a sound of its own. */
    fun quiet() {
        quietAt = clock()
    }

    companion object {
        /** Explicit feedback this far before the release still counts (the click handler runs just ahead of it). */
        const val WINDOW_BEFORE_MS = 100L

        /** How long the default waits for an explicit answer. */
        const val DEFER_MS = 40L

        /** A press held this long was a long press, which has its own feedback. */
        const val LONG_PRESS_MS = 350L
    }
}

/** What the press-answering Indication talks to; the real engine implements it next to [Feedback]. */
interface TapFeedback {
    /** A press was released inside its target: answer with the default tap unless the moment claims it. */
    fun defaultTap(heldMs: Long)

    /**
     * [cue] unless some cue was requested in the last [windowMs]: a dialog or
     * menu closing right after the action it hosted already has that
     * action's sound, and the close would only muddy it.
     */
    fun cueUnlessRecent(cue: Cue, windowMs: Long = 250)

    /** A menu item was chosen: the close that follows stays silent. */
    fun quietClose()
}
