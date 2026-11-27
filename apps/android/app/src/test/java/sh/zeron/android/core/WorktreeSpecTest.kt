package sh.zeron.android.core

import org.junit.Assert.assertEquals
import org.junit.Assert.assertNull
import org.junit.Test

/**
 * The worktree a new session asks its host for: off a branch in a git
 * project, of the repositories chosen in a folder holding several, and none
 * anywhere else (the host would refuse it and leave an empty chat).
 */
class WorktreeSpecTest {
    private fun draft(worktree: Boolean, branch: String? = null, repositories: List<String>? = null) =
        NewSessionDraft(worktree = worktree, branch = branch, repositories = repositories)

    @Test
    fun a_git_project_gets_the_worktree_it_was_asked() {
        val spec = worktreeSpec(draft(true, branch = "release"), "/code/atlas", "atlas", gitDetected = true)!!
        assertEquals("/code/atlas", spec.repoPath)
        assertEquals("release", spec.base)
        assertEquals("atlas", spec.spaceId)
        assertEquals(emptyList<String>(), spec.repositories)
        assertEquals("HEAD", worktreeSpec(draft(true), "/code/atlas", "atlas", true)!!.base)
    }

    @Test
    fun a_folder_of_repositories_gets_one_worktree_of_those_chosen() {
        val spec = worktreeSpec(draft(true, repositories = listOf("api", "packages/web")), "/code/acme", "acme", gitDetected = false)!!
        assertEquals("/code/acme", spec.repoPath)
        assertEquals("HEAD", spec.base)
        assertEquals(listOf("api", "packages/web"), spec.repositories)
    }

    @Test
    fun a_plain_folder_or_none_chosen_runs_in_place() {
        // (A pick kept from a git project is never sent for a plain folder.)
        assertNull(worktreeSpec(draft(true), "/code/notes", "notes", gitDetected = false))
        assertNull(worktreeSpec(draft(true, repositories = emptyList()), "/code/acme", "acme", gitDetected = false))
    }

    @Test
    fun no_pick_no_worktree() {
        assertNull(worktreeSpec(draft(false, repositories = listOf("api")), "/code/acme", "acme", gitDetected = false))
        assertNull(worktreeSpec(draft(false), "/code/atlas", "atlas", gitDetected = true))
    }
}
