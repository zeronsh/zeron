package sh.zeron.android.core

import org.json.JSONObject
import org.junit.Assert.*
import org.junit.Test
import java.time.Instant
import java.time.ZoneId
import java.util.Locale

class AccountUsageTest {
    private fun snapshot(rows: String) = Agents.accounts(JSONObject("""{"accounts":[$rows]}"""))

    @Test fun bindingLimitUsesOnlyTheActiveAccountOfTheSessionHarness() {
        val accounts = snapshot("""
            {"id":"other","harness":"codex","active":true,"usageWindows":[{"label":"Weekly","usedFraction":0.99}]},
            {"id":"inactive","harness":"claude-code","active":false,"usageWindows":[{"label":"Weekly","usedFraction":1}]},
            {"id":"live","harness":"claude-code","active":true,"usageWindows":[
              {"label":"5 hour","usedFraction":0.38},{"label":"Weekly","usedFraction":0.64}]}
        """)
        assertEquals(0.64f, AccountUsage.fraction(accounts, "claude-code")!!, 0.001f)
        assertEquals(0.99f, AccountUsage.fraction(accounts, "codex")!!, 0.001f)
        assertNull(AccountUsage.fraction(accounts, "grok"))
    }

    @Test fun unreportedUsageIsUnknownRatherThanZero() {
        val accounts = snapshot("""
            {"id":"live","harness":"codex","active":true},
            {"id":"old","harness":"codex","active":false,"usageWindows":[{"usedFraction":0.7}]}
        """)
        assertNull(AccountUsage.fraction(accounts, "codex"))
        assertNull(AccountUsage.fraction(null, "codex"))
        assertEquals(emptyList<Agents.UsageWindow>(), accounts.accounts.first().usageWindows)
    }

    @Test fun preservesProviderSubscriptionAllWindowsAndStaleUsageError() {
        val account = snapshot("""
            {"id":"live","harness":"pi","email":"me@example.com","provider":"anthropic",
             "planLabel":"Max","active":true,"usageError":"Could not refresh",
             "usageWindows":[{"label":"5 hour","usedFraction":0.2,"resetsAt":"2026-10-01T15:00:00Z"},
               {"label":"Weekly","usedFraction":0.6},{"label":"Sonnet weekly","usedFraction":0.4}]}
        """).accounts.single()
        assertEquals("anthropic", account.provider)
        assertEquals("Max", account.plan)
        assertEquals("me@example.com", account.title)
        assertEquals(listOf("5 hour", "Weekly", "Sonnet weekly"), account.usageWindows.map { it.label })
        assertEquals("2026-10-01T15:00:00Z", account.usageWindows.first().resetsAt)
        assertNull(account.usageWindows.last().resetsAt)
        assertEquals("Could not refresh", account.usageError)
    }

    @Test fun malformedWindowsAreSkippedAndReportedFractionsClamped() {
        val account = snapshot("""
            {"id":"live","harness":"codex","active":true,"usageWindows":[
              null,{}, {"usedFraction":null},{"usedFraction":"NaN"},{"usedFraction":"Infinity"},
              {"label":"Empty","usedFraction":-0.1},{"label":"Full","usedFraction":1.2}]}
        """).accounts.single()
        assertEquals(listOf(0f, 1f), account.usageWindows.map { it.usedFraction })
        assertEquals("100% used", AccountUsage.percent(account.usageWindows.last().usedFraction))
    }

    @Test fun zeroUsageIsReportedAndRoundedLikeDesktop() {
        val accounts = snapshot("""{"id":"live","harness":"codex","active":true,"usageWindows":[{"usedFraction":0}]}""")
        assertEquals(0f, AccountUsage.fraction(accounts, "codex")!!, 0f)
        assertEquals("0% used", AccountUsage.percent(0f))
        assertEquals("64% used", AccountUsage.percent(.644f))
    }

    private val now = Instant.parse("2026-10-01T12:00:00Z")
    private fun reset(at: String?, zone: String = "UTC") = AccountUsage.reset(at, now, ZoneId.of(zone), Locale.US)

    @Test fun resetUsesLocalTimeThenWeekdayThenDateAtDesktopBoundaries() {
        assertEquals("resets 8:00 AM", reset("2026-10-01T15:00:00Z", "America/Los_Angeles"))
        assertEquals("resets 9:59 AM", reset("2026-10-02T09:59:00Z"))
        assertEquals("resets Fri", reset("2026-10-02T10:00:00Z"))
        assertEquals("resets Thu", reset("2026-10-08T11:59:00Z"))
        assertEquals("resets Oct 8", reset("2026-10-08T12:00:00Z"))
    }

    @Test fun resetAcceptsOffsetsAndOmitsUnavailableOrMalformedTimes() {
        assertEquals("resets 3:00 PM", reset("2026-10-01T08:00:00-07:00"))
        assertNull(reset(null))
        assertNull(reset(""))
        assertNull(reset("unknown"))
    }
}
