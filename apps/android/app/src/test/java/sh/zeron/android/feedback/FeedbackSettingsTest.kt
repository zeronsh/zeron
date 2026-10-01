package sh.zeron.android.feedback

import kotlin.math.log10
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class FeedbackSettingsTest {
    @Test fun defaultsAreOnAndRestrained() {
        val d = FeedbackSettings()
        assertEquals(true, d.master && d.sounds && d.interfaceSounds && d.sessionSounds && d.completionSound && d.inputSound && d.errorSound && d.haptics)
        assertEquals(HapticStrength.Standard, d.strength)
    }

    @Test fun roundTrips() {
        val s = FeedbackSettings(master = false, sounds = false, interfaceSounds = false, sessionSounds = false, completionSound = false, inputSound = false, errorSound = false, volume = 0.4f, haptics = false, strength = HapticStrength.Strong)
        val stored = FeedbackSettingsCodec.write(s)
        assertEquals(s, FeedbackSettingsCodec.read { stored[it] })
    }

    @Test fun missingOrMalformedReadAsDefaults() {
        assertEquals(FeedbackSettings(), FeedbackSettingsCodec.read { null })
        val bad = mapOf<String, Any>("feedback.volume" to Float.NaN, "feedback.strength" to "Thunder", "feedback.sounds" to "yes")
        assertEquals(FeedbackSettings(), FeedbackSettingsCodec.read { bad[it] })
        val loud = mapOf<String, Any>("feedback.volume" to 9f)
        assertEquals(1f, FeedbackSettingsCodec.read { loud[it] }.volume, 0f)
    }

    @Test fun categorySwitchesAreIndependentUnderTheMaster() {
        val s = FeedbackSettings(completionSound = false)
        assertEquals(false, s.allows(CueCategory.Completion))
        assertEquals(true, s.allows(CueCategory.Input))
        assertEquals(true, s.allows(CueCategory.Errors))
        assertEquals(true, s.allows(CueCategory.Interface))
        assertEquals(false, FeedbackSettings(sounds = false).allows(CueCategory.Interface))
        // The master is above everything: off, no category is allowed whatever its own switch says.
        for (category in CueCategory.entries) assertEquals(false, FeedbackSettings(master = false).allows(category))
        assertEquals(false, FeedbackSettings(master = false).soundsOn)
        assertEquals(false, FeedbackSettings(master = false).hapticsOn)
        assertEquals(true, FeedbackSettings().soundsOn && FeedbackSettings().hapticsOn)
        assertEquals(false, FeedbackSettings(haptics = false).hapticsOn)
    }

    // --- volume model: the slider's 100% is twice the default, the default (50%) is gain 1

    @Test fun defaultIsHalfWayAtUnitGain() {
        assertEquals(0.5f, FeedbackSettings().volume, 0f)
        assertEquals(1f, FeedbackSettings().gain, 1e-6f)
    }

    @Test fun fullSliderIsSixDecibelsLouderThanTheDefault() {
        val max = FeedbackSettings(volume = 1f).gain
        assertEquals(FeedbackSettings.MAX_GAIN, max, 1e-6f)
        assertEquals(6.02, 20 * log10(max.toDouble()), 0.01)
        assertEquals(0f, FeedbackSettings(volume = 0f).gain, 0f)
        assertEquals(FeedbackSettings.MAX_GAIN, FeedbackSettings(volume = 7f).gain, 1e-6f) // clamped
    }

    @Test fun curveIsMonotoneContinuousAndSquaredBelowTheDefault() {
        var last = -1f
        for (i in 0..100) {
            val g = FeedbackSettings.gainFor(i / 100f)
            assertTrue("$i%", g > last || i == 0)
            last = g
        }
        assertEquals(0.25f, FeedbackSettings.gainFor(0.25f), 1e-6f) // slider 25% = -12 dB
        assertEquals(0.0625f, FeedbackSettings.gainFor(0.125f), 1e-6f)
        assertEquals(FeedbackSettings.gainFor(0.5f), FeedbackSettings.gainFor(0.5001f), 1e-3f) // no jump at the join
        // Above the default every step is the same number of decibels.
        val step = 20 * log10(FeedbackSettings.gainFor(0.75f).toDouble() / FeedbackSettings.gainFor(0.5f))
        assertEquals(3.01, step, 0.01)
    }

    @Test fun soundPoolVolumeNeverExceedsOneAndTheDefaultPlaysTheFilesAtHalf() {
        val spec = CueTable.spec(Cue.Tap)
        // The files carry the headroom (ASSET_BOOST = 2): the default plays them at half volume, 100% at full.
        assertEquals(0.5f, CueTable.volume(spec, FeedbackSettings().gain), 1e-6f)
        assertEquals(1f, CueTable.volume(spec, FeedbackSettings(volume = 1f).gain), 1e-6f)
        for (s in CueTable.all) for (i in 0..100) assertTrue(CueTable.volume(s, FeedbackSettings.gainFor(i / 100f)) <= 1f)
        // A trimmed cue keeps its trim at every position.
        val fast = CueTable.spec(Cue.FastOn)
        assertEquals(0.5f * fast.gain, CueTable.volume(fast, FeedbackSettings().gain), 1e-6f)
    }

    // --- stored settings and their one-time migration (v3: louder files, new default; the override is gone)

    @Test fun anyOlderVolumeIsResetToTheNewDefaultOnce() {
        for (version in listOf(null, 1, 2)) for (old in listOf(0f, 0.1f, 0.4f, 0.7f, 1f)) {
            val stored = mutableMapOf<String, Any>("feedback.volume" to old, "feedback.sounds" to false)
            if (version != null) stored["feedback.prefs_version"] = version
            val migration = FeedbackSettingsCodec.migrate { stored[it] }
            stored.putAll(migration.put)
            migration.remove.forEach(stored::remove)
            val read = FeedbackSettingsCodec.read { stored[it] }
            assertEquals("v$version $old", 0.5f, read.volume, 0f)
            assertEquals("other choices are kept", false, read.sounds)
        }
    }

    @Test fun migrationRunsOnce() {
        val stored = mutableMapOf<String, Any>("feedback.volume" to 0.8f)
        val first = FeedbackSettingsCodec.migrate { stored[it] }
        stored.putAll(first.put)
        assertEquals(0.5f, stored["feedback.volume"] as Float, 0f)
        stored["feedback.volume"] = 0.9f // the user moves the slider afterwards
        assertTrue(FeedbackSettingsCodec.migrate { stored[it] }.isEmpty)
        assertEquals(0.9f, FeedbackSettingsCodec.read { stored[it] }.volume, 0f)
    }

    @Test fun retiredOverrideIsDroppedSilently() {
        val stored = mutableMapOf<String, Any>("feedback.ignore_touch_sounds" to true, "feedback.volume" to 0.3f)
        val migration = FeedbackSettingsCodec.migrate { stored[it] }
        assertTrue("feedback.ignore_touch_sounds" in migration.remove)
        // Nothing that is written any more carries it either.
        assertTrue("feedback.ignore_touch_sounds" !in FeedbackSettingsCodec.write(FeedbackSettings()).keys)
        // And reading never looks at it: the settings have no such field.
        assertEquals(FeedbackSettings(volume = 0.3f), FeedbackSettingsCodec.read { stored[it] })
    }

    @Test fun freshInstallGetsTheDefaultAndIsJustStamped() {
        val migration = FeedbackSettingsCodec.migrate { null }
        assertEquals(setOf("feedback.volume", "feedback.prefs_version"), migration.put.keys)
        assertEquals(0.5f, FeedbackSettingsCodec.read { migration.put[it] }.volume, 0f)
    }

    @Test fun writtenSettingsCarryTheCurrentVersionSoTheyAreNeverMigratedAgain() {
        val stored = FeedbackSettingsCodec.write(FeedbackSettings(volume = 0.9f))
        assertTrue(FeedbackSettingsCodec.migrate { stored[it] }.isEmpty)
        assertEquals(0.9f, FeedbackSettingsCodec.read { stored[it] }.volume, 0f)
        assertEquals(3, stored["feedback.prefs_version"])
    }

    @Test fun malformedStoredVolumeMigratesToTheDefault() {
        val stored = mutableMapOf<String, Any>("feedback.volume" to Float.NaN)
        stored.putAll(FeedbackSettingsCodec.migrate { stored[it] }.put)
        assertEquals(0.5f, FeedbackSettingsCodec.read { stored[it] }.volume, 0f)
    }

    @Test fun masterRoundTripsAndDefaultsOn() {
        val stored = FeedbackSettingsCodec.write(FeedbackSettings(master = false))
        assertEquals(false, FeedbackSettingsCodec.read { stored[it] }.master)
        assertEquals(true, FeedbackSettingsCodec.read { null }.master)
    }
}
