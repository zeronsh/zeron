package sh.zeron.android.feedback

import android.content.SharedPreferences
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow

/** How hard haptics hit: scales composition primitives (and waveform amplitudes) where the hardware allows. */
enum class HapticStrength(val label: String, val scale: Float) {
    Subtle("Subtle", 0.5f),
    Standard("Standard", 1f),
    Strong("Strong", 1.5f),
}

/**
 * Which in-app switch governs a sound. Mirrors the desktop's Notifications
 * settings: one master, independent completion / input / error chimes, plus
 * the phone's own interface sounds.
 */
enum class CueCategory {
    /** Taps, toggles, sheets, detents: the quiet interface layer. */
    Interface,

    /** A turn finished. */
    Completion,

    /** A question or approval is waiting. */
    Input,

    /** A run failed or the connection dropped (and came back). */
    Errors,
}

/**
 * The user's choices for sound and haptics. Everything defaults on.
 *
 * [master] ("Sounds & haptics") is absolute: off silences every sound and every vibration, in the app and in the
 * notifications it posts; on hands the decision to the switches below it, and nothing the phone itself has set
 * (touch sounds, touch feedback, silent mode, Do Not Disturb) is consulted. See [FeedbackGate].
 */
data class FeedbackSettings(
    /** The one switch above all others: off = no sound and no haptic anywhere. */
    val master: Boolean = true,
    /** Every sound, under [master]. */
    val sounds: Boolean = true,
    val interfaceSounds: Boolean = true,
    /** Master for the three session chimes below. */
    val sessionSounds: Boolean = true,
    val completionSound: Boolean = true,
    val inputSound: Boolean = true,
    val errorSound: Boolean = true,
    /** 0..1 slider; the audible gain follows [gain]. 50% plays the (mastered, loud) files at half volume, 100% at full volume: 6 dB more. */
    val volume: Float = DEFAULT_VOLUME,
    val haptics: Boolean = true,
    val strength: HapticStrength = HapticStrength.Standard,
) {
    /** Sounds are wanted at all: the master and the sounds switch. */
    val soundsOn: Boolean get() = master && sounds

    /** Haptics are wanted at all: the master and the haptics switch. */
    val hapticsOn: Boolean get() = master && haptics

    fun allows(category: CueCategory): Boolean = soundsOn && when (category) {
        CueCategory.Interface -> interfaceSounds
        CueCategory.Completion -> sessionSounds && completionSound
        CueCategory.Input -> sessionSounds && inputSound
        CueCategory.Errors -> sessionSounds && errorSound
    }

    /** Linear gain on the cue's native level, 0..[MAX_GAIN]; see [gainFor]. */
    val gain: Float get() = gainFor(volume)

    companion object {
        /** Half way. */
        const val DEFAULT_VOLUME = 0.5f

        /** At 100% every cue is twice as loud (+6 dB) as at the default 50%; the files carry the headroom (see [CueTable.ASSET_BOOST]). */
        const val MAX_GAIN = 2f

        /**
         * The slider is linear, loudness is not. The slider position doubles to `v' = 2 * slider` and goes through
         * the curve the app always used up to its old maximum (v' <= 1: gain = v'^2, so 25% of the old slider reads
         * about -12 dB), then continues in equal dB steps to +6 dB at the new maximum (gain = 2^(v' - 1)). Both
         * pieces meet at gain 1 with no jump, and the default (50%) is gain 1.
         */
        fun gainFor(slider: Float): Float {
            val v = 2f * slider.coerceIn(0f, 1f)
            return if (v <= 1f) v * v else Math.pow(2.0, (v - 1f).toDouble()).toFloat()
        }
    }
}

/** Plain key/value form of [FeedbackSettings], kept free of Android types so it can be tested. */
object FeedbackSettingsCodec {
    private const val P = "feedback."

    fun write(s: FeedbackSettings): Map<String, Any> = mapOf(
        P + "master" to s.master,
        P + "sounds" to s.sounds,
        P + "interface" to s.interfaceSounds,
        P + "session" to s.sessionSounds,
        P + "completion" to s.completionSound,
        P + "input" to s.inputSound,
        P + "errors" to s.errorSound,
        P + "volume" to s.volume,
        P + "haptics" to s.haptics,
        P + "strength" to s.strength.name,
        VERSION_KEY to VERSION,
    )

    /** Missing or malformed values read as the defaults. */
    fun read(get: (String) -> Any?): FeedbackSettings {
        val d = FeedbackSettings()
        fun bool(key: String, default: Boolean) = get(P + key) as? Boolean ?: default
        return FeedbackSettings(
            master = bool("master", d.master),
            sounds = bool("sounds", d.sounds),
            interfaceSounds = bool("interface", d.interfaceSounds),
            sessionSounds = bool("session", d.sessionSounds),
            completionSound = bool("completion", d.completionSound),
            inputSound = bool("input", d.inputSound),
            errorSound = bool("errors", d.errorSound),
            volume = (get(P + "volume") as? Float ?: d.volume).takeIf { it.isFinite() }?.coerceIn(0f, 1f) ?: d.volume,
            haptics = bool("haptics", d.haptics),
            strength = HapticStrength.entries.firstOrNull { it.name == get(P + "strength") } ?: d.strength,
        )
    }

    /**
     * Layout of the stored values. 1 (absent): volume on the first scale. 2: the half-scale slider. 3: the slider is
     * the same but the files are 6 dB hotter (twice the default's amplitude) and the user asked for a louder default, so the stored volume is reset
     * to 50% once, and the removed "play sounds anyway" override is dropped.
     */
    const val VERSION = 3
    private const val VERSION_KEY = P + "prefs_version"
    private const val RETIRED_OVERRIDE_KEY = P + "ignore_touch_sounds"

    /** What to write and what to delete so stored values from an older layout fit this one; see [VERSION]. */
    class Migration(val put: Map<String, Any>, val remove: Set<String>) {
        val isEmpty: Boolean get() = put.isEmpty() && remove.isEmpty()
    }

    /** Runs once (keyed on the stored version): the volume goes back to the default, the retired override is dropped. Empty when current. */
    fun migrate(get: (String) -> Any?): Migration {
        val stored = (get(VERSION_KEY) as? Int) ?: 1
        if (stored >= VERSION) return Migration(emptyMap(), emptySet())
        return Migration(
            put = mapOf(P + "volume" to FeedbackSettings.DEFAULT_VOLUME, VERSION_KEY to VERSION),
            remove = setOf(RETIRED_OVERRIDE_KEY),
        )
    }
}

/** [FeedbackSettings] persisted in the app's `settings` preferences. */
class FeedbackStore(private val prefs: SharedPreferences) {
    init {
        val migration = FeedbackSettingsCodec.migrate { prefs.all[it] }
        if (!migration.isEmpty) {
            val edit = prefs.edit()
            migration.remove.forEach(edit::remove)
            save(edit, migration.put)
        }
    }

    private val _settings = MutableStateFlow(FeedbackSettingsCodec.read { prefs.all[it] })
    val settings: StateFlow<FeedbackSettings> = _settings.asStateFlow()

    val current: FeedbackSettings get() = _settings.value

    fun update(change: FeedbackSettings.() -> FeedbackSettings) {
        val next = _settings.value.change()
        if (next == _settings.value) return
        _settings.value = next
        save(prefs.edit(), FeedbackSettingsCodec.write(next))
    }

    private fun save(edit: SharedPreferences.Editor, values: Map<String, Any>) {
        for ((key, value) in values) {
            when (value) {
                is Boolean -> edit.putBoolean(key, value)
                is Float -> edit.putFloat(key, value)
                is Int -> edit.putInt(key, value)
                is String -> edit.putString(key, value)
            }
        }
        edit.apply()
    }
}
