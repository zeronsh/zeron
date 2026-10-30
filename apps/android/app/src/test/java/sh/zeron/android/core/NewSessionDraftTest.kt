package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertSame
import org.junit.Test
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ProjectView

private fun project(id: String, cloud: Boolean = false) = ProjectView(
    id = id, name = id, path = "/srv/$id", colorIndex = 0u, deviceId = "d1", deviceName = "laptop", deviceOnline = true,
    isCloud = cloud, githubRepo = null, gitDetected = true, createdAtMs = 0, groupKey = id, groupName = id,
    indicator = ChatIndicator.IDLE, unseenCount = 0u, sessions = emptyList(),
)

class NewSessionDraftTest {
    private val draft = NewSessionDraft(
        projectId = "a", hostId = "d2", harness = "codex", model = "gpt-5", effort = "high", branch = "main", worktree = true, cloud = true,
    )

    @Test fun anotherProjectDropsTheOldHostAndBranch() {
        val moved = draft.inProject("b", listOf(project("a"), project("b")))
        assertEquals(draft.copy(projectId = "b", hostId = null, branch = null, cloud = false), moved)
    }

    @Test fun keepsTheHarnessModelAndWorktreeChoice() {
        val moved = draft.inProject("b", listOf(project("b")))
        assertEquals(listOf("codex", "gpt-5", "high", true), listOf(moved.harness, moved.model, moved.effort, moved.worktree))
    }

    @Test fun aCloudOnlyProjectRunsOnCloud() {
        assertEquals(true, NewSessionDraft().inProject("c", listOf(project("c", cloud = true))).cloud)
    }

    @Test fun theSameProjectIsLeftAsIs() {
        assertSame(draft, draft.inProject("a", listOf(project("a"))))
    }

    @Test fun anUnknownProjectIsntCloud() {
        assertEquals(NewSessionDraft(projectId = "x"), NewSessionDraft(cloud = true).inProject("x", null))
    }
}
