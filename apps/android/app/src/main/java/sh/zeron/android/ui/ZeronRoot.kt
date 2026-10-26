package sh.zeron.android.ui

import android.net.Uri
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ExtendedFloatingActionButton
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.PlainTooltip
import androidx.compose.material3.SnackbarDuration
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TooltipAnchorPosition
import androidx.compose.material3.TooltipBox
import androidx.compose.material3.TooltipDefaults
import androidx.compose.material3.rememberTooltipState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.key.Key
import androidx.compose.ui.input.key.KeyEventType
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isMetaPressed
import androidx.compose.ui.input.key.isShiftPressed
import androidx.compose.ui.input.key.isAltPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import androidx.compose.ui.input.nestedscroll.nestedScroll
import androidx.compose.ui.unit.dp
import androidx.navigation.NavHostController
import androidx.navigation.NavType
import androidx.navigation.compose.NavHost
import androidx.navigation.compose.composable
import androidx.navigation.compose.currentBackStackEntryAsState
import androidx.navigation.compose.rememberNavController
import androidx.navigation.navArgument
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.design.ZeronTheme

@Composable
fun ZeronRoot(model: AppModel) {
    val appearance by model.appearance.collectAsState()
    ZeronTheme(appearance) {
        Surface(Modifier.fillMaxSize(), color = MaterialTheme.colorScheme.background) {
            val client by model.client.collectAsState()
            Box(Modifier.fillMaxSize()) {
                AnimatedContent(client != null, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "root") { signedIn ->
                    if (signedIn) MainNav(model) else SignInScreen(model)
                }
                // Mic prompt, keep-awake and the off-stage "call ended" snackbar.
                sh.zeron.android.voice.VoiceHost(model.voice)
            }
        }
    }
}

object Routes {
    const val HOME = "home"
    const val CHAT = "chat/{id}"
    const val NEW = "new"
    const val SEARCH = "search"
    const val SETTINGS = "settings"
    const val ARCHIVED = "archived"
    const val VOICE = "voice"
    const val VOICE_SETTINGS = "voice-settings"
    const val PROVIDERS = "providers?target={target}"
    const val CLOUD = "cloud"
    const val BILLING = "billing"
    const val CLOUD_REPOS = "cloud-repos"
    const val ACCOUNTS = "accounts/{target}?signin={signin}"
    const val WELCOME = "welcome?step={step}"
    const val TERMINAL = "terminal/{chat}?id={id}"
    const val SUBAGENT = "subagent/{parent}/{doc}?title={title}&running={running}"
    fun chat(id: String) = "chat/$id"
    fun providers(target: String?) = "providers" + (target?.let { "?target=${Uri.encode(it)}" } ?: "")
    fun accounts(target: String, signIn: Boolean) = "accounts/${Uri.encode(target)}?signin=$signIn"
    fun welcome(step: String?) = "welcome" + (step?.let { "?step=${Uri.encode(it)}" } ?: "")
    fun terminal(chat: String, id: String?) = "terminal/${Uri.encode(chat)}" + (id?.let { "?id=${Uri.encode(it)}" } ?: "")
    fun subagent(parent: String, doc: String, title: String, running: Boolean) =
        "subagent/${Uri.encode(parent)}/${Uri.encode(doc)}?title=${Uri.encode(title)}&running=$running"
}

@Composable
private fun MainNav(model: AppModel) {
    val nav = rememberNavController()
    val context = androidx.compose.ui.platform.LocalContext.current
    // Large screens (tablets, unfolded foldables): sessions beside the open chat.
    androidx.compose.foundation.layout.BoxWithConstraints(Modifier.fillMaxSize()) {
    // (Width alone would split a phone turned sideways.)
    // From about an iPad mini in portrait up (iPad shows its split there too).
    val wide = maxWidth >= 700.dp && maxHeight >= 480.dp
    val roomy = maxWidth >= 1100.dp
    val sidebarWidth = if (maxWidth < 1000.dp) 320.dp else 380.dp
    val shell = remember { TabletShellState() }
    val open: (String) -> Unit = { id -> if (wide) shell.select(id) else nav.navigate(Routes.chat(id)) }
    // Folding back to a phone keeps the open chat, full screen; unfolding brings it beside the list.
    LaunchedEffect(wide) {
        if (!wide) {
            val id = shell.selected ?: return@LaunchedEffect
            shell.select(null)
            nav.navigate(Routes.chat(id))
        } else {
            val entry = nav.currentBackStackEntry ?: return@LaunchedEffect
            if (entry.destination.route == Routes.CHAT) {
                val id = entry.arguments?.getString("id")
                nav.popBackStack(Routes.HOME, inclusive = false)
                shell.select(id)
            }
        }
    }
    LaunchedEffect(Unit) {
        val route = model.launch.route ?: return@LaunchedEffect
        val parts = route.split(':')
        when (parts[0]) {
            "new" -> if (wide) shell.newSession() else nav.navigate(Routes.NEW)
            "search" -> if (wide) shell.searching = true else nav.navigate(Routes.SEARCH)
            "settings", "more" -> if (wide) shell.settings = Routes.SETTINGS else nav.navigate(Routes.SETTINGS)
            "archived" -> nav.navigate(Routes.ARCHIVED)
            "chat" -> open(route.removePrefix("chat:"))
            "welcome" -> nav.navigate(Routes.welcome(parts.getOrNull(1)))
            // Cloud pages straight away (screenshots, tests): `cloud` · `cloud:billing` ·
            // `cloud:repos` · `billing` · `providers[:cloud|:<device id>]` · `accounts:<target>`.
            "cloud", "billing", "providers", "accounts" -> {
                val target = when {
                    parts[0] == "cloud" && parts.getOrNull(1) == "billing" -> Routes.BILLING
                    parts[0] == "cloud" && parts.getOrNull(1) == "repos" -> Routes.CLOUD_REPOS
                    parts[0] == "cloud" -> Routes.CLOUD
                    parts[0] == "billing" -> Routes.BILLING
                    parts[0] == "providers" -> Routes.providers(parts.getOrNull(1))
                    else -> Routes.accounts(route.removePrefix("accounts:"), signIn = false)
                }
                if (wide) shell.settings = target else nav.navigate(target)
            }
        }
    }
    // First run after signing in: the welcome pages (once per account).
    val client by model.client.collectAsState()
    LaunchedEffect(client) {
        if (client != null && model.launch.route == null && shouldShowOnboarding(context, model)) nav.navigate(Routes.welcome(null))
    }
    // The notification, a snackbar Retry or `voice-stage` asks for the call.
    LaunchedEffect(Unit) {
        model.voice.stageRequests.collect {
            if (nav.currentDestination?.route != Routes.VOICE) nav.navigate(Routes.VOICE) { launchSingleTop = true }
        }
    }
    // A debug `companion` extra opens over the launch route's chat, once.
    var companionPending by rememberSaveable { mutableStateOf(model.launch.companion) }

    /** A session page with its companions wired to the nav stack (phone) or the inspector (large screens). */
    @Composable
    fun Session(id: String, onBack: () -> Unit, beside: Boolean = false) {
        val launchChat = model.launch.route?.removePrefix("chat:")
        SessionScreen(
            model,
            id,
            onBack = onBack,
            onOpenVoice = { nav.navigate(Routes.VOICE) { launchSingleTop = true } },
            onVoiceSettings = { nav.navigate(Routes.VOICE_SETTINGS) },
            onOpenSideChat = { side -> if (beside) shell.push(id, InspectorPage.SideChat(side)) else nav.navigate(Routes.chat(side)) },
            // On the phone the sender's chat goes over this one, so Back returns
            // here; beside the sidebar it takes the main column (as on iPad).
            onOpenChat = { chat -> if (beside) shell.select(chat) else nav.navigate(Routes.chat(chat)) },
            onOpenTerminal = { chat, terminal -> if (beside) shell.push(chat, InspectorPage.Terminal(terminal)) else nav.navigate(Routes.terminal(chat, terminal)) },
            onOpenSubagent = { parent, doc, title, running ->
                if (beside) shell.push(id, InspectorPage.Subagent(parent, doc, title, running)) else nav.navigate(Routes.subagent(parent, doc, title, running))
            },
            openCompanion = companionPending?.takeIf { id == launchChat }?.also { companionPending = null },
            onOpenAccounts = { target, signIn -> nav.navigate(Routes.accounts(target.encode(), signIn)) },
            onOpenBilling = { if (wide) shell.settings = Routes.BILLING else nav.navigate(Routes.BILLING) },
            onShowCompanions = if (beside) ({ section -> shell.toggleCompanions(id, section) }) else null,
        )
    }

    Box(
        Modifier.fillMaxSize().onPreviewKeyEvent { e ->
            // Hardware keyboards (iPad's): Ctrl+N new session, Ctrl+Shift+F search, Ctrl+B sidebar,
            // Ctrl+J terminal, Ctrl+Alt+0 agents, side chats and terminals.
            if (e.type != KeyEventType.KeyDown || !(e.isCtrlPressed || e.isMetaPressed)) return@onPreviewKeyEvent false
            val chat = if (wide) shell.selected else nav.currentBackStackEntry?.takeIf { it.destination.route == Routes.CHAT }?.arguments?.getString("id")
            when {
                e.key == Key.N -> true.also { if (wide) shell.newSession() else nav.navigate(Routes.NEW) }
                e.key == Key.F && e.isShiftPressed -> true.also { if (wide) shell.searching = true else nav.navigate(Routes.SEARCH) }
                e.key == Key.B && wide -> true.also { shell.sidebar = !shell.sidebar }
                e.key == Key.J && chat != null -> true.also {
                    val live = model.terminals.tabs(chat).firstOrNull { it.isLive }?.ref?.id
                    if (wide) shell.push(chat, InspectorPage.Terminal(live)) else nav.navigate(Routes.terminal(chat, live))
                }
                e.key == Key.Zero && e.isAltPressed && chat != null && wide -> true.also { shell.toggleCompanions(chat, null) }
                else -> false
            }
        },
    ) {
    NavHost(nav, startDestination = Routes.HOME) {
        composable(Routes.HOME) {
            if (!wide) {
                Home(
                    model,
                    open,
                    onSettings = { nav.navigate(Routes.SETTINGS) },
                    onSearch = { nav.navigate(Routes.SEARCH) },
                    onNew = { nav.navigate(Routes.NEW) },
                    onArchived = { nav.navigate(Routes.ARCHIVED) },
                    onVoice = { nav.navigate(Routes.VOICE) { launchSingleTop = true } },
                    onVoiceSettings = { nav.navigate(Routes.VOICE_SETTINGS) },
                )
            } else {
                TabletShell(
                    model,
                    shell,
                    roomy = roomy,
                    // About a phone's width; narrower windows hold it at its minimum.
                    sidebarWidth = sidebarWidth,
                    sidebar = {
                        if (shell.searching) {
                            SearchScreen(model, onBack = { shell.searching = false }, onOpen = { id ->
                                shell.searching = false
                                open(id)
                            })
                        } else {
                            Home(
                                model,
                                open,
                                selected = shell.selected,
                                onSettings = { shell.settings = Routes.SETTINGS },
                                onSearch = { shell.searching = true },
                                onNew = { shell.newSession() },
                                onArchived = { shell.settings = Routes.ARCHIVED },
                                onVoice = { nav.navigate(Routes.VOICE) { launchSingleTop = true } },
                                onVoiceSettings = { nav.navigate(Routes.VOICE_SETTINGS) },
                            )
                        }
                    },
                    session = { id -> Session(id, onBack = { shell.select(null) }, beside = true) },
                    draft = {
                        NewSessionScreen(
                            model,
                            onClose = null,
                            onCreated = { id -> shell.select(id) },
                            onOpenBilling = { shell.settings = Routes.BILLING },
                            focusOnAppear = shell.draftToken > 0,
                        )
                    },
                    page = { chat, page -> InspectorPageContent(model, shell, chat, page) },
                )
            }
        }
        settingsGraph(nav, model, openChat = { id ->
            nav.popBackStack(Routes.HOME, inclusive = false)
            open(id)
        })
        composable(Routes.CHAT) { entry ->
            val id = entry.arguments?.getString("id") ?: return@composable
            Session(id, onBack = { nav.popBackStack() })
        }
        composable(
            Routes.TERMINAL,
            arguments = listOf(navArgument("chat") { type = NavType.StringType }, navArgument("id") { type = NavType.StringType; nullable = true }),
        ) { entry ->
            val chat = entry.arguments?.getString("chat") ?: return@composable
            TerminalScreen(
                model,
                chat,
                terminalId = entry.arguments?.getString("id"),
                onBack = { nav.popBackStack() },
                script = if (model.launch.terminalDemo) TERMINAL_DEMO else emptyList(),
            )
        }
        composable(
            Routes.SUBAGENT,
            arguments = listOf(
                navArgument("parent") { type = NavType.StringType },
                navArgument("doc") { type = NavType.StringType },
                navArgument("title") { type = NavType.StringType; defaultValue = "Subagent" },
                navArgument("running") { type = NavType.BoolType; defaultValue = false },
            ),
        ) { entry ->
            val args = entry.arguments ?: return@composable
            SubagentScreen(
                model,
                parentId = args.getString("parent") ?: return@composable,
                docId = args.getString("doc") ?: return@composable,
                title = args.getString("title") ?: "Subagent",
                running = args.getBoolean("running"),
                onBack = { nav.popBackStack() },
                onOpenSubagent = { parent, doc, title, running -> nav.navigate(Routes.subagent(parent, doc, title, running)) },
            )
        }
        composable(Routes.VOICE) {
            sh.zeron.android.voice.VoiceStageScreen(
                model.voice,
                onClose = { nav.popBackStack() },
                onOpenChat = { chatId ->
                    nav.popBackStack()
                    open(chatId)
                },
                wallpaper = model.wallpaper,
            )
        }
        composable(Routes.NEW) {
            NewSessionScreen(
                model,
                onClose = { nav.popBackStack() },
                onCreated = { id ->
                    nav.popBackStack()
                    open(id)
                },
                onOpenBilling = { nav.navigate(Routes.BILLING) },
            )
        }
        composable(Routes.SEARCH) {
            SearchScreen(model, onBack = { nav.popBackStack() }, onOpen = { id ->
                if (wide) nav.popBackStack()
                open(id)
            })
        }
        composable(Routes.WELCOME, arguments = listOf(navArgument("step") { type = NavType.StringType; nullable = true })) { entry ->
            OnboardingScreen(model, entry.arguments?.getString("step")) { startSession ->
                nav.popBackStack()
                if (startSession) {
                    if (wide) shell.newSession() else nav.navigate(Routes.NEW)
                }
            }
        }
    }
    // Settings on a large screen: a dialog over the shell with its own pages (iPad's form sheet).
    shell.settings?.let { start ->
        SettingsDialog(model, start, onDismiss = { shell.settings = null }, openChat = { id ->
            shell.settings = null
            open(id)
        })
    }
    Toasts(model, nav)
    }
    }
}

private val TERMINAL_DEMO = listOf("git log --oneline -5", "cargo test -p zeron-veil")

/**
 * Settings and the pages it leads to (Providers, Cloud, Billing, an agent's
 * logins, Archived, voice): on the phone's stack, or in the large-screen
 * Settings dialog's own. `onClose` ends the dialog from its first page.
 */
private fun androidx.navigation.NavGraphBuilder.settingsGraph(
    nav: NavHostController,
    model: AppModel,
    openChat: (String) -> Unit,
    onClose: (() -> Unit)? = null,
) {
    fun back() {
        if (!nav.popBackStack()) onClose?.invoke()
    }
    composable(Routes.SETTINGS) {
        Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
            SettingsScreen(
                model,
                onArchived = { nav.navigate(Routes.ARCHIVED) },
                onOpenVoiceSettings = { nav.navigate(Routes.VOICE_SETTINGS) },
                onBack = ::back,
                onOpenProviders = { nav.navigate(Routes.providers(null)) },
                onOpenCloud = { nav.navigate(Routes.CLOUD) },
                onOpenBilling = { nav.navigate(Routes.BILLING) },
            )
        }
    }
    composable(Routes.ARCHIVED) {
        ArchivedScreen(model, onBack = ::back, onOpen = openChat)
    }
    composable(Routes.VOICE_SETTINGS) {
        sh.zeron.android.voice.VoiceSettingsScreen(model.voice, onBack = ::back)
    }
    composable(Routes.PROVIDERS, arguments = listOf(navArgument("target") { type = NavType.StringType; nullable = true })) { entry ->
        ProvidersScreen(
            model,
            entry.arguments?.getString("target"),
            onBack = ::back,
            onOpenAccounts = { target -> nav.navigate(Routes.accounts(target.encode(), signIn = false)) },
            onOpenCloud = { nav.navigate(Routes.CLOUD) },
        )
    }
    composable(Routes.CLOUD) {
        CloudSettingsScreen(
            model,
            onBack = ::back,
            onOpenBilling = { nav.navigate(Routes.BILLING) },
            onOpenAccounts = { target -> nav.navigate(Routes.accounts(target.encode(), signIn = false)) },
            onOpenRepoPicker = { nav.navigate(Routes.CLOUD_REPOS) },
        )
    }
    composable(Routes.BILLING) {
        BillingScreen(model, onBack = ::back)
    }
    composable(Routes.CLOUD_REPOS) {
        CloudRepoPickerScreen(model, onBack = ::back, onCreated = { back() })
    }
    composable(
        Routes.ACCOUNTS,
        arguments = listOf(navArgument("target") { type = NavType.StringType }, navArgument("signin") { type = NavType.BoolType; defaultValue = false }),
    ) { entry ->
        val target = entry.arguments?.getString("target")?.let { AccountsTarget.decode(it, model::deviceName) } ?: return@composable
        AgentAccountsScreen(model, target, signIn = entry.arguments?.getBoolean("signin") == true, onBack = ::back)
    }
}

/** Large screens: Settings over the shell as a dialog with its own stack of pages. */
@Composable
private fun SettingsDialog(model: AppModel, start: String, onDismiss: () -> Unit, openChat: (String) -> Unit) {
    androidx.compose.ui.window.Dialog(
        onDismissRequest = onDismiss,
        properties = androidx.compose.ui.window.DialogProperties(usePlatformDefaultWidth = false),
    ) {
        val nav = rememberNavController()
        Surface(
            shape = androidx.compose.foundation.shape.RoundedCornerShape(28.dp),
            color = MaterialTheme.colorScheme.background,
            modifier = Modifier.fillMaxWidth(0.9f).widthIn(max = 720.dp).fillMaxHeight(0.92f),
        ) {
            NavHost(nav, startDestination = start) {
                settingsGraph(nav, model, openChat = openChat, onClose = onDismiss)
            }
        }
    }
}

/** App-wide snackbars ("Archived · Undo"), just above whatever bottom chrome the page has. */
@Composable
private fun androidx.compose.foundation.layout.BoxScope.Toasts(model: AppModel, nav: NavHostController) {
    val host = remember { SnackbarHostState() }
    LaunchedEffect(Unit) {
        model.toasts.collect { toast ->
            // One at a time: a new toast replaces the one showing.
            host.currentSnackbarData?.dismiss()
            launch {
                val result = host.showSnackbar(toast.message, actionLabel = toast.action, withDismissAction = toast.action != null, duration = SnackbarDuration.Long)
                if (result == SnackbarResult.ActionPerformed) toast.run?.invoke()
            }
        }
    }
    val entry by nav.currentBackStackEntryAsState()
    val inset = when (entry?.destination?.route) {
        Routes.HOME -> 96.dp
        Routes.CHAT, Routes.NEW -> 96.dp
        Routes.TERMINAL, Routes.SUBAGENT -> 72.dp
        Routes.WELCOME -> 120.dp
        else -> 16.dp
    }
    SnackbarHost(host, Modifier.align(Alignment.BottomCenter).imePadding().navigationBarsPadding().padding(bottom = inset))
}

/**
 * The front page: the Sessions list over a Material 3 bottom app bar —
 * Settings, Search and voice as its actions, New session as its FAB (iOS's
 * bottom bar: Settings · Search · voice · New session). The bar slides away
 * while the list scrolls down and comes back on the way up.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class, androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun Home(
    model: AppModel,
    open: (String) -> Unit,
    selected: String? = null,
    onSettings: () -> Unit,
    onSearch: () -> Unit,
    onNew: () -> Unit,
    onArchived: () -> Unit,
    onVoice: () -> Unit,
    onVoiceSettings: () -> Unit,
) {
    // The FAB shows its label at rest and tucks to its icon while the list scrolls down (Gmail's Compose).
    var fabExpanded by rememberSaveable { mutableStateOf(true) }
    val fabScroll = remember {
        object : androidx.compose.ui.input.nestedscroll.NestedScrollConnection {
            override fun onPreScroll(available: androidx.compose.ui.geometry.Offset, source: androidx.compose.ui.input.nestedscroll.NestedScrollSource): androidx.compose.ui.geometry.Offset {
                if (available.y < -4f) fabExpanded = false else if (available.y > 4f) fabExpanded = true
                return androidx.compose.ui.geometry.Offset.Zero
            }
        }
    }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Box(Modifier.fillMaxSize().nestedScroll(fabScroll)) {
            SessionsScreen(
                model,
                onOpen = open,
                onArchived = onArchived,
                selected = selected,
                onSearch = onSearch,
                onSettings = onSettings,
                searchActions = {
                    // Tap calls (or opens the live call); hold picks the host and style.
                    if (sh.zeron.android.voice.rememberVoiceVisible(model.voice)) {
                        sh.zeron.android.voice.VoiceHeaderButton(
                            model.voice,
                            onOpenStage = onVoice,
                            onOpenSettings = onVoiceSettings,
                            size = 40.dp,
                            container = Color.Transparent,
                        )
                    }
                },
            )
        }
        ExtendedFloatingActionButton(
            text = { Text("New session") },
            icon = { ZIcon(ZIcons.NewSession, null, Modifier.size(24.dp)) },
            onClick = onNew,
            expanded = fabExpanded,
            modifier = Modifier
                .align(Alignment.BottomEnd)
                .navigationBarsPadding()
                .padding(16.dp),
        )
    }
}

/** A long-press label for an icon-only control. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3Api::class)
@Composable
private fun Tip(text: String, content: @Composable () -> Unit) {
    TooltipBox(
        positionProvider = TooltipDefaults.rememberTooltipPositionProvider(TooltipAnchorPosition.Above),
        tooltip = { PlainTooltip { Text(text) } },
        state = rememberTooltipState(),
        content = content,
    )
}
