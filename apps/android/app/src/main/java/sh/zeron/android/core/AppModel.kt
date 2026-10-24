package sh.zeron.android.core

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Build
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import android.provider.Settings
import android.util.Log
import kotlinx.coroutines.CompletableDeferred
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.ThemeMode
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.withTimeoutOrNull
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.ProjectView
import uniffi.zeron_core.SendState
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import uniffi.zeron_core.AuthCallback
import uniffi.zeron_core.AuthOrg
import uniffi.zeron_core.ChatConfig
import uniffi.zeron_core.ClientEvent
import uniffi.zeron_core.ClientListener
import uniffi.zeron_core.Connectivity
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreConfig
import uniffi.zeron_core.Credentials
import uniffi.zeron_core.DemoFixture
import uniffi.zeron_core.DemoOptions
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
    val route: String? = null,
    val wallpaper: String? = null,
    val wallpaperEffect: String? = null,
    /** `--ez voice-preview true`: the scripted call (debuggable builds). */
    val voicePreview: Boolean = false,
    /** `--ez voice-stage true`: open the call stage at launch (with the preview). */
    val voiceStage: Boolean = false,
    /** `--ez remote-voice true`: offer voice before any host advertises it. */
    val remoteVoice: Boolean = false,
    /** `--ez voice-selftest true`: log a host-less WebRTC offer (debuggable builds). */
    val voiceSelfTest: Boolean = false,
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

    /** The app-wide voice call (one at a time, outlives screens). */
    val voice: sh.zeron.android.voice.VoiceController by lazy {
        sh.zeron.android.voice.VoiceController(app, client, workspace)
    }

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

    /** App-wide snackbars ("Archived · Undo", open failures) shown by the shell. */
    val toasts = MutableSharedFlow<Toast>(extraBufferCapacity = 8)

    fun toast(message: String, action: String? = null, run: (() -> Unit)? = null) {
        toasts.tryEmit(Toast(message, action, run))
    }

    private val settings = app.getSharedPreferences("settings", 0)
    private val _appearance = MutableStateFlow(
        Appearance(
            runCatching { ThemeMode.valueOf(settings.getString("theme", "System")!!) }.getOrDefault(ThemeMode.System),
            settings.getBoolean("dynamicColor", false),
        ),
    )
    val appearance: StateFlow<Appearance> = _appearance.asStateFlow()

    fun setAppearance(value: Appearance) {
        _appearance.value = value
        settings.edit().putString("theme", value.mode.name).putBoolean("dynamicColor", value.dynamicColor).apply()
    }

    /** Sessions grouped by project in place of the flat Recent list. */
    private val _groupByProject = MutableStateFlow(settings.getBoolean("groupByProject", false))
    val groupByProject: StateFlow<Boolean> = _groupByProject.asStateFlow()

    fun setGroupByProject(on: Boolean) {
        _groupByProject.value = on
        settings.edit().putBoolean("groupByProject", on).apply()
    }

    /** Folded sidebar groups (pinned, recent, project groups), kept on this device. */
    private val _collapsed = MutableStateFlow(settings.getStringSet("collapsedGroups", emptySet())!!.toSet())
    val collapsed: StateFlow<Set<String>> = _collapsed.asStateFlow()

    fun setGroupCollapsed(id: String, collapsed: Boolean) {
        _collapsed.value = if (collapsed) _collapsed.value + id else _collapsed.value - id
        settings.edit().putStringSet("collapsedGroups", _collapsed.value).apply()
    }

    /** Unsent composer text per chat (and the new-session page), kept across launches. */
    val drafts = Drafts(app)

    /** The demo keeps the new-session page in memory only: one run's leftovers must not seed the next. */
    private val persistsNewSession get() = !launch.demo
    private var volatileNewSessionText = ""

    /** What was typed on the new-session page when it was closed unsent. */
    var newSessionText: String
        get() = if (persistsNewSession) drafts.load(Drafts.NEW_SESSION) else volatileNewSessionText
        set(value) {
            if (persistsNewSession) drafts.save(Drafts.NEW_SESSION, value) else volatileNewSessionText = value
        }

    /** Images staged there (this launch only). */
    var newSessionImages: List<sh.zeron.android.ui.StagedImage> = emptyList()

    /** The new-session page's options (project, host, branch, model, effort), kept across launches. */
    var lastDraft: NewSessionDraft = storedDraft()
        set(value) {
            field = value
            if (persistsNewSession) {
                settings.edit()
                    .putString("draft.project", value.projectId)
                    .putString("draft.host", value.hostId)
                    .putString("draft.harness", value.harness)
                    .putString("draft.model", value.model)
                    .putString("draft.effort", value.effort)
                    .apply()
            }
        }

    private fun storedDraft() = NewSessionDraft(
        projectId = settings.getString("draft.project", null),
        hostId = settings.getString("draft.host", null),
        harness = settings.getString("draft.harness", null) ?: "claude-code",
        model = settings.getString("draft.model", null),
        effort = settings.getString("draft.effort", null),
    )

    var launch = LaunchOptions()
        private set

    val isDemo: Boolean get() = _client.value?.isDemo() == true
    private var refreshScheduled = false
    /** The pending sign-in's OAuth `state` (persisted: the process may die while the browser is up). */
    private var authState: String?
        get() = settings.getString("authState", null)
        set(value) = settings.edit().putString("authState", value).apply()

    private val listener = object : ClientListener {
        override fun onEvent(event: ClientEvent) {
            main.post { handle(event) }
        }
    }

    fun boot(options: LaunchOptions) {
        launch = options
        if (_client.value != null) return
        if (options.signedOut) credentials.clear()
        // The demo starts from the in-memory defaults, never a real account's options.
        if (options.demo) lastDraft = NewSessionDraft()
        watchNetwork()
        // `wallpaper <path>` / `wallpaper none` and `wallpaper-effect <name>`:
        // set the wallpaper at launch (screenshots, tests).
        options.wallpaper?.let { path ->
            if (path == "none") wallpaper.remove() else scope.launch { wallpaper.set(File(path)) }
        }
        options.wallpaperEffect?.let { name ->
            WallpaperStore.effects.firstOrNull { it.name.equals(name, ignoreCase = true) }?.let(wallpaper::setEffect)
        }
        if (options.remoteVoice) voice.forced = true
        if (options.voicePreview && voice.preview() && options.voiceStage) voice.requestStage()
        if (options.voiceSelfTest) voice.selfTest()
        val stored = credentials.stored()
        when {
            // A `route` with nobody signed in opens on the demo (screenshots, tests).
            options.demo || options.route != null && stored == null -> start(Credentials.Demo(demoOptions()))
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
            is Credentials.Dev -> "${credentials.userId}/${credentials.orgId}"
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
            edgeUrl = edgeUrl,
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
            backfillProfile()
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
            is AuthCallback.Code -> {
                // Only the sign-in this app started (a forged callback could sign in as someone else).
                if (authState == null || cb.state != authState) {
                    signInError.value = "Sign-in was interrupted. Try again."
                    return
                }
                authState = null
                scope.launch { signIn(cb.code) }
            }
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

    /** Logins from before profiles were saved: recover the org name. */
    private fun backfillProfile() {
        val stored = credentials.stored() as? Credentials.WorkOs ?: return
        if (credentials.profile.orgName != null) return
        scope.launch {
            val orgs = runCatching { authListOrgs(edgeUrl, stored.tokens.accessToken) }.getOrNull() ?: return@launch
            val org = orgs.firstOrNull { it.organizationId == stored.orgId } ?: return@launch
            credentials.profile = credentials.profile.copy(orgName = org.name)
            refreshWorkspace()
        }
    }

    /** The user signing out: this device forgets the account's local docs. */
    fun signOut() = signOutLocally(forget = true)

    /**
     * Drop the client. Only a sign-out the user asked for deletes the store:
     * an expired session keeps it (the outbox may hold unsent messages), and
     * the `.owner` marker still keeps a different account out of it.
     */
    private fun signOutLocally(forget: Boolean) {
        voice.stop()
        val wasDemo = isDemo
        endDeliveryHold()
        lastDeliveryActivity = 0L
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
        if (!wasDemo) {
            credentials.clear()
            if (forget) coreDir.deleteRecursively()
        }
        // Nothing of the account stays in memory (or seeds the next one's new session).
        lastDraft = NewSessionDraft()
        newSessionText = ""
        newSessionImages = emptyList()
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
            is ClientEvent.AuthExpired -> signOutLocally(forget = false)
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
        endDeliveryHold()
        _client.value?.onForeground()
        refreshWorkspace()
    }

    /** When the user last sent something (elapsed realtime). */
    private var lastDeliveryActivity = 0L

    fun noteDeliveryActivity() {
        lastDeliveryActivity = SystemClock.elapsedRealtime()
    }

    fun onBackground() {
        // Give the already-durable room outbox and host wake time to reach
        // the edge when the user leaves right after tapping Send.
        endDeliveryHold()
        val recent = lastDeliveryActivity != 0L && SystemClock.elapsedRealtime() - lastDeliveryActivity < 120_000
        val pending = _workspace.value?.let { ws ->
            (ws.front.pinned + ws.front.sections.flatMap { it.sessions } + ws.front.recent)
                .any { it.sendState == SendState.SENDING || it.sendState == SendState.QUEUED }
        } == true
        if (_client.value != null && !isDemo && (recent || pending)) DeliveryService.hold(app, 25_000)
        _client.value?.onBackground()
    }

    private fun endDeliveryHold() = DeliveryService.release(app)

    /**
     * Pull to refresh: redial the registry and rooms and re-probe health (the
     * same resync as returning to the foreground), then return once the
     * workspace has updated — at least [minimum] ms so the spinner doesn't
     * flicker, at most [maximum] if nothing changes.
     */
    suspend fun refresh(minimum: Long = 600, maximum: Long = 3_000) {
        val started = SystemClock.elapsedRealtime()
        val before = _workspace.value
        onForeground()
        withTimeoutOrNull(maximum) {
            // A reconnecting/offline blip from the redial isn't the resync landing.
            combine(_workspace, _connectivity) { ws, c -> ws to c }.first { (ws, c) ->
                ws != before && c?.state != ConnectivityState.RECONNECTING && c?.state != ConnectivityState.OFFLINE
            }
        }
        val wait = minimum - (SystemClock.elapsedRealtime() - started)
        if (wait > 0) delay(wait)
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

    /** Archive from anywhere, with an Undo snackbar. */
    fun archive(id: String) {
        attempt("archive") { _client.value?.archiveSession(id) }
        toast("Archived", "Undo") { unarchive(id) }
    }

    fun unarchive(id: String) = attempt("unarchive") { _client.value?.unarchiveSession(id) }
    fun rename(id: String, title: String) = attempt("rename") { _client.value?.renameSession(id, title) }
    fun markSeen(id: String) = attempt("seen") { _client.value?.markSeen(id) }
    fun setSectionCollapsed(id: String, collapsed: Boolean) = attempt("collapse") { _client.value?.setSectionCollapsed(id, collapsed) }
    fun move(id: String, toSection: String?) = attempt("assign") { _client.value?.assignSection(id, toSection) }
    fun movePin(id: String, after: String?, before: String?) = attempt("move pin") { _client.value?.movePin(id, after, before) }
    fun createSection(name: String): String? = try {
        _client.value?.createSection(name)
    } catch (e: Exception) {
        Log.w("Zeron", "section failed", e)
        null
    }
    fun renameSection(id: String, name: String) = attempt("rename section") { _client.value?.renameSection(id, name) }
    fun deleteSection(id: String) = attempt("delete section") { _client.value?.deleteSection(id) }

    fun row(id: String): SessionRow? = _client.value?.sessionRow(id)

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
            val chatId = client.createSession(NewSession(target, config, if (draft.worktree) null else draft.branch, null, null))
            val handle = client.openSession(chatId)
            val project = _workspace.value?.projects?.firstOrNull { it.id == draft.projectId }
            val worktree = if (draft.worktree && project != null) WorktreeSpec(project.path, draft.branch ?: "HEAD", project.id) else null
            handle.send(SendRequest(text, attachments, worktree, BusyPolicy.QUEUE))
            noteDeliveryActivity()
            refreshWorkspace()
            chatId
        } catch (e: Exception) {
            Log.w("Zeron", "create session failed", e)
            null
        }
    }
}

/** A snackbar raised from anywhere in the app. */
class Toast(val message: String, val action: String? = null, val run: (() -> Unit)? = null)

/** A Group by Project bucket: checkouts of one repository, or a device's home. */
data class ProjectGroup(val id: String, val name: String, val sessions: List<SessionRow>)

/**
 * Like the desktop's project grouping: checkouts of one repository share a
 * group (named for its representative), project-less sessions group per
 * device. Groups and rows keep the incoming recency order.
 */
fun projectGroups(rows: List<SessionRow>, projects: List<ProjectView>): List<ProjectGroup> {
    val byId = HashMap<String, ProjectView>()
    for (p in projects) byId.putIfAbsent(p.id, p)
    val groups = LinkedHashMap<String, Pair<String, MutableList<SessionRow>>>()
    for (row in rows) {
        val ref = row.project
        val (id, name) = if (ref != null) {
            val project = byId[ref.id]
            ("project:" + (project?.groupKey ?: ref.id)) to (project?.groupName ?: ref.name)
        } else {
            ("home:" + row.deviceId) to (row.deviceName?.let { "Home · $it" } ?: "Home")
        }
        groups.getOrPut(id) { name to mutableListOf() }.second.add(row)
    }
    return groups.map { (id, v) -> ProjectGroup(id, v.first, v.second) }
}

/** The new-session page's options (kept across launches). */
data class NewSessionDraft(
    val projectId: String? = null,
    val hostId: String? = null,
    val harness: String = "claude-code",
    val model: String? = null,
    val effort: String? = null,
    val branch: String? = null,
    val worktree: Boolean = false,
)
