package sh.zeron.android.ui

import androidx.compose.runtime.snapshots.Snapshot
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.launch
import kotlinx.coroutines.runBlocking
import kotlinx.coroutines.yield
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Assert.fail
import org.junit.Before
import org.junit.Test
import uniffi.zeron_core.ContextUsage
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.DeviceAccountInfo
import uniffi.zeron_core.NoHandle
import uniffi.zeron_core.UsageWindowInfo

private fun w(label: String, used: Float) = UsageWindowInfo(label, used, null)

private fun login(
    id: String,
    active: Boolean = false,
    usage: List<UsageWindowInfo> = emptyList(),
    provider: String? = null,
    harness: String = "claude-code",
    plan: String? = null,
    switchable: Boolean = true,
) = DeviceAccountInfo(
    id = id,
    harness = harness,
    email = "$id@example.com",
    displayName = null,
    planLabel = plan,
    active = active,
    switchable = switchable,
    apiKey = false,
    organization = null,
    provider = provider,
    usage = usage,
    usageError = null,
)

/** A client whose account calls answer from the test (UniFFI's fake-object constructor). */
private class FakeCore : CoreClient(NoHandle) {
    val calls = mutableListOf<String>()
    var accounts: suspend () -> List<DeviceAccountInfo> = { emptyList() }
    var activate: suspend (String) -> List<DeviceAccountInfo> = { emptyList() }

    override suspend fun deviceAgentAccounts(deviceId: String): List<DeviceAccountInfo> {
        calls += "accounts"
        return accounts()
    }

    override suspend fun deviceAgentUsage(deviceId: String): List<DeviceAccountInfo> {
        calls += "usage"
        return accounts()
    }

    override suspend fun deviceActivateAccount(deviceId: String, harness: String, accountId: String): List<DeviceAccountInfo> {
        calls += "activate $accountId"
        return activate(accountId)
    }
}

class AccountUsageTest {
    // The cache is app-wide: each test starts signed out.
    @Before fun signedOut() = DeviceAccountsCache.reset()

    @Test fun noLoginInUseWhenNoneIsActive() {
        assertNull(loginInUse(emptyList()))
        assertNull(loginInUse(listOf(login("a"), login("b"))))
    }

    @Test fun theActiveLoginIsInUse() {
        val logins = listOf(login("a"), login("b", active = true), login("c"))
        assertEquals("b", loginInUse(logins)?.id)
    }

    @Test fun perProviderLoginsLeadWithTheOneReportingUsage() {
        // OpenCode: one login in use per model provider; only some report plan usage.
        val logins = listOf(
            login("anthropic-key", active = true, provider = "anthropic", harness = "opencode"),
            login("openai-1", active = true, provider = "openai", harness = "opencode", usage = listOf(w("5h", 0.4f))),
            login("openai-2", provider = "openai", harness = "opencode", usage = listOf(w("5h", 0.1f))),
        )
        assertEquals("openai-1", loginInUse(logins)?.id)
        // None reporting: the first in use still drives the pill (it shows the plan's name).
        assertEquals("a", loginInUse(listOf(login("a", active = true, provider = "x"), login("b", active = true, provider = "y")))?.id)
    }

    @Test fun peakUsageIsTheBusiestWindowClamped() {
        assertNull(login("a").peakUsage)
        assertEquals(0.78f, login("a", usage = listOf(w("Session", 0.42f), w("Week", 0.78f))).peakUsage!!, 0f)
        assertEquals(1f, login("a", usage = listOf(w("Session", 1.3f))).peakUsage!!, 0f)
        assertEquals(0f, login("a", usage = listOf(w("Session", -0.2f))).peakUsage!!, 0f)
    }

    @Test fun pillReadsTheBusiestWindowElseThePlan() {
        assertEquals("78%", pillLabel(login("a", usage = listOf(w("Session", 0.42f), w("Week", 0.78f)))))
        assertEquals("Max", pillLabel(login("a", plan = "Max")))
        assertEquals("Plan", pillLabel(login("a")))
    }

    @Test fun usageLevelThresholds() {
        assertEquals(UsageLevel.Normal, usageLevel(0f))
        assertEquals(UsageLevel.Normal, usageLevel(0.69f))
        assertEquals(UsageLevel.High, usageLevel(0.7f))
        assertEquals(UsageLevel.High, usageLevel(0.89f))
        assertEquals(UsageLevel.Critical, usageLevel(0.9f))
        assertEquals(UsageLevel.Critical, usageLevel(1f))
    }

    @Test fun switcherListsTheInUseLoginsGroup() {
        val a = login("a", active = true)
        val logins = listOf(a, login("b"), login("c", switchable = false))
        assertEquals(listOf("a", "b", "c"), switchChoices(logins, a, switches = true).map { it.id })
    }

    @Test fun switcherIsEmptyForASingleLoginOrAnAgentThatCantSwitch() {
        val a = login("a", active = true)
        assertEquals(emptyList<DeviceAccountInfo>(), switchChoices(listOf(a), a, switches = true))
        assertEquals(emptyList<DeviceAccountInfo>(), switchChoices(listOf(a, login("b")), a, switches = false))
    }

    @Test fun switcherKeepsToTheInUseLoginsProvider() {
        val openai = login("openai-1", active = true, provider = "openai", harness = "opencode")
        val logins = listOf(
            login("anthropic-1", active = true, provider = "anthropic", harness = "opencode"),
            openai,
            login("openai-2", provider = "openai", harness = "opencode"),
        )
        assertEquals(listOf("openai-1", "openai-2"), switchChoices(logins, openai, switches = true).map { it.id })
        // Alone for its provider: nothing to switch to.
        assertEquals(emptyList<DeviceAccountInfo>(), switchChoices(logins, logins[0], switches = true))
    }

    @Test fun switchingMovesInUseWithinItsGroupOnly() {
        val list = listOf(
            login("c1", active = true),
            login("c2"),
            login("x1", active = true, harness = "codex"),
            login("o1", active = true, provider = "openai", harness = "opencode"),
            login("o2", provider = "openai", harness = "opencode"),
            login("a1", active = true, provider = "anthropic", harness = "opencode"),
        )
        val active = { l: List<DeviceAccountInfo> -> l.filter { it.active }.map { it.id } }
        assertEquals(listOf("c2", "x1", "o1", "a1"), active(withActive(list, list[1])))
        assertEquals(listOf("c1", "x1", "o2", "a1"), active(withActive(list, list[4])))
    }

    @Test fun contextChipShowsFromHalfFullUnlessThePillCarriesIt() {
        val half = ContextUsage(100_000uL, 200_000uL)
        assertEquals(0.5f, contextChipFraction(half, pillShown = false)!!, 0f)
        assertNull(contextChipFraction(half, pillShown = true))
        assertNull(contextChipFraction(ContextUsage(90_000uL, 200_000uL), pillShown = false))
        assertNull(contextChipFraction(ContextUsage(null, 200_000uL), pillShown = false))
        assertNull(contextChipFraction(ContextUsage(10uL, 0uL), pillShown = false))
    }

    @Test fun aReaderThatFoundNothingIsToldWhenTheListLands() {
        // A page opened before any list (a cold start on the accounts page) must still recompose when one lands.
        val core = CoreClient(NoHandle)
        val reads = mutableListOf<Any>()
        val before = Snapshot.observe(readObserver = { reads += it }) { DeviceAccountsCache.get(core, "dev") }
        assertNull(before)
        assertTrue(reads.isNotEmpty())
        DeviceAccountsCache.put(core, "dev", listOf(login("a", active = true)))
        assertEquals("a", DeviceAccountsCache.get(core, "dev")?.single()?.id)
        // Another sign-in: nothing of the last one carries over.
        assertNull(DeviceAccountsCache.get(CoreClient(NoHandle), "dev"))
    }

    @Test fun signOutForgetsTheListsTheThrottleAndTheClient() {
        val core = CoreClient(NoHandle)
        DeviceAccountsCache.put(core, "dev", listOf(login("a", active = true)))
        assertTrue(DeviceAccountsCache.takeForce("dev", explicit = false))
        DeviceAccountsCache.reset()
        // Even for the very same client: nothing of the signed-out account answers.
        assertNull(DeviceAccountsCache.get(core, "dev"))
        assertTrue(DeviceAccountsCache.takeForce("dev", explicit = false))
    }

    @Test fun forcedProbesAreThrottledPerComputerUnlessAskedFor() {
        val t = 1_000_000L
        assertTrue(DeviceAccountsCache.takeForce("a", explicit = false, now = t))
        assertFalse(DeviceAccountsCache.takeForce("a", explicit = false, now = t + 29_999))
        // Another computer has its own window.
        assertTrue(DeviceAccountsCache.takeForce("b", explicit = false, now = t + 1))
        assertTrue(DeviceAccountsCache.takeForce("a", explicit = false, now = t + 30_000))
        // The refresh button always goes through, and restarts the window.
        assertTrue(DeviceAccountsCache.takeForce("a", explicit = true, now = t + 30_001))
        assertFalse(DeviceAccountsCache.takeForce("a", explicit = false, now = t + 59_999))
        assertTrue(DeviceAccountsCache.takeForce("a", explicit = false, now = t + 60_001))
    }

    @Test fun aForcedLoadProbesOnceThenReadsTheCacheUntilTheWindowPasses() = runBlocking {
        val core = FakeCore()
        core.accounts = { listOf(login("a", active = true)) }
        val state = AccountUsageState(core, "dev", "claude-code", this)
        state.load(force = false)
        state.load(force = true)
        state.load(force = true)
        assertEquals(listOf("accounts", "usage", "accounts"), core.calls)
        assertEquals("a", state.active?.id)
    }

    @Test fun aListAskedForBeforeASwitchDoesntUndoIt() = runBlocking {
        val before = listOf(login("a", active = true), login("b"))
        val core = FakeCore()
        DeviceAccountsCache.put(core, "dev", before)
        val stale = CompletableDeferred<List<DeviceAccountInfo>>()
        core.accounts = { stale.await() }
        core.activate = { withActive(before, before[1]) }
        val state = AccountUsageState(core, "dev", "claude-code", this)
        val load = launch { state.load(force = false) }
        yield()
        assertEquals(listOf("accounts"), core.calls)
        state.activate(before[1]) { fail(it) }
        while (state.busy != null) yield()
        // The list from before the switch lands last.
        stale.complete(before)
        load.join()
        assertEquals("b", state.active?.id)
    }

    @Test fun aRefusedSwitchPutsTheListBack() = runBlocking {
        val before = listOf(login("a", active = true), login("b"), login("c"))
        val core = FakeCore()
        DeviceAccountsCache.put(core, "dev", before)
        val answer = CompletableDeferred<List<DeviceAccountInfo>>()
        core.activate = { answer.await() }
        val errors = mutableListOf<String>()
        val state = AccountUsageState(core, "dev", "claude-code", this)
        state.activate(before[1]) { errors += it }
        // Shown in use before the computer answers, and no second switch meanwhile.
        assertEquals("b", state.active?.id)
        assertEquals("b", state.busy)
        state.activate(before[2]) { errors += it }
        answer.completeExceptionally(CoreException.HostException("Claude Code is busy"))
        while (state.busy != null) yield()
        assertEquals(before, DeviceAccountsCache.get(core, "dev"))
        assertEquals(listOf("Couldn't switch: Claude Code is busy"), errors)
        assertEquals(listOf("activate b"), core.calls)
    }
}
