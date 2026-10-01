package sh.zeron.android.feedback

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test

class EngineTransitionsTest {
    private val t = EngineTransitions()

    @Test fun firstStateIsABaseline() {
        assertNull(t.observe(EngineStage.Failed))
        assertNull(t.observe(EngineStage.Failed))
    }

    @Test fun setupProgressIsSilentAndCompletionFiresOnce() {
        assertNull(t.observe(EngineStage.Idle))
        assertNull(t.observe(EngineStage.Setup))
        assertNull(t.observe(EngineStage.Setup))
        assertEquals(EngineEvent.SetupDone, t.observe(EngineStage.Starting))
        assertNull(t.observe(EngineStage.Running))
    }

    @Test fun setupStraightToRunning() {
        t.observe(EngineStage.Setup)
        assertEquals(EngineEvent.SetupDone, t.observe(EngineStage.Running))
    }

    @Test fun anOrdinaryStartIsNotSetup() {
        t.observe(EngineStage.Stopped)
        assertNull(t.observe(EngineStage.Starting))
        assertNull(t.observe(EngineStage.Running))
    }

    @Test fun failureFiresOncePerFailure() {
        t.observe(EngineStage.Running)
        assertEquals(EngineEvent.Failed, t.observe(EngineStage.Failed))
        assertNull(t.observe(EngineStage.Failed))
        assertNull(t.observe(EngineStage.Starting))
        assertEquals(EngineEvent.Failed, t.observe(EngineStage.Failed))
    }

    @Test fun failedSetupIsAFailureNotACompletion() {
        t.observe(EngineStage.Setup)
        assertEquals(EngineEvent.Failed, t.observe(EngineStage.Failed))
    }

    @Test fun resetStartsOver() {
        t.observe(EngineStage.Running)
        t.reset()
        assertNull(t.observe(EngineStage.Failed))
    }
}

class DeviceFeedbackPolicyTest {
    private val rec = Recorder()
    private var foreground = true
    private var now = 100_000L
    private val policy = DeviceFeedbackPolicy(rec, { foreground }, { now })

    @Test fun transferCues() {
        policy.transfer("t1", TransferEvent.Asked)
        assertEquals(listOf("c:Request", "h:Attention"), rec.events)
        rec.events.clear()
        policy.transfer("t1", TransferEvent.Received)
        assertEquals(listOf("c:UploadReady", "h:Success"), rec.events)
        rec.events.clear()
        policy.transfer("t2", TransferEvent.Sent)
        assertEquals(listOf("c:UploadReady", "h:Success"), rec.events)
        rec.events.clear()
        policy.transfer("t3", TransferEvent.Failed)
        assertEquals(listOf("c:Attention", "h:Error"), rec.events)
    }

    @Test fun sameEventForSameTransferFiresOnce() {
        policy.transfer("t1", TransferEvent.Received)
        policy.transfer("t1", TransferEvent.Received)
        assertEquals(2, rec.events.size)
        now += DeviceFeedbackPolicy.DEDUPE_MS + 1
        policy.transfer("t1", TransferEvent.Received)
        assertEquals(4, rec.events.size)
    }

    @Test fun backgroundPlaysNothingInApp() {
        foreground = false
        policy.transfer("t1", TransferEvent.Asked)
        policy.engine(EngineEvent.Failed)
        assertTrue(rec.events.isEmpty())
    }

    @Test fun engineCues() {
        policy.engine(EngineEvent.SetupDone)
        assertEquals(listOf("c:UploadReady", "h:Success"), rec.events)
        rec.events.clear()
        policy.engine(EngineEvent.Failed)
        assertEquals(listOf("c:Error", "h:Error"), rec.events)
    }
}
