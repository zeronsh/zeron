package sh.zeron.android.core

import android.app.Application
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.net.ConnectivityManager
import android.net.Network
import android.os.Handler
import android.os.Looper
import android.os.SystemClock
import androidx.appcompat.app.AppCompatDelegate
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshots.SnapshotStateList
import androidx.lifecycle.AndroidViewModel
import androidx.lifecycle.viewModelScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import sh.zeron.android.BuildConfig
import uniffi.zeron_core.AuthCallback
import uniffi.zeron_core.AuthOrg
import uniffi.zeron_core.ChatConfig
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ClientEvent
import uniffi.zeron_core.ClientListener
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.CoreConfig
import uniffi.zeron_core.Credentials
import uniffi.zeron_core.DemoFixture
import uniffi.zeron_core.DemoOptions
import uniffi.zeron_core.FaceData
import uniffi.zeron_core.FaceRole
import uniffi.zeron_core.PlatformMeasurer
import uniffi.zeron_core.SandboxLevel
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.StreamSpeed
import uniffi.zeron_core.TextSystem
import uniffi.zeron_core.TranscriptScale
import uniffi.zeron_core.WallpaperEffect
import uniffi.zeron_core.WorkspaceSnapshot
import uniffi.zeron_core.authExchangeCode
import uniffi.zeron_core.authListOrgs
import uniffi.zeron_core.authProductionEdgeUrl
import uniffi.zeron_core.authRefresh
import uniffi.zeron_core.parseAuthCallback
import uniffi.zeron_core.projectColorIndex
import uniffi.zeron_core.wallpaperRender
import uniffi.zeron_core.wallpaperSafeOpacity
import uniffi.zeron_core.workosAuthorizeUrl
import uniffi.zeron_core.Connectivity
import uniffi.zeron_core.ProbeResult
import uniffi.zeron_core.SshException
import uniffi.zeron_core.sshProbe
import kotlinx.coroutines.Job
import java.io.File
import java.util.UUID
import android.graphics.Paint
import android.graphics.Typeface

class ZeronModel(app: Application) : AndroidViewModel(app) {
    sealed interface Phase {
        data object Loading : Phase
        data object Ready : Phase
        data object SignedOut : Phase
        data class Failed(val message: String) : Phase
    }

    sealed interface Route {
        data class Folder(val id: String, val title: String) : Route
        data class Session(val id: String) : Route
    }

    enum class Tab { Sessions, Settings, Search }

    var phase by mutableStateOf<Phase>(Phase.Loading)
    var workspace by mutableStateOf<WorkspaceSnapshot?>(null)
    var epoch by mutableIntStateOf(0)
    var appearance by mutableIntStateOf(0)
    var toast by mutableStateOf<String?>(null)
    var toastUndo by mutableStateOf<(() -> Unit)?>(null)
        private set
    private var toastToken = 0
    var tab by mutableStateOf(Tab.Sessions)
    val sessionStack: SnapshotStateList<Route> = mutableStateListOf()
    val settingsStack: SnapshotStateList<Route> = mutableStateListOf()
    var showNewSession by mutableStateOf(false)
    /** Project the next New Session opens on (just added from the folder browser). */
    var newSessionProject by mutableStateOf<String?>(null)
    var showSignIn by mutableStateOf(false)
    var searchQuery by mutableStateOf("")
    /** Set by the `spaces` launch route so a screenshot can open the filter. */
    var pendingSpaceMenu by mutableStateOf(false)
    var wallpaper by mutableStateOf<Bitmap?>(null)
    var wallpaperOpacity by mutableStateOf(0.42f)
    var wallpaperEffect by mutableStateOf(WallpaperEffect.NONE)
    var signInError by mutableStateOf<String?>(null)
    var signInBusy by mutableStateOf(false)
    var authOrgs by mutableStateOf<List<AuthOrg>?>(null)

    // ── machines (direct SSH) ─────────────────────────────────────────────
    val machineStore = MachineStore(app)
    var machines by mutableStateOf(machineStore.list())
        private set
    /** Probe results per machine id: true online, false offline, absent unknown. */
    var machineOnline by mutableStateOf<Map<String, Boolean>>(emptyMap())
        private set
    /** `demo`, `cloud`, or a machine id. */
    var activeMachine by mutableStateOf("demo")
        private set
    var showMachines by mutableStateOf(false)
    var editMachine by mutableStateOf<Machine?>(null)
    var connectivity by mutableStateOf<Connectivity?>(null)
        private set
    private var probeJob: Job? = null
    /** Direct link phase/errors/counters, polled while a machine is active. */
    var directStatus by mutableStateOf<uniffi.zeron_core.DirectStatus?>(null)
        private set
    var showLinkDetails by mutableStateOf(false)
    private var directJob: Job? = null

    var client: CoreClient? = null
        private set
    var text: TextSystem? = null
        private set

    private val main = Handler(Looper.getMainLooper())
    private val prefs = app.getSharedPreferences("zeron", 0)
    private var authState: String? = null
    private var pendingExchange: Pair<String, uniffi.zeron_core.AuthTokens>? = null
    private val collapsed = prefs.getStringSet("collapsed", emptySet())?.toMutableSet() ?: mutableSetOf()

    val faces: Map<FaceRole, Typeface> = loadFaces(app)

    init {
        appearance = if (prefs.contains("appearance")) prefs.getInt("appearance", 2) else 2
        applyNight(appearance, recreate = false)
        wallpaperEffect = effectFrom(prefs.getString("wallpaperEffect", "none"))
        loadWallpaper()
        startDefault()
        watchNetwork()
    }

    /** Folded front-page groups ("pinned", "recent", section ids); persisted. */
    var collapsedIds by mutableStateOf<Set<String>>(collapsed.toSet())
        private set

    fun isCollapsed(id: String): Boolean = collapsedIds.contains(id)

    fun toggleCollapsed(id: String) {
        if (!collapsed.add(id)) collapsed.remove(id)
        prefs.edit().putStringSet("collapsed", HashSet(collapsed)).apply()
        collapsedIds = collapsed.toSet()
    }

    fun collapseAll() {
        val ws = workspace ?: return
        if (ws.front.pinned.isNotEmpty()) collapsed.add("pinned")
        ws.front.sections.forEach { collapsed.add(it.id) }
        prefs.edit().putStringSet("collapsed", HashSet(collapsed)).apply()
        collapsedIds = collapsed.toSet()
    }

    fun applyAppearance(mode: Int) {
        appearance = mode
        prefs.edit().putInt("appearance", mode).apply()
        applyNight(mode, recreate = true)
        loadWallpaper()
    }

    private fun applyNight(mode: Int, recreate: Boolean) {
        val night = when (mode) {
            1 -> AppCompatDelegate.MODE_NIGHT_NO
            2 -> AppCompatDelegate.MODE_NIGHT_YES
            else -> AppCompatDelegate.MODE_NIGHT_FOLLOW_SYSTEM
        }
        if (recreate && AppCompatDelegate.getDefaultNightMode() != night) {
            AppCompatDelegate.setDefaultNightMode(night)
        } else if (!recreate) {
            AppCompatDelegate.setDefaultNightMode(night)
        }
    }

    fun showToast(message: String) {
        val token = ++toastToken
        toast = message
        toastUndo = null
        main.postDelayed({ if (toastToken == token) toast = null }, 2400)
    }

    /** A toast with an Undo button (pin, unpin, archive), up a little longer. */
    fun showUndo(message: String, undo: () -> Unit) {
        val token = ++toastToken
        toast = message
        toastUndo = undo
        main.postDelayed({
            if (toastToken == token) {
                toast = null
                toastUndo = null
            }
        }, 4500)
    }

    fun runUndo() {
        val undo = toastUndo ?: return
        toastToken++
        toast = null
        toastUndo = null
        undo()
    }

    fun back(): Boolean {
        if (editMachine != null) {
            editMachine = null
            return true
        }
        if (showMachines && phase is Phase.Ready) {
            showMachines = false
            return true
        }
        if (showNewSession) {
            showNewSession = false
            return true
        }
        if (showSignIn && phase is Phase.Ready) {
            showSignIn = false
            return true
        }
        val stack = if (tab == Tab.Settings) settingsStack else sessionStack
        if (stack.isNotEmpty()) {
            stack.removeAt(stack.lastIndex)
            return true
        }
        return false
    }

    fun openSession(id: String) {
        tab = Tab.Sessions
        showNewSession = false
        sessionStack.clear()
        sessionStack.add(Route.Session(id))
        runCatching { client?.markSeen(id) }
    }

    fun openFolder(id: String, title: String) {
        val stack = if (tab == Tab.Settings) settingsStack else sessionStack
        stack.add(Route.Folder(id, title))
    }

    private fun startDefault() {
        val forced = prefs.getString("boot", null)
        val last = prefs.getString("lastMachine", null)
        val lastMachine = machines.firstOrNull { it.id == last }
        when {
            lastMachine != null -> connectMachine(lastMachine)
            forced == "signedout" -> {
                phase = Phase.SignedOut
                showSignIn = true
            }
            forced == "demo" || (BuildConfig.DEBUG && prefs.getString("account", null) == null) -> start(demoCredentials(), demo = true)
            prefs.getString("account", null) != null -> start(storedCredentials() ?: demoCredentials(), demo = prefs.getString("account", null) == null)
            else -> {
                phase = Phase.SignedOut
                showSignIn = true
            }
        }
    }

    fun enterDemo() {
        prefs.edit().remove("account").putString("boot", "demo").putString("lastMachine", "demo").apply()
        shutdownClient()
        resetNavigation()
        start(demoCredentials(), demo = true)
        showSignIn = false
        showMachines = false
    }

    /** The edge ("Zeron Cloud") account: stored sign-in, else the sign-in screen. */
    fun useCloud() {
        prefs.edit().putString("lastMachine", "cloud").apply()
        shutdownClient()
        resetNavigation()
        showMachines = false
        val stored = storedCredentials()
        if (stored != null) {
            start(stored, demo = false)
        } else {
            phase = Phase.SignedOut
            showSignIn = true
        }
    }

    fun connectMachine(machine: Machine) {
        prefs.edit().putString("lastMachine", machine.id).apply()
        shutdownClient()
        resetNavigation()
        showMachines = false
        showSignIn = false
        val target = runCatching { machineStore.target(machine) }.getOrElse {
            phase = Phase.Failed(it.message ?: "Couldn't read this machine's key")
            return
        }
        start(uniffi.zeron_core.Credentials.Direct(target), demo = false, dir = "direct-${machine.id}", active = machine.id)
    }

    private fun resetNavigation() {
        sessionStack.clear()
        settingsStack.clear()
        tab = Tab.Sessions
        workspace = null
        connectivity = null
        directStatus = null
        showLinkDetails = false
    }

    fun activeTitle(): String = when (activeMachine) {
        "demo" -> "Demo"
        "cloud" -> "Zeron Cloud"
        else -> machines.firstOrNull { it.id == activeMachine }?.title() ?: "Machine"
    }

    fun saveMachine(machine: Machine, secret: String?) {
        machineStore.save(machine, secret)
        machines = machineStore.list()
    }

    fun deleteMachine(id: String) {
        machineStore.delete(id)
        machines = machineStore.list()
        if (activeMachine == id) enterDemo()
    }

    fun phonePublicKey(): String = runCatching { machineStore.phoneKey().second }.getOrDefault("")

    /** SSH + engine probe with an explicit pin (null = first contact). */
    suspend fun testMachine(machine: Machine, secret: String?, hostKey: String?): ProbeResult {
        val target = machineStore.target(machine, hostKey = hostKey, secretOverride = secret)
        return sshProbe(target)
    }

    /** Refresh the online dots (pinned machines only; never auto-trusts). */
    fun probeMachines() {
        probeJob?.cancel()
        probeJob = viewModelScope.launch {
            for (m in machines) {
                launch {
                    val ok = if (m.hostKey == null) false else runCatching { sshProbe(machineStore.target(m)) }.isSuccess
                    machineOnline = machineOnline + (m.id to ok)
                }
            }
        }
    }

    fun signOut() {
        shutdownClient()
        prefs.edit().remove("account").putString("boot", "signedout").apply()
        workspace = null
        sessionStack.clear()
        settingsStack.clear()
        phase = Phase.SignedOut
        showSignIn = true
        showToast("Signed out")
    }

    private fun demoCredentials() = Credentials.Demo(
        DemoOptions(
            fixture = DemoFixture.STANDARD,
            transcriptScale = TranscriptScale.Normal,
            streamSpeed = StreamSpeed.REALISTIC,
            longReply = false,
        ),
    )

    private fun start(credentials: Credentials, demo: Boolean, dir: String? = null, active: String? = null) {
        phase = Phase.Loading
        activeMachine = active ?: if (demo) "demo" else "cloud"
        viewModelScope.launch(Dispatchers.Default) {
            try {
                val app = getApplication<Application>()
                val dir = File(app.filesDir, dir ?: if (demo) "demo" else "core").apply { mkdirs() }
                if (text == null) {
                    val bytes = faces.map { (role, _) ->
                        val name = FACE_FILES.getValue(role)
                        FaceData(role, app.assets.open(name).use { it.readBytes() })
                    }
                    text = TextSystem(bytes, AndroidMeasurer(faces))
                }
                val id = prefs.getString("deviceId", null) ?: ("android-" + UUID.randomUUID().toString().take(8)).also {
                    prefs.edit().putString("deviceId", it).apply()
                }
                val config = CoreConfig(
                    edgeUrl = authProductionEdgeUrl(),
                    dataDir = dir.absolutePath,
                    deviceId = id,
                    deviceName = android.os.Build.MODEL ?: "Android",
                    platform = "android",
                    appVersion = BuildConfig.VERSION_NAME ?: "0.2.94",
                )
                val created = CoreClient(config, credentials, object : ClientListener {
                    override fun onEvent(event: ClientEvent) {
                        // Must name the model's handler: a bare `onEvent(event)`
                        // here resolves to this listener method, which re-posted
                        // every event to the main thread forever (the main
                        // thread spun at 100% and the UI never saw the event).
                        main.post { handleEvent(event) }
                    }
                })
                created.preloadSessions()
                withContext(Dispatchers.Main) {
                    // Read state here, not before the hop: events posted while
                    // `client` was still null were dropped (a direct machine
                    // can finish its first sync in ~100 ms).
                    client = created
                    workspace = created.workspace()
                    connectivity = created.connectivity()
                    epoch++
                    phase = Phase.Ready
                    showSignIn = false
                    if (created.isDirect()) watchDirect(created)
                }
            } catch (t: Throwable) {
                withContext(Dispatchers.Main) {
                    phase = Phase.Failed(t.message ?: "Couldn't open the workspace")
                }
            }
        }
    }

    private fun handleEvent(event: ClientEvent) {
        val c = client ?: return
        when (event) {
            is ClientEvent.WorkspaceChanged -> requestRefresh()
            // A streaming turn fires these many times a second; one UI pass per
            // quarter second is plenty for the chat chrome and the list.
            is ClientEvent.SessionChanged, is ClientEvent.ComposerChanged -> bumpEpochSoon()
            is ClientEvent.ConnectivityChanged -> {
                connectivity = c.connectivity()
                epoch++
            }
            is ClientEvent.AuthExpired -> {
                showToast(event.reason)
                signOut()
            }
            else -> epoch++
        }
    }

    /**
     * True while the sessions list or a chat is being dragged or flung.
     * Workspace refreshes wait for it (at most [SCROLL_DEFER_MS]) so the list
     * does not rebuild under the finger.
     */
    @Volatile
    var scrolling = false

    private var refreshJob: Job? = null
    private var refreshAgain = false
    private var lastRefreshAt = 0L
    private var epochJob: Job? = null

    /**
     * Coalesced workspace refresh. A busy engine (a streaming turn touches its
     * session several times a second) used to re-read the whole workspace
     * and rebuild the list on every event, on the main thread, which made
     * scrolling stutter and let drags turn into taps. Now there is at most
     * one read every [REFRESH_GAP_MS], done off the main thread, and it waits
     * while a list is moving. The list only recomposes when the snapshot
     * actually changed.
     */
    private fun requestRefresh() {
        val c = client ?: return
        if (refreshJob?.isActive == true) {
            refreshAgain = true
            return
        }
        refreshJob = viewModelScope.launch {
            do {
                refreshAgain = false
                val wait = lastRefreshAt + REFRESH_GAP_MS - SystemClock.uptimeMillis()
                if (wait > 0) delay(wait)
                val deadline = SystemClock.uptimeMillis() + SCROLL_DEFER_MS
                while (scrolling && SystemClock.uptimeMillis() < deadline) delay(50)
                if (client !== c) return@launch
                val current = workspace
                val read = withContext(Dispatchers.Default) {
                    runCatching {
                        val next = c.workspace()
                        Triple(next, next != current, c.connectivity())
                    }.getOrNull()
                }
                if (client !== c) return@launch
                lastRefreshAt = SystemClock.uptimeMillis()
                if (read != null) {
                    if (read.second) workspace = read.first
                    if (read.third != connectivity) connectivity = read.third
                    epoch++
                }
            } while (refreshAgain)
        }
    }

    private fun bumpEpochSoon() {
        if (epochJob?.isActive == true) return
        epochJob = viewModelScope.launch {
            delay(EPOCH_GAP_MS)
            epoch++
        }
    }

    private fun shutdownClient() {
        directJob?.cancel()
        directJob = null
        refreshJob?.cancel()
        refreshJob = null
        epochJob?.cancel()
        epochJob = null
        directStatus = null
        runCatching { client?.shutdown() }
        runCatching { client?.close() }
        client = null
    }

    /**
     * Poll the direct link once a second (off the main thread): the sessions
     * page shows its phase and errors instead of a blank list. If frames keep
     * arriving but no refresh happened for a while, re-read the workspace
     * (belt and braces if an event was missed).
     */
    private fun watchDirect(created: CoreClient) {
        directJob?.cancel()
        directJob = viewModelScope.launch {
            var lastFrames = -1L
            while (client === created) {
                val status = withContext(Dispatchers.Default) { runCatching { created.directStatus() }.getOrNull() }
                if (client !== created) break
                if (status != directStatus) directStatus = status
                val frames = status?.streams?.sumOf { it.frames.toLong() } ?: 0L
                if (frames != lastFrames) {
                    lastFrames = frames
                    if (SystemClock.uptimeMillis() - lastRefreshAt > STALE_REFRESH_MS) requestRefresh()
                }
                delay(1000)
            }
        }
    }

    private var dismissedNotice by mutableStateOf(prefs.getString("dismissedNotice", null))

    fun noticeDismissed(notice: String) = dismissedNotice == notice

    fun dismissNotice(notice: String) {
        dismissedNotice = notice
        prefs.edit().putString("dismissedNotice", notice).apply()
    }

    /** Drop the current SSH link (even a stalled one) and dial again. */
    fun retryDirect() {
        val c = client ?: return
        c.reconnectDirect()
        directStatus = runCatching { c.directStatus() }.getOrNull()
    }

    /** Plain-text diagnostics for "Copy" (no secrets: host, phases, counters). */
    fun linkReport(): String {
        val s = directStatus ?: return "Not connected to a machine"
        val m = machines.firstOrNull { it.id == activeMachine }
        val fmt = java.text.SimpleDateFormat("HH:mm:ss", java.util.Locale.US)
        return buildString {
            appendLine("Zeron Android ${sh.zeron.android.BuildConfig.VERSION_NAME} (${sh.zeron.android.BuildConfig.VERSION_CODE})")
            m?.let { appendLine("Machine: ${it.user}@${it.host}:${it.port} → 127.0.0.1:${it.enginePort}") }
            appendLine("Phase: ${s.phase}")
            appendLine("Engine: ${s.engineVersion ?: "?"} (device ${s.engineDeviceId?.take(8) ?: "?"})")
            s.lastError?.let { appendLine("Last error: $it") }
            s.notice?.let { appendLine("Note: $it") }
            for (st in s.streams) {
                append("${st.name}: ${st.frames} frames, ${st.rows} rows")
                if (st.skippedRows > 0u) append(", ${st.skippedRows} skipped")
                if (st.repairedRows > 0u) append(", ${st.repairedRows} repaired")
                st.error?.let { append(" — $it") }
                appendLine()
            }
            val ws = workspace
            appendLine("Workspace: ${ws?.projects?.size ?: 0} projects, ${ws?.front?.recent?.size ?: 0} recent, ${ws?.archived?.size ?: 0} archived")
            appendLine("Log:")
            for (line in s.log) appendLine("  ${fmt.format(java.util.Date(line.atMs))} ${line.message}")
        }
    }

    fun refreshPull() {
        if (directStatus != null && directStatus?.phase != uniffi.zeron_core.DirectPhase.LIVE) retryDirect()
        client?.onForeground()
        workspace = client?.workspace()
        epoch++
    }

    fun liveCounts(): Pair<Int, Int> {
        val ws = workspace ?: return 0 to 0
        val rows = buildList {
            addAll(ws.front.pinned)
            ws.front.sections.forEach { addAll(it.sessions) }
            addAll(ws.front.recent)
        }
        val seen = HashSet<String>()
        var working = 0
        var awaiting = 0
        for (row in rows) {
            if (!seen.add(row.id)) continue
            if (row.indicator == ChatIndicator.WORKING) working++
            if (row.indicator == ChatIndicator.AWAITING_INPUT) awaiting++
        }
        return working to awaiting
    }

    fun sessionsIn(id: String): List<SessionRow> {
        val ws = workspace ?: return emptyList()
        return when (id) {
            "pinned" -> ws.front.pinned
            "archived" -> ws.archived
            "recent" -> ws.front.recent
            else -> ws.front.sections.firstOrNull { it.id == id }?.sessions ?: emptyList()
        }
    }

    fun search(query: String): List<SessionRow> {
        val c = client ?: return emptyList()
        val q = query.trim()
        if (q.isEmpty()) return workspace?.front?.recent ?: emptyList()
        return c.search(q, 60u).map { it.session }
    }

    /** Pin or unpin, then offer Undo (a swipe or tap can land by accident). */
    fun pin(id: String, pinned: Boolean) {
        val done = attempt { if (pinned) it.pinSession(id) else it.unpinSession(id) }
        if (done) showUndo(if (pinned) "Pinned" else "Unpinned") { attempt { if (pinned) it.unpinSession(id) else it.pinSession(id) } }
    }
    fun archive(id: String) {
        if (attempt { it.archiveSession(id) }) showUndo("Archived") { attempt { it.unarchiveSession(id) } }
    }
    fun unarchive(id: String) {
        if (attempt { it.unarchiveSession(id) }) showUndo("Unarchived") { attempt { it.archiveSession(id) } }
    }
    fun rename(id: String, title: String) = attempt { it.renameSession(id, title) }
    fun move(id: String, section: String?) = attempt { it.assignSection(id, section) }
    fun createSection(name: String) = attempt { it.createSection(name) }
    fun renameSection(id: String, name: String) = attempt { it.renameSection(id, name) }
    fun deleteSection(id: String) = attempt { it.deleteSection(id) }

    private fun attempt(body: (CoreClient) -> Unit): Boolean {
        val c = client ?: return false
        val ok = try {
            body(c)
            true
        } catch (t: Throwable) {
            showToast(t.message ?: "Couldn't update")
            false
        }
        requestRefresh()
        return ok
    }

    fun homeColorIndex(): Int = projectColorIndex("home").toInt()

    // ── auth ──────────────────────────────────────────────────────────────

    fun authorizeUrl(): String {
        val state = UUID.randomUUID().toString()
        authState = state
        return workosAuthorizeUrl(state)
    }

    fun completeAuth(url: String) {
        val callback = parseAuthCallback(url) ?: run {
            signInError = "Sign-in didn't complete. Try again."
            return
        }
        when (callback) {
            is AuthCallback.Error -> signInError = callback.description ?: callback.error
            is AuthCallback.Code -> {
                if (authState != null && callback.state != null && callback.state != authState) {
                    signInError = "Sign-in didn't complete. Try again."
                    return
                }
                viewModelScope.launch {
                    signInBusy = true
                    signInError = null
                    try {
                        val edge = authProductionEdgeUrl()
                        val exchange = authExchangeCode(edge, callback.code)
                        val orgs = authListOrgs(edge, exchange.tokens.accessToken)
                        if (orgs.isEmpty()) {
                            signInError = "This account isn't in an organization yet."
                        } else if (orgs.size == 1) {
                            finishOrg(exchange.user.id, orgs[0], exchange.tokens.refreshToken)
                        } else {
                            pendingExchange = exchange.user.id to exchange.tokens
                            authOrgs = orgs
                        }
                    } catch (t: Throwable) {
                        signInError = t.message ?: "Sign-in didn't complete."
                    } finally {
                        signInBusy = false
                    }
                }
            }
        }
    }

    fun chooseOrg(org: AuthOrg) {
        val pending = pendingExchange ?: return
        authOrgs = null
        viewModelScope.launch {
            signInBusy = true
            try {
                finishOrg(pending.first, org, pending.second.refreshToken)
            } catch (t: Throwable) {
                signInError = t.message ?: "Couldn't join that organization."
            } finally {
                signInBusy = false
            }
        }
    }

    private suspend fun finishOrg(userId: String, org: AuthOrg, refresh: String) {
        val edge = authProductionEdgeUrl()
        val tokens = authRefresh(edge, refresh, org.organizationId)
        prefs.edit()
            .putString("account", "$userId\n${org.organizationId}\n${tokens.accessToken}\n${tokens.refreshToken}")
            .putString("boot", "account")
            .apply()
        withContext(Dispatchers.Main) {
            shutdownClient()
            authOrgs = null
        }
        start(
            Credentials.WorkOs(userId, org.organizationId, tokens),
            demo = false,
        )
    }

    private fun storedCredentials(): Credentials? {
        val raw = prefs.getString("account", null) ?: return null
        val parts = raw.split('\n')
        if (parts.size < 4) return null
        return Credentials.WorkOs(
            parts[0],
            parts[1],
            uniffi.zeron_core.AuthTokens(parts[2], parts[3]),
        )
    }

    // ── wallpaper ─────────────────────────────────────────────────────────

    fun setWallpaper(bytes: ByteArray, name: String) {
        val dir = File(getApplication<Application>().filesDir, "wallpaper").apply { mkdirs() }
        File(dir, "source.jpg").writeBytes(bytes)
        prefs.edit().putString("wallpaperName", name).apply()
        loadWallpaper()
    }

    fun clearWallpaper() {
        File(getApplication<Application>().filesDir, "wallpaper/source.jpg").delete()
        wallpaper = null
    }

    fun applyWallpaperEffect(effect: WallpaperEffect) {
        wallpaperEffect = effect
        prefs.edit().putString("wallpaperEffect", effectKey(effect)).apply()
        loadWallpaper()
    }

    private fun loadWallpaper() {
        val file = File(getApplication<Application>().filesDir, "wallpaper/source.jpg")
        if (!file.exists()) {
            wallpaper = null
            return
        }
        viewModelScope.launch(Dispatchers.Default) {
            val src = BitmapFactory.decodeFile(file.absolutePath) ?: return@launch
            val max = 900
            val scale = minOf(1f, max / maxOf(src.width, src.height).toFloat())
            val w = (src.width * scale).toInt().coerceAtLeast(1)
            val h = (src.height * scale).toInt().coerceAtLeast(1)
            val scaled = if (w == src.width && h == src.height) src else Bitmap.createScaledBitmap(src, w, h, true)
            val pixels = IntArray(w * h)
            scaled.getPixels(pixels, 0, w, 0, 0, w, h)
            val rgba = ByteArray(w * h * 4)
            for (i in pixels.indices) {
                val p = pixels[i]
                rgba[i * 4] = ((p shr 16) and 0xFF).toByte()
                rgba[i * 4 + 1] = ((p shr 8) and 0xFF).toByte()
                rgba[i * 4 + 2] = (p and 0xFF).toByte()
                rgba[i * 4 + 3] = ((p shr 24) and 0xFF).toByte()
            }
            val dark = appearance == 2 || (appearance == 0 && isNight())
            val rendered = wallpaperRender(rgba, w.toUInt(), h.toUInt(), wallpaperEffect, !dark)
            val text = if (dark) 0xE8E8EAu else 0x27272Cu
            val bg = if (dark) 0x060606u else 0xF3F3F5u
            val opacity = wallpaperSafeOpacity(rendered, w.toUInt(), h.toUInt(), text, bg, 0.35f, 4.5f, 0.55f)
            val out = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
            val argb = IntArray(w * h)
            for (i in argb.indices) {
                val r = rendered[i * 4].toInt() and 0xFF
                val g = rendered[i * 4 + 1].toInt() and 0xFF
                val b = rendered[i * 4 + 2].toInt() and 0xFF
                val a = rendered[i * 4 + 3].toInt() and 0xFF
                argb[i] = (a shl 24) or (r shl 16) or (g shl 8) or b
            }
            out.setPixels(argb, 0, w, 0, 0, w, h)
            withContext(Dispatchers.Main) {
                wallpaper = out
                wallpaperOpacity = opacity
            }
        }
    }

    private fun isNight(): Boolean {
        val mode = getApplication<Application>().resources.configuration.uiMode and android.content.res.Configuration.UI_MODE_NIGHT_MASK
        return mode == android.content.res.Configuration.UI_MODE_NIGHT_YES
    }

    fun wallpaperName(): String? = prefs.getString("wallpaperName", null)

    fun defaultConfig(harness: String, model: String?, effort: String?) = ChatConfig(
        harness = harness,
        model = model,
        reasoning = effort,
        modelOptions = emptyMap(),
        sandbox = SandboxLevel.WORKSPACE_WRITE,
    )

    private fun watchNetwork() {
        val cm = getApplication<Application>().getSystemService(ConnectivityManager::class.java) ?: return
        cm.registerDefaultNetworkCallback(object : ConnectivityManager.NetworkCallback() {
            override fun onAvailable(network: Network) {
                main.post { client?.setNetworkOnline(true) }
            }
            override fun onLost(network: Network) {
                main.post { client?.setNetworkOnline(false) }
            }
        })
    }

    fun onForeground() {
        client?.onForeground()
    }
    fun onBackground() { client?.onBackground() }

    /** Screenshot / deep-link routes, analogous to the iOS `-route` argument. */
    fun applyLaunch(route: String?, chat: String?, theme: String?, query: String?) {
        when (theme) {
            "dark" -> applyAppearance(2)
            "light" -> applyAppearance(1)
            "system" -> applyAppearance(0)
        }
        when (route) {
            "settings", "more" -> tab = Tab.Settings
            "search" -> {
                tab = Tab.Search
                if (!query.isNullOrEmpty()) searchQuery = query
            }
            "signin", "signedout" -> {
                signOut()
            }
            "new" -> showNewSession = true
            "spaces", "menu" -> pendingSpaceMenu = true
            "session" -> if (!chat.isNullOrEmpty()) openSession(chat)
            "machines" -> showMachines = true
        }
    }

    /** `addmachine` launch route: prefill the editor (screenshots/e2e). */
    fun launchAddMachine(name: String?, host: String?, port: String?, user: String?) {
        showMachines = true
        editMachine = Machine(
            name = name.orEmpty(),
            host = host.orEmpty(),
            port = port?.toIntOrNull() ?: 22,
            user = user.orEmpty(),
        )
    }

    companion object {
        const val REFRESH_GAP_MS = 350L
        const val SCROLL_DEFER_MS = 1200L
        const val EPOCH_GAP_MS = 250L
        const val STALE_REFRESH_MS = 4000L
        val FACE_FILES: Map<FaceRole, String> get() = sh.zeron.android.design.FontChain.files

        fun loadFaces(app: Application): Map<FaceRole, Typeface> = sh.zeron.android.design.FontChain.faces(app)

        fun effectKey(effect: WallpaperEffect) = when (effect) {
            WallpaperEffect.NONE -> "none"
            WallpaperEffect.DITHER -> "dither"
            WallpaperEffect.ASCII -> "ascii"
            WallpaperEffect.HALFTONE -> "halftone"
            WallpaperEffect.SCANLINES -> "scanlines"
        }

        fun effectFrom(key: String?) = when (key) {
            "dither" -> WallpaperEffect.DITHER
            "ascii" -> WallpaperEffect.ASCII
            "halftone" -> WallpaperEffect.HALFTONE
            "scanlines" -> WallpaperEffect.SCANLINES
            else -> WallpaperEffect.NONE
        }

        fun effectLabel(effect: WallpaperEffect) = when (effect) {
            WallpaperEffect.NONE -> "None"
            WallpaperEffect.DITHER -> "Dither"
            WallpaperEffect.ASCII -> "ASCII"
            WallpaperEffect.HALFTONE -> "Halftone"
            WallpaperEffect.SCANLINES -> "Scanlines"
        }
    }
}

/**
 * CoreText stand-in: widths in the same point space Rust measures with, taken
 * from the same [sh.zeron.android.design.FontChain] Typefaces the painter draws
 * with. Clusters are measured in context (a cluster's width on its first
 * scalar, zero on the rest). In mono faces a wide (CJK) cluster is exactly two
 * Geist Mono cells, which is also how [sh.zeron.android.ui.TranscriptListView]
 * places it.
 */
class AndroidMeasurer(private val faces: Map<FaceRole, Typeface>) : PlatformMeasurer {
    private val paints = ThreadLocal.withInitial { Paint(Paint.ANTI_ALIAS_FLAG or Paint.LINEAR_TEXT_FLAG) }
    private val breaks = ThreadLocal.withInitial { android.icu.text.BreakIterator.getCharacterInstance() }

    override fun measure(face: FaceRole, size: Float, ligatures: Boolean, text: String): Float {
        val paint = paint(face, size, ligatures)
        if (!sh.zeron.android.design.FontChain.isMono(face) || !sh.zeron.android.design.FontChain.hasWide(text)) {
            return paint.measureText(text)
        }
        return measureRun(face, size, ligatures, text).sum()
    }

    override fun measureRun(face: FaceRole, size: Float, ligatures: Boolean, text: String): List<Float> {
        val paint = paint(face, size, ligatures)
        val mono = sh.zeron.android.design.FontChain.isMono(face)
        val cell = if (mono) sh.zeron.android.design.FontChain.cellWidth(paint) else 0f
        val out = ArrayList<Float>(text.length)
        val chars = text.toCharArray()
        val units = FloatArray(chars.size)
        if (chars.isNotEmpty()) paint.getTextRunAdvances(chars, 0, chars.size, 0, chars.size, false, units, 0)
        val it = breaks.get()!!
        it.setText(text)
        var start = it.first()
        var end = it.next()
        while (end != android.icu.text.BreakIterator.DONE) {
            val first = text.codePointAt(start)
            val width = if (mono && sh.zeron.android.design.FontChain.isWide(first)) {
                cell * 2f
            } else {
                var w = 0f
                for (u in start until end) w += units[u]
                w
            }
            out.add(width)
            var i = start + Character.charCount(first)
            while (i < end) {
                out.add(0f)
                i += Character.charCount(text.codePointAt(i))
            }
            start = end
            end = it.next()
        }
        return out
    }

    private fun paint(face: FaceRole, size: Float, ligatures: Boolean): Paint {
        val paint = paints.get()!!
        paint.typeface = faces[face] ?: faces[FaceRole.SANS]
        paint.textSize = size
        paint.fontFeatureSettings = if (ligatures) "\"liga\" 1, \"calt\" 1" else "\"liga\" 0, \"calt\" 0"
        return paint
    }
}
