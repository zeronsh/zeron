package sh.zeron.android.feedback

import kotlin.math.pow

/**
 * One sound cue as the engine plays it: which file, how loud relative to the
 * set, how important it is when sounds compete, and which in-app switch
 * governs it. Files live in `res/raw` as `fx_<name>.wav`: the interface
 * cues and promoted desktop auditions are committed (see
 * `scripts/generate-android-sounds.py`), and the desktop's `done` /
 * `request` / `attention` are copied from `crates/ui/assets/sounds` by Gradle.
 */
data class CueSpec(
    val cue: Cue,
    /** `res/raw` resource name, without extension. */
    val resource: String,
    val category: CueCategory,
    /** Level trim against the rest of the set, 0..1 (before the user's volume). */
    val gain: Float,
    /** 0 interface, 1 action feedback, 2 session event, 3 alert. Higher wins when streams are scarce. */
    val priority: Int,
    /** The same cue is never stacked within this many ms. */
    val minGapMs: Long = 60,
    /** Approximate length, used to count streams still sounding. */
    val approxMs: Long = 120,
)

object CueTable {
    /**
     * Every `fx_*.wav` is generated this much hotter (+6.02 dB) than the level the app used to ship, to give the
     * volume slider its headroom: [android.media.SoundPool] volume cannot exceed 1.0, so "twice as loud at 100%"
     * has to live in the asset, and the default volume plays it at half (see [volume]).
     */
    const val ASSET_BOOST = 2f

    /** SoundPool volume (0..1) for [spec] at user gain [userGain] (0..[FeedbackSettings.MAX_GAIN]). */
    fun volume(spec: CueSpec, userGain: Float): Float = (userGain * spec.gain / ASSET_BOOST).coerceIn(0f, 1f)

    fun spec(cue: Cue): CueSpec = specs[cue.ordinal]

    private val specs: List<CueSpec> by lazy { Cue.entries.map(::build) }

    private fun build(cue: Cue): CueSpec = when (cue) {
        // Session events: the desktop's own chimes (2-4 dB above the interface set). The in-app copies are
        // the same sound mono, trimmed and ASSET_BOOST louder; `fx_done` & co. stay as the notification sounds.
        Cue.Done -> CueSpec(cue, "fx_chime_done", CueCategory.Completion, 1f, 2, 400, 520)
        Cue.Request -> CueSpec(cue, "fx_chime_request", CueCategory.Input, 1f, 2, 400, 600)
        Cue.Attention -> CueSpec(cue, "fx_chime_attention", CueCategory.Errors, 1f, 3, 400, 650)

        // Desktop audition cues promoted for the phone (about 2 dB hotter than the interface set, so trimmed).
        Cue.Send -> CueSpec(cue, "fx_send", CueCategory.Interface, 0.8f, 1, 120, 160)
        Cue.Queued -> CueSpec(cue, "fx_queued", CueCategory.Interface, 0.8f, 1, 120, 200)
        Cue.UploadReady -> CueSpec(cue, "fx_upload_ready", CueCategory.Interface, 0.8f, 2, 250, 300)
        Cue.Reconnected -> CueSpec(cue, "fx_reconnected", CueCategory.Errors, 0.8f, 2, 400, 450)
        Cue.Undo -> CueSpec(cue, "fx_undo", CueCategory.Interface, 0.8f, 1, 120, 250)

        // Interface cues: subliminal, matched in loudness by the audit.
        Cue.Tap -> CueSpec(cue, "fx_tap", CueCategory.Interface, 1f, 0, 60, 60)
        Cue.Select -> CueSpec(cue, "fx_select", CueCategory.Interface, 1f, 0, 60, 80)
        Cue.ToggleOn -> CueSpec(cue, "fx_toggle_on", CueCategory.Interface, 1f, 1, 80, 90)
        Cue.ToggleOff -> CueSpec(cue, "fx_toggle_off", CueCategory.Interface, 1f, 1, 80, 90)
        Cue.Open -> CueSpec(cue, "fx_open", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Close -> CueSpec(cue, "fx_close", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Detent -> CueSpec(cue, "fx_detent", CueCategory.Interface, 1f, 0, 60, 60)
        Cue.Star -> CueSpec(cue, "fx_star", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Unstar -> CueSpec(cue, "fx_unstar", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Pin -> CueSpec(cue, "fx_pin", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Archive -> CueSpec(cue, "fx_archive", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Delete -> CueSpec(cue, "fx_delete", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Copy -> CueSpec(cue, "fx_copy", CueCategory.Interface, 1f, 1, 120, 120)
        Cue.Error -> CueSpec(cue, "fx_error", CueCategory.Interface, 1f, 3, 250, 180)
        Cue.Refresh -> CueSpec(cue, "fx_refresh", CueCategory.Interface, 1f, 1, 200, 120)

        // Round 2: thinking power and fast mode. Surge swells for half a second, so it ranks as an action
        // (a later cue does not cut it); the rest are as short and light as their moments.
        Cue.Surge -> CueSpec(cue, "fx_surge", CueCategory.Interface, 1f, 1, 400, 490)
        Cue.Zip -> CueSpec(cue, "fx_zip", CueCategory.Interface, 1f, 0, 100, 85)
        Cue.Rebound -> CueSpec(cue, "fx_rebound", CueCategory.Interface, 1f, 1, 150, 120)
        Cue.FastOn -> CueSpec(cue, "fx_fast_on", CueCategory.Interface, 0.7f, 1, 300, 195)
        Cue.FastOff -> CueSpec(cue, "fx_fast_off", CueCategory.Interface, 0.8f, 1, 200, 100)

        // One motif per provider on the picker's rail. Scrubbed along quickly, so they are the lightest priority
        // and keep a short gap; each is its own file (timbre and interval motif, see docs/sound-design/android.md).
        Cue.ProviderClaude -> provider(cue, "claude", 115)
        Cue.ProviderCodex -> provider(cue, "codex", 85)
        Cue.ProviderCursor -> provider(cue, "cursor", 110)
        Cue.ProviderDevin -> provider(cue, "devin", 115)
        Cue.ProviderGrok -> provider(cue, "grok", 115)
        Cue.ProviderHermes -> provider(cue, "hermes", 95)
        Cue.ProviderPi -> provider(cue, "pi", 110)
        Cue.ProviderOpenCode -> provider(cue, "opencode", 115)
        Cue.ProviderAntigravity -> provider(cue, "antigravity", 115)
        Cue.ProviderFavorites -> provider(cue, "favorites", 115)
        Cue.ProviderOther -> provider(cue, "other", 60)
    }

    private fun provider(cue: Cue, name: String, approxMs: Long) =
        CueSpec(cue, "fx_provider_$name", CueCategory.Interface, 1f, 0, 60, approxMs)

    val all: List<CueSpec> get() = specs
}

/**
 * The Detent cue climbs a major-pentatonic ladder (the family's key), played
 * by resampling one 880 Hz tick: step 0 sits a fourth below the reference
 * and each step walks up the scale, so dragging a slider "plays" it. Rates are
 * kept inside SoundPool's 0.5..2.0 range; beyond the top the ladder holds.
 */
object DetentLadder {
    private val degrees = intArrayOf(0, 2, 4, 7, 9)
    const val BASE_SEMITONES = -7
    const val MAX_SEMITONES = 12
    const val MIN_RATE = 0.5f
    const val MAX_RATE = 2.0f

    fun semitones(step: Int): Int {
        val octave = Math.floorDiv(step, degrees.size)
        val degree = Math.floorMod(step, degrees.size)
        return (BASE_SEMITONES + 12 * octave + degrees[degree]).coerceIn(-12, MAX_SEMITONES)
    }

    fun rate(step: Int): Float = 2.0.pow(semitones(step) / 12.0).toFloat().coerceIn(MIN_RATE, MAX_RATE)
}
