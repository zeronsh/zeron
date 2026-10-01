package sh.zeron.android.core

import android.app.Application
import android.net.ConnectivityManager
import android.net.Network
import android.net.NetworkCapabilities
import android.os.Handler
import android.os.Looper
import android.util.Log
import kotlinx.coroutines.CompletableDeferred
import kotlinx.coroutines.Job
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.MutableSharedFlow
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.SharedFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import org.json.JSONObject
import sh.zeron.android.design.Appearance
import sh.zeron.android.design.ThemeMode
import sh.zeron.runtime.CustomServer
import sh.zeron.runtime.RuntimeState
import sh.zeron.android.feedback.AndroidFeedback
import sh.zeron.android.feedback.AppFeedback
import sh.zeron.android.feedback.DeviceFeedbackPolicy
import sh.zeron.android.feedback.EngineStage
import sh.zeron.android.feedback.EngineTransitions
import sh.zeron.android.feedback.FeedbackStore
import sh.zeron.android.feedback.LinkTransitions
import sh.zeron.android.feedback.Phase
import sh.zeron.android.feedback.SessionFeedbackPolicy
import sh.zeron.android.feedback.SessionTransitions
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.AuthCallback
import uniffi.zeron_core.AuthOrg
import uniffi.zeron_core.BusyPolicy
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
import uniffi.zeron_core.EngineAccount
import uniffi.zeron_core.EngineEdge
import uniffi.zeron_core.EngineIdentity
import uniffi.zeron_core.EngineLink
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.SandboxLevel
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.StreamSpeed
import uniffi.zeron_core.TranscriptScale
import uniffi.zeron_core.WorkspaceSnapshot
import uniffi.zeron_core.WorktreeSpec
import uniffi.zeron_core.authProductionEdgeUrl
import uniffi.zeron_core.parseAuthCallback
import java.io.File

/** How the app was launched (adb extras mirror the iOS launch arguments). */
data class LaunchOptions(
    val demo: Boolean = false,
    val fast: Boolean = false,
    val longReply: Boolean = false,
    val big: Boolean = false,
    val huge: Boolean = false,
    val noProjects: Boolean = false,
    /** Back to the first-run screen (the engine's sign-in is kept). */
    val signedOut: Boolean = false,
    /** Skip the first-run screen: continue without an account. */
    val local: Boolean = false,
    /** Developer: join this edge (`zeron local-edge`) with [serverToken]; `none` clears it. */
    val server: String? = null,
    val serverToken: String? = null,
    val route: String? = null,
    val wallpaper: String? = null,
    val wallpaperEffect: String? = null,
)

/**
 * App-wide state owner. The phone is a regular Zeron device: its engine (the
 * `:runtime` guest) owns the account, and the app is that device's viewer —
 * a [CoreClient] with the engine's device id, edge and identity, whose bearer
 * comes from the engine ([Credentials.Engine]; docs/android.md). The engine
 * sets itself up on first launch and restarts with the app; signing in or out
 * goes through it and restarts it into the new workspace.
 *
 * Core events arrive on Rust threads and hop to the main thread here.
 */
class AppModel(private val app: Application) {
    private val scope = MainScope()
    private val main = Handler(Looper.getMainLooper())
    val wallpaper = WallpaperStore(app)
    val phone by lazy { PhoneEngine(app) }
    val notifier by lazy { Notifier(app, { feedback.store.current }, { foreground }) }
    val transfers by lazy { TransferCenter(app, this) }
    val workspaceApi by lazy { sh.zeron.android.tools.WorkspaceApi(this) }
    val downloads by lazy { sh.zeron.android.tools.Downloads(app, this) }

    private val settings = app.getSharedPreferences("settings", 0)

    /** Past the first-run screen (signed in, or continuing without an account). */
    private val _onboarded = MutableStateFlow(settings.getBoolean(ONBOARDED, false))
    val onboarded: StateFlow<Boolean> = _onboarded.asStateFlow()

    /** A route to open once the main UI is up (`chat:<id>` from a notification). */
    val pendingRoute = MutableStateFlow<String?>(null)

    /** Debug builds: a bottom-nav tab to switch to ("sessions" | "settings"), see the DEBUG_EVENT `tab` kind. */
    val tabRequest = MutableStateFlow<String?>(null)

    /** Debug builds: bumped to rebuild the home page from scratch, the way coming back from a chat does. */
    val homeEpoch = MutableStateFlow(0)

    /** Debug builds: false rebuilds a tab on every switch (the old behaviour), for A/B timing in the same session. */
    val retainTabs = MutableStateFlow(true)

    private val _client = MutableStateFlow<CoreClient?>(null)
    val client: StateFlow<CoreClient?> = _client.asStateFlow()

    private val _workspace = MutableStateFlow<WorkspaceSnapshot?>(null)
    val workspace: StateFlow<WorkspaceSnapshot?> = _workspace.asStateFlow()

    private val _connectivity = MutableStateFlow<Connectivity?>(null)
    val connectivity: StateFlow<Connectivity?> = _connectivity.asStateFlow()

    /**
     * Running-subagent counts this app knows from the chats it has open, merged
     * into the session rows' published counts ([SessionActivity.merged]).
     */
    private val liveSubagentCounts = LiveSubagents()
    private val _liveSubagents = MutableStateFlow<Map<String, Int>>(emptyMap())
    val liveSubagents: StateFlow<Map<String, Int>> = _liveSubagents.asStateFlow()
    private var liveSubagentsTick: Job? = null

    /** The open chat [chatId] shows [count] running subagents. */
    fun reportLiveSubagents(chatId: String, count: Int) {
        if (liveSubagentCounts.report(chatId, count, System.currentTimeMillis())) _liveSubagents.value = liveSubagentCounts.snapshot()
        scheduleLiveSubagentsExpiry()
    }

    /** [chatId] was closed: its count stays for a short grace, then yields to the engine's row. */
    fun releaseLiveSubagents(chatId: String) {
        liveSubagentCounts.release(chatId, System.currentTimeMillis())
        scheduleLiveSubagentsExpiry()
    }

    private fun scheduleLiveSubagentsExpiry() {
        val due = liveSubagentCounts.nextExpiry() ?: return
        liveSubagentsTick?.cancel()
        liveSubagentsTick = scope.launch {
            delay((due - System.currentTimeMillis()).coerceAtLeast(0) + 50)
            if (liveSubagentCounts.expire(System.currentTimeMillis())) _liveSubagents.value = liveSubagentCounts.snapshot()
            scheduleLiveSubagentsExpiry()
        }
    }

    /** Chat ids whose session or composer changed. */
    private val _sessionEvents = MutableSharedFlow<String>(extraBufferCapacity = 256)
    val sessionEvents: SharedFlow<String> = _sessionEvents

    /**
     * This phone's device id — its engine's (`Identity`), which the app's
     * client shares. Null until the running engine has answered.
     */
    private val _engineDeviceId = MutableStateFlow<String?>(null)
    val engineDeviceId: StateFlow<String?> = _engineDeviceId.asStateFlow()

    /** Who this device is signed in as, per its engine. */
    private val _account = MutableStateFlow<Account>(Account.Unknown)
    val account: StateFlow<Account> = _account.asStateFlow()

    private val _signIn = MutableStateFlow<SignIn>(SignIn.Idle)
    val signIn: StateFlow<SignIn> = _signIn.asStateFlow()

    /** Organizations to pick from mid sign-in (more than one). */
    val orgChoice = MutableStateFlow<Pair<List<AuthOrg>, CompletableDeferred<AuthOrg?>>?>(null)

    /** Haptics and sound for the whole app; Compose reaches it through `LocalFeedback`, everything else through [AppFeedback]. */
    val feedback: AndroidFeedback by lazy { AndroidFeedback(app, FeedbackStore(settings)).also { AppFeedback.current = it } }
    private val sessionTransitions = SessionTransitions()
    private val linkTransitions = LinkTransitions()
    private val sessionFeedback by lazy { SessionFeedbackPolicy(feedback, notifier, { foreground }, android.os.SystemClock::uptimeMillis) }

    /**
     * Device events that are not a response to a tap: file transfers (an ask, one received, sent or failed: in-app
     * cue in front, the notification's channel sound behind) and the phone engine's setup and failures.
     */
    val deviceFeedback by lazy { DeviceFeedbackPolicy(feedback, { foreground }, android.os.SystemClock::uptimeMillis) }
    private val engineTransitions = EngineTransitions()

    /** The notification permission has been asked for once (it is asked in context, not at launch). */
    var notificationsAsked: Boolean
        get() = settings.getBoolean("asked.notifications", false)
        set(value) = settings.edit().putBoolean("asked.notifications", value).apply()

    /** The user stopped [chatId]: the idle that follows is not a completion. */
    fun noteInterrupted(chatId: String) = sessionFeedback.interrupted(chatId)

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

    /** Settings → About: seven taps on the version reveal the developer options. */
    private val _developer = MutableStateFlow(settings.getBoolean("developer", false))
    val developer: StateFlow<Boolean> = _developer.asStateFlow()

    fun setDeveloper(on: Boolean) {
        _developer.value = on
        settings.edit().putBoolean("developer", on).apply()
    }

    private fun storedDraft() = NewSessionDraft(
        projectId = settings.getString("draft.project", null),
        hostId = settings.getString("draft.host", null),
        harness = settings.getString("draft.harness", null) ?: "claude-code",
        model = settings.getString("draft.model", null),
        effort = settings.getString("draft.effort", null),
    )

    var lastDraft: NewSessionDraft = storedDraft()
        set(value) {
            field = value
            if (!isDemo) {
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
    private var foreground = false
    private var booted = false

    /** The engine the current client speaks for, and who it is (a changed key → a new client). */
    private var engine: EngineLink? = null
    private var engineKey: String? = null
    private var connecting: Job? = null

    private val listener = object : ClientListener {
        override fun onEvent(event: ClientEvent) {
            main.post { handle(event) }
        }
    }

    /** Entry points other than MainActivity (the share sheet): boot once with defaults. */
    fun ensureBooted() {
        if (!booted) boot(LaunchOptions())
    }

    fun boot(options: LaunchOptions) {
        launch = options
        if (booted) return
        booted = true
        if (isDebuggable) registerDebugAlerts()
        pendingRoute.value = options.route
        if (options.signedOut) setOnboarded(false)
        if (options.local) setOnboarded(true)
        options.server?.let { url ->
            val next = if (url == "none") null else CustomServer.of(url, options.serverToken.orEmpty())
            if (next != phone.customServer) {
                phone.customServer = next
                phone.restart()
            }
        }
        // Foreground means any of the app's activities (the share sheet included), not only MainActivity.
        androidx.lifecycle.ProcessLifecycleOwner.get().lifecycle.addObserver(object : androidx.lifecycle.DefaultLifecycleObserver {
            override fun onStart(owner: androidx.lifecycle.LifecycleOwner) = onForeground()
            override fun onStop(owner: androidx.lifecycle.LifecycleOwner) = onBackground()
        })
        watchNetwork()
        // Incoming-transfer notifications and Downloads export, whenever this
        // phone's engine runs (docs/android.md § File transfers).
        transfers.start()
        // `wallpaper <path>` / `wallpaper none` and `wallpaper-effect <name>`:
        // set the wallpaper at launch (screenshots, tests).
        options.wallpaper?.let { path ->
            if (path == "none") wallpaper.remove() else scope.launch { wallpaper.set(File(path)) }
        }
        options.wallpaperEffect?.let { name ->
            WallpaperStore.effects.firstOrNull { it.name.equals(name, ignoreCase = true) }?.let(wallpaper::setEffect)
        }
        // The engine is part of the app: set up on first launch (in the
        // background, while the first-run screen is up), started on every
        // launch unless the user stopped it, restarted by the runtime if it dies.
        if (phone.isSupportedAbi && settings.getBoolean(AUTOSTART, true) && phone.state.value.isIdle()) phone.start()
        scope.launch { phone.state.collect(::onEngine) }
        if (options.demo) startDemo()
        // Relative times ("4m") and staleness age without events.
        scope.launch {
            while (true) {
                delay(30_000)
                refreshWorkspace(announce = false)
            }
        }
    }

    /**
     * Debuggable builds: `adb shell am broadcast -a sh.zeron.android.DEBUG_EVENT -p sh.zeron.android --es kind
     * done|input|failed [--es chat <id>]` (or `haptic` / `cue` with `--es name <entry>`, or engine-setup|engine-failed|transfer-asked|received|sent|failed) runs an event through the real policy (in-app cue in front,
     * notification behind), to check the sensory layer without waiting for an agent.
     */
    val isDebuggable: Boolean get() = (app.applicationInfo.flags and android.content.pm.ApplicationInfo.FLAG_DEBUGGABLE) != 0

    private fun registerDebugAlerts() {
        val receiver = object : android.content.BroadcastReceiver() {
            override fun onReceive(context: android.content.Context, intent: android.content.Intent) {
                // `--es kind haptic --es name Surge [--ef level 0.5]` / `--es kind cue --es name ProviderClaude`
                // play one vocabulary entry straight through the engine (logs, dumpsys vibrator_manager).
                when (intent.getStringExtra("kind")) {
                    "haptic" -> sh.zeron.android.feedback.Haptic.entries.firstOrNull { it.name == intent.getStringExtra("name") }?.let {
                        feedback.haptic(it, intent.getFloatExtra("level", 0.5f))
                    }
                    "cue" -> sh.zeron.android.feedback.Cue.entries.firstOrNull { it.name == intent.getStringExtra("name") }?.let { feedback.cue(it) }
                }
                if (intent.getStringExtra("kind") in setOf("haptic", "cue")) return
                // Device events: engine-setup | engine-failed | transfer-asked | -received | -sent | -failed.
                when (val kind = intent.getStringExtra("kind").orEmpty()) {
                    // `--es kind profile --ei ms 3000`: sample the main thread for a while (what an idle screen costs).
                    "profile" -> {
                        Perf.sampling = true
                        Perf.begin("window")
                        main.postDelayed({ Perf.finish("window") }, intent.getIntExtra("ms", 3000).toLong())
                        return
                    }
                    // `--es kind route --es route engine|agents|transfers|sounds|search|new|chat:<id>`: time a screen opening.
                    "route" -> {
                        val route = intent.getStringExtra("route").orEmpty()
                        Perf.sampling = intent.getBooleanExtra("sample", false)
                        Perf.tailMs = intent.getIntExtra("tail", 250).toLong()
                        Perf.begin("route->$route")
                        Perf.finishAfterFrames(12)
                        pendingRoute.value = route
                        return
                    }
                    "tab" -> {
                        val tab = intent.getStringExtra("tab").orEmpty()
                        Perf.sampling = intent.getBooleanExtra("sample", false)
                        Perf.tailMs = intent.getIntExtra("tail", 250).toLong()
                        if (intent.hasExtra("retain")) retainTabs.value = intent.getBooleanExtra("retain", true)
                        Perf.begin("tab->$tab" + (if (retainTabs.value) "" else " [rebuild]") + (if (intent.getBooleanExtra("cold", false)) " [cold]" else "") + (if (intent.getBooleanExtra("warm", false)) " [warm-up]" else ""))
                        tabRequest.value = tab
                        if (intent.getBooleanExtra("cold", false)) homeEpoch.value++
                        return
                    }
                    "engine-setup" -> deviceFeedback.engine(sh.zeron.android.feedback.EngineEvent.SetupDone).also { return }
                    "engine-failed" -> deviceFeedback.engine(sh.zeron.android.feedback.EngineEvent.Failed).also { return }
                    "transfer-asked" -> deviceFeedback.transfer("debug", sh.zeron.android.feedback.TransferEvent.Asked).also { return }
                    "transfer-received" -> deviceFeedback.transfer("debug", sh.zeron.android.feedback.TransferEvent.Received).also { return }
                    "transfer-sent" -> deviceFeedback.transfer("debug", sh.zeron.android.feedback.TransferEvent.Sent).also { return }
                    "transfer-failed" -> deviceFeedback.transfer("debug", sh.zeron.android.feedback.TransferEvent.Failed).also { return }
                    else -> kind
                }
                val event = when (intent.getStringExtra("kind")) {
                    "input" -> sh.zeron.android.feedback.SessionEvent.NeedsInput
                    "failed" -> sh.zeron.android.feedback.SessionEvent.Failed
                    else -> sh.zeron.android.feedback.SessionEvent.Done
                }
                val chat = intent.getStringExtra("chat") ?: _workspace.value?.let { phases(it).keys.firstOrNull() } ?: return
                // `--ez background true` takes the backgrounded branch while the app stays up (emulators crash on task changes).
                if (intent.getBooleanExtra("background", false)) notifier.alert(chat, event) else sessionFeedback.session(chat, event)
            }
        }
        androidx.core.content.ContextCompat.registerReceiver(
            app, receiver, android.content.IntentFilter("sh.zeron.android.DEBUG_EVENT"), androidx.core.content.ContextCompat.RECEIVER_EXPORTED,
        )
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

    private fun setOnboarded(value: Boolean) {
        _onboarded.value = value
        settings.edit().putBoolean(ONBOARDED, value).apply()
    }

    // ── the engine ─────────────────────────────────────────────────────────

    /** Engine state → the device's client: (re)built whenever the engine answers. */
    private fun onEngine(state: RuntimeState) {
        engineTransitions.observe(
            when (state) {
                RuntimeState.NotInstalled -> EngineStage.Idle
                is RuntimeState.Bootstrapping -> EngineStage.Setup
                RuntimeState.Starting -> EngineStage.Starting
                is RuntimeState.Running -> EngineStage.Running
                RuntimeState.Stopped -> EngineStage.Stopped
                is RuntimeState.Failed -> EngineStage.Failed
            },
        )?.let(deviceFeedback::engine)
        if (state is RuntimeState.Running && !isDemo) {
            connecting?.cancel()
            connecting = scope.launch { connect(state) }
        }
    }

    /**
     * Ask the running engine who this device is, where it syncs and as whom,
     * and (re)build the client when any of that changed. A restart with the
     * same answers keeps the client (it redials on its own).
     */
    private suspend fun connect(running: RuntimeState.Running) {
        val link = EngineLink(running.ipcUrl, running.ipcToken)
        var answer: Pair<EngineIdentity, EngineEdge>? = null
        for (attempt in 1..10) {
            answer = runCatching { link.identity() to link.edge() }
                .onFailure { Log.w("Zeron", "engine didn't answer (attempt $attempt)", it) }
                .getOrNull()
            if (answer != null) break
            delay(500L * attempt)
        }
        val (identity, edge) = answer ?: return
        _engineDeviceId.value = identity.deviceId
        if (edge.signedOut) {
            // A synced engine whose session ended stops itself; bring it back local-only.
            phone.restart()
            return
        }
        engine = link
        refreshAccount(link, identity.workspaceScope)
        // The first-run screen is up (or the demo): the client waits for the user.
        if (!_onboarded.value || isDemo) return
        val key = DeviceIdentity.key(identity.deviceId, edge.edgeUrl, edge.userId, edge.orgId)
        if (key == engineKey && _client.value != null) return
        dropClient()
        engineKey = key
        start(
            Credentials.Engine(running.ipcUrl, running.ipcToken, edge.userId, edge.orgId),
            edgeUrl = edge.edgeUrl,
            deviceId = identity.deviceId,
            name = running.deviceName,
            owner = key,
        )
        if (_signIn.value == SignIn.Restarting) _signIn.value = SignIn.Idle
    }

    private suspend fun refreshAccount(link: EngineLink, workspaceScope: String) {
        val server = phone.customServer
        val account = runCatching { link.account() }.getOrNull()
        _account.value = when {
            server != null -> Account.Server(server.edgeUrl)
            account is EngineAccount.SignedIn && workspaceScope == "synced" -> {
                val org = account.orgId?.let { orgName(link, it) }
                Account.SignedIn(account.name, account.email, org)
            }
            // Signed in on the engine, but it still runs local-only until restarted.
            account is EngineAccount.SignedIn || account is EngineAccount.NeedsOrganization -> Account.Local(pendingRestart = true)
            else -> Account.Local(pendingRestart = false)
        }
    }

    private suspend fun orgName(link: EngineLink, orgId: String): String? {
        settings.getString("org.$orgId", null)?.let { return it }
        val name = runCatching { link.listOrgs() }.getOrNull()?.firstOrNull { it.organizationId == orgId }?.name ?: return null
        settings.edit().putString("org.$orgId", name).apply()
        return name
    }

    /** Settings → This phone → Start: relaunches bring it back up too. */
    fun startEngine() {
        settings.edit().putBoolean(AUTOSTART, true).apply()
        phone.start()
    }

    fun stopEngine() {
        settings.edit().putBoolean(AUTOSTART, false).apply()
        phone.stop()
    }

    /**
     * Wipe the guest — projects, agents, their sign-ins and the device's
     * account — and set it up again. Runs in the app's scope: the engine
     * stopping swaps the screen that asked for it away.
     */
    fun resetEngine() {
        dropClient()
        engineKey = null
        _engineDeviceId.value = null
        _account.value = Account.Unknown
        scope.launch {
            phone.reset()
            File(app.filesDir, "core").deleteRecursively()
            if (settings.getBoolean(AUTOSTART, true)) phone.start()
        }
    }

    /** Developer: join a `zeron local-edge` (or back to Zeron with null), then restart. */
    fun setCustomServer(server: CustomServer?) {
        phone.customServer = server
        dropClient()
        engineKey = null
        // A custom server's engine has its own data dir, hence device id.
        _engineDeviceId.value = null
        phone.restart()
    }

    // ── first run and sign-in ──────────────────────────────────────────────

    /** "Continue without an account": this phone's local workspace. */
    fun continueLocally() {
        setOnboarded(true)
        (phone.state.value as? RuntimeState.Running)?.let(::onEngine)
    }

    /**
     * Sign in *through the engine*, like the desktop: it builds the WorkOS
     * URL (redirecting to the app's `zeron://callback`), exchanges the code
     * and keeps the session. `open` shows the URL (a Custom Tab). Waits for
     * the engine if it is still setting up.
     */
    fun signIn(open: (String) -> Unit) {
        if (_signIn.value is SignIn.Busy) return
        scope.launch {
            try {
                _signIn.value = SignIn.Preparing
                if (phone.state.value.isIdle()) startEngine()
                val running = phone.state.first { it is RuntimeState.Running || it is RuntimeState.Failed }
                if (running !is RuntimeState.Running) {
                    _signIn.value = SignIn.Failed("This phone's engine didn't start. See Settings → This phone.")
                    return@launch
                }
                val link = EngineLink(running.ipcUrl, running.ipcToken)
                val url = link.signInUrl(CALLBACK)
                if (url.isEmpty()) {
                    _signIn.value = SignIn.Failed("This phone uses a custom server, which has no Zeron accounts.")
                    return@launch
                }
                engine = link
                _signIn.value = SignIn.Browser
                open(url)
            } catch (e: Exception) {
                _signIn.value = SignIn.Failed(e.userMessage())
            }
        }
    }

    /** `zeron://callback?code=…&state=…` → the engine; then the org; then restart it synced. */
    fun handleCallback(url: String) {
        when (val cb = parseAuthCallback(url)) {
            is AuthCallback.Code -> scope.launch { completeSignIn(cb.state.orEmpty(), cb.code) }
            is AuthCallback.Error -> _signIn.value = SignIn.Failed(cb.description ?: cb.error)
            null -> Unit
        }
    }

    private suspend fun completeSignIn(state: String, code: String) {
        val link = engine ?: (phone.state.value as? RuntimeState.Running)?.let { EngineLink(it.ipcUrl, it.ipcToken) }
        if (link == null) {
            _signIn.value = SignIn.Failed("This phone's engine isn't running.")
            return
        }
        try {
            _signIn.value = SignIn.Completing
            link.completeSignIn(state, code)
            if (link.account() is EngineAccount.NeedsOrganization) {
                val orgs = link.listOrgs()
                val org = when {
                    orgs.isEmpty() -> {
                        _signIn.value = SignIn.Failed("This account isn't in an organization yet. Create one in Zeron on your computer.")
                        link.signOut()
                        return
                    }
                    orgs.size == 1 -> orgs[0]
                    else -> {
                        val choice = CompletableDeferred<AuthOrg?>()
                        orgChoice.value = orgs to choice
                        choice.await().also { orgChoice.value = null }
                    }
                }
                if (org == null) {
                    link.signOut()
                    _signIn.value = SignIn.Idle
                    return
                }
                link.selectOrg(org.organizationId)
                settings.edit().putString("org.${org.organizationId}", org.name).apply()
            }
            if (link.account() !is EngineAccount.SignedIn) {
                _signIn.value = SignIn.Failed("Sign-in didn't complete. Try again.")
                return
            }
            // The engine's workspace is fixed per run: restart it into the account.
            setOnboarded(true)
            _signIn.value = SignIn.Restarting
            dropClient()
            engineKey = null
            phone.restart()
        } catch (e: Exception) {
            _signIn.value = SignIn.Failed(e.userMessage())
        }
    }

    fun dismissSignInError() {
        if (_signIn.value is SignIn.Failed) _signIn.value = SignIn.Idle
    }

    /** Sign the device out: its engine forgets the account and restarts local-only. */
    fun signOut() {
        val link = engine ?: return
        scope.launch {
            _signIn.value = SignIn.Restarting
            runCatching { link.signOut() }
            dropClient()
            engineKey = null
            _account.value = Account.Local(pendingRestart = false)
            phone.restart()
        }
    }

    // ── the demo ───────────────────────────────────────────────────────────

    /** The offline demo (first-run screen only); never remembered. */
    fun startDemo() {
        dropClient()
        engineKey = null
        start(Credentials.Demo(demoOptions()), edgeUrl = authProductionEdgeUrl(), deviceId = "android-demo", name = "Pixel", owner = null)
    }

    fun leaveDemo() {
        dropClient()
        (phone.state.value as? RuntimeState.Running)?.let(::onEngine)
    }

    // ── the client ─────────────────────────────────────────────────────────

    private fun start(credentials: Credentials, edgeUrl: String, deviceId: String, name: String, owner: String?): Boolean {
        val dir = when (credentials) {
            is Credentials.Demo -> File(app.filesDir, "demo")
            else -> claimCoreDir(owner.orEmpty())
        }
        dir.mkdirs()
        val config = CoreConfig(
            edgeUrl = edgeUrl,
            dataDir = dir.path,
            deviceId = deviceId,
            deviceName = name,
            platform = "android",
            appVersion = app.packageManager.getPackageInfo(app.packageName, 0).versionName ?: "0",
        )
        return try {
            val client = CoreClient(config, credentials, listener)
            _client.value = client
            lastDraft = storedDraft()
            refreshWorkspace()
            client.preloadSessions()
            true
        } catch (e: Exception) {
            Log.e("Zeron", "core start failed", e)
            false
        }
    }

    /** The client's cache belongs to one device identity: another starts empty. */
    private fun claimCoreDir(owner: String): File {
        val dir = File(app.filesDir, "core")
        val marker = File(dir, ".owner")
        if (runCatching { marker.readText() }.getOrNull() != owner) dir.deleteRecursively()
        dir.mkdirs()
        marker.writeText(owner)
        return dir
    }

    private fun dropClient() {
        _client.value?.shutdown()
        _client.value = null
        _workspace.value = null
        _connectivity.value = null
        sessionTransitions.reset()
        linkTransitions.reset()
    }

    val accountName: String
        get() = if (isDemo) "Demo" else _account.value.title

    val accountDetail: String
        get() = if (isDemo) "Offline demo workspace" else _account.value.detail

    // ── events ─────────────────────────────────────────────────────────────

    private fun handle(event: ClientEvent) {
        when (event) {
            is ClientEvent.WorkspaceChanged -> scheduleRefresh()
            is ClientEvent.SessionChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ComposerChanged -> _sessionEvents.tryEmit(event.chatId)
            is ClientEvent.ConnectivityChanged -> {
                _connectivity.value = event.connectivity
                observeLink(event.connectivity)
            }
            // The engine refreshes; the app never holds tokens.
            is ClientEvent.AuthRefreshed -> Unit
            // The engine signed out (or now serves another account): it
            // restarts local-only; the next Running builds the new client.
            is ClientEvent.AuthExpired -> {
                dropClient()
                engineKey = null
                phone.restart()
            }
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

    /** [announce] false for clock-driven refreshes: a status that merely aged out (staleness gate) is not an event. */
    fun refreshWorkspace(announce: Boolean = true) {
        val client = _client.value ?: return
        val snapshot = client.workspace()
        _workspace.value = snapshot
        observeSessions(snapshot, announce)
    }

    private fun phases(ws: WorkspaceSnapshot): Map<String, Phase> {
        val out = HashMap<String, Phase>()
        for (row in ws.projects.flatMap { it.sessions } + ws.projectless) {
            if (row.parentChatId != null) continue // subagents do not chime
            out[row.id] = when (row.hostIndicator) {
                ChatIndicator.WORKING -> Phase.Working
                ChatIndicator.AWAITING_INPUT -> Phase.AwaitingInput
                ChatIndicator.ERRORED -> Phase.Errored
                ChatIndicator.COMPLETED -> Phase.Completed
                ChatIndicator.IDLE -> Phase.Idle
            }
        }
        return out
    }

    /** Turn changes of session state into one-shot events: in-app cue in front, notification behind. */
    private fun observeSessions(ws: WorkspaceSnapshot, announce: Boolean) {
        val events = sessionTransitions.observe(phases(ws))
        if (announce) for ((id, event) in events) sessionFeedback.session(id, event)
    }

    private fun observeLink(c: Connectivity) {
        val degraded = c.state == ConnectivityState.OFFLINE || c.state == ConnectivityState.RECONNECTING
        val running = _workspace.value?.let { phases(it).values.any { p -> p == Phase.Working } } == true
        linkTransitions.observe(degraded, running)?.let(sessionFeedback::link)
    }

    fun onForeground() {
        foreground = true
        feedback.setForeground(true)
        // What happened while away was already announced by a notification: start from what is on screen.
        sessionTransitions.reset()
        notifier.clearSessionAlerts()
        _client.value?.onForeground()
        refreshWorkspace()
    }

    fun onBackground() {
        foreground = false
        feedback.setForeground(false)
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

    /** Devices that run agents — this phone among them, like any computer. */
    fun executionDevices(): List<DeviceView> = runCatching { _client.value?.executionDevices() }.getOrNull().orEmpty()

    // ── host calls ─────────────────────────────────────────────────────────

    /** Untyped engine RPC (harness installs, agent sign-ins). Throws on failure. */
    suspend fun hostCall(deviceId: String, method: String, params: JSONObject = JSONObject()): Any {
        val c = _client.value ?: throw IllegalStateException("Not connected")
        return Agents.parse(c.hostCall(deviceId, method, params.toString()))
    }

    /**
     * Clone a repository, or create an empty one, on a device and make it a
     * project. The device's engine does it (`CloneRepo` / `CreateRepo`) —
     * on this phone into /home/zeron/projects, like any other device.
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
        val config = ChatConfig(draft.harness, draft.model, draft.effort, draft.options, SandboxLevel.WORKSPACE_WRITE)
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

    companion object {
        /** The app's WorkOS redirect (registered for the engine's client id). */
        const val CALLBACK = "zeron://callback"
        private const val ONBOARDED = "onboarded"
        private const val AUTOSTART = "engineAutostart"
    }
}

/** The engine isn't running and nothing is on its way to start it. */
fun RuntimeState.isIdle(): Boolean =
    this == RuntimeState.NotInstalled || this == RuntimeState.Stopped || this is RuntimeState.Failed

/** Who the device is signed in as (its engine's account). */
sealed interface Account {
    val title: String
    val detail: String

    data object Unknown : Account {
        override val title = "This phone"
        override val detail = "Connecting to this phone's engine…"
    }

    /** No account: a local workspace, on this phone only. */
    data class Local(val pendingRestart: Boolean) : Account {
        override val title = "Not signed in"
        override val detail = "Local workspace on this phone"
    }

    data class SignedIn(val name: String?, val email: String, val orgName: String?) : Account {
        override val title = name?.takeIf { it.isNotBlank() } ?: email
        override val detail = listOfNotNull(if (name.isNullOrBlank()) null else email, orgName).joinToString(" · ").ifEmpty { "Zeron account" }
    }

    /** Developer: a `zeron local-edge` instead of Zeron's servers. */
    data class Server(val edgeUrl: String) : Account {
        override val title = "Custom server"
        override val detail = edgeUrl
    }
}

/** Sign-in through the engine, as the first-run screen and Settings show it. */
sealed interface SignIn {
    /** Something is in flight; the buttons wait. */
    sealed interface Busy : SignIn

    data object Idle : SignIn
    /** Waiting for this phone's engine (first launch sets it up). */
    data object Preparing : Busy
    data object Browser : SignIn
    data object Completing : Busy
    /** Signed in; the engine restarts into the account's workspace. */
    data object Restarting : Busy
    data class Failed(val message: String) : SignIn
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
    /** Model option id → choice id picked for [model] (fast mode, context window…); empty = the defaults. */
    val options: Map<String, String> = emptyMap(),
    val branch: String? = null,
    val worktree: Boolean = false,
    /** Run in this existing worktree of [branch] instead of the project folder. */
    val cwd: String? = null,
)
