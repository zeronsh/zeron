package sh.zeron.android.core

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.CompletableDeferred
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.ThemeMode
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import org.json.JSONObject
import uniffi.zeron_core.AuthCallback
import uniffi.zeron_core.AuthOrg
import uniffi.zeron_core.ChatConfig
import uniffi.zeron_core.ClientEvent
import uniffi.zeron_core.ClientListener
import uniffi.zeron_core.Connectivity
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreConfig
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.Credentials
import uniffi.zeron_core.DemoFixture
import uniffi.zeron_core.DemoOptions
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.SandboxLevel
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.StreamSpeed
import uniffi.zeron_core.TranscriptScale
import uniffi.zeron_core.WorkspaceSnapshot
import uniffi.zeron_core.WorktreeSpec
import uniffi.zeron_core.authExchangeCode
import uniffi.zeron_core.authListOrgs
import uniffi.zeron_core.authProductionEdgeUrl
import uniffi.zeron_core.authRefresh
import uniffi.zeron_core.parseAuthCallback
import uniffi.zeron_core.workosAuthorizeUrl
import java.io.File
import java.util.UUID

/** How the app was launched (adb extras mirror the iOS launch arguments). */
data class LaunchOptions(
    val demo: Boolean = false,
    val fast: Boolean = false,
    val longReply: Boolean = false,
    val big: Boolean = false,
    val huge: Boolean = false,
    val noProjects: Boolean = false,
    val signedOut: Boolean = false,
    /** Developer (debuggable builds): sign in to an `AUTH_MODE=dev` edge as `devUser@devOrg`. */
    val devEdge: String? = null,
    val devUser: String? = null,
    val devOrg: String? = null,
    val route: String? = null,
    val wallpaper: String? = null,
    val wallpaperEffect: String? = null,
)

/**
 * App-wide state owner: holds the Rust [CoreClient], republishes its
 * snapshots as flows, and fans session events out to screens. Core events
 * arrive on Rust threads and hop to the main thread here.
 */
class AppModel(private val app: Application) {
    private val scope = MainScope()
    private val main = Handler(Looper.getMainLooper())
    val credentials = CredentialStore(app)
    val wallpaper = WallpaperStore(app)
    val notifier by lazy { Notifier(app) }
    val workspaceApi by lazy { sh.zeron.android.tools.WorkspaceApi(this) }
    val downloads by lazy { sh.zeron.android.tools.Downloads(app, this) }

    private val _client = MutableStateFlow<CoreClient?>(null)
    val client: StateFlow<CoreClient?> = _client.asStateFlow()

    private val _workspace = MutableStateFlow<WorkspaceSnapshot?>(null)
    val workspace: StateFlow<WorkspaceSnapshot?> = _workspace.asStateFlow()

    private val _connectivity = MutableStateFlow<Connectivity?>(null)
    val connectivity: StateFlow<Connectivity?> = _connectivity.asStateFlow()

    /** Chat ids whose session or composer changed. */
    private val _sessionEvents = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val sessionEvents: SharedFlow<String> = _sessionEvents

    /** Organizations to pick from mid sign-in (more than one). */
    val orgChoice = MutableStateFlow<Pair<List<AuthOrg>, CompletableDeferred<AuthOrg?>>?>(null)
    val signInError = MutableStateFlow<String?>(null)

    private val settings = app.getSharedPreferences("settings", 0)
    private val _appearance = MutableStateFlow(
        Appearance(
            runCatching { ThemeMode.valueOf(settings.getString("theme", "System")!!) }.getOrDefault(ThemeMode.System),
            settings.getBoolean("dynamicColor", false),
        ),
    )
    val appearance: StateFlow<Appearance> = _appearance.asStateFlow()

    /** Starred models in the model picker (device-local, every mode). */
    val favorites = FavoritesStore(settings)

    fun setAppearance(value: Appearance) {
        _appearance.value = value
        settings.edit().putString("theme", value.mode.name).putBoolean("dynamicColor", value.dynamicColor).apply()
    }

    var lastDraft: NewSessionDraft = NewSessionDraft(
        projectId = settings.getString("draft.project", null),
        hostId = settings.getString("draft.host", null),
        harness = settings.getString("draft.harness", null) ?: "claude-code",
        model = settings.getString("draft.model", null),
        effort = settings.getString("draft.effort", null),
    )
        set(value) {
            field = value
            if (!launch.demo) {
                settings.edit()
                    .putString("draft.project", value.projectId)
                    .putString("draft.host", value.hostId)
                    .putString("draft.harness", value.harness)
                    .putString("draft.model", value.model)
                    .putString("draft.effort", value.effort)
                    .apply()
            }
        }

    var launch = LaunchOptions()
        private set

    val isDemo: Boolean get() = _client.value?.isDemo() == true
    private var refreshScheduled = false
    private var authState: String? = null

    private val listener = object : ClientListener {
        override fun onEvent(event: ClientEvent) {
            main.post { handle(event) }
        }
    }

    fun boot(options: LaunchOptions) {
        launch = options
        if (_client.value != null) return
        if (options.signedOut) credentials.clear()
        watchNetwork()
        // `wallpaper <path>` / `wallpaper none` and `wallpaper-effect <name>`:
        // set the wallpaper at launch (screenshots, tests).
        options.wallpaper?.let { path ->
            if (path == "none") wallpaper.remove() else scope.launch { wallpaper.set(File(path)) }
        }
        options.wallpaperEffect?.let { name ->
            WallpaperStore.effects.firstOrNull { it.name.equals(name, ignoreCase = true) }?.let(wallpaper::setEffect)
        }
        val stored = credentials.stored()
        val dev = options.devEdge?.takeIf { isDebuggable }
        when {
            options.demo -> start(Credentials.Demo(demoOptions()))
            dev != null -> devSignIn(dev, options.devUser ?: "dev-user", options.devOrg ?: "dev-org")
            stored != null -> start(stored)
        }
        // Relative times ("4m") and staleness age without events.
        scope.launch {
            while (true) {
                delay(30_000)
                refreshWorkspace()
            }
        }
    }

    fun demoOptions() = DemoOptions(
        fixture = if (launch.noProjects) DemoFixture.NO_PROJECTS else DemoFixture.STANDARD,
        transcriptScale = when {
            launch.huge -> TranscriptScale.Huge
            launch.big -> TranscriptScale.Big
            else -> TranscriptScale.Normal
        },
        streamSpeed = if (launch.fast) StreamSpeed.FAST else StreamSpeed.REALISTIC,
        longReply = launch.longReply,
    )

    private val coreDir get() = File(app.filesDir, "core")

    /** Local docs belong to one identity: another account starts empty. */
    private fun claimCoreDir(credentials: Credentials) {
        val owner = when (credentials) {
            is Credentials.WorkOs -> "${credentials.userId}/${credentials.orgId}"
            is Credentials.Dev -> "${credentials.userId}/${credentials.orgId}@${this.credentials.devEdge}"
            is Credentials.Demo -> return
        }
        val marker = File(coreDir, ".owner")
        if (runCatching { marker.readText() }.getOrNull() != owner) coreDir.deleteRecursively()
        coreDir.mkdirs()
        marker.writeText(owner)
    }

    private fun start(credentials: Credentials): Boolean {
        val demo = credentials is Credentials.Demo
        val dir = if (demo) File(app.filesDir, "demo") else coreDir
        if (!demo) claimCoreDir(credentials)
        dir.mkdirs()
        val config = CoreConfig(
            edgeUrl = if (credentials is Credentials.Dev) this.credentials.devEdge ?: edgeUrl else edgeUrl,
            dataDir = dir.path,
            deviceId = deviceId,
            deviceName = deviceName(),
            platform = "android",
            appVersion = app.packageManager.getPackageInfo(app.packageName, 0).versionName ?: "0",
        )
        return try {
            val client = CoreClient(config, credentials, listener)
            if (credentials is Credentials.WorkOs) this.credentials.store(credentials)
            _client.value = client
            refreshWorkspace()
            client.preloadSessions()
            true
        } catch (e: Exception) {
            Log.e("Zeron", "core start failed", e)
            false
        }
    }

    fun startDemo() {
        start(Credentials.Demo(demoOptions()))
    }

    val edgeUrl: String get() = authProductionEdgeUrl()

    /** Developer sign-in is offered in debuggable builds only. */
    val isDebuggable: Boolean get() = (app.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0

    /**
     * Developer: join an `AUTH_MODE=dev` edge (`wrangler dev --var
     * AUTH_MODE:dev`) as `user@org` — no WorkOS. Debuggable builds only;
     * hidden behind seven taps on the sign-in screen's mark (or the
     * `dev-edge` launch extra).
     */
    fun devSignIn(edge: String, user: String, org: String): Boolean {
        if (!isDebuggable) return false
        val url = edge.trim().trimEnd('/')
        if (!(url.startsWith("http://") || url.startsWith("https://")) || user.isBlank()) {
            signInError.value = "Enter the edge URL (http://…) and a user id."
            return false
        }
        _client.value?.shutdown()
        _client.value = null
        val dev = Credentials.Dev(user.trim(), org.trim())
        credentials.store(dev, devEdge = url)
        credentials.profile = CredentialStore.Profile(user.trim(), url, org.trim().ifEmpty { null })
        return start(dev).also { if (!it) signInError.value = "Couldn't open the dev workspace." }
    }

    private val deviceId: String
        get() {
            val prefs = app.getSharedPreferences("device", 0)
            prefs.getString("deviceId", null)?.let { return it }
            val id = "android-" + UUID.randomUUID().toString().take(8)
            prefs.edit().putString("deviceId", id).apply()
            return id
        }

    private fun deviceName(): String =
        Settings.Global.getString(app.contentResolver, Settings.Global.DEVICE_NAME) ?: Build.MODEL

    // ── sign-in ────────────────────────────────────────────────────────────

    /** The WorkOS authorize URL to open in a Custom Tab. */
    fun beginSignIn(): String {
        val state = UUID.randomUUID().toString()
        authState = state
        return workosAuthorizeUrl(state)
    }

    /** `zeron://callback?code=…`: code → tokens → org → client. */
    fun handleCallback(url: String) {
        when (val cb = parseAuthCallback(url)) {
            is AuthCallback.Code -> scope.launch { signIn(cb.code) }
            is AuthCallback.Error -> signInError.value = cb.description ?: cb.error
            null -> Unit
        }
    }

    private suspend fun signIn(code: String) {
        try {
            val exchange = authExchangeCode(edgeUrl, code)
            val orgs = authListOrgs(edgeUrl, exchange.tokens.accessToken)
            if (orgs.isEmpty()) {
                signInError.value = "This account isn't in an organization yet."
                return
            }
            val org = if (orgs.size == 1) orgs[0] else {
                val choice = CompletableDeferred<AuthOrg?>()
                orgChoice.value = orgs to choice
                choice.await().also { orgChoice.value = null } ?: return
            }
            val tokens = authRefresh(edgeUrl, exchange.tokens.refreshToken, org.organizationId)
            val user = exchange.user
            val name = listOfNotNull(user.firstName, user.lastName).map { it.trim() }.filter { it.isNotEmpty() }.joinToString(" ")
            credentials.profile = CredentialStore.Profile(name.ifEmpty { null }, user.email, org.name)
            if (!start(Credentials.WorkOs(user.id, org.organizationId, tokens))) {
                signInError.value = "Couldn't open your workspace. Try signing in again."
            }
        } catch (e: Exception) {
            signInError.value = e.message ?: "Sign-in failed"
        }
    }

    fun signOut() {
        val wasDemo = isDemo
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
        if (!wasDemo) {
            credentials.clear()
            coreDir.deleteRecursively()
        }
    }

    val accountName: String
        get() = when {
            isDemo -> "Demo"
            _client.value == null -> "Signed out"
            else -> credentials.profile.let { it.name ?: it.email ?: "Signed in" }
        }

    val accountDetail: String
        get() = if (isDemo) "Offline demo workspace" else credentials.profile.let { p ->
            listOfNotNull(if (p.name != null) p.email else null, p.orgName).joinToString(" · ").ifEmpty { "Zeron account" }
        }

    // ── events ─────────────────────────────────────────────────────────────

    private fun handle(event: ClientEvent) {
        when (event) {
            is ClientEvent.WorkspaceChanged -> scheduleRefresh()
            is ClientEvent.SessionChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ComposerChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ConnectivityChanged -> _connectivity.value = event.connectivity
            is ClientEvent.AuthRefreshed -> credentials.updateTokens(event.tokens)
            is ClientEvent.AuthExpired -> signOut()
        }
    }

    /** Coalesce bursts of registry frames into one rebuild per loop turn. */
    private fun scheduleRefresh() {
        if (refreshScheduled) return
        refreshScheduled = true
        main.post {
            refreshScheduled = false
            refreshWorkspace()
        }
    }

    fun refreshWorkspace() {
        val client = _client.value ?: return
        _workspace.value = client.workspace()
    }

    fun onForeground() {
        _client.value?.onForeground()
        refreshWorkspace()
    }

    fun onBackground() {
        _client.value?.onBackground()
    }

    /** Pull to refresh: redial and re-probe, then settle for a beat. */
    suspend fun refresh() {
        onForeground()
        delay(700)
    }

    private fun watchNetwork() {
        val cm = app.getSystemService(ConnectivityManager::class.java) ?: return
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                main.post { _client.value?.setNetworkOnline(true) }
            }

            override fun onLost(network: Network) {
                main.post {
                    val active = cm.getNetworkCapabilities(cm.activeNetwork)
                    val online = active?.hasCapability(NetworkCapabilities.NET_CAPABILITY_INTERNET) == true
                    _client.value?.setNetworkOnline(online)
                }
            }
        })
    }

    // ── writes ─────────────────────────────────────────────────────────────

    private inline fun attempt(what: String, body: () -> Unit) {
        try {
            body()
        } catch (e: Exception) {
            Log.w("Zeron", "$what failed", e)
        }
    }

    fun setPinned(id: String, pinned: Boolean) = attempt("pin") {
        val c = _client.value ?: return
        if (pinned) c.pinSession(id) else c.unpinSession(id)
    }

    fun archive(id: String) = attempt("archive") { _client.value?.archiveSession(id) }
    fun unarchive(id: String) = attempt("unarchive") { _client.value?.unarchiveSession(id) }
    fun rename(id: String, title: String) = attempt("rename") { _client.value?.renameSession(id, title) }
    fun setSectionCollapsed(id: String, collapsed: Boolean) = attempt("collapse") { _client.value?.setSectionCollapsed(id, collapsed) }

    fun row(id: String): SessionRow? = _client.value?.sessionRow(id)

    /** The workspace a chat runs in, for the developer tools. */
    fun workspaceRef(chatId: String): sh.zeron.android.tools.WorkspaceRef? {
        val row = row(chatId) ?: return null
        val project = row.project?.let { runCatching { _client.value?.project(it.id) }.getOrNull() }
        return sh.zeron.android.tools.WorkspaceRef(
            deviceId = row.deviceId,
            chatId = chatId,
            spaceId = row.project?.id,
            root = row.cwd ?: project?.path,
            title = row.project?.name ?: row.cwd?.substringAfterLast('/')?.ifEmpty { null } ?: "Home",
            deviceName = row.deviceName,
        )
    }

    /** A project's folder on its device (no chat). */
    fun projectRef(spaceId: String): sh.zeron.android.tools.WorkspaceRef? {
        val p = runCatching { _client.value?.project(spaceId) }.getOrNull() ?: return null
        return sh.zeron.android.tools.WorkspaceRef(p.deviceId, null, p.id, p.path, p.name, p.deviceName)
    }

    /** Devices that run agents (the account's computers). */
    fun executionDevices(): List<DeviceView> = runCatching { _client.value?.executionDevices() }.getOrNull().orEmpty()

    // ── host calls ─────────────────────────────────────────────────────────

    /** Untyped engine RPC (harness installs, agent sign-ins). Throws on failure. */
    suspend fun hostCall(deviceId: String, method: String, params: JSONObject = JSONObject()): Any {
        val c = _client.value ?: throw IllegalStateException("Not connected")
        return Agents.parse(c.hostCall(deviceId, method, params.toString()))
    }

    /**
     * Clone a repository, or create an empty one, on a device and make it a
     * project. The device's engine does it (`CloneRepo` / `CreateRepo`).
     */
    suspend fun addProject(deviceId: String, source: ProjectSource, input: String): Result<String> {
        val (method, params) = when (source) {
            ProjectSource.Clone -> "CloneRepo" to JSONObject().put("url", input.trim())
            ProjectSource.Empty -> "CreateRepo" to JSONObject().put("name", input.trim())
        }
        val path = runCatching {
            val reply = hostCall(deviceId, method, params) as? JSONObject
            reply?.optString("path")?.ifEmpty { null } ?: error("The device didn't say where the project is.")
        }.getOrElse { return Result.failure(it) }
        val c = _client.value ?: return Result.failure(IllegalStateException("Not connected"))
        return runCatching { c.createProject(deviceId, path, true) }.onSuccess { refreshWorkspace() }
    }

    /** Create the chat and send its first message. */
    fun createSession(draft: NewSessionDraft, text: String, attachments: List<uniffi.zeron_core.OutgoingAttachment> = emptyList()): String? {
        val client = _client.value ?: return null
        val target = when {
            draft.projectId != null -> SessionTarget.Project(draft.projectId)
            draft.hostId != null -> SessionTarget.Projectless(draft.hostId)
            else -> return null
        }
        val config = ChatConfig(draft.harness, draft.model, draft.effort, emptyMap(), SandboxLevel.WORKSPACE_WRITE)
        return try {
            // A new worktree is minted with the first send; otherwise the
            // session runs in the checkout — or in the picked branch's own
            // worktree, reused as is (the desktop's "current worktree").
            val chatId = client.createSession(
                NewSession(target, config, if (draft.worktree) null else draft.branch, if (draft.worktree) null else draft.cwd, null),
            )
            val handle = client.openSession(chatId)
            val project = _workspace.value?.projects?.firstOrNull { it.id == draft.projectId }
            val worktree = if (draft.worktree && project != null) WorktreeSpec(project.path, draft.branch ?: "HEAD", project.id) else null
            handle.send(SendRequest(text, attachments, worktree, BusyPolicy.QUEUE))
            refreshWorkspace()
            chatId
        } catch (e: Exception) {
            Log.w("Zeron", "create session failed", e)
            null
        }
    }
}

/** Human wording for core errors. */
fun Throwable.userMessage(): String = when (this) {
    is CoreException.HostUnavailable ->
        if (Agents.isTimeout(reason)) "The device took too long to answer." else "The device isn't reachable right now."
    is CoreException.Unsupported -> "Not supported by this device's engine."
    is CoreException.Closed -> "Not connected."
    // Host errors arrive as "Method: reason" — the reason is what people read.
    is CoreException.HostException -> reason.substringAfter(": ", reason).ifBlank { "The device couldn't do that." }
    is CoreException.NotFound -> reason
    is CoreException.InvalidArgument -> reason
    is CoreException.Network -> reason
    is CoreException.Auth -> reason
    is CoreException.Storage -> reason
    is CoreException.NotImplemented -> reason
    is CoreException.Internal -> reason
    else -> message ?: "Something went wrong."
}

enum class ProjectSource { Clone, Empty }

/** The new-session page's options (kept across launches). */
data class NewSessionDraft(
    val projectId: String? = null,
    val hostId: String? = null,
    val harness: String = "claude-code",
    val model: String? = null,
    val effort: String? = null,
    val branch: String? = null,
    val worktree: Boolean = false,
    /** Run in this existing worktree of [branch] instead of the project folder. */
    val cwd: String? = null,
)
