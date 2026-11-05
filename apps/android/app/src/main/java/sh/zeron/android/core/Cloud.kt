package sh.zeron.android.core

import android.content.Context
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Job
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.zeron_core.BillingInfo
import uniffi.zeron_core.CloudAccountInfo
import uniffi.zeron_core.CloudInfo
import uniffi.zeron_core.CloudSessionInfo
import uniffi.zeron_core.CloudUsageInfo
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.GithubRepoInfo
import uniffi.zeron_core.VaultInfo
import java.text.NumberFormat
import java.text.SimpleDateFormat
import java.util.Date
import java.util.Locale
import kotlin.coroutines.cancellation.CancellationException
import kotlin.math.floor

/**
 * Cloud for this account (iOS `CloudStore`): status and credits, GitHub, the
 * agents' logins, billing. Every screen reads it; reads are cached so a page
 * paints at once and revalidates. Snapshot state, main thread only.
 */
@Stable
class CloudStore(private val scope: CoroutineScope, private val client: () -> CoreClient?, private val workspaceChanged: () -> Unit) {
    /** The agents Cloud runs, with the vault providers behind them. */
    data class Agent(
        val harness: String,
        val name: String,
        /** Subscription sign-in provider (`claude` · `codex`). */
        val login: String,
        /** API key provider (`anthropic-key` · `openai-key`). */
        val key: String,
        /** What signing in is called ("Sign in with Claude"). */
        val signInTitle: String,
        val keyName: String,
    )

    /** What's wrong with Cloud as a whole, for the top of its page. */
    data class Problem(val title: String, val detail: String, /** Credits fix it: the row leads to Billing. */ val billing: Boolean)

    var info by mutableStateOf<CloudInfo?>(null); private set
    var vault by mutableStateOf<VaultInfo?>(null); private set
    var repos by mutableStateOf<List<GithubRepoInfo>?>(null); private set
    var accounts by mutableStateOf<Map<String, List<CloudAccountInfo>>>(emptyMap()); private set
    /** Each Cloud login's plan usage by `provider:slot` (read by the vault), and why it couldn't be read. */
    var usage by mutableStateOf<Map<String, CloudUsageInfo>>(emptyMap()); private set
    var usageErrors by mutableStateOf<Map<String, String>>(emptyMap()); private set
    var billing by mutableStateOf<BillingInfo?>(null); private set
    /** Why the last billing read failed (shown with Retry until one succeeds). */
    var billingError by mutableStateOf<String?>(null); private set
    /** The account's session machines (read when a Cloud chat needs them). */
    var sessions by mutableStateOf<List<CloudSessionInfo>>(emptyList()); private set
    var loading by mutableStateOf(false); private set
    var error by mutableStateOf<String?>(null); private set

    // ── reading ───────────────────────────────────────────────────────────

    val available: Boolean get() = info?.available == true
    val credits: Double? get() = billing?.creditsTotal ?: info?.credits

    /** Cloud can't start or wake a machine (the desktop's rule). */
    val outOfCredits: Boolean get() = (info?.credits ?: billing?.creditsTotal)?.let { it <= 0 } == true

    val problem: Problem?
        get() {
            val i = info ?: return null
            if (!i.available) return null
            if (i.state == "error") return Problem("Cloud ran into a problem", i.error ?: "Something went wrong with Cloud.", billing = false)
            if (outOfCredits) return Problem("Out of credits", "Cloud sessions can't start or wake until you add credits.", billing = true)
            return null
        }

    /** Why a Cloud chat's machine isn't running, when the edge said. */
    fun sessionError(chatId: String, device: String?): String? {
        val s = sessions.firstOrNull { it.chatId == chatId } ?: sessions.firstOrNull { it.deviceId == device } ?: return null
        return if (s.state == "error") s.error ?: "Its machine failed to start." else null
    }

    val githubAccount: String? get() = vault?.connections?.firstOrNull { it.provider == "github" && !it.needsReconnect }?.account
    val githubConnected: Boolean get() = vault?.connections?.any { it.provider == "github" && !it.needsReconnect } == true

    /** The active login (or key) Cloud uses for `agent`. */
    fun activeAccount(agent: Agent): CloudAccountInfo? =
        accounts[agent.login]?.firstOrNull { it.active } ?: accounts[agent.key]?.firstOrNull { it.active }

    fun hasAccount(agent: Agent): Boolean =
        !accounts[agent.login].isNullOrEmpty() || !accounts[agent.key].isNullOrEmpty() ||
            vault?.connections?.any { (it.provider == agent.login || it.provider == agent.key) && it.accounts > 0u } == true

    /** Cloud reaches `repo` (`owner/name`): its sessions can run there. */
    fun reaches(repo: String?): Boolean {
        if (!available || repo == null) return false
        return repos?.any { it.fullName.equals(repo, ignoreCase = true) } == true
    }

    // ── loading ───────────────────────────────────────────────────────────

    private var refreshJob: Job? = null
    /** How long until a failed status read is tried again (30 s, doubling to 5 min). */
    private var retryDelayMs = 30_000L
    private var sessionsReadAt = 0L

    /** Re-read everything (status first: the rest needs Cloud set up). */
    fun refresh() {
        val c = client() ?: return
        if (refreshJob?.isActive == true) return
        loading = true
        refreshJob = scope.launch {
            try {
                var status = c.cloudStatus()
                // Cloud is part of every signed-in account: set it up.
                if (status.available && status.state == "off") status = runCatching { c.cloudEnable() }.getOrDefault(status)
                if (client() !== c) return@launch
                info = status
                error = null
                if (!status.available) return@launch
                coroutineScope {
                    val v = async { runCatching { c.vaultStatus() }.getOrNull() }
                    val r = async { runCatching { c.githubRepos(null) }.getOrNull() }
                    val b = async { runCatching { c.billingStatus() }.getOrNull() }
                    val lists = readAccounts(c)
                    if (client() !== c) return@coroutineScope
                    vault = v.await()
                    repos = r.await() ?: emptyList()
                    billing = b.await()
                    accounts = lists
                }
                retryDelayMs = 30_000L
                refreshUsage()
            } catch (e: CancellationException) {
                throw e
            } catch (e: Exception) {
                if (client() !== c) return@launch
                error = message(e)
                // Until a read answers, Cloud reads as not offered: ask again rather than hide it for good.
                val wait = retryDelayMs
                retryDelayMs = minOf(wait * 2, 300_000L)
                scope.launch {
                    delay(wait)
                    if (client() === c && (info == null || error != null)) refresh()
                }
            } finally {
                if (client() === c) loading = false
            }
        }
    }

    private suspend fun readAccounts(c: CoreClient): Map<String, List<CloudAccountInfo>> = coroutineScope {
        agents.flatMap { listOf(it.login, it.key) }
            .map { provider -> async { provider to (runCatching { c.vaultAccounts(provider) }.getOrNull() ?: emptyList()) } }
            .awaitAll()
            .toMap()
    }

    /** Re-read the session machines (at most every few seconds). */
    fun refreshSessions() {
        val c = client() ?: return
        if (!available && info != null) return
        val now = System.currentTimeMillis()
        if (now - sessionsReadAt < 8_000) return
        sessionsReadAt = now
        scope.launch {
            val list = runCatching { c.cloudSessions() }.getOrNull()
            val status = runCatching { c.cloudStatus() }.getOrNull()
            if (client() !== c) return@launch
            list?.let { sessions = it }
            status?.let { info = it }
        }
    }

    fun refreshAccounts() {
        val c = client() ?: return
        scope.launch {
            val lists = readAccounts(c)
            val v = runCatching { c.vaultStatus() }.getOrNull()
            if (client() !== c) return@launch
            accounts = lists
            v?.let { vault = it }
            refreshUsage()
        }
    }

    /** Re-read every Cloud login's plan usage (keys have none). */
    fun refreshUsage() {
        val c = client() ?: return
        val logins = agents.flatMap { agent -> accounts[agent.login].orEmpty().map { agent.login to it.slot } }
        scope.launch {
            val results = logins.map { (provider, slot) ->
                async { "$provider:$slot" to runCatching { c.vaultUsage(provider, slot) } }
            }.awaitAll()
            if (client() !== c) return@launch
            val u = usage.toMutableMap()
            val errors = usageErrors.toMutableMap()
            for ((key, result) in results) {
                result.onSuccess { u[key] = it; errors.remove(key) }.onFailure { errors[key] = message(it) }
            }
            usage = u
            usageErrors = errors
        }
    }

    suspend fun refreshGitHub() {
        val c = client() ?: return
        val v = runCatching { c.vaultStatus() }.getOrNull()
        val r = runCatching { c.githubRepos(null) }.getOrNull()
        if (client() !== c) return
        v?.let { vault = it }
        r?.let { repos = it }
    }

    suspend fun refreshBilling() {
        val c = client() ?: return
        val read = runCatching { c.billingStatus() }
        val status = runCatching { c.cloudStatus() }.getOrNull()
        if (client() !== c) return
        read.onSuccess { billing = it; billingError = null }.onFailure { billingError = message(it) }
        status?.let { info = it }
    }

    /** Forget everything (sign-out). */
    fun reset() {
        refreshJob?.cancel()
        refreshJob = null
        info = null
        vault = null
        repos = null
        accounts = emptyMap()
        usage = emptyMap()
        usageErrors = emptyMap()
        billing = null
        billingError = null
        retryDelayMs = 30_000L
        sessions = emptyList()
        sessionsReadAt = 0L
        error = null
        loading = false
    }

    // ── accounts ──────────────────────────────────────────────────────────

    suspend fun activate(account: CloudAccountInfo, provider: String) {
        val c = client() ?: return
        accounts = accounts + (provider to c.vaultActivate(provider, account.slot))
    }

    suspend fun remove(account: CloudAccountInfo, provider: String) {
        val c = client() ?: return
        accounts = accounts + (provider to c.vaultForget(provider, account.slot))
        refreshAccounts()
    }

    suspend fun saveKey(key: String, agent: Agent) {
        val c = client() ?: return
        c.vaultPutApiKey(agent.key, key)
        refreshAccounts()
    }

    // ── projects ──────────────────────────────────────────────────────────

    /** Make `repo` (`owner/name`) a project on Cloud alone; returns its id. */
    suspend fun createProject(repo: String): String {
        val c = client() ?: throw CoreException.Closed()
        return c.createCloudProject(repo).also { workspaceChanged() }
    }

    companion object {
        val agents = listOf(
            Agent("claude-code", "Claude Code", "claude", "anthropic-key", "Sign in with Claude", "Anthropic API key"),
            Agent("codex", "Codex", "codex", "openai-key", "Sign in with ChatGPT", "OpenAI API key"),
        )

        /** The agents Cloud runs. */
        val harnesses = agents.map { it.harness }

        /** Short enough for a snackbar beside its button. */
        const val OUT_OF_CREDITS = "Out of Cloud credits"

        fun message(e: Throwable): String = when (e) {
            is CoreException.Closed -> "You're signed out."
            is CoreException.NotFound -> e.reason
            is CoreException.InvalidArgument -> e.reason
            is CoreException.HostUnavailable -> e.reason
            is CoreException.Unsupported -> e.reason
            is CoreException.HostException -> e.reason
            is CoreException.Network -> e.reason
            is CoreException.Auth -> e.reason
            is CoreException.Storage -> e.reason
            is CoreException.NotImplemented -> e.reason
            is CoreException.Internal -> e.reason
            else -> e.message ?: "Something went wrong."
        }

        /** A Cloud error worth a button: out of credits gets "Add credits". */
        fun isOutOfCredits(e: Throwable) = message(e).contains("out of Cloud credits", ignoreCase = true)

        /** "41,250". */
        fun credits(value: Double): String = NumberFormat.getIntegerInstance().format(floor(value).toLong())

        /** "$20" / "$12.50". */
        fun price(cents: UInt): String = if (cents % 100u == 0u) "$${cents / 100u}" else String.format(Locale.US, "$%.2f", cents.toDouble() / 100)

        fun shortDate(ms: Long): String = SimpleDateFormat("MMM d", Locale.getDefault()).format(Date(ms))
    }
}

/**
 * Pages opened in the browser that come back through a `zeron://` link (a
 * provider's sign-in, GitHub, Stripe): tells a waiting flow whether the
 * browser returned through its link or was just closed.
 */
class BrowserReturns {
    /** `zeron://…` links the app was opened with (not the account callback). */
    val links = MutableSharedFlow<Uri>(extraBufferCapacity = 8)
    /** The app came back to the foreground. */
    val resumes = MutableSharedFlow<Unit>(extraBufferCapacity = 8)

    /** Open `url` in a Custom Tab. */
    fun open(context: Context, url: String) {
        CustomTabsIntent.Builder().setShowTitle(true).build().launchUrl(context, Uri.parse(url))
    }

    /**
     * Open `url` and wait until the user is back: true when the browser came
     * back through a `zeron://<host>` link, false when it was closed (the
     * app resumed with no link).
     */
    suspend fun browse(context: Context, url: String, host: String): Boolean = coroutineScope {
        val returned = kotlinx.coroutines.CompletableDeferred<Boolean>()
        val link = launch { links.first { it.host == host }; returned.complete(true) }
        val resume = launch {
            resumes.first()
            // The link (if any) is delivered just before the resume.
            delay(900)
            returned.complete(false)
        }
        open(context, url)
        val result = returned.await()
        link.cancel()
        resume.cancel()
        result
    }

    /** Like [browse], but also done when `finished` completes first (the server saw the sign-in). */
    suspend fun <T> browseUntil(context: Context, url: String, host: String, finished: suspend () -> T): T? = coroutineScope {
        val work = async { runCatching { finished() } }
        val closed = async { !browse(context, url, host) }
        // Wait for whichever lands first; a closed browser gives the server a moment to catch up.
        while (!work.isCompleted && !closed.isCompleted) delay(100)
        if (closed.isCompleted && closed.await() && !work.isCompleted) {
            withTimeoutOrNull(4_000) { work.await() }?.getOrThrow() ?: run {
                work.cancel()
                null
            }
        } else {
            closed.cancel()
            work.await().getOrThrow()
        }
    }

    companion object {
        /** Where a provider sign-in sends the browser once it's done. */
        const val SIGNED_IN = "zeron://signed-in"
    }
}

/**
 * Signs in to Claude or ChatGPT for Cloud (iOS `ProviderSignIn`): the
 * provider's own page in a Custom Tab, which hands back through
 * `zeron://signed-in`; the login then lands in Cloud's vault (no token stays
 * on the phone). Throws [CancellationException] when the page was closed.
 */
suspend fun AppModel.signInToCloud(context: Context, agent: CloudStore.Agent) {
    val c = client.value ?: throw CoreException.Closed()
    val start = c.providerLoginStart(agent.login, BrowserReturns.SIGNED_IN)
    val done = browser.browseUntil(context, start.url, "signed-in") {
        c.providerLoginFinish(start.loginId)
        true
    }
    if (done == null) {
        c.providerLoginCancel(start.loginId)
        throw CancellationException("The sign-in page was closed")
    }
    cloud.refreshAccounts()
}
