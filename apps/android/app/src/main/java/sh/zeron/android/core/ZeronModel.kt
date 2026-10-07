package sh.zeron.android.core

import sh.zeron.android.R
import android.app.Application
import android.content.Intent
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
import kotlinx.coroutines.async
import kotlinx.coroutines.awaitAll
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

    /** Home list view: grouped by project (default) or one list by agent activity. */
    enum class ListMode(val key: String) {
        Project("project"),
        Activity("activity");

        companion object {
            fun from(key: String?): ListMode = entries.firstOrNull { it.key == key } ?: Project
        }
    }

    var phase by mutableStateOf<Phase>(Phase.Loading)
    var workspace by mutableStateOf<WorkspaceSnapshot?>(null)
    var epoch by mutableIntStateOf(0)
    var appearance by mutableIntStateOf(0)
    /** Theme ids per appearance and the accent override, same model as the desktop's ThemeSelection. */
    var themeLight by mutableStateOf(sh.zeron.android.design.ZeronThemes.DEFAULT_LIGHT)
        private set
    var themeDark by mutableStateOf(sh.zeron.android.design.ZeronThemes.DEFAULT_DARK)
        private set
    var accent by mutableStateOf(sh.zeron.android.design.AccentChoice.THEME)
        private set
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
        internal set
    var showMachines by mutableStateOf(false)
    var editMachine by mutableStateOf<Machine?>(null)
    var connectivity by mutableStateOf<Connectivity?>(null)
        private set
    private var probeJob: Job? = null
    /** Direct link phase/errors/counters, polled while a machine is active. */
    var directStatus by mutableStateOf<uniffi.zeron_core.DirectStatus?>(null)
        internal set
    /** The network as the dial order sees it (Wi-Fi subnet, mobile data, which VPN). */
    var network by mutableStateOf(NetworkSnapshot.UNKNOWN)
        internal set
    private var networkWatcher: NetworkWatcher? = null
    /** The Tailscale app is installed on this phone (re-read on connect and foreground). */
    var tailscaleInstalled by mutableStateOf(Tailscale.installed(app))
        internal set
    private var rerouteJob: Job? = null
    /** When the link last changed address (uptime ms), to damp upgrades. */
    private var lastRouteMoveAt = 0L
    private var lastActiveEndpoint: String? = null

    // ── connection chip (home title bar) ─────────────────────────────────
    enum class ConnectionSheet { SWITCHER, FAILURE }

    /** Bottom sheet over the home screen: the quick switcher or the failure reason. */
    var connectionSheet by mutableStateOf<ConnectionSheet?>(null)

    /**
     * The chip jumped straight into Settings > Accounts & Computers > the
     * computer page: one Back unwinds the whole shortcut back home.
     * 快捷入口进来，一次返回直接回主页。
     */
    private var chipShortcut = false

    /** Screenshots only: a fixed chip state instead of the live one. */
    internal var previewConnection by mutableStateOf<ConnectionState.View?>(null)

    private val episodes = ConnectionState.Episodes()
    var showLinkDetails by mutableStateOf(false)

    /** The crash the previous run ended with, until its dialog is dismissed ([CrashLog]). */
    var lastCrash by mutableStateOf<CrashLog.Entry?>(null)

    /** Settings > About > Crash logs. */
    var showCrashLogs by mutableStateOf(false)
    var crashLogs by mutableStateOf<List<CrashLog.Entry>>(emptyList())
        private set
    private var directJob: Job? = null

    // ── updates ───────────────────────────────────────────────────────────
    val updater = Updater(app)
    var updateRelease by mutableStateOf<Updater.Release?>(null)
    var updateChecking by mutableStateOf(false)
    var updateProgress by mutableStateOf<Float?>(null)
    var updateError by mutableStateOf<String?>(null)
    var updateStatus by mutableStateOf<UpdateStatus?>(null)
    var showUpdate by mutableStateOf(false)
    internal var downloadedApk by mutableStateOf<File?>(null)
    private var downloadJob: Job? = null
    private var checkJob: Job? = null
    /** The running download was started by auto-update (the badge stays hidden until it's ready). */
    private var downloadIsAuto by mutableStateOf(false)
    /** Set to stop the running download; each run has its own, so a stopped run can't touch the next one's state. */
    private var downloadCancel: java.util.concurrent.atomic.AtomicBoolean? = null
    /** Release tag whose download was cancelled once (its partial file kept); a second cancel deletes it. */
    private var cancelledTag: String? = null
    /** Badge tap while downloading: the small sheet with progress, source, 换个镜像 (Switch mirror) and 取消下载 (Cancel download). */
    var showDownloadSheet by mutableStateOf(false)
    /** The 换个镜像 (Switch mirror) list. */
    var showSourcePicker by mutableStateOf(false)
    /** What each download source ([UpdateSources.Source.key]) did last in this session. */
    var sourceStats by mutableStateOf<Map<String, SourceStat>>(emptyMap())
    var autoUpdate by mutableStateOf(updater.autoUpdate)
        private set
    /** Settings > 自动选择线路 / Auto-select route (see [MachineStore.autoRoute]). */
    var autoRoute by mutableStateOf(machineStore.autoRoute)
        private set
    /** Bumped when an address is picked by hand, so pickers re-read [pinnedAddress]. */
    var routePicks by mutableStateOf(0)
        private set

    var client: CoreClient? = null
        private set

    /**
     * Where plan usage (composer ring, Usage sheet) comes from: the host's
     * signed-in agent accounts. A seam so JVM screenshot tests can supply
     * fixture accounts; the demo host reports none.
     */
    var agentUsageSource: suspend (deviceId: String, force: Boolean) -> List<uniffi.zeron_core.AgentUsage> = { deviceId, force ->
        (client ?: error(str(R.string.not_connected))).listAgentUsage(deviceId, force)
    }
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
        themeLight = prefs.getString("themeLight", null) ?: themeLight
        themeDark = prefs.getString("themeDark", null) ?: themeDark
        accent = sh.zeron.android.design.AccentChoice.of(prefs.getString("accent", null))
        wallpaperEffect = effectFrom(prefs.getString("wallpaperEffect", "none"))
        loadWallpaper()
        regroupComputers()
        startDefault()
        watchNetwork()
        quietUpdateCheck()
        // Computer names crash logs must not contain (saved computers are read by CrashLog itself).
        CrashLog.extraNames = { workspace?.devices.orEmpty().map { it.name } + machines.map { it.title() } }
        crashLogs = CrashLog.list(app)
        lastCrash = CrashLog.pending(app)
    }

    override fun onCleared() {
        CrashLog.extraNames = { emptyList() }
        super.onCleared()
    }

    fun dismissLastCrash() {
        lastCrash = null
        CrashLog.markSeen(getApplication())
        crashLogs = CrashLog.list(getApplication())
    }

    fun openCrashLogs() {
        crashLogs = CrashLog.list(getApplication())
        showCrashLogs = true
    }

    fun clearCrashLogs() {
        CrashLog.clear(getApplication())
        crashLogs = emptyList()
        lastCrash = null
    }

    fun copyCrashLog(text: String) {
        val cm = getApplication<Application>().getSystemService(android.content.ClipboardManager::class.java)
        cm?.setPrimaryClip(android.content.ClipData.newPlainText("Zeron crash log", text))
        showToast(str(R.string.copied))
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
        if (listMode == ListMode.Project) {
            ws.front.recent.forEach { collapsed.add(sh.zeron.android.ui.SessionGrouping.projectKey(it.project?.id)) }
        }
        prefs.edit().putStringSet("collapsed", HashSet(collapsed)).apply()
        collapsedIds = collapsed.toSet()
    }

    /** Persisted home list view ("listMode"); unknown values read as By Project. */
    var listMode by mutableStateOf(ListMode.from(prefs.getString("listMode", null)))
        private set

    /** New Session project picker order (see ProjectOrder); remembered. */
    enum class ProjectSort(val key: String) {
        Recent("recent"),
        Name("name");

        companion object {
            fun from(key: String?): ProjectSort = entries.firstOrNull { it.key == key } ?: Recent
        }
    }

    var projectSort by mutableStateOf(ProjectSort.from(prefs.getString("projectSort", null)))
        private set

    fun applyProjectSort(sort: ProjectSort) {
        projectSort = sort
        prefs.edit().putString("projectSort", sort.key).apply()
    }

    fun applyListMode(mode: ListMode) {
        listMode = mode
        prefs.edit().putString("listMode", mode.key).apply()
    }

    fun applyTheme(dark: Boolean, id: String) {
        if (dark) themeDark = id else themeLight = id
        prefs.edit().putString(if (dark) "themeDark" else "themeLight", id).apply()
    }

    fun applyAccent(choice: sh.zeron.android.design.AccentChoice) {
        accent = choice
        prefs.edit().putString("accent", choice.key).apply()
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

    /** A string in the app's chosen language (the model has no activity context). */
    private fun str(id: Int, vararg args: Any): String = AppLanguage.string(getApplication(), id, *args)

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
        if (chipShortcut) {
            chipShortcut = false
            if (editMachine != null || showMachines || tab != Tab.Sessions) {
                editMachine = null
                showMachines = false
                settingsStack.clear()
                sessionStack.clear()
                tab = Tab.Sessions
                return true
            }
        }
        if (showUpdate) {
            showUpdate = false
            return true
        }
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
            forced == "demo" || (BuildConfig.DEMO_BY_DEFAULT && prefs.getString("account", null) == null) -> start(demoCredentials(), demo = true)
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
        network = networkWatcher?.snapshot ?: NetworkWatcher.current(getApplication())
        refreshTailscale()
        lastActiveEndpoint = null
        val target = runCatching { machineStore.target(machine, route = machineStore.plan(machine, network)) }.getOrElse {
            phase = Phase.Failed(it.message ?: str(R.string.machine_key_failed))
            return
        }
        start(uniffi.zeron_core.Credentials.Direct(target), demo = false, dir = CoreConnect.dataDirName(machine.id), active = machine.id)
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

    /** What the chip shows; reads Compose state, so composables update with the link. */
    fun connectionView(): ConnectionState.View {
        previewConnection?.let { return it }
        val workspaceKind = when (activeMachine) {
            "demo" -> ConnectionState.Workspace.DEMO
            "cloud" -> ConnectionState.Workspace.CLOUD
            else -> ConnectionState.Workspace.DIRECT
        }
        val status = directStatus
        var dot = ConnectionState.dot(workspaceKind, phase is Phase.Loading, status?.phase, connectivity?.state)
        val diagnosis = if (workspaceKind == ConnectionState.Workspace.DIRECT) connectionDiagnosis() else null
        // Fail fast: nothing in the dial order can work on this network
        // (Tailscale off away from home…): red at once, with the reason,
        // while the core keeps dialling in case the guess is wrong.
        if (diagnosis?.early == true && dot == ConnectionState.Dot.CONNECTING) dot = ConnectionState.Dot.FAILED
        return ConnectionState.View(
            title = activeTitle(),
            workspace = workspaceKind,
            dot = dot,
            error = status?.lastError?.takeIf { dot == ConnectionState.Dot.FAILED },
            retryAtMs = status?.retryAtMs?.takeIf { dot == ConnectionState.Dot.FAILED && status?.phase == uniffi.zeron_core.DirectPhase.FAILED },
            id = activeMachine,
            route = activeRoute()?.takeIf { dot == ConnectionState.Dot.CONNECTED },
            diagnosis = diagnosis.takeIf { dot == ConnectionState.Dot.FAILED },
        )
    }

    /**
     * The active computer's link in terms of the phone's network: why
     * nothing can work here (before the dial gives up), or why it failed.
     * Null while connected, or with nothing to add. Reads Compose state.
     */
    fun connectionDiagnosis(): ConnectionDiagnosis.Result? {
        val machine = machines.firstOrNull { it.id == activeMachine } ?: return null
        val status = directStatus
        if (status?.phase == uniffi.zeron_core.DirectPhase.LIVE || status?.phase == uniffi.zeron_core.DirectPhase.SYNCING) return null
        routePicks
        val addresses = machine.addresses()
        val input = ConnectionDiagnosis.Input(
            addresses = addresses,
            dialled = if (autoRoute) addresses else listOf(pinnedAddress(machine)),
            net = network,
            tailscaleInstalled = tailscaleInstalled,
            autoRoute = autoRoute,
        )
        ConnectionDiagnosis.preflight(input)?.let { return it }
        if (status?.phase != uniffi.zeron_core.DirectPhase.FAILED) return null
        val errors = status.endpoints.associate { Endpoint(it.host, it.port.toInt()).key to it.lastError }
        return ConnectionDiagnosis.diagnose(input, status.lastError, errors)
    }

    fun refreshTailscale() {
        tailscaleInstalled = Tailscale.installed(getApplication())
    }

    /** "打开 Tailscale" ("Open Tailscale"): its app, or its store page (with a note) when it isn't installed. */
    fun openTailscale() {
        val app = getApplication<Application>()
        if (!Tailscale.open(app)) {
            refreshTailscale()
            showToast(str(R.string.conn_tailscale_missing_toast))
            Tailscale.openStore(app)
        }
    }

    /** "安装 Tailscale" ("Install Tailscale"): its store page. */
    fun installTailscale() {
        Tailscale.openStore(getApplication())
    }

    /** Pops the failure sheet once per failure episode; closes it once connected. */
    private fun noteConnection() {
        val dot = connectionView().dot
        if (episodes.update(dot) && connectionSheet == null) connectionSheet = ConnectionSheet.FAILURE
        if (dot == ConnectionState.Dot.CONNECTED && connectionSheet == ConnectionSheet.FAILURE) connectionSheet = null
    }

    /**
     * Chip tap: Settings > 账户与电脑 (Accounts & Computers) > the active computer's page (its link
     * state, the failure reason and fixes on top); Demo / Cloud open the
     * computers list. Back walks up that path.
     */
    fun openConnectionChip() {
        connectionSheet = null
        val id = connectionView().id
        tab = Tab.Settings
        showMachines = true
        editMachine = machines.firstOrNull { it.id == id }
        // One back tap returns home wherever the chip led — a computer's
        // page, or the computers list for Demo/Cloud.
        chipShortcut = true
    }

    /** Chip long-press (and "切换电脑" ("Switch Computer") on the computer page): the quick switcher over home. */
    fun openSwitcher() {
        editMachine = null
        showMachines = false
        if (tab != Tab.Sessions) tab = Tab.Sessions
        sessionStack.clear()
        connectionSheet = ConnectionSheet.SWITCHER
    }

    /** Switcher pick: "demo", "cloud" or a machine id. The current one retries if it's down. */
    fun switchConnection(target: String) {
        connectionSheet = null
        if (target == activeMachine) {
            if (connectionView().dot == ConnectionState.Dot.FAILED) retryConnection()
            return
        }
        when (target) {
            "demo" -> enterDemo()
            "cloud" -> useCloud()
            else -> machines.firstOrNull { it.id == target }?.let { connectMachine(it) }
        }
    }

    /** Failure sheet "Retry": redial now (direct) or re-open the workspace. */
    fun retryConnection() {
        connectionSheet = null
        if (client?.isDirect() == true) retryDirect() else refreshPull()
    }

    fun activeTitle(): String = when (activeMachine) {
        "demo" -> str(R.string.demo)
        "cloud" -> "Zeron Cloud"
        else -> machines.firstOrNull { it.id == activeMachine }?.title() ?: str(R.string.machine_fallback)
    }

    fun saveMachine(machine: Machine, secret: String?) {
        machineStore.save(machine, secret)
        machines = machineStore.list()
        if (machine.id == activeMachine) reroute(force = false)
    }

    /**
     * Computers saved twice (LAN IP and Tailscale IP as two entries) become
     * one, once; whatever pointed at a merged-away entry follows it.
     */
    private fun regroupComputers() {
        val moved = runCatching { machineStore.groupDuplicates() }.getOrDefault(emptyMap())
        if (moved.isNotEmpty()) {
            prefs.getString("lastMachine", null)?.let { last -> moved[last]?.let { prefs.edit().putString("lastMachine", it).apply() } }
            val scheduled = sh.zeron.android.schedule.ScheduledStore(getApplication())
            scheduled.list().filter { it.workspace in moved }.forEach { scheduled.add(it.copy(workspace = moved.getValue(it.workspace))) }
        }
        machines = machineStore.list()
    }

    /** Accounts & Computers: [otherId]'s addresses join [intoId]. */
    fun mergeMachines(intoId: String, otherId: String) {
        machineStore.merge(intoId, otherId) ?: return
        machines = machineStore.list()
        if (activeMachine == otherId) machines.firstOrNull { it.id == intoId }?.let { connectMachine(it) }
        else if (activeMachine == intoId) reroute(force = false)
        showToast(str(R.string.computers_merged))
    }

    /** Edit computer: [endpoint] becomes a computer of its own. */
    fun splitAddress(machine: Machine, endpoint: Endpoint): Machine? {
        val alone = machineStore.split(machine.id, endpoint, "${machine.title()} (${endpoint.display()})") ?: return null
        machines = machineStore.list()
        if (activeMachine == machine.id) reroute(force = false)
        return alone
    }

    /** The address kind the direct link runs over now (chip route label). */
    /** Key of the address [machineId]'s link runs over right now, if it's up. */
    fun connectedAddress(machineId: String): String? {
        if (activeMachine != machineId || activeRoute() == null) return null
        val active = directStatus?.endpoints?.firstOrNull { it.active } ?: return null
        return Endpoint(active.host, active.port.toInt()).key
    }

    fun activeRoute(): EndpointKind? {
        val status = directStatus ?: return null
        if (status.phase != uniffi.zeron_core.DirectPhase.LIVE && status.phase != uniffi.zeron_core.DirectPhase.SYNCING) return null
        val active = status.endpoints.firstOrNull { it.active } ?: return null
        return EndpointKind.fromWire(active.kind)
    }

    /**
     * The network changed (or the addresses did): hand the core the new dial
     * order, and move the link when [RouteSwitch] says so. The client and
     * every open session stay; only the SSH link is redialled.
     */
    private fun reroute(force: Boolean) {
        val c = client ?: return
        if (!c.isDirect()) return
        val machine = machines.firstOrNull { it.id == activeMachine } ?: return
        val plan = machineStore.plan(machine, network)
        runCatching { c.setDirectEndpoints(sshEndpoints(plan)) }
        val status = runCatching { c.directStatus() }.getOrNull() ?: return
        val link = when (status.phase) {
            uniffi.zeron_core.DirectPhase.LIVE, uniffi.zeron_core.DirectPhase.SYNCING -> RouteSwitch.Link.LIVE
            uniffi.zeron_core.DirectPhase.FAILED -> RouteSwitch.Link.FAILED
            uniffi.zeron_core.DirectPhase.CONNECTING -> RouteSwitch.Link.CONNECTING
        }
        val active = status.endpoints.firstOrNull { it.active }?.let { Endpoint(it.host, it.port.toInt()).key }
        val since = SystemClock.uptimeMillis() - lastRouteMoveAt
        if (force || RouteSwitch.decide(plan, active, link, since) == RouteSwitch.Action.RECONNECT) {
            c.reconnectDirect()
        }
        directStatus = runCatching { c.directStatus() }.getOrNull()
    }

    /**
     * Settings > 自动选择线路 (Auto-select route). Turning it off keeps the link where it is: the
     * address in use becomes the active computer's picked one.
     */
    fun applyAutoRoute(on: Boolean) {
        if (on == autoRoute) return
        if (!on) {
            directStatus?.endpoints?.firstOrNull { it.active }?.let { a ->
                if (machines.any { it.id == activeMachine }) machineStore.pinAddress(activeMachine, Endpoint(a.host, a.port.toInt()).key)
            }
        }
        machineStore.autoRoute = on
        autoRoute = on
        routePicks++
        reroute(force = false)
    }

    /** The address [machine] uses with auto-select off. */
    fun pinnedAddress(machine: Machine): Endpoint = machineStore.pinnedAddress(machine)

    /**
     * Manual route: [machine] now uses only [endpoint]. Moves the link at once
     * if [machine] is the active computer, else switches to it.
     */
    fun pickRoute(machine: Machine, endpoint: Endpoint) {
        machineStore.pinAddress(machine.id, endpoint.key)
        routePicks++
        if (activeMachine == machine.id && client?.isDirect() == true) {
            val c = client ?: return
            runCatching { c.setDirectEndpoints(sshEndpoints(machineStore.plan(machine, network))) }
            val active = directStatus?.endpoints?.firstOrNull { it.active }?.let { Endpoint(it.host, it.port.toInt()).key }
            if (active != endpoint.key) c.reconnectDirect()
            directStatus = runCatching { c.directStatus() }.getOrNull()
        } else if (machine.hostKey != null) {
            switchConnection(machine.id)
        }
        val kind = str(when (endpoint.kind) {
            EndpointKind.LAN -> R.string.route_lan
            EndpointKind.TAILSCALE -> R.string.route_tailscale
            EndpointKind.OTHER -> R.string.route_other
        })
        showToast(str(R.string.route_picked, "$kind · ${endpoint.display()}"))
    }

    /** Remember which address works on this network, and when the link moved. */
    private fun noteRoute(status: uniffi.zeron_core.DirectStatus?) {
        val active = status?.endpoints?.firstOrNull { it.active } ?: return
        val key = Endpoint(active.host, active.port.toInt()).key
        if (key != lastActiveEndpoint) {
            if (lastActiveEndpoint != null) lastRouteMoveAt = SystemClock.uptimeMillis()
            lastActiveEndpoint = key
        }
        if (status.phase == uniffi.zeron_core.DirectPhase.LIVE && machines.any { it.id == activeMachine }) {
            machineStore.rememberRoute(activeMachine, network.key, key)
        }
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
                    // Online if any of its addresses answers (probed side by side).
                    val ok = m.hostKey != null && RoutePlanner.plan(m.addresses(), network).map { planned ->
                        async { runCatching { sshProbe(machineStore.target(m, route = listOf(planned))) }.isSuccess }
                    }.awaitAll().any { it }
                    machineOnline = machineOnline + (m.id to ok)
                }
            }
        }
    }

    // ── updates ───────────────────────────────────────────────────────────

    /** Title-bar update badge: arrow (tap = download), ring while downloading, checkmark (tap = install). */
    enum class UpdateBadge { AVAILABLE, DOWNLOADING, READY }

    val updateBadge: UpdateBadge?
        get() {
            if (updateRelease?.newer != true) return null
            val p = updateProgress
            return when {
                p == 1f && downloadedApk != null -> UpdateBadge.READY
                p != null && downloadIsAuto -> null
                p != null -> UpdateBadge.DOWNLOADING
                else -> UpdateBadge.AVAILABLE
            }
        }

    /** The start-up / foreground update check (and its cache sweep) is still running. */
    internal val updateCheckRunning: Boolean get() = checkJob?.isActive == true

    /**
     * App start, every return to the foreground, and every Updater.QUIET_MS
     * (30 min) while in the foreground ([startForegroundChecks]). With
     * auto-update on and a check due, asks for the latest release and
     * remembers it; auto-update off makes no automatic check at all (Settings
     * > Check for updates still does), though what an earlier check found
     * still shows. Then drops stale APKs, picks up an APK that's already
     * downloaded, and with auto-update on downloads a newer release while on
     * an unmetered network. Never installs.
     */
    private fun quietUpdateCheck() {
        if (checkJob?.isActive == true) return
        val check = autoUpdate && updater.dueForQuietCheck()
        // Stamped before trying: a failed check waits the same 30 minutes
        // instead of retrying on every return to the foreground.
        if (check) updater.lastAttemptMs = System.currentTimeMillis()
        checkJob = viewModelScope.launch {
            if (updateRelease == null) updateRelease = updater.lastKnown()?.takeIf { it.newer }
            if (check) {
                runCatching { updater.latest() }.onSuccess { found ->
                    // The check screen may have loaded something newer meanwhile.
                    if (updateRelease == null || found.versionCode >= updateRelease!!.versionCode) updateRelease = found
                }
            }
            val release = updateRelease
            // Re-read at the last moment: a download started while the check
            // ran must not have its partial file swept away.
            withContext(Dispatchers.IO) { updater.cleanStale(updateRelease ?: release) }
            if (release == null || !release.newer) {
                downloadedApk = null
                return@launch
            }
            if (downloadedApk == null) {
                updater.cached(release)?.let { apk ->
                    downloadedApk = apk
                    updateProgress = 1f
                }
            }
            if (downloadedApk == null && autoUpdate && !networkMetered()) startDownload(install = false, auto = true)
        }
    }

    private fun networkMetered(): Boolean =
        getApplication<Application>().getSystemService(android.net.ConnectivityManager::class.java)?.isActiveNetworkMetered ?: true

    fun applyAutoUpdate(on: Boolean) {
        autoUpdate = on
        updater.autoUpdate = on
        if (on) {
            quietUpdateCheck()
        } else if (downloadIsAuto && downloadJob?.isActive == true) {
            // Keeps the .part file; a later download resumes it.
            stopDownload()
        }
    }

    /** Badge tap: download (arrow), progress sheet with cancel / switch mirror (downloading), install (checkmark). */
    fun tapUpdateBadge() {
        when (updateBadge) {
            UpdateBadge.AVAILABLE -> startDownload(install = false, auto = false)
            UpdateBadge.DOWNLOADING -> showDownloadSheet = true
            UpdateBadge.READY -> {
                if (downloadedApk?.exists() == true) {
                    installUpdate()
                } else {
                    // The system cleared the cache: back to the arrow.
                    downloadedApk = null
                    updateProgress = null
                    showToast(str(R.string.badge_download_failed))
                }
            }
            null -> {}
        }
    }

    fun checkForUpdates() {
        showUpdate = true
        updateChecking = true
        updateError = null
        viewModelScope.launch {
            try {
                updateRelease = updater.latest()
            } catch (t: Throwable) {
                updateError = t.message ?: str(R.string.update_check_failed)
            } finally {
                updateChecking = false
            }
        }
    }

    /** Update screen button: download (or pick up the running one), then install. */
    fun downloadUpdate() {
        if (downloadJob?.isActive == true) {
            downloadIsAuto = false
            installWhenDone = true
            return
        }
        startDownload(install = true, auto = false)
    }

    private var installWhenDone = false

    /**
     * One download at a time; [auto] ones stay silent and hide the badge
     * until ready. [prefer] puts that source first (the 换个镜像 / Switch mirror pick); the
     * others still follow if it fails. Resumes whatever part is on disk.
     */
    private fun startDownload(install: Boolean, auto: Boolean, prefer: String? = null) {
        val release = updateRelease ?: return
        if (downloadJob?.isActive == true) return
        val flag = java.util.concurrent.atomic.AtomicBoolean(false)
        downloadCancel = flag
        updateError = null
        val have = updater.partialBytes(release)
        updateProgress = if (release.size > 0) (have.toFloat() / release.size).coerceIn(0f, 0.999f) else 0f
        updateStatus = null
        downloadIsAuto = auto
        installWhenDone = install
        val live = { downloadCancel === flag && !flag.get() }
        downloadJob = viewModelScope.launch {
            try {
                val apk = updater.download(
                    release,
                    prefer = prefer,
                    cancelled = { flag.get() },
                    onFailure = { f -> main.post { sourceStats = sourceStats + (f.key to SourceStat(error = updater.reason(f))) } },
                ) { p ->
                    val f = if (p.total > 0) (p.done.toFloat() / p.total).coerceAtMost(0.999f) else 0f
                    val status = UpdateStatus(p.source, p.done, p.total, p.bytesPerSec, p.key)
                    main.post {
                        if (!live()) return@post
                        updateProgress = f
                        updateStatus = status
                        if (p.bytesPerSec > 0) sourceStats = sourceStats + (p.key to SourceStat(bytesPerSec = p.bytesPerSec))
                    }
                }
                downloadedApk = apk
                updateProgress = 1f
                updateStatus = null
                downloadIsAuto = false
                cancelledTag = null
                showDownloadSheet = false
                showSourcePicker = false
                if (installWhenDone) installUpdate()
            } catch (c: kotlinx.coroutines.CancellationException) {
                if (downloadCancel === flag) resetDownloadState()
                throw c
            } catch (c: UpdateDownloader.Cancelled) {
                if (downloadCancel === flag) resetDownloadState()
            } catch (t: Throwable) {
                if (downloadCancel !== flag) return@launch
                val wasAuto = downloadIsAuto
                val sheet = showDownloadSheet || showSourcePicker
                resetDownloadState()
                updateError = t.message ?: str(R.string.update_download_failed)
                // Auto: quiet, the arrow shows up to retry by hand. Badge: say why.
                if (!wasAuto && (!showUpdate || sheet)) showToast(str(R.string.badge_download_failed))
            }
        }
    }

    private fun resetDownloadState() {
        updateProgress = null
        updateStatus = null
        downloadIsAuto = false
        showDownloadSheet = false
        showSourcePicker = false
    }

    /** Stop the running download; it keeps its .part file. */
    private fun stopDownload() {
        downloadCancel?.set(true)
        updater.abortDownload()
        downloadJob?.cancel()
    }

    /**
     * 取消下载 (Cancel download): stop now and put the arrow back. The bytes so far stay for
     * the next try; cancelling the same release a second time deletes them
     * (a new release drops old parts anyway, see [Updater.download]).
     */
    fun cancelDownload() {
        val release = updateRelease ?: return
        stopDownload()
        downloadCancel = null
        installWhenDone = false
        resetDownloadState()
        val second = cancelledTag == release.tag
        cancelledTag = if (second) null else release.tag
        // No need to wait for the stopped run: it can't write any more.
        viewModelScope.launch {
            if (second) {
                withContext(Dispatchers.IO) { updater.discardPartial(release) }
                showToast(str(R.string.download_cancelled_clean))
            } else {
                val kept = withContext(Dispatchers.IO) { updater.partialBytes(release) }
                showToast(if (kept > 0) str(R.string.download_cancelled_keep, updater.speed(kept)) else str(R.string.download_cancelled))
            }
        }
    }

    /**
     * 换个镜像 (Switch mirror): restart the download from [key], resuming from the bytes
     * already on disk. Not counted as a cancel.
     */
    fun switchSource(key: String) {
        showSourcePicker = false
        val install = installWhenDone
        stopDownload()
        // The stopped run must not reset the progress the next one carries on,
        // and its read may stay blocked for a while: don't wait for it.
        downloadCancel = null
        downloadJob = null
        startDownload(install = install, auto = false, prefer = key)
        if (downloadJob == null) resetDownloadState()
    }

    /** One row of the 换个镜像 (Switch mirror) list. */
    data class SourceChoice(val source: UpdateSources.Source, val current: Boolean, val custom: Boolean, val stat: SourceStat?)

    fun sourceChoices(): List<SourceChoice> {
        val release = updateRelease ?: return emptyList()
        val current = updateStatus?.key
        val custom = updater.customMirrorKey()
        return updater.sources(release).map { SourceChoice(it, it.key == current, it.key == custom, sourceStats[it.key]) }
    }

    /** Where the running download comes from and how fast (for the progress line). */
    data class UpdateStatus(val source: String, val done: Long, val total: Long, val bytesPerSec: Long, val key: String = "")

    /** A source's last speed, or why it was last given up on. */
    data class SourceStat(val bytesPerSec: Long = 0, val error: String? = null)

    /** Hand the download to the browser (it may have its own proxy or download manager). */
    fun openUpdateInBrowser() {
        val release = updateRelease ?: return
        runCatching {
            getApplication<Application>().startActivity(
                Intent(Intent.ACTION_VIEW, android.net.Uri.parse(updater.browserUrl(release))).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
            )
        }.onFailure { showToast(str(R.string.update_no_browser)) }
    }

    fun copyUpdateLink() {
        val release = updateRelease ?: return
        val cm = getApplication<Application>().getSystemService(android.content.ClipboardManager::class.java)
        cm?.setPrimaryClip(android.content.ClipData.newPlainText("APK", updater.browserUrl(release)))
        showToast(str(R.string.copied))
    }

    /** Needs the "install unknown apps" grant first; resumes on return. */
    fun installUpdate() {
        val apk = downloadedApk ?: return
        val app = getApplication<Application>()
        if (!updater.canInstall()) {
            awaitingInstallGrant = true
            app.startActivity(updater.unknownSourcesIntent())
            return
        }
        awaitingInstallGrant = false
        app.startActivity(updater.installIntent(apk))
    }

    private var awaitingInstallGrant = false

    fun signOut() {
        shutdownClient()
        prefs.edit().remove("account").putString("boot", "signedout").apply()
        workspace = null
        sessionStack.clear()
        settingsStack.clear()
        phase = Phase.SignedOut
        showSignIn = true
        showToast(str(R.string.toast_signed_out))
    }

    private fun demoCredentials() = CoreConnect.demoCredentials()

    private fun start(credentials: Credentials, demo: Boolean, dir: String? = null, active: String? = null) {
        phase = Phase.Loading
        activeMachine = active ?: if (demo) "demo" else "cloud"
        viewModelScope.launch(Dispatchers.Default) {
            try {
                val app = getApplication<Application>()
                val dir = File(app.filesDir, dir ?: CoreConnect.dataDirName(if (demo) CoreConnect.DEMO else CoreConnect.CLOUD)).apply { mkdirs() }
                if (text == null) {
                    val bytes = faces.map { (role, _) ->
                        val name = FACE_FILES.getValue(role)
                        FaceData(role, app.assets.open("fonts/$name").use { it.readBytes() })
                    }
                    text = TextSystem(bytes, AndroidMeasurer(faces))
                }
                val config = CoreConnect.config(app, dir)
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
                    CoreConnect.live = CoreConnect.Live(activeMachine, created)
                    workspace = created.workspace()
                    connectivity = created.connectivity()
                    epoch++
                    phase = Phase.Ready
                    showSignIn = false
                    if (created.isDirect()) watchDirect(created)
                }
            } catch (t: Throwable) {
                withContext(Dispatchers.Main) {
                    phase = Phase.Failed(t.message ?: str(R.string.open_workspace_failed))
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
                    if (read.third != connectivity) {
                        connectivity = read.third
                        noteConnection()
                    }
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
        episodes.reset()
        if (connectionSheet == ConnectionSheet.FAILURE) connectionSheet = null
        directJob?.cancel()
        directJob = null
        refreshJob?.cancel()
        refreshJob = null
        epochJob?.cancel()
        epochJob = null
        directStatus = null
        if (client != null && CoreConnect.live?.client === client) CoreConnect.live = null
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
                if (status != directStatus) {
                    directStatus = status
                    noteRoute(status)
                    noteConnection()
                }
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
            appendLine("Network: ${network.key} (VPN ${network.vpn.name.lowercase()})")
            for (e in s.endpoints) {
                val what = when {
                    e.active -> "in use" + (e.latencyMs?.let { " (${it} ms)" } ?: "")
                    e.lastError != null -> "failed: ${e.lastError}"
                    e.lastOkMs != null -> "worked at ${fmt.format(java.util.Date(e.lastOkMs!!))}"
                    else -> "not tried"
                }
                appendLine("  ${e.kind} ${e.host}:${e.port} — $what")
            }
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
        if (done) showUndo(str(if (pinned) R.string.toast_pinned else R.string.toast_unpinned)) { attempt { if (pinned) it.unpinSession(id) else it.pinSession(id) } }
    }
    fun archive(id: String) {
        if (attempt { it.archiveSession(id) }) showUndo(str(R.string.toast_archived)) { attempt { it.unarchiveSession(id) } }
    }
    fun unarchive(id: String) {
        if (attempt { it.unarchiveSession(id) }) showUndo(str(R.string.toast_unarchived)) { attempt { it.archiveSession(id) } }
    }
    fun rename(id: String, title: String) = attempt { it.renameSession(id, title) }
    fun move(id: String, section: String?) = attempt { it.assignSection(id, section) }
    fun movePin(id: String, after: String?, before: String?) = attempt { it.movePin(id, after, before) }
    fun createSection(name: String) = attempt { it.createSection(name) }
    fun renameSection(id: String, name: String) = attempt { it.renameSection(id, name) }
    fun deleteSection(id: String) = attempt { it.deleteSection(id) }

    private fun attempt(body: (CoreClient) -> Unit): Boolean {
        val c = client ?: return false
        val ok = try {
            body(c)
            true
        } catch (t: Throwable) {
            showToast(t.message ?: str(R.string.generic_update_failed))
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
            signInError = str(R.string.signin_incomplete_retry)
            return
        }
        when (callback) {
            is AuthCallback.Error -> signInError = callback.description ?: callback.error
            is AuthCallback.Code -> {
                if (authState != null && callback.state != null && callback.state != authState) {
                    signInError = str(R.string.signin_incomplete_retry)
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
                            signInError = str(R.string.signin_no_org)
                        } else if (orgs.size == 1) {
                            finishOrg(exchange.user.id, orgs[0], exchange.tokens.refreshToken)
                        } else {
                            pendingExchange = exchange.user.id to exchange.tokens
                            authOrgs = orgs
                        }
                    } catch (t: Throwable) {
                        signInError = t.message ?: str(R.string.signin_incomplete)
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
                signInError = t.message ?: str(R.string.join_org_failed)
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

    private fun storedCredentials(): Credentials? = CoreConnect.storedCredentials(getApplication())

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

    fun defaultConfig(harness: String, model: String?, effort: String?, modelOptions: Map<String, String> = emptyMap()) = NewSessionConfig.chatConfig(harness, model, effort, modelOptions)

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
        // Wi-Fi <-> mobile data, VPN up / down: settle for a moment (a
        // hand-over fires several callbacks), then re-plan the route.
        networkWatcher = NetworkWatcher(getApplication()) { snap ->
            main.post {
                rerouteJob?.cancel()
                rerouteJob = viewModelScope.launch {
                    delay(NETWORK_SETTLE_MS)
                    refreshTailscale()
                    if (snap != network) {
                        network = snap
                        // Auto-select off: the picked address stays, whatever the
                        // network (the manual plan holds only it); a down link
                        // still redials it, e.g. once Tailscale is switched on.
                        reroute(force = false)
                        noteConnection()
                    }
                }
            }
        }.also {
            network = it.snapshot
            it.start()
        }
    }

    fun onForeground() {
        client?.onForeground()
        val hadTailscale = tailscaleInstalled
        refreshTailscale()
        if (hadTailscale != tailscaleInstalled) noteConnection()
        if (awaitingInstallGrant && updater.canInstall()) installUpdate()
        startForegroundChecks()
    }
    fun onBackground() {
        client?.onBackground()
        foregroundChecks?.cancel()
        foregroundChecks = null
    }

    /** Update checks while the app is in the foreground; cancelled in [onBackground]. */
    private var foregroundChecks: Job? = null
    internal val foregroundChecksRunning: Boolean get() = foregroundChecks?.isActive == true

    /**
     * A quiet check now, then whenever the next one falls due (every
     * Updater.QUIET_MS) until the app leaves the foreground. No background
     * work: nothing is scheduled while the app is not visible.
     */
    private fun startForegroundChecks() {
        foregroundChecks?.cancel()
        foregroundChecks = viewModelScope.launch {
            Updater.repeatQuietChecks(
                nextInMs = { if (autoUpdate) updater.msUntilQuietCheck() else Updater.QUIET_MS },
                check = ::quietUpdateCheck,
            )
        }
    }

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
            "updates" -> checkForUpdates()
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
        /** A Wi-Fi <-> mobile hand-over fires several callbacks; re-plan once it settles. */
        const val NETWORK_SETTLE_MS = 2_000L
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

        @androidx.annotation.StringRes
        fun effectLabel(effect: WallpaperEffect): Int = when (effect) {
            WallpaperEffect.NONE -> R.string.effect_none
            WallpaperEffect.DITHER -> R.string.effect_dither
            WallpaperEffect.ASCII -> R.string.effect_ascii
            WallpaperEffect.HALFTONE -> R.string.effect_halftone
            WallpaperEffect.SCANLINES -> R.string.effect_scanlines
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

    companion object {
        val calls = java.util.concurrent.atomic.AtomicLong()
        val nanos = java.util.concurrent.atomic.AtomicLong()
        fun report() {
            val c = calls.get()
            if (c > 0) android.util.Log.i("ZeronTranscript", "measurer: $c calls, ${nanos.get() / 1_000_000}ms total")
        }

        // Every fallback call is a JNA round-trip; repeated runs (labels,
        // timestamps, reused text) and re-layouts hit this instead.
        private data class MKey(val face: FaceRole, val size: Float, val ligatures: Boolean, val text: String, val run: Boolean)
        private val memo = android.util.LruCache<MKey, Any>(4096)
    }

    override fun measure(face: FaceRole, size: Float, ligatures: Boolean, text: String): Float {
        val key = MKey(face, size, ligatures, text, false)
        memo.get(key)?.let { return it as Float }
        val t = System.nanoTime()
        val paint = paint(face, size, ligatures)
        val r = if (!sh.zeron.android.design.FontChain.isMono(face) || !sh.zeron.android.design.FontChain.hasWide(text)) {
            paint.measureText(text)
        } else {
            measureRun(face, size, ligatures, text).sum()
        }
        calls.incrementAndGet(); nanos.addAndGet(System.nanoTime() - t)
        memo.put(key, r)
        return r
    }

    override fun measureRun(face: FaceRole, size: Float, ligatures: Boolean, text: String): List<Float> {
        val key = MKey(face, size, ligatures, text, true)
        @Suppress("UNCHECKED_CAST")
        (memo.get(key) as? List<Float>)?.let { return it }
        val t = System.nanoTime()
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
        calls.incrementAndGet(); nanos.addAndGet(System.nanoTime() - t)
        memo.put(key, out)
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
