package sh.zeron.android.feedback

import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test

class Recorder : Feedback {
    val events = ArrayList<String>()
    override fun haptic(haptic: Haptic) { events += "h:$haptic" }
    override fun cue(cue: Cue, step: Int) { events += "c:$cue" }
}

class SessionFeedbackTest {
    private val rec = Recorder()
    private val alerts = ArrayList<Pair<String, SessionEvent>>()
    private var foreground = true
    private var now = 100_000L
    private val policy = SessionFeedbackPolicy(rec, { id, e -> alerts += id to e }, { foreground }, { now })

    @Test fun finishingATurnFiresDoneOnce() {
        policy.session("a", SessionEvent.Done)
        policy.session("a", SessionEvent.Done)
        assertEquals(listOf("c:Done", "h:Success"), rec.events)
        assertTrue(alerts.isEmpty())
    }

    @Test fun questionAndFailure() {
        policy.session("a", SessionEvent.NeedsInput)
        assertEquals(listOf("c:Request", "h:Attention"), rec.events)
        rec.events.clear()
        policy.session("a", SessionEvent.Failed)
        assertEquals(listOf("c:Attention", "h:Error"), rec.events)
    }

    @Test fun backgroundGoesToNotificationsOnly() {
        foreground = false
        policy.session("a", SessionEvent.Done)
        assertTrue(rec.events.isEmpty())
        assertEquals(listOf("a" to SessionEvent.Done), alerts)
        policy.link(LinkEvent.Lost)
        assertTrue(rec.events.isEmpty())
    }

    @Test fun differentSessionsAndLaterEventsStillFire() {
        policy.session("a", SessionEvent.Done)
        policy.session("b", SessionEvent.Done)
        assertEquals(4, rec.events.size)
        now += SessionFeedbackPolicy.DEDUPE_MS + 1
        policy.session("a", SessionEvent.Done)
        assertEquals(6, rec.events.size)
    }

    @Test fun stoppingASessionIsNotACompletion() {
        policy.interrupted("a")
        now += 500
        policy.session("a", SessionEvent.Done)
        assertTrue(rec.events.isEmpty())
        now += SessionFeedbackPolicy.INTERRUPT_MS
        policy.session("a", SessionEvent.Done)
        assertEquals(2, rec.events.size)
        // Questions and failures after a stop are still real.
        policy.interrupted("b")
        policy.session("b", SessionEvent.Failed)
        assertEquals(4, rec.events.size)
    }

    @Test fun linkEvents() {
        policy.link(LinkEvent.Lost)
        assertEquals(listOf("c:Attention", "h:Attention"), rec.events)
        rec.events.clear()
        policy.link(LinkEvent.Restored)
        assertEquals(listOf("c:Reconnected", "h:Confirm"), rec.events)
    }

    // ── transitions ────────────────────────────────────────────────────────

    private fun snap(vararg p: Pair<String, Phase>) = mapOf(*p)

    @Test fun firstSnapshotIsABaseline() {
        val t = SessionTransitions()
        assertTrue(t.observe(snap("a" to Phase.Working, "b" to Phase.Errored)).isEmpty())
        assertEquals(listOf("a" to SessionEvent.Done), t.observe(snap("a" to Phase.Completed, "b" to Phase.Errored)))
    }

    @Test fun eventsFireOncePerChange() {
        val t = SessionTransitions()
        t.observe(snap("a" to Phase.Idle))
        assertTrue(t.observe(snap("a" to Phase.Working)).isEmpty())
        assertEquals(listOf("a" to SessionEvent.NeedsInput), t.observe(snap("a" to Phase.AwaitingInput)))
        assertTrue(t.observe(snap("a" to Phase.AwaitingInput)).isEmpty())
        assertTrue(t.observe(snap("a" to Phase.Working)).isEmpty())
        assertEquals(listOf("a" to SessionEvent.Failed), t.observe(snap("a" to Phase.Errored)))
        assertTrue(t.observe(snap("a" to Phase.Errored)).isEmpty())
    }

    @Test fun newRowsAndMarkingSeenAreSilent() {
        val t = SessionTransitions()
        t.observe(snap("a" to Phase.Completed))
        assertTrue(t.observe(snap("a" to Phase.Idle, "new" to Phase.Working)).isEmpty())
        assertEquals(listOf("new" to SessionEvent.Done), t.observe(snap("a" to Phase.Idle, "new" to Phase.Idle)))
    }

    @Test fun resetStartsOver() {
        val t = SessionTransitions()
        t.observe(snap("a" to Phase.Working))
        t.reset()
        assertTrue(t.observe(snap("a" to Phase.Completed)).isEmpty())
    }

    @Test fun linkLossIsAnnouncedOnlyDuringATurnAndRestoreOnlyIfSo() {
        val l = LinkTransitions()
        assertEquals(null, l.observe(degraded = false, turnRunning = true)) // seed
        assertEquals(LinkEvent.Lost, l.observe(degraded = true, turnRunning = true))
        assertEquals(null, l.observe(degraded = true, turnRunning = true)) // still down
        assertEquals(LinkEvent.Restored, l.observe(degraded = false, turnRunning = false))
        assertEquals(null, l.observe(degraded = false, turnRunning = false))

        assertEquals(null, l.observe(degraded = true, turnRunning = false)) // idle outage: quiet
        assertEquals(null, l.observe(degraded = false, turnRunning = false)) // and quiet restore
    }

    @Test fun bootingIntoAnOutageIsSilent() {
        val l = LinkTransitions()
        assertEquals(null, l.observe(degraded = true, turnRunning = true))
        assertEquals(null, l.observe(degraded = false, turnRunning = true))
    }

    // ── tap claims ─────────────────────────────────────────────────────────

    @Test fun defaultTapYieldsToExplicitFeedback() {
        var t = 1_000L
        val c = ClaimTracker { t }
        val released = t
        assertEquals(false, c.hapticClaimed(released))
        t += 5
        c.claimHaptic()
        assertEquals(true, c.hapticClaimed(released))
        assertEquals(false, c.cueClaimed(released)) // haptic and cue are claimed separately
        c.claimCue()
        assertEquals(true, c.cueClaimed(released))
        // An old claim does not suppress a later tap.
        t += 1_000
        assertEquals(false, c.hapticClaimed(t))
    }

    @Test fun aChosenMenuItemQuietsTheCloseThatFollows() {
        var t = 5_000L
        val c = ClaimTracker { t }
        assertEquals(false, c.cueWithin(250))
        c.quiet()
        t += 100
        assertEquals(true, c.cueWithin(250))
        t += 200
        assertEquals(false, c.cueWithin(250))
        // An explicit cue counts the same way, and a later one extends it.
        c.claimCue()
        t += 100
        assertEquals(true, c.cueWithin(250))
    }
}
