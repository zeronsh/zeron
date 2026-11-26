package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertNull
import org.junit.Assert.assertTrue
import org.junit.Test
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.InputRequest
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.SubagentRow
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.UserInputQuestion

class CompanionsTest {
    private fun child(id: String, delegated: Boolean = false, archived: Boolean = false, waitingOn: List<String> = emptyList()) = SessionRow(
        id = id, revision = 0u, title = id, hasTitle = true, preview = null, project = null,
        deviceId = "mac", deviceName = null, deviceOnline = true, isCloud = false,
        harness = null, harnessLabel = null, model = null, modelLabel = null, reasoning = null,
        branch = null, cwd = null, indicator = ChatIndicator.IDLE, hostIndicator = ChatIndicator.IDLE,
        workingSinceMs = null, lastActivityMs = 0, timeLabel = "", createdAtMs = 0,
        unseen = false, archived = archived, pinned = false, sectionId = null, pullRequest = null,
        sendState = null, parentChatId = "parent", delegated = delegated, waitingOn = waitingOn,
        waitingSinceMs = null, childAsks = false, roomGen = 2u, sleepUntilMs = null, wokeAtMs = null, waitsOnProcess = false,
    )

    private fun agent(id: String, chat: String? = null, state: SubagentState = SubagentState.RUNNING, asks: Boolean = false) = SubagentRow(
        id = id, chatId = chat, deviceName = null, harness = null, asks = asks, docId = null,
        title = id, model = null, kind = null, state = state, tail = null, startedAtMs = 0,
    )

    @Test
    fun delegated_children_are_subagents_not_side_chats() {
        val sides = sideChatsOf(listOf(child("side"), child("audit", delegated = true), child("old", archived = true)))
        assertEquals(listOf("side"), sides.map { it.id })
    }

    @Test
    fun delegated_chats_follow_the_agents_own_spawns() {
        val order = orderedAgents(
            listOf(
                agent("spawn-done", state = SubagentState.DONE),
                agent("bench", chat = "c-bench", state = SubagentState.DONE),
                agent("spawn-live"),
                agent("notes", chat = "c-notes"),
                agent("audit", chat = "c-audit", asks = true),
            ),
        )
        // Spawns live first; then delegated chats, the one that asks first, live before done.
        assertEquals(listOf("spawn-live", "spawn-done", "audit", "notes", "bench"), order.map { it.id })
    }

    @Test
    fun a_delegated_rows_detail_is_its_agent_and_other_device() {
        assertEquals("Codex · Build box", delegatedDetail(agent("a", chat = "c").copy(kind = "Codex", deviceName = "Build box")))
        assertEquals("Claude Code", delegatedDetail(agent("a", chat = "c").copy(kind = "Claude Code")))
    }

    @Test
    fun an_approval_shows_what_it_would_run() {
        assertEquals("cargo fuzz run parse -- -max_total_time=60", askCommand("Run `cargo fuzz run parse -- -max_total_time=60`?"))
        assertEquals("rm -rf build\nls", askCommand("Allow this?\n```sh\nrm -rf build\nls\n```"))
        assertNull(askCommand("Edit src/parse.rs?"))
        assertNull(askCommand("Run ``?"))
    }

    private fun question(options: List<String>, multi: Boolean = false) =
        UserInputQuestion(id = "q", header = "", question = "Which branch?", options = options, multiSelect = multi, prefill = null, multiline = false)

    @Test
    fun only_a_short_single_pick_answers_in_place() {
        fun input(vararg q: UserInputQuestion, approval: Boolean = false) = InputRequest("e", "r", q.toList(), approval)
        assertTrue(answersInPlace(input(question(listOf("main", "release/1.9", "next")))))
        assertFalse(answersInPlace(input(question(listOf("a", "b", "c", "d", "e", "f")))))
        assertFalse(answersInPlace(input(question(listOf("a", "b"), multi = true))))
        assertFalse(answersInPlace(input(question(emptyList()))))
        assertFalse(answersInPlace(input(question(listOf("a")), question(listOf("b")))))
        assertFalse(answersInPlace(input(question(listOf("Yes", "No")), approval = true)))
    }

    @Test
    fun a_waiting_chat_says_what_it_waits_on() {
        assertNull(waitingLine(child("idle")))
        assertEquals("Waiting on Codex · Build box · Audit the parser", waitingLine(child("w", waitingOn = listOf("Codex · Build box · Audit the parser"))))
        assertEquals(
            "Waiting on Codex · Build box · Audit the parser (+1 more)",
            waitingLine(child("w", waitingOn = listOf("Codex · Build box · Audit the parser", "Claude Code · Draft the release notes"))),
        )
    }

    @Test
    fun a_child_heads_itself_by_how_it_was_made() {
        assertEquals("Subagent", childHeading(child("audit", delegated = true)))
        assertEquals("Side chat", childHeading(child("side")))
        assertEquals("Side chat", childHeading(null))
    }
}
