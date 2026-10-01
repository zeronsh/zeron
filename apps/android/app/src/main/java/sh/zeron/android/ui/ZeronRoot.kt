package sh.zeron.android.ui

import sh.zeron.android.feedback.AppFeedback
import sh.zeron.android.feedback.tapAction
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
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
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
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
                AnimatedContent(client != null, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "root") { signedIn ->
                    if (signedIn) MainNav(model) else SignInScreen(model)
                }
            }
        }
    }
}

object Routes {
    const val HOME = "home"
    const val CHAT = "chat/{id}?subagents={subagents}"
    const val SUBAGENT = "subagent/{chat}/{doc}"
    const val NEW = "new"
    const val SEARCH = "search"
    const val AGENTS = "agents"
    const val SOUNDS = "settings/sounds"
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

/** Launch and notification routes: `chat:<id>`, `new`, `search`, `agents`, and the developer-tool routes. */
private fun NavController.openRoute(route: String) {
    val nav = this
    when {
        route == "new" -> nav.open(Routes.NEW)
        route == "search" -> nav.open(Routes.SEARCH)
        route == "agents" -> nav.open(Routes.AGENTS)
        route == "sounds" -> nav.open(Routes.SOUNDS)
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
    // A screen stays composed through NavHost's exit fade (700 ms), and so
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
    LaunchedEffect(Unit) {
        model.launch.route?.let { nav.openRoute(it) }
        // A notification tapped while the app is running.
        model.routeRequests.collect { nav.openRoute(it) }
    }
    NavHost(nav, startDestination = Routes.HOME) {
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
        composable(Routes.AGENTS) { AgentsScreen(model, onBack = { nav.back() }) }
        composable(Routes.SOUNDS) { SoundsScreen(model, onBack = { nav.back() }) }
    }
}

private enum class Tab { Sessions, Settings }

@Composable
private fun Home(model: AppModel, nav: NavHostController) {
    var tab by rememberSaveable { mutableStateOf(if (model.launch.route == "settings") Tab.Settings else Tab.Sessions) }
    val workspace by model.workspace.collectAsState()
    val summary = remember(workspace) { workspace?.let { liveSummary(it) } }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        when (tab) {
            Tab.Sessions -> SessionsScreen(model, onOpen = { nav.open(Routes.chat(it)) })
            Tab.Settings -> SettingsScreen(model, onOpen = { nav.open(it) })
        }
        // Floating chrome over a soft scrim: new session, then the nav capsule.
        Column(
            Modifier
                .align(Alignment.BottomCenter)
                .fillMaxWidth()
                .background(Brush.verticalGradient(0f to Color.Transparent, 0.25f to MaterialTheme.colorScheme.background.copy(alpha = 0.94f), 0.6f to MaterialTheme.colorScheme.background))
                .navigationBarsPadding()
                .padding(top = 28.dp, bottom = 8.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            if (tab == Tab.Sessions) NewSessionBar(summary, onClick = { nav.open(Routes.NEW) })
            FloatingNavBar(
                listOf(
                    NavItem("Sessions", ZIcons.TabSessions, tab == Tab.Sessions) { tab = Tab.Sessions },
                    NavItem("Settings", ZIcons.TabSettings, tab == Tab.Settings) { tab = Tab.Settings },
                ),
                trailing = {
                    TonalCircleButton(ZIcons.Search, "Search", onClick = { nav.open(Routes.SEARCH) }, size = 72.dp)
                },
            )
        }
    }
}

@Composable
private fun Unavailable(onBack: () -> Unit) {
    Column(Modifier.fillMaxSize().navigationBarsPadding().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        Text("This workspace isn't available.", style = MaterialTheme.typography.titleMedium)
        androidx.compose.material3.TextButton(onClick = tapAction(action = onBack)) { Text("Back") }
    }
}
