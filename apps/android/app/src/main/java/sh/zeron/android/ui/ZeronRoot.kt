package sh.zeron.android.ui

import sh.zeron.android.feedback.AppFeedback
import sh.zeron.android.feedback.tapAction
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.core.tween
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.ChatBubble
import androidx.compose.material.icons.filled.Settings
import androidx.compose.material.icons.outlined.ChatBubbleOutline
import androidx.compose.material.icons.outlined.Settings
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.ShortNavigationBar
import androidx.compose.material3.ShortNavigationBarItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.snapshotFlow
import androidx.compose.runtime.withFrameNanos
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.delay
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.zIndex
import androidx.compose.ui.Alignment
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.unit.dp
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.runtime.remember
import sh.zeron.android.design.ZIcons
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.navigation.NavController
import androidx.navigation.NavHostController
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.rememberNavController
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZeronTheme
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.ProvideFeedback
import android.net.Uri
import androidx.navigation.navArgument
import sh.zeron.android.tools.BrowserScreen
import sh.zeron.android.tools.FileScreen
import sh.zeron.android.tools.FilesScreen
import sh.zeron.android.tools.TerminalScreen
import sh.zeron.android.tools.WorkspaceRef

@Composable
fun ZeronRoot(model: AppModel) {
    val appearance by model.appearance.collectAsState()
    ZeronTheme(appearance) {
        ProvideFeedback(model.feedback) {
            Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
                val client by model.client.collectAsState()
                val onboarded by model.onboarded.collectAsState()
                // First run: sign in, continue without an account, or the demo.
                // Past it, the main UI shows while this phone's engine comes up.
                val gate = if (onboarded || client?.isDemo() == true) Gate.Main else Gate.FirstRun
                AnimatedContent(gate, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "root") {
                    when (it) {
                        Gate.Main -> MainNav(model)
                        Gate.FirstRun -> SignInScreen(model)
                    }
                }
            }
        }
    }
}

private enum class Gate { Main, FirstRun }

private const val NAV_FADE_IN_MS = 180
private const val NAV_FADE_OUT_MS = 120

object Routes {
    const val HOME = "home"
    const val CHAT = "chat/{id}?subagents={subagents}"
    const val SUBAGENT = "subagent/{chat}/{doc}"
    const val NEW = "new"
    const val SEARCH = "search"
    const val ENGINE = "engine"
    const val AGENTS = "agents"
    const val TRANSFERS = "transfers"
    const val SOUNDS = "settings/sounds"
    const val BADGES = "badges/{mode}"
    const val FILES = "files/{ws}"
    const val FILE = "file/{ws}?path={path}"
    const val TERMINAL = "terminal/{ws}"
    const val BROWSER = "browser?ws={ws}&url={url}"
    fun chat(id: String) = "chat/$id"

    /** The chat with its Subagents panel open. */
    fun chatSubagents(id: String) = "chat/$id?subagents=true"

    /** One subagent of [chat], read-only. */
    fun subagent(chat: String, doc: String) = "subagent/${Uri.encode(chat)}/${Uri.encode(doc)}"

    /** Developer tools address a workspace by chat id, or `space:<id>` for a project. */
    fun files(ws: String) = "files/${Uri.encode(ws)}"
    fun file(ws: String, path: String) = "file/${Uri.encode(ws)}?path=${Uri.encode(path)}"
    fun terminal(ws: String) = "terminal/${Uri.encode(ws)}"
    fun browser(ws: String?, url: String?) = "browser?ws=${Uri.encode(ws ?: "")}&url=${Uri.encode(url ?: "")}"
}

private fun AppModel.refFor(ws: String): WorkspaceRef? =
    if (ws.startsWith("space:")) projectRef(ws.removePrefix("space:")) else workspaceRef(ws)

/**
 * Navigation announces itself when it is asked for, not when the transition
 * lands (that can be a second later on a slow device): going deeper opens,
 * coming back closes.
 */
private object NavCues {
    var announced = false
}

private fun NavController.open(route: String) {
    AppFeedback.current.cue(Cue.Open)
    NavCues.announced = true
    navigate(route)
}

private fun NavController.back() {
    AppFeedback.current.cue(Cue.Close)
    NavCues.announced = true
    popBackStack()
}

/** Launch and notification routes: `chat:<id>`, `new`, `search`, `engine`, `agents`, `transfers`, and the developer-tool routes. */
private fun NavController.openRoute(route: String) {
    val nav = this
    when {
        route == "new" -> nav.open(Routes.NEW)
        route == "search" -> nav.open(Routes.SEARCH)
        route == "agents" -> nav.open(Routes.AGENTS)
        route == "engine" -> nav.open(Routes.ENGINE)
        route == "transfers" -> { AppFeedback.current.cue(Cue.Open); NavCues.announced = true; nav.navigate(Routes.TRANSFERS) { launchSingleTop = true } }
        route == "sounds" -> nav.open(Routes.SOUNDS)
        // Debug builds only (the route is not registered otherwise): the badge contact sheets.
        (route == "badges" || route.startsWith("badges:")) && nav.graph.findNode(Routes.BADGES) != null -> nav.open("badges/" + route.substringAfter(':', "grid"))
        route.startsWith("chat:") -> nav.open(Routes.chat(route.removePrefix("chat:")))
        // subagents:<chat> opens its panel; subagent:<chat>|<doc> one subagent.
        route.startsWith("subagents:") -> nav.open(Routes.chatSubagents(route.removePrefix("subagents:")))
        route.startsWith("subagent:") -> route.removePrefix("subagent:").split('|', limit = 2).let {
            if (it.size == 2) nav.open(Routes.subagent(it[0], it[1]))
        }
        // Developer tools: files:<chat> / terminal:<chat> / browser:<chat>|<url> / file:<chat>|<path>
        route.startsWith("files:") -> nav.open(Routes.files(route.removePrefix("files:")))
        route.startsWith("terminal:") -> nav.open(Routes.terminal(route.removePrefix("terminal:")))
        route.startsWith("file:") -> route.removePrefix("file:").split('|', limit = 2).let { nav.open(Routes.file(it[0], it.getOrElse(1) { "" })) }
        route.startsWith("browser:") -> route.removePrefix("browser:").split('|', limit = 2).let { nav.open(Routes.browser(it[0], it.getOrNull(1))) }
    }
}

@Composable
private fun MainNav(model: AppModel) {
    val nav = rememberNavController()
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    // A screen stays composed through NavHost's exit fade, and so
    // does its focused text field — the IME only went away once the field was
    // disposed, a second after leaving the chat. Drop focus and the keyboard
    // the moment the destination changes instead.
    val feedback = LocalFeedback.current
    DisposableEffect(nav, focus, keyboard, feedback) {
        var depth = 0
        val listener = NavController.OnDestinationChangedListener { controller, _, _ ->
            focus.clearFocus(force = true)
            keyboard?.hide()
            // Pages announce themselves when asked to open or close (see open / back). The listener only
            // covers what nobody asked for with a tap: the system back gesture.
            val now = controller.currentBackStack.value.count { it.destination !is androidx.navigation.NavGraph }
            if (NavCues.announced) NavCues.announced = false
            else if (depth > 0 && now < depth) feedback.cue(Cue.Close)
            depth = now
        }
        nav.addOnDestinationChangedListener(listener)
        onDispose { nav.removeOnDestinationChangedListener(listener) }
    }
    val pending by model.pendingRoute.collectAsState()
    LaunchedEffect(pending) {
        pending?.let { nav.openRoute(it) }
        model.pendingRoute.value = null
    }
    // NavHost's default is a 700 ms fade in and out: a page that starts transparent and takes most of a second to
    // arrive does not feel instant, however fast it composed. Short fades keep the cross-dissolve and lose the wait.
    NavHost(
        nav,
        startDestination = Routes.HOME,
        enterTransition = { fadeIn(tween(NAV_FADE_IN_MS)) },
        exitTransition = { fadeOut(tween(NAV_FADE_OUT_MS)) },
        popEnterTransition = { fadeIn(tween(NAV_FADE_IN_MS)) },
        popExitTransition = { fadeOut(tween(NAV_FADE_OUT_MS)) },
    ) {
        composable(Routes.HOME) { Home(model, nav) }
        composable(Routes.CHAT, arguments = listOf(navArgument("subagents") { defaultValue = "false" })) { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            val subagents = entry.arguments?.getString("subagents") == "true"
            SessionScreen(model, id, onBack = { nav.back() }, onNavigate = { nav.open(it) }, showSubagents = subagents)
        }
        composable(Routes.SUBAGENT) { entry ->
            val chat = entry.arguments?.getString("chat") ?: return@composable
            val doc = entry.arguments?.getString("doc") ?: return@composable
            SubagentScreen(model, chat, doc, onBack = { nav.back() }, onNavigate = { nav.open(it) })
        }
        composable(Routes.FILES) { entry ->
            val ws = entry.arguments?.getString("ws") ?: return@composable
            val ref = remember(ws) { model.refFor(ws) } ?: return@composable Unavailable { nav.back() }
            FilesScreen(
                model,
                ref,
                onBack = { nav.back() },
                onOpenFile = { nav.open(Routes.file(ws, it)) },
                onTerminal = { nav.open(Routes.terminal(ws)) },
                onBrowser = { nav.open(Routes.browser(ws, it)) },
            )
        }
        composable(Routes.FILE, arguments = listOf(navArgument("path") { defaultValue = "" })) { entry ->
            val ws = entry.arguments?.getString("ws") ?: return@composable
            val path = entry.arguments?.getString("path").orEmpty()
            val ref = remember(ws) { model.refFor(ws) } ?: return@composable Unavailable { nav.back() }
            FileScreen(model, ref, path, onBack = { nav.back() }, onBrowser = { nav.open(Routes.browser(ws, it)) })
        }
        composable(Routes.TERMINAL) { entry ->
            val ws = entry.arguments?.getString("ws") ?: return@composable
            val ref = remember(ws) { model.refFor(ws) } ?: return@composable Unavailable { nav.back() }
            TerminalScreen(model, ref, onBack = { nav.back() })
        }
        composable(
            Routes.BROWSER,
            arguments = listOf(navArgument("ws") { defaultValue = "" }, navArgument("url") { defaultValue = "" }),
        ) { entry ->
            val ws = entry.arguments?.getString("ws").orEmpty()
            val url = entry.arguments?.getString("url").orEmpty()
            val ref = remember(ws) { ws.ifEmpty { null }?.let { model.refFor(it) } }
            BrowserScreen(model, ref, url.ifEmpty { null }, onBack = { nav.back() }, onOpenFile = { if (ws.isNotEmpty()) nav.open(Routes.file(ws, it)) })
        }
        composable(Routes.NEW) {
            NewSessionScreen(model, onClose = { nav.back() }, onCreated = { id ->
                // Creating a session already sounded (send); the page swap itself stays quiet.
                NavCues.announced = true
                nav.popBackStack()
                nav.navigate(Routes.chat(id))
            })
        }
        composable(Routes.SEARCH) {
            SearchScreen(model, onBack = { nav.back() }, onOpen = { nav.open(Routes.chat(it)) })
        }
        composable(Routes.ENGINE) { EngineScreen(model, onBack = { nav.back() }, onAgents = { nav.open(Routes.AGENTS) }) }
        composable(Routes.AGENTS) { AgentsScreen(model, onBack = { nav.back() }) }
        composable(Routes.TRANSFERS) { TransfersScreen(model, onBack = { nav.back() }) }
        composable(Routes.SOUNDS) { SoundsScreen(model, onBack = { nav.back() }) }
        if (model.isDebuggable) composable(Routes.BADGES) { entry ->
            BadgePreviewScreen(model, entry.arguments?.getString("mode").orEmpty(), onBack = { nav.back() })
        }
    }
}

private enum class Tab { Sessions, Settings }

private fun String?.toTab(): Tab? = when (this) {
    "sessions" -> Tab.Sessions
    "settings" -> Tab.Settings
    else -> null
}

@Composable
private fun Home(model: AppModel, nav: NavHostController) {
    // A cold-open benchmark (`--ez cold true` on the debug tab event) rebuilds the page like coming back from a chat does.
    val epoch by model.homeEpoch.collectAsState()
    androidx.compose.runtime.key(epoch) { HomeTabs(model, nav) }
}

@Composable
private fun HomeTabs(model: AppModel, nav: NavHostController) {
    // `tab` is read only inside layer / effect / child scopes below, never in this body: a switch recomposes the
    // two pages' wrappers and the chrome, not the whole home page.
    val tab = rememberSaveable { mutableStateOf(model.tabRequest.value.toTab() ?: if (model.launch.route == "settings") Tab.Settings else Tab.Sessions) }
    // Debug builds: `--es kind tab --es tab sessions|settings` flips the tab (scripts/android/measure-tab-switch.sh).
    val tabRequest by model.tabRequest.collectAsState()
    LaunchedEffect(tabRequest) {
        tabRequest?.toTab()?.let { tab.value = it }
        model.tabRequest.value = null
    }
    sh.zeron.android.core.PerfFrame(tab)
    // Both tabs stay composed once they have been, so switching is a property change rather than a composition.
    // The one shown first composes at once; the other idles in after the first frames (never in the way of the
    // launch or the return from a chat). A tab asked for before that shows a wireframe for the frame it takes.
    val sessionsReady = remember { mutableStateOf(tab.value == Tab.Sessions) }
    val settingsReady = remember { mutableStateOf(tab.value == Tab.Settings) }
    LaunchedEffect(Unit) {
        snapshotFlow { tab.value }.collectLatest { shown ->
            withFrameNanos { }
            if (shown == Tab.Sessions) sessionsReady.value = true else settingsReady.value = true
            delay(350)
            withFrameNanos { }
            sessionsReady.value = true
            settingsReady.value = true
        }
    }
    val retain by model.retainTabs.collectAsState()
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        TabPage(
            active = { tab.value == Tab.Sessions },
            ready = { if (retain) sessionsReady.value else tab.value == Tab.Sessions },
            skeleton = { SessionsSkeleton() },
        ) { SessionsScreen(model, onOpen = { nav.open(Routes.chat(it)) }) }
        TabPage(
            active = { tab.value == Tab.Settings },
            ready = { if (retain) settingsReady.value else tab.value == Tab.Settings },
            skeleton = { SettingsSkeleton() },
        ) { SettingsScreen(model, onOpen = { nav.open(it) }) }
        // Above the pages: the shown one is lifted over its hidden sibling (see TabPage).
        HomeChrome(model, nav, tab, Modifier.align(Alignment.BottomCenter).zIndex(2f))
    }
}

/** Floating chrome over a soft scrim: new session, then the nav capsule. */
@Composable
private fun HomeChrome(model: AppModel, nav: NavHostController, tab: MutableState<Tab>, modifier: Modifier) {
    Column(
        modifier
            .fillMaxWidth()
            .background(Brush.verticalGradient(0f to Color.Transparent, 0.25f to MaterialTheme.colorScheme.background.copy(alpha = 0.94f), 0.6f to MaterialTheme.colorScheme.background))
            .navigationBarsPadding()
            .padding(top = 28.dp, bottom = 8.dp),
        verticalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        if (tab.value == Tab.Sessions) {
            val workspace by model.workspace.collectAsState()
            val live by model.liveSubagents.collectAsState()
            val summary = remember(workspace, live) { workspace?.let { liveSummary(it, live) } }
            NewSessionBar(summary, onClick = { nav.open(Routes.NEW) })
        }
        FloatingNavBar(
            listOf(
                NavItem("Sessions", ZIcons.TabSessions, tab.value == Tab.Sessions) { sh.zeron.android.core.Perf.begin("tab->sessions"); tab.value = Tab.Sessions },
                NavItem("Settings", ZIcons.TabSettings, tab.value == Tab.Settings) { sh.zeron.android.core.Perf.begin("tab->settings"); tab.value = Tab.Settings },
            ),
            trailing = {
                TonalCircleButton(ZIcons.Search, "Search", onClick = { nav.open(Routes.SEARCH) }, size = 72.dp)
            },
        )
    }
}

@Composable
private fun Unavailable(onBack: () -> Unit) {
    Column(Modifier.fillMaxSize().navigationBarsPadding().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Text("This workspace isn't available.", style = MaterialTheme.typography.titleMedium)
        androidx.compose.material3.TextButton(onClick = tapAction(action = onBack)) { Text("Back") }
    }
}
