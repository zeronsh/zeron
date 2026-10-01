package sh.zeron.android.feedback

import androidx.compose.runtime.staticCompositionLocalOf

/**
 * The app's touch vocabulary. Names describe the *moment*, not the vibration:
 * the engine behind [Feedback] decides how each one is felt on this device
 * (platform effects, composition primitives, or nothing at all).
 */
enum class Haptic {
    /** A light detent: slider steps, scrubbing through discrete items. */
    Tick,

    /** A choice was made: a chip, tab, menu item or list row. */
    Select,

    /** A switch or checkbox turned on / off. */
    ToggleOn,
    ToggleOff,

    /** A press that begins something: press-and-hold, a drag picked up. */
    Press,

    /** A user action was committed: send, create, save, start. */
    Confirm,

    /** Something finished well: a turn completed, a transfer arrived. */
    Success,

    /** Something needs the user: a question, an approval. */
    Attention,

    /** Something failed or was refused. */
    Error,

    /** A long press was recognised: a menu or selection started. */
    LongPress,

    /** A drag or swipe crossed its commit threshold. */
    Threshold,

    /** A weighty, destructive step: delete, uninstall, discard. */
    Heavy,

    /** A small pop: starred, pinned. */
    Pop,

    /**
     * Round 2 (thinking-power picker). A model's thinking-power detent: firmer
     * than [Tick], and `haptic(h, level)` ramps it from firm (0) to heavy (1).
     */
    EffortStep,

    /** Dragging the effort thumb past the end of its track: light rubber-band resistance, ramping with `level`. */
    Stretch,

    /** The thumb snapping back from a stretch: one firm thud. */
    Rebound,

    /** The highest thinking power was chosen: a rising surge of power. */
    Surge,

    /** The lowest thinking power was chosen: a quick light flick (fast and light). */
    Zip,

    /** Fast mode switched on: a ragged lightning crackle that builds to a click. */
    Lightning,

    /** Jumping to a provider on the picker's rail: a soft detent. */
    RailTick,
}

/**
 * Sound cues. The first group reuses the desktop's notification family
 * (`docs/sound-design`); the rest are interface cues synthesised in the same
 * "rounded pressure pulse" style (`scripts/generate-android-sounds.py`).
 */
enum class Cue {
    // Session events (desktop assets: done.wav, request.wav, attention.wav).
    Done,
    Request,
    Attention,

    // Desktop audition cues promoted for the phone.
    Send,
    Queued,
    UploadReady,
    Reconnected,
    Undo,

    // Interface.
    Tap,
    Select,
    ToggleOn,
    ToggleOff,
    Open,
    Close,

    /** A slider / stepper detent; `step` raises the pitch a little per position. */
    Detent,
    Star,
    Unstar,
    Pin,
    Archive,
    Delete,
    Copy,
    Error,
    Refresh,

    // Round 2: thinking power, fast mode, providers.
    /** The highest thinking power was chosen: a bright, rising swell with sparkle (power, intelligence). */
    Surge,

    /** The lowest thinking power was chosen: a quick airy flick (speed, lightness). */
    Zip,

    /** The effort thumb snapped back after being stretched past the end of its track. */
    Rebound,

    /** Fast mode on: a crackle of lightning with a bloom. */
    FastOn,

    /** Fast mode off: the charge draining away. */
    FastOff,

    // One short motif per provider on the picker's rail (see [Cue.forProvider]).
    ProviderClaude,
    ProviderCodex,
    ProviderCursor,
    ProviderDevin,
    ProviderGrok,
    ProviderHermes,
    ProviderPi,
    ProviderOpenCode,
    ProviderAntigravity,
    ProviderFavorites,
    ProviderOther,
    ;

    companion object {
        /** The rail motif for a harness id such as `claude-code`, `codex`, `opencode`; anything unknown gets [ProviderOther]. */
        fun forProvider(harness: String): Cue = when {
            harness.startsWith("claude") -> ProviderClaude
            harness.startsWith("codex") -> ProviderCodex
            harness.startsWith("cursor") -> ProviderCursor
            harness.startsWith("devin") -> ProviderDevin
            harness.startsWith("grok") -> ProviderGrok
            harness.startsWith("hermes") -> ProviderHermes
            harness == "pi" || harness.startsWith("pi-") -> ProviderPi
            harness.startsWith("opencode") -> ProviderOpenCode
            harness.startsWith("antigravity") -> ProviderAntigravity
            harness == "favorites" -> ProviderFavorites
            else -> ProviderOther
        }
    }
}

interface Feedback {
    fun haptic(haptic: Haptic)

    /**
     * [haptic] scaled by [level] (0..1) where the effect has a range: [Haptic.EffortStep] climbs from firm
     * to heavy, [Haptic.Stretch] resists harder the further the thumb is pulled. Engines that do not
     * scale simply play the plain haptic.
     */
    fun haptic(haptic: Haptic, level: Float) = haptic(haptic)

    /** [step] only matters to cues that climb a scale (see [Cue.Detent]). */
    fun cue(cue: Cue, step: Int = 0)

    /** A haptic and a sound for the same moment, the common case. */
    fun both(haptic: Haptic, cue: Cue, step: Int = 0) {
        cue(cue, step) // sound first: it is the channel perceived late (see docs/sound-design/android.md, Latency)
        haptic(haptic)
    }
}

/** Does nothing: previews, tests, and the default until the app installs the real one. */
object NoFeedback : Feedback {
    override fun haptic(haptic: Haptic) = Unit
    override fun cue(cue: Cue, step: Int) = Unit
}

val LocalFeedback = staticCompositionLocalOf<Feedback> { NoFeedback }
