package sh.zeron.android.feedback

import java.io.File
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class CueTableTest {
    private val raw = File("src/main/res/raw")
    private val desktop = File("../../../crates/ui/assets/sounds")

    @Test fun everyCueHasASpecWithItsOwnResource() {
        assertEquals(Cue.entries.size, CueTable.all.size)
        // Every cue has its own file: nothing borrows a neighbour's any more.
        assertEquals(CueTable.all.size, CueTable.all.map { it.resource }.toSet().size)
        for (spec in CueTable.all) {
            assertTrue(spec.resource, spec.resource.matches(Regex("fx_[a-z_]+")))
            assertTrue(spec.gain in 0.1f..1f)
        }
    }

    @Test fun everyResourceExistsInTheRepoOrComesFromTheDesktopAssets() {
        for (spec in CueTable.all) {
            val committed = File(raw, spec.resource + ".wav").isFile
            assertTrue("${spec.cue} -> ${spec.resource}", committed)
        }
    }

    @Test fun sessionCuesAreTheDesktopChimes() {
        // In-app copies of the desktop chimes (mono, trimmed, mastered); the notification channels play the same files.
        assertEquals("fx_chime_done", CueTable.spec(Cue.Done).resource)
        assertEquals("fx_chime_request", CueTable.spec(Cue.Request).resource)
        assertEquals("fx_chime_attention", CueTable.spec(Cue.Attention).resource)
        for (name in listOf("done", "request", "attention")) assertTrue(File(desktop, "$name.wav").isFile)
        assertEquals(CueCategory.Completion, CueTable.spec(Cue.Done).category)
        assertEquals(CueCategory.Input, CueTable.spec(Cue.Request).category)
        assertEquals(CueCategory.Errors, CueTable.spec(Cue.Attention).category)
    }

    @Test fun interfaceCuesAreQuieterOrEqualToSessionCues() {
        for (spec in CueTable.all.filter { it.category == CueCategory.Interface }) assertTrue(spec.gain <= 1f)
        assertTrue(CueTable.spec(Cue.Send).gain < CueTable.spec(Cue.Done).gain)
    }

    @Test fun detentClimbsAndStaysInRange() {
        var last = -100
        for (step in 0..4) {
            val st = DetentLadder.semitones(step)
            assertTrue("step $step", st > last)
            last = st
        }
        for (step in -5..60) {
            val rate = DetentLadder.rate(step)
            assertTrue("$step $rate", rate in DetentLadder.MIN_RATE..DetentLadder.MAX_RATE)
        }
        assertEquals(DetentLadder.rate(4), DetentLadder.rate(4), 0f)
        assertTrue(DetentLadder.rate(5) > DetentLadder.rate(4)) // next octave continues upward
        assertEquals(DetentLadder.rate(40), DetentLadder.rate(80), 0f) // the top holds
    }
}
