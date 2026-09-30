package sh.zeron.android.core

import org.json.JSONArray
import org.json.JSONObject
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import sh.zeron.android.design.ZIcons
import sh.zeron.runtime.CustomServer
import sh.zeron.runtime.RuntimeState
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.ProjectView

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

class PhoneEngineTest {
    @Test fun repoNames() {
        assertEquals("widgets", PhoneEngine.repoName("https://github.com/acme/widgets.git"))
        assertEquals("widgets", PhoneEngine.repoName("git@github.com:acme/widgets.git"))
        assertEquals("dot.files", PhoneEngine.repoName("https://example.com/me/dot.files/"))
        assertNull(PhoneEngine.repoName("widgets"))
        assertNull(PhoneEngine.repoName("https://example.com/a b"))
        assertNull(PhoneEngine.repoName("https://example.com/.."))
    }

    @Test fun folderNames() {
        assertEquals("my-app", PhoneEngine.folderName(" my-app "))
        assertNull(PhoneEngine.folderName("a/b"))
        assertNull(PhoneEngine.folderName(""))
    }

    @Test fun shellQuote() {
        assertEquals("'it'\\''s'", PhoneEngine.shellQuote("it's"))
    }

    @Test fun logColoursStripped() {
        assertEquals("2026 INFO engine", PhoneEngine.stripAnsi("\u001B[2m2026\u001B[0m \u001B[32mINFO\u001B[0m engine"))
    }

    @Test fun stateLabels() {
        assertEquals("Setting up", PhoneEngine.stateLabel(RuntimeState.Bootstrapping("Extracting", 0.5f)))
        assertEquals("Stopped with an error", PhoneEngine.stateLabel(RuntimeState.Failed("boom", "")))
    }
}

class NotifierTest {
    @Test fun transitions() {
        assertEquals(Notifier.Kind.Done, Notifier.transition(ChatIndicator.WORKING, ChatIndicator.IDLE))
        assertEquals(Notifier.Kind.Done, Notifier.transition(ChatIndicator.WORKING, ChatIndicator.COMPLETED))
        assertEquals(Notifier.Kind.Input, Notifier.transition(ChatIndicator.WORKING, ChatIndicator.AWAITING_INPUT))
        assertEquals(Notifier.Kind.Failed, Notifier.transition(ChatIndicator.IDLE, ChatIndicator.ERRORED))
        assertNull(Notifier.transition(ChatIndicator.IDLE, ChatIndicator.WORKING))
        assertNull(Notifier.transition(ChatIndicator.IDLE, ChatIndicator.IDLE))
    }
}

class DeviceModelTest {
    private fun device(id: String, name: String, platform: String, online: Boolean = true, self: Boolean = false, host: Boolean = true) =
        DeviceView(id, name, platform, online, null, null, if (host) listOf("cap") else emptyList(), host, self, 0u)

    private fun project(id: String, device: String, online: Boolean = true) =
        ProjectView(id, id, "/home/zeron/projects/$id", 0u, device, null, online, true, 0L, ChatIndicator.IDLE, 0u, emptyList())

    @Test fun thePhoneIsAMachineLikeAnyComputer() {
        val phone = device("android-1", "Pixel", "android", self = true)
        val mac = device("mac-1", "MacBook", "macos")
        val box = device("box-1", "build box", "linux", online = false)
        val viewer = device("ios-1", "iPhone", "ios", host = false)
        val groups = Machines.groups(
            listOf(project("zeron", "mac-1"), project("notes", "android-1"), project("old", "gone-1", online = false)),
            listOf(box, viewer, mac, phone),
        )
        // This device first, then online ones, then the rest; viewers never.
        assertEquals(listOf("android-1", "mac-1", "box-1", "gone-1"), groups.map { it.deviceId })
        assertEquals(listOf("notes"), groups[0].projects.map { it.id })
        assertTrue(groups[0].isSelf)
        // A host without projects still offers "No project" on it.
        assertTrue(groups[2].projects.isEmpty())
        assertFalse(groups[2].online)
        assertEquals("Device", groups[3].name)
    }

    @Test fun deviceIcons() {
        assertEquals(ZIcons.Phone, DeviceIdentity.icon("android"))
        assertEquals(ZIcons.Server, DeviceIdentity.icon("linux"))
        assertEquals(ZIcons.Laptop, DeviceIdentity.icon("macos"))
    }

    @Test fun aClientIsBuiltForOneEngineIdentity() {
        val local = DeviceIdentity.key("android-1", "http://127.0.0.1:27655", "local", "local")
        val synced = DeviceIdentity.key("android-1", "https://edge.zeron.sh", "user_1", "org_1")
        assertTrue(local != synced)
        assertEquals(synced, DeviceIdentity.key("android-1", "https://edge.zeron.sh", "user_1", "org_1"))
    }

    @Test fun accounts() {
        assertEquals("Wing", Account.SignedIn("Wing", "wing@x.dev", "Acme").title)
        assertEquals("wing@x.dev · Acme", Account.SignedIn("Wing", "wing@x.dev", "Acme").detail)
        assertEquals("wing@x.dev", Account.SignedIn(null, "wing@x.dev", null).title)
        assertEquals("Zeron account", Account.SignedIn(null, "wing@x.dev", null).detail)
        assertEquals("Not signed in", Account.Local(false).title)
        assertEquals("http://10.0.2.2:27700", Account.Server("http://10.0.2.2:27700").detail)
    }

    @Test fun idleEngines() {
        assertTrue(RuntimeState.NotInstalled.isIdle())
        assertTrue(RuntimeState.Failed("x", "").isIdle())
        assertFalse(RuntimeState.Starting.isIdle())
        assertFalse(RuntimeState.Running(27654, "t", "Pixel").isIdle())
    }
}

class CustomServerTest {
    @Test fun validatesLikeTheEdge() {
        val token = "xferdev0123456789abcdef"
        assertNull(CustomServer.problem("http://10.0.2.2:27720", token))
        assertNull(CustomServer.problem(" https://edge.example.dev/ ", token))
        assertTrue(CustomServer.problem("10.0.2.2:27720", token) != null)
        assertTrue(CustomServer.problem("http://", token) != null)
        assertTrue(CustomServer.problem("http://10.0.2.2:27720", "short") != null)
        assertTrue(CustomServer.problem("http://10.0.2.2:27720", "has spaces in the token") != null)
        assertEquals(CustomServer("http://10.0.2.2:27720", token), CustomServer.of(" http://10.0.2.2:27720/ ", " $token "))
    }
}
