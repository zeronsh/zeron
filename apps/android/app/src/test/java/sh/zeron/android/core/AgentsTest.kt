package sh.zeron.android.core

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.design.ZIcons

class AgentsTest {
    @Test fun harnessCatalog() {
        val json = JSONArray(
            """[{"id":"claude-code","name":"Claude Code","installed":true,"canInstall":true,"enabled":true},
               {"id":"grok","name":"Grok","installed":false,"canInstall":true},
               {"id":"mock","name":"Mock"},
               {"id":"old","name":"Old engine"}]""",
        )
        val list = Agents.harnesses(json)
        assertEquals(listOf("claude-code", "grok", "old"), list.map { it.id })
        assertFalse(list[1].installed)
        assertNull(list[1].enabled)
        assertTrue("engines predating the field read as installed", list[2].installed)
        assertFalse(list[2].canInstall)
    }

    @Test fun accountsAndLogins() {
        val snap = Agents.accounts(JSONObject("""{"accounts":[{"id":"a1","harness":"codex","email":"me@x.dev","planLabel":"Pro","active":true}],"warnings":[{"harness":"claude-code","message":"Keychain denied"}]}"""))
        assertEquals("me@x.dev", snap.forHarness("codex").single().title)
        assertEquals("Pro", snap.forHarness("codex").single().plan)
        assertEquals("Keychain denied", snap.warnings["claude-code"])
        assertEquals(Agents.LoginMode.PasteCode, Agents.loginStart(JSONObject("""{"loginId":"l1","url":"https://x","mode":"paste-code"}"""))?.mode)
        assertEquals(Agents.LoginMode.Browser, Agents.loginStart(JSONObject("""{"loginId":"l2","url":"https://y","mode":"browser"}"""))?.mode)
        assertNull(Agents.loginStart(JSONObject("{}")))
        assertEquals(Agents.LoginPoll.Done, Agents.loginPoll(JSONObject("""{"status":"done"}""")))
        assertEquals(Agents.LoginPoll.Failed("nope"), Agents.loginPoll(JSONObject("""{"status":"error","message":"nope"}""")))
        assertEquals(Agents.LoginPoll.Pending("https://late"), Agents.loginPoll(JSONObject("""{"status":"pending","url":"https://late"}""")))
        assertEquals("2.1.0", Agents.versions(JSONArray("""[{"harness":"codex","installedVersion":"2.1.0","phase":"current"}]"""))["codex"]?.installed)
    }

    @Test fun updateStatuses() {
        val statuses = Agents.versions(
            JSONArray(
                """[{"harness":"claude-code","installedVersion":"2.1.10","latestVersion":"2.1.14","phase":"available","canApply":true},
                   {"harness":"codex","installedVersion":"0.41.0","latestVersion":"0.42.0","phase":"available","canApply":false,"manualCommand":"brew upgrade codex"},
                   {"harness":"grok","phase":"installing","progress":{"message":"Downloading"}},
                   {"harness":"pi","phase":"failed","error":{"message":"npm exited with 1","retryable":true}},
                   {"harness":"opencode"}]""",
            ),
        )
        assertTrue(statuses.getValue("claude-code").updatable)
        assertFalse("package-manager installs are reported, not applied", statuses.getValue("codex").updatable)
        assertTrue(statuses.getValue("codex").available)
        assertEquals("brew upgrade codex", statuses.getValue("codex").manualCommand)
        assertTrue(statuses.getValue("grok").busy)
        assertEquals("Downloading", statuses.getValue("grok").progress)
        assertEquals("npm exited with 1", statuses.getValue("pi").error)
        assertEquals("dormant", statuses.getValue("opencode").phase)
    }

    @Test fun uninstallAndUpdateAllReplies() {
        val done = Agents.uninstall(
            JSONObject(
                """{"harness":"claude-code","removed":["~/.local/bin/claude","~/.local/share/claude"],"dryRun":false,
                   "remaining":"Claude Code is installed outside Zeron (/opt/homebrew/bin/claude); remove it with `brew uninstall --cask claude-code`",
                   "harnesses":[{"id":"claude-code","name":"Claude Code","installed":true}]}""",
            ),
        )
        assertEquals(listOf("~/.local/bin/claude", "~/.local/share/claude"), done.removed)
        assertTrue(done.remaining!!.contains("brew uninstall"))
        assertEquals("claude-code", done.harnesses!!.single().id)
        assertNull(Agents.uninstall(JSONObject("""{"removed":[]}""")).harnesses)

        val all = Agents.updateAll(
            JSONObject(
                """{"updated":[{"harness":"grok","version":"1.0.41"}],"failed":[{"harness":"hermes","error":"network unreachable"}],
                   "manual":["codex"],"statuses":[{"harness":"grok","installedVersion":"1.0.41","phase":"updated"}]}""",
            ),
        )
        assertEquals(listOf("grok"), all.updated)
        assertEquals("network unreachable", all.failed["hermes"])
        assertEquals(listOf("codex"), all.manual)
        assertEquals("1.0.41", all.statuses["grok"]?.installed)
    }

    @Test fun hostCallReplies() {
        assertTrue(Agents.parse(" [1]") is JSONArray)
        assertTrue(Agents.parse("{}") is JSONObject)
        assertEquals("null", Agents.parse("null"))
    }

    @Test fun relayTimeouts() {
        assertTrue(Agents.isTimeout("InstallHarness on dev-1 timed out"))
        assertFalse(Agents.isTimeout("dev-1: connection closed"))
    }
}

class DeviceIdentityTest {
    @Test fun deviceIcons() {
        assertEquals(ZIcons.Phone, DeviceIdentity.icon("android"))
        assertEquals(ZIcons.Server, DeviceIdentity.icon("linux"))
        assertEquals(ZIcons.Laptop, DeviceIdentity.icon("macos"))
    }
}
