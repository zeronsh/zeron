package sh.zeron.android.schedule

import sh.zeron.android.core.AppLanguage
import sh.zeron.android.R
import android.content.Context
import android.os.SystemClock
import kotlinx.coroutines.delay
import sh.zeron.android.core.CoreConnect
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.DirectPhase
import uniffi.zeron_core.SendOutcome
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SendState
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.WorktreeSpec
import sh.zeron.android.core.NewSessionConfig

/**
 * Delivers one scheduled message: reuse the app's client when it is on the
 * same workspace, otherwise open one the way the app does (CoreConnect),
 * wait for the link, send like the composer's normal send (queue behind a
 * running turn), and wait for the host to take it. Connection problems are
 * retried until [budgetMs] runs out (the core also reconnects on its own).
 */
class ScheduledSender(private val context: Context) {
    sealed interface Result {
        data class Sent(val message: ScheduledMessage) : Result
        /** Handed to the core's outbox but not confirmed; it goes out on reconnect. */
        data class Pending(val message: ScheduledMessage) : Result
        data class Failed(val message: ScheduledMessage, val reason: String) : Result
        data object Gone : Result
    }

    suspend fun deliver(id: String, budgetMs: Long = BUDGET_MS): Result {
        // Claim it first: a second alarm or a restart can never send it twice.
        val message = ScheduledStore(context).take(id) ?: return Result.Gone
        val deadline = SystemClock.elapsedRealtime() + budgetMs
        val live = CoreConnect.live?.takeIf { it.workspace == message.workspace }
        var owned: CoreClient? = null
        try {
            val client = live?.client ?: openWithRetry(message.workspace, deadline).also { owned = it }
            if (!awaitLink(client, deadline)) return Result.Failed(message, linkError(client) ?: AppLanguage.string(context, R.string.sched_connect_timeout))
            return send(client, message, deadline)
        } catch (t: CoreConnect.Unavailable) {
            return Result.Failed(message, t.message ?: AppLanguage.string(context, R.string.sched_unavailable))
        } catch (t: Throwable) {
            return Result.Failed(message, t.message ?: t.javaClass.simpleName)
        } finally {
            owned?.let {
                runCatching { it.shutdown() }
                runCatching { it.close() }
            }
        }
    }

    private suspend fun openWithRetry(workspace: String, deadline: Long): CoreClient {
        var last: Throwable? = null
        while (SystemClock.elapsedRealtime() < deadline) {
            try {
                return CoreConnect.open(context, workspace)
            } catch (t: CoreConnect.Unavailable) {
                throw t
            } catch (t: Throwable) {
                last = t
                delay(RETRY_GAP_MS)
            }
        }
        throw last ?: IllegalStateException("Couldn't open the workspace")
    }

    /** Waits for a usable link; kicks a stuck direct link every [RETRY_GAP_MS]. */
    private suspend fun awaitLink(client: CoreClient, deadline: Long): Boolean {
        var lastKick = SystemClock.elapsedRealtime()
        while (SystemClock.elapsedRealtime() < deadline) {
            if (client.isDemo()) return true
            if (client.isDirect()) {
                val status = runCatching { client.directStatus() }.getOrNull()
                if (status?.phase == DirectPhase.LIVE) return true
                if (status?.phase == DirectPhase.FAILED && SystemClock.elapsedRealtime() - lastKick > RETRY_GAP_MS) {
                    runCatching { client.reconnectDirect() }
                    lastKick = SystemClock.elapsedRealtime()
                }
            } else {
                val state = runCatching { client.connectivity().state }.getOrNull()
                if (state == ConnectivityState.CONNECTED || state == ConnectivityState.DISABLED) return true
            }
            delay(1_000)
        }
        return false
    }

    private fun linkError(client: CoreClient): String? =
        runCatching { client.directStatus()?.lastError }.getOrNull()

    private suspend fun send(client: CoreClient, original: ScheduledMessage, deadline: Long): Result {
        // The session list may still be syncing right after the link comes up.
        var message = original
        val spec = message.newSession
        var outcome: SendOutcome? = null
        var handle: uniffi.zeron_core.SessionHandle? = null
        while (outcome == null) {
            try {
                // New session: create it exactly like an immediate send from
                // the New Session screen, once (the id is kept across retries).
                if (spec != null && message.chatId.isEmpty()) message = message.copy(chatId = createSession(client, spec))
                val h = handle ?: client.openSession(message.chatId).also { handle = it }
                outcome = h.send(SendRequest(text = message.text, attachments = emptyList(), worktree = spec?.let { worktreeSpec(it) }, busy = BusyPolicy.QUEUE))
            } catch (t: Throwable) {
                if (SystemClock.elapsedRealtime() >= deadline) return Result.Failed(message, t.message ?: AppLanguage.string(context, R.string.sched_send_failed))
                delay(2_000)
            }
        }
        val h = handle!!
        val messageId = (outcome as? SendOutcome.Started)?.messageId
        // Wait for the host to take it; retry delivery if the core marks it failed.
        val confirmBy = maxOf(deadline, SystemClock.elapsedRealtime() + CONFIRM_MS)
        var lastRetry = SystemClock.elapsedRealtime()
        while (SystemClock.elapsedRealtime() < confirmBy) {
            val composer = runCatching { h.composer() }.getOrNull()
            if (composer != null) {
                val pending = messageId != null && composer.pendingSends.any { it.messageId == messageId }
                val state = composer.sendState
                if (!pending && state != SendState.FAILED && state != SendState.QUEUED) return Result.Sent(message)
                if (state == SendState.FAILED && SystemClock.elapsedRealtime() - lastRetry > 15_000) {
                    runCatching { h.retryDelivery() }
                    lastRetry = SystemClock.elapsedRealtime()
                }
            }
            delay(1_000)
        }
        return Result.Pending(message)
    }

    private fun createSession(client: CoreClient, spec: NewSessionSpec): String {
        val target = when {
            spec.projectId != null -> SessionTarget.Project(spec.projectId)
            spec.hostId != null -> SessionTarget.Projectless(spec.hostId)
            else -> throw IllegalStateException(AppLanguage.string(context, R.string.choose_project_or_host))
        }
        return client.createSession(NewSession(target, NewSessionConfig.chatConfig(spec.harness, spec.model, spec.effort), if (spec.worktree) null else spec.branch, null, null))
    }

    /** "New worktree" rides on the first send, as in the New Session screen. */
    private fun worktreeSpec(spec: NewSessionSpec): WorktreeSpec? =
        if (spec.worktree && spec.projectPath != null && spec.projectId != null) WorktreeSpec(spec.projectPath, spec.branch ?: "HEAD", spec.projectId) else null

    companion object {
        /** About two minutes of connection retries. */
        const val BUDGET_MS = 120_000L
        const val RETRY_GAP_MS = 30_000L
        const val CONFIRM_MS = 20_000L
    }
}
