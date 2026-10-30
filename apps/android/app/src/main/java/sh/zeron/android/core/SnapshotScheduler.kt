package sh.zeron.android.core

import java.util.concurrent.Executor
import java.util.concurrent.atomic.AtomicBoolean

/**
 * When [AppModel] builds a workspace snapshot ([C] is the client, [S] the
 * snapshot) and which builds it publishes. Generic so the rules can be tested
 * without the native core.
 *
 * Registry frames build on [background] (one thread, so builds finish in the
 * order they started) and publish on [main]; bursts coalesce to at most one
 * build running and one queued. Revisions only move forward: a build that
 * finishes after a fresher one, or after its client was replaced, is dropped.
 */
internal class SnapshotScheduler<C : Any, S : Any>(
    private val background: Executor,
    private val main: Executor,
    /** The client in use now (main thread). */
    private val currentClient: () -> C?,
    /** The published snapshot; null until the first one (and after sign-out). */
    private val published: () -> S?,
    private val setPublished: (S) -> Unit,
    private val revisionOf: (S) -> ULong,
    /** The client's current revision, without building (`workspaceRevision()`). */
    private val clientRevision: (C) -> ULong,
    /** The FFI build (`workspace()`). */
    private val build: (C) -> S,
) {
    private val queued = AtomicBoolean(false)

    /** The client [published] was built from (revisions are per client). */
    @Volatile private var publishedBy: C? = null

    /** Main thread. Rebuild [client]'s snapshot in the background; [onPublished] runs on [main] if it's published. */
    fun schedule(client: C, onPublished: () -> Unit) {
        if (!queued.compareAndSet(false, true)) return
        background.execute {
            queued.set(false)
            val snapshot = runCatching { buildIfChanged(client) }.getOrNull() ?: return@execute
            main.execute { if (tryPublish(client, snapshot)) onPublished() }
        }
    }

    /** Main thread. Rebuild now, for callers that read the result straight back. */
    fun refresh(client: C) {
        buildIfChanged(client)?.let { tryPublish(client, it) }
    }

    /** [client]'s snapshot, or null when the published one is already its current revision. */
    private fun buildIfChanged(client: C): S? {
        val current = published()
        if (current != null && publishedBy === client && clientRevision(client) == revisionOf(current)) return null
        return build(client)
    }

    /** Main thread. True when published. */
    private fun tryPublish(client: C, snapshot: S): Boolean {
        if (client !== currentClient()) return false
        val current = published()
        if (current != null && publishedBy === client && revisionOf(snapshot) <= revisionOf(current)) return false
        publishedBy = client
        setPublished(snapshot)
        return true
    }
}
