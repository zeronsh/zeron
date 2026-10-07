@file:OptIn(
    androidx.compose.foundation.ExperimentalFoundationApi::class,
    androidx.compose.material3.ExperimentalMaterial3Api::class,
)

package sh.zeron.android.ui

import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.platform.testTag
import sh.zeron.android.design.BackButton
import sh.zeron.android.R
import androidx.compose.ui.res.stringResource
import sh.zeron.android.core.AppLanguage
import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.windowInsetsTopHeight
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.input.pointer.pointerInput
import kotlin.math.abs
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.zIndex
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import kotlinx.coroutines.launch
import sh.zeron.android.core.Machine
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AnchoredMenu
import sh.zeron.android.design.AssetIcon
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.menuSection
import sh.zeron.android.design.PinSlashGlyph
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.onSizeChanged
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.TitleLineMark
import sh.zeron.android.design.ChevronMark
import sh.zeron.android.design.EllipsisMark
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.MarkKind
import sh.zeron.android.design.PlusMark
import sh.zeron.android.design.ProfileMark
import sh.zeron.android.design.ProjectTile
import sh.zeron.android.design.TileBesideLabel
import sh.zeron.android.design.PullRequestIcon
import sh.zeron.android.design.ReorderMark
import sh.zeron.android.design.StatusMark
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronDark
import sh.zeron.android.design.ZeronThemes
import sh.zeron.android.design.AccentChoice
import sh.zeron.android.design.ZeronLight
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.PullRequest
import uniffi.zeron_core.PullRequestState
import uniffi.zeron_core.SectionView
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.WallpaperEffect
import kotlin.math.roundToInt

@Composable
fun ZeronApp(model: ZeronModel) {
    val systemDark = isSystemInDarkTheme()
    val dark = when (model.appearance) {
        1 -> false
        2 -> true
        else -> systemDark
    }
    val colors = remember(dark, model.themeLight, model.themeDark, model.accent) {
        ZeronThemes.colors(dark, model.themeLight, model.themeDark, model.accent)
    }
    val view = LocalView.current
    SideEffect {
        val window = (view.context as? android.app.Activity)?.window ?: return@SideEffect
        val controller = WindowCompat.getInsetsController(window, view)
        controller.isAppearanceLightStatusBars = !dark
        controller.isAppearanceLightNavigationBars = !dark
        // The window shows through during any redraw of the whole view tree:
        // keep it the app's background (the in-app Light/Dark choice can
        // differ from the theme's day/night resource).
        window.setBackgroundDrawable(android.graphics.drawable.ColorDrawable(colors.background.toArgb()))
    }
    androidx.compose.runtime.CompositionLocalProvider(LocalZeronColors provides colors) {
        sh.zeron.android.design.ZeronMaterialTheme(colors) { AppContent(model, colors) }
    }
}

@Composable
private fun AppContent(model: ZeronModel, colors: ZeronColors) {
    val phase = model.phase
    when {
        phase is ZeronModel.Phase.Loading -> CenterMessage(colors, stringResource(R.string.opening_workspace))
        phase is ZeronModel.Phase.Failed -> Column(
            Modifier.fillMaxSize().background(colors.background).padding(24.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(phase.message, color = colors.danger, fontFamily = ZeronType.Sans)
            Spacer(Modifier.height(16.dp))
            Text(stringResource(R.string.try_demo), color = colors.background, modifier = Modifier.clip(RoundedCornerShape(20.dp)).background(colors.text).clickable { model.enterDemo() }.padding(horizontal = 18.dp, vertical = 12.dp))
            Spacer(Modifier.height(10.dp))
            Text(stringResource(R.string.accounts_computers_ellipsis), color = colors.text, modifier = Modifier.clip(RoundedCornerShape(20.dp)).background(colors.controlFill).clickable { model.showMachines = true }.padding(horizontal = 18.dp, vertical = 12.dp))
        }
        phase is ZeronModel.Phase.SignedOut || model.showSignIn -> SignInScreen(model)
        else -> Shell(model, colors)
    }
    if (model.showMachines) {
        BackHandler { if (!model.back()) model.showMachines = false }
        MachinesScreen(model)
    }
    model.editMachine?.let { machine ->
        // Route through model.back() so the chip shortcut can unwind the
        // whole path in one step; otherwise this just clears the editor.
        BackHandler { model.back() }
        androidx.compose.runtime.key(machine.id) { MachineEditScreen(model, machine) }
    }
    if (model.showLinkDetails) {
        BackHandler { model.showLinkDetails = false }
        LinkDetailsScreen(model)
    }
    if (model.showUpdate) {
        BackHandler { model.showUpdate = false }
        UpdateScreen(model)
    }
    if (model.showCrashLogs) {
        BackHandler { model.showCrashLogs = false }
        CrashLogsScreen(model)
    }
    model.lastCrash?.let { LastCrashDialog(model, it) }
    UpdateDownloadSheets(model, colors)
    model.toast?.let { message ->
        Box(Modifier.fillMaxSize().padding(bottom = 120.dp), contentAlignment = Alignment.BottomCenter) {
            Row(
                Modifier.clip(RoundedCornerShape(20.dp)).background(colors.text).padding(start = 16.dp, end = if (model.toastUndo != null) 6.dp else 16.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(message, color = colors.background, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(vertical = 10.dp))
                if (model.toastUndo != null) {
                    Spacer(Modifier.width(6.dp))
                    Text(
                        stringResource(R.string.undo),
                        color = colors.undoTint,
                        fontFamily = ZeronType.Sans,
                        fontWeight = FontWeight.SemiBold,
                        fontSize = 14.sp,
                        modifier = Modifier.clip(RoundedCornerShape(14.dp)).clickable { model.runUndo() }.padding(horizontal = 10.dp, vertical = 10.dp),
                    )
                }
            }
        }
    }
}

@Composable
private fun CenterMessage(colors: ZeronColors, text: String) {
    Box(Modifier.fillMaxSize().background(colors.background), contentAlignment = Alignment.Center) {
        Text(text, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 16.sp)
    }
}

@Composable
private fun Shell(model: ZeronModel, colors: ZeronColors) {
    val stack = if (model.tab == ZeronModel.Tab.Settings) model.settingsStack else model.sessionStack
    val top = stack.lastOrNull()
    val inSession = top is ZeronModel.Route.Session && model.tab == ZeronModel.Tab.Sessions
    val frontPage = model.tab == ZeronModel.Tab.Sessions && !inSession && top !is ZeronModel.Route.Folder
    val page = if (frontPage) colors.shell else colors.background
    // Wide screens (unfolded foldable, tablet) split like the iPad shell:
    // sessions column on the left, detail column with the open session or the
    // new-session page, and an optional right column the session can fill.
    val splitView = model.tab == ZeronModel.Tab.Sessions &&
        androidx.compose.ui.platform.LocalConfiguration.current.screenWidthDp >= 840
    val sidePanel = remember { SidePanelState() }
    if (!splitView && sidePanel.content != null) SideEffect { sidePanel.content = null }
    BackHandler(enabled = model.showNewSession || model.showSignIn || stack.isNotEmpty() || model.tab != ZeronModel.Tab.Sessions) {
        if (model.showNewSession || model.showSignIn || stack.isNotEmpty()) model.back()
        else model.tab = ZeronModel.Tab.Sessions
    }
    var prompt by remember { mutableStateOf<Prompt?>(null) }
    val menus = remember { RowMenuHost() }
    val swipe = remember { SwipeCoordinator() }
    androidx.compose.runtime.CompositionLocalProvider(LocalRowMenus provides menus, LocalSwipe provides swipe) {
    Box(Modifier.fillMaxSize().background(page)) {
        if (frontPage) {
            model.wallpaper?.let { bmp ->
                Image(
                    bmp.asImageBitmap(),
                    contentDescription = null,
                    modifier = Modifier.fillMaxSize(),
                    contentScale = ContentScale.Crop,
                    alpha = model.wallpaperOpacity,
                )
            }
        }
        if (splitView) {
            androidx.compose.runtime.CompositionLocalProvider(LocalSidePanel provides sidePanel) {
            Row(Modifier.fillMaxSize()) {
                // With a file tree parked on the right the sessions column
                // folds away so the transcript keeps a readable width.
                if (sidePanel.content == null) {
                    Column(Modifier.width(320.dp).fillMaxHeight()) {
                        val folder = stack.lastOrNull { it is ZeronModel.Route.Folder } as? ZeronModel.Route.Folder
                        if (folder != null) FolderScreen(model, colors, folder)
                        else SessionsScreen(model, colors, onPrompt = { prompt = it })
                    }
                    Box(Modifier.width(1.dp).fillMaxHeight().background(colors.hairline))
                }
                Box(Modifier.weight(1f).fillMaxHeight()) {
                    // A folder pushed over the list leaves the session in place,
                    // matching the iPad sidebar/detail split.
                    val session = stack.lastOrNull { it is ZeronModel.Route.Session } as? ZeronModel.Route.Session
                    if (session != null && !model.showNewSession) {
                        androidx.compose.runtime.key(session.id) { SessionScreen(model, session.id) }
                    } else {
                        NewSessionSheet(model, onDismiss = { model.showNewSession = false }, embedded = true, autofocus = model.showNewSession)
                    }
                }
                sidePanel.content?.let { panel ->
                    Box(Modifier.width(1.dp).fillMaxHeight().background(colors.hairline))
                    Box(Modifier.width(340.dp).fillMaxHeight()) { panel() }
                }
            }
            }
        } else when {
            inSession -> {
                val id = (top as ZeronModel.Route.Session).id
                // A launch intent can swap chats without leaving composition; key the
                // screen so the transcript view is rebuilt on the new engine.
                androidx.compose.runtime.key(id) { SessionScreen(model, id) }
            }
            top is ZeronModel.Route.Folder -> FolderScreen(model, colors, top)
            model.tab == ZeronModel.Tab.Settings -> SettingsScreen(model, colors)
            model.tab == ZeronModel.Tab.Search -> SearchScreen(model, colors)
            else -> SessionsScreen(model, colors, onPrompt = { prompt = it })
        }
        if (frontPage && !model.showNewSession) ConnectionSheetHost(model, colors)
        if (model.showNewSession && !splitView) {
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.35f))) {
                NewSessionSheet(model, onDismiss = { model.showNewSession = false })
            }
        }
        RowMenusOverlay(model, colors, menus) { prompt = it }
    }
    }
    prompt?.let { current ->
        NameDialog(colors, current, onDismiss = { prompt = null }) { value ->
            current.onSubmit(value)
            prompt = null
        }
    }
}

/**
 * The long-press menus (iOS context menus): Pin/Unpin, move between sections,
 * Rename…, Archive in red; archived rows get Unarchive and Rename…. The row is
 * re-read when the menu opens so the pin label is never stale.
 */
@Composable
private fun RowMenusOverlay(model: ZeronModel, colors: ZeronColors, menus: RowMenuHost, onPrompt: (Prompt) -> Unit) {
    val context = LocalContext.current
    val screenHeight = with(androidx.compose.ui.platform.LocalDensity.current) {
        androidx.compose.ui.platform.LocalConfiguration.current.screenHeightDp.dp.toPx()
    }
    menus.row?.let { target ->
        val row = model.client?.sessionRow(target.id)
            ?: model.workspace?.archived?.firstOrNull { it.id == target.id }
        if (row == null) {
            menus.row = null
            return@let
        }
        val sections = model.workspace?.front?.sections.orEmpty()
        val rename = MenuEntry(stringResource(R.string.rename_ellipsis), icon = { c -> Glyph(Glyphs.Rename, 17.dp, c) }) {
            onPrompt(Prompt(context.getString(R.string.rename), row.title, context.getString(R.string.rename)) { value -> if (value.isNotEmpty()) model.rename(row.id, value) })
        }
        val entries = if (target.archived || row.archived) {
            listOf(
                MenuEntry(stringResource(R.string.unarchive), icon = { c -> Glyph(Glyphs.Unarchive, 18.dp, c) }) { model.unarchive(row.id) },
                rename,
            )
        } else {
            buildList {
                add(
                    MenuEntry(stringResource(if (row.pinned) R.string.unpin else R.string.pin), icon = { c -> if (row.pinned) PinSlashGlyph(17.dp, c) else Glyph(Glyphs.Pin, 17.dp, c) }) {
                        model.pin(row.id, !row.pinned)
                    },
                )
                sections.filter { it.id != row.sectionId }.forEach { section ->
                    add(MenuEntry(stringResource(R.string.move_to_named, section.name), icon = { c -> Glyph(Glyphs.Folder, 17.dp, c) }) { model.move(row.id, section.id) })
                }
                if (row.sectionId != null) add(MenuEntry(stringResource(R.string.no_section), icon = { c -> Glyph(Glyphs.Tray, 17.dp, c) }) { model.move(row.id, null) })
                add(
                    MenuEntry(stringResource(R.string.new_section_ellipsis), icon = { c -> Glyph(Glyphs.FolderPlus, 17.dp, c) }) {
                        onPrompt(Prompt(context.getString(R.string.new_section), "", context.getString(R.string.create)) { value -> if (value.isNotEmpty()) model.createSection(value) })
                    },
                )
                add(rename)
                add(MenuEntry(stringResource(R.string.archive), destructive = true, icon = { c -> Glyph(Glyphs.Archive, 17.dp, c) }) { model.archive(row.id) })
            }
        }
        AnchoredMenu(colors, target.anchor, title = null, entries = entries, above = target.anchor.center.y > screenHeight * 0.55f) { menus.row = null }
    }
    menus.move?.let { target ->
        // The swipe "Move" action's menu (iOS presentMoveMenu): every user
        // section with a checkmark on the row's, then No Section, then New.
        val row = model.client?.sessionRow(target.id)
            ?: model.workspace?.archived?.firstOrNull { it.id == target.id }
        if (row == null) {
            menus.move = null
            return@let
        }
        val entries = buildList {
            model.workspace?.front?.sections.orEmpty().forEach { section ->
                add(
                    MenuEntry(section.name, checked = section.id == row.sectionId, icon = { c -> Glyph(Glyphs.Folder, 17.dp, c) }) {
                        model.move(row.id, section.id)
                    },
                )
            }
            add(MenuEntry(stringResource(R.string.no_section), checked = row.sectionId == null, icon = { c -> Glyph(Glyphs.Tray, 17.dp, c) }) { model.move(row.id, null) })
            add(
                MenuEntry(stringResource(R.string.new_section_ellipsis), icon = { c -> Glyph(Glyphs.FolderPlus, 17.dp, c) }) {
                    onPrompt(Prompt(context.getString(R.string.new_section), "", context.getString(R.string.create)) { value -> if (value.isNotEmpty()) model.createSection(value) })
                },
            )
        }
        AnchoredMenu(colors, target.anchor, title = stringResource(R.string.move_to_section), entries = entries, above = target.anchor.center.y > screenHeight * 0.55f) { menus.move = null }
    }
    menus.header?.let { target ->
        // iOS headerMenu: Pinned gets Open + Reorder… (the folder screen), a
        // section gets Open + Rename/Delete. "Recent" never opens a menu.
        val entries = buildList {
            add(MenuEntry(stringResource(R.string.open), icon = { c -> Glyph(Glyphs.Folder, 17.dp, c) }) { model.openFolder(target.id, target.title) })
            val section = target.section
            if (section == null) {
                add(MenuEntry(stringResource(R.string.reorder_ellipsis), icon = { c -> Glyph(Glyphs.ArrowUp, 17.dp, c) }) { model.openFolder(target.id, target.title) })
            } else {
                add(
                    MenuEntry(stringResource(R.string.rename_section_ellipsis), icon = { c -> Glyph(Glyphs.Rename, 17.dp, c) }) {
                        onPrompt(Prompt(context.getString(R.string.rename_section), section.name, context.getString(R.string.rename)) { value -> if (value.isNotEmpty()) model.renameSection(section.id, value) })
                    },
                )
                add(MenuEntry(stringResource(R.string.delete_section), destructive = true, icon = { c -> Glyph(Glyphs.Archive, 17.dp, c) }) { model.deleteSection(section.id) })
            }
        }
        AnchoredMenu(colors, target.anchor, title = target.title, entries = entries, above = target.anchor.center.y > screenHeight * 0.55f) { menus.header = null }
    }
}


/** The list's floating header row: 6 + 44 (capsules) + 6. */
private val ListHeaderHeight = 56.dp

/** Shortest time the pull-to-refresh spinner stays up, so it never just flashes. */
private const val MinRefreshSpinMs = 600L

/** Gap below the header where rows fade out. */
private val ListHeaderGap = 14.dp

private data class Prompt(val title: String, val initial: String, val confirm: String, val onSubmit: (String) -> Unit)

@Composable
private fun NameDialog(colors: ZeronColors, prompt: Prompt, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var text by remember(prompt) { mutableStateOf(prompt.initial) }
    AlertDialog(
        onDismissRequest = onDismiss,
        title = { Text(prompt.title, fontFamily = ZeronType.Sans) },
        text = {
            AutoFocusNameField(
                prompt.initial,
                colors,
                onChange = { text = it },
                modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
                onDone = { onConfirm(text.trim()) },
            )
        },
        confirmButton = { TextButton(onClick = { onConfirm(text.trim()) }) { Text(prompt.confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text(stringResource(R.string.cancel)) } },
    )
}

@Composable
private fun SessionsScreen(model: ZeronModel, colors: ZeronColors, onPrompt: (Prompt) -> Unit) {
    val front = model.workspace?.front
    var menu by remember { mutableStateOf(false) }
    var menuAnchor by remember { mutableStateOf(androidx.compose.ui.geometry.Rect.Zero) }
    var statsOpen by remember { mutableStateOf(false) }
    var statsAnchor by remember { mutableStateOf(androidx.compose.ui.geometry.Rect.Zero) }
    var refreshing by remember { mutableStateOf(false) }
    val refreshScope = rememberCoroutineScope()
    val mode = model.listMode
    val homeColor = remember(model.client) { model.homeColorIndex() }
    val context = LocalContext.current
    val pinnedTitle = stringResource(R.string.pinned)
    // Keyed on the snapshot itself: ZeronModel only swaps `workspace` when its
    // content changed, so streaming updates elsewhere do not rebuild the list.
    // By Project (default): Pinned, the user's sections, then one foldable
    // group per project ("~" for project-less sessions). By Activity: one flat
    // list, live turns first, then unread, then the rest.
    val groups = remember(front, mode, homeColor, pinnedTitle) {
        val seen = HashSet<String>()
        fun fresh(list: List<SessionRow>) = list.filter { seen.add(it.id) }
        val out = ArrayList<ListGroup>()
        if (mode == ZeronModel.ListMode.Activity) {
            val all = front?.pinned.orEmpty() + front?.sections.orEmpty().flatMap { it.sessions } + front?.recent.orEmpty()
            out.add(ListGroup("activity", null, SessionGrouping.activityOrder(all), menu = false))
            return@remember out
        }
        val pinned = fresh(front?.pinned.orEmpty())
        if (pinned.isNotEmpty()) out.add(ListGroup("pinned", pinnedTitle, pinned))
        front?.sections.orEmpty().forEach { section -> out.add(ListGroup(section.id, section.name, fresh(section.sessions), section = section)) }
        SessionGrouping.projectGroups(fresh(front?.recent.orEmpty())).forEach { g ->
            // The "~" group wears the desktop's home tile ("H").
            val tile = if (g.projectId == null) "Home" to homeColor else g.title to (g.colorIndex ?: 0)
            out.add(ListGroup(g.key, g.title, g.rows, tile = tile, menu = false))
        }
        out
    }
    val rows = remember(groups) { groups.flatMap { it.rows } }
    val listState = rememberLazyListState()
    val guard = remember { TapGuard() }
    val swipe = LocalSwipe.current
    val menus = LocalRowMenus.current
    // Workspace refreshes wait while the list moves (see ZeronModel.requestRefresh).
    LaunchedEffect(listState) {
        snapshotFlow { listState.isScrollInProgress }.collect {
            model.scrolling = it
            // Scrolling closes revealed swipe actions, like UITableView.
            if (it) swipe.openId = null
        }
    }
    DisposableEffect(listState) { onDispose { model.scrolling = false } }
    // Rows show relative times; LocalNow ticks every 30 s.
    NowProvider {
    Box(Modifier.fillMaxSize()) {
        val pullState = androidx.compose.material3.pulltorefresh.rememberPullToRefreshState()
        val headerBottom = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + ListHeaderHeight
        PullToRefreshBox(isRefreshing = refreshing, onRefresh = {
            // Material3 only animates the indicator when it SEES isRefreshing
            // flip (false → true shows the spinner, true → false hides it).
            // Flipping both ways in one frame left the arrow stuck on screen,
            // so hold the spinner for a moment, then let it animate away.
            if (!refreshing) {
                refreshing = true
                refreshScope.launch {
                    val started = android.os.SystemClock.uptimeMillis()
                    try {
                        model.refreshPull()
                    } finally {
                        val left = MinRefreshSpinMs - (android.os.SystemClock.uptimeMillis() - started)
                        if (left > 0) kotlinx.coroutines.delay(left)
                        refreshing = false
                    }
                }
            }
        }, modifier = Modifier.fillMaxSize(), state = pullState, indicator = {
            // Below the solid header band, or it would pull out of sight.
            androidx.compose.material3.pulltorefresh.PullToRefreshDefaults.Indicator(
                state = pullState,
                isRefreshing = refreshing,
                modifier = Modifier.align(Alignment.TopCenter).padding(top = headerBottom),
            )
        }) {
            val scheduledNew = rememberScheduledNewSessions(model.activeMachine)
            val cancelScheduled = { message: sh.zeron.android.schedule.ScheduledMessage ->
                sh.zeron.android.schedule.ScheduledAlarms.cancel(context, message.id)
                model.showToast(context.getString(R.string.schedule_cancelled))
            }
            if (rows.isEmpty()) {
                val topInset = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = topInset + ListHeaderHeight + ListHeaderGap)) {
                    DirectBanner(model, colors)
                    ScheduledNewSessionRows(colors, scheduledNew, onCancel = cancelScheduled)
                    Box(Modifier.fillMaxWidth().padding(32.dp).padding(top = 120.dp), contentAlignment = Alignment.Center) {
                        Text(emptySessionsText(model), color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 16.sp)
                    }
                }
            } else {
                val topInset = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
                LazyColumn(
                    state = listState,
                    modifier = Modifier.fillMaxSize().tapGuard(guard) { listState.isScrollInProgress },
                    contentPadding = PaddingValues(top = topInset + ListHeaderHeight + ListHeaderGap, bottom = 28.dp),
                ) {
                    item(key = "direct-banner") { DirectBanner(model, colors) }
                    if (scheduledNew.isNotEmpty()) item(key = "scheduled-new") { ScheduledNewSessionRows(colors, scheduledNew, onCancel = cancelScheduled) }
                    groups.forEach { group ->
                        val folded = group.title != null && model.isCollapsed(group.id)
                        if (group.title != null) {
                            item(key = "head-${group.id}", contentType = "head") {
                                GroupHeader(
                                    colors,
                                    title = group.title,
                                    count = group.rows.size,
                                    collapsed = folded,
                                    live = groupHeaderMark(group.rows, colors.input),
                                    onToggle = { model.toggleCollapsed(group.id) },
                                    onLongPress = if (!group.menu) null else { rect: androidx.compose.ui.geometry.Rect ->
                                        menus.header = HeaderMenuTarget(group.id, group.title, group.section, rect)
                                    },
                                    tile = group.tile,
                                )
                            }
                        }
                        if (!folded) {
                            items(group.rows, key = { it.id }, contentType = { "row" }) { row ->
                                SessionRowView(row, colors, archived = false, model = model, tapGuard = guard, showPin = mode == ZeronModel.ListMode.Activity && row.pinned)
                            }
                        }
                    }
                }
            }
        }
        // Solid band behind the status bar and the floating capsules, then a
        // fade: rows scrolling up dissolve a gap below the header instead of
        // running under the counts capsule and the buttons (like the chat's top edge).
        val band = colors.shell
        Column(Modifier.fillMaxWidth()) {
            Box(Modifier.fillMaxWidth().height(headerBottom).background(band))
            Box(Modifier.fillMaxWidth().height(ListHeaderGap).background(Brush.verticalGradient(listOf(band, band.copy(alpha = 0f)))))
        }
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 16.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            // Running / failed counts in place of the old "Sessions" title (it
            // stays the heading for TalkBack); tap for the sessions themselves.
            val stats = HomeStats.of(model.workspace)
            HomeStatsCapsule(
                stats.running.size,
                stats.failed.size,
                colors,
                Modifier.onGloballyPositioned { statsAnchor = it.boundsInRoot() },
                onOpen = { statsOpen = true },
            )
            Spacer(Modifier.width(4.dp))
            // Current computer + link state; takes what's left between the
            // counts and the capsule, and ellipsizes rather than pushing it.
            Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                ConnectionChip(model, colors, Modifier.weight(1f, fill = false))
                if (model.updateBadge != null) {
                    Spacer(Modifier.width(6.dp))
                    UpdateBadge(model, colors)
                }
            }
            Spacer(Modifier.width(6.dp))
            Row(
                Modifier.height(44.dp).glassSurface(colors, 22.dp).padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                val moreLabel = stringResource(R.string.more_options)
                Box(
                    Modifier.size(40.dp).onGloballyPositioned { menuAnchor = it.boundsInRoot() }.testTag("home-more")
                        .semantics { contentDescription = moreLabel }.clickable { menu = true },
                    contentAlignment = Alignment.Center,
                ) {
                    EllipsisMark(colors.text, Modifier.size(18.dp))
                }
                Box(Modifier.size(40.dp).clickable { model.showNewSession = true }, contentAlignment = Alignment.Center) {
                    PlusMark(colors.text, Modifier.size(18.dp))
                }
                Box(Modifier.size(36.dp).clickable { model.tab = ZeronModel.Tab.Settings }, contentAlignment = Alignment.Center) {
                    // (A newer release shows as the badge beside the computer chip.)
                    ProfileMark(colors.text, Modifier.size(28.dp))
                }
            }
        }
        if (statsOpen) {
            val stats = HomeStats.of(model.workspace)
            // Everything finished while it was open: nothing left to list.
            LaunchedEffect(stats.idle) { if (stats.idle) statsOpen = false }
            if (!stats.idle) {
                AnchoredMenu(
                    colors,
                    statsAnchor,
                    title = null,
                    entries = homeStatsEntries(stats, colors) { id -> model.openSession(id) },
                    above = false,
                ) { statsOpen = false }
            }
        }
        if (menu) {
            // iOS optionsMenu() on the "ellipsis" bar item (AnchoredMenu's own
            // BackHandler closes it).
            AnchoredMenu(
                colors,
                menuAnchor,
                title = null,
                entries = buildList {
                    add(menuSection(stringResource(R.string.list_view)))
                    add(MenuEntry(stringResource(R.string.by_project), checked = mode == ZeronModel.ListMode.Project, icon = { c -> Glyph(Glyphs.Folder, 17.dp, c) }) { model.applyListMode(ZeronModel.ListMode.Project) })
                    add(MenuEntry(stringResource(R.string.by_activity), checked = mode == ZeronModel.ListMode.Activity, icon = { c -> Glyph(Glyphs.Recent, 17.dp, c) }) { model.applyListMode(ZeronModel.ListMode.Activity) })
                    add(menuSection())
                    add(
                        MenuEntry(stringResource(R.string.new_section_ellipsis), icon = { c -> Glyph(Glyphs.FolderPlus, 17.dp, c) }) {
                            onPrompt(Prompt(context.getString(R.string.new_section), "", context.getString(R.string.create)) { value -> if (value.isNotEmpty()) model.createSection(value) })
                        },
                    )
                    if (mode == ZeronModel.ListMode.Project) add(MenuEntry(stringResource(R.string.collapse_all), icon = { c -> Glyph(Glyphs.Tray, 17.dp, c) }) { model.collapseAll() })
                    add(MenuEntry(stringResource(R.string.archived), icon = { c -> Glyph(Glyphs.Archive, 17.dp, c) }) { model.openFolder("archived", context.getString(R.string.archived)) })
                },
                above = false,
            ) { menu = false }
        }
    }
    }
}

/**
 * One front-page group: Pinned, a user section, a project (with its [tile]:
 * name + colour index), or the headerless By Activity list. [menu] = the
 * header has a long-press menu (Pinned and sections).
 */
private class ListGroup(
    val id: String,
    val title: String?,
    val rows: List<SessionRow>,
    val section: SectionView? = null,
    val tile: Pair<String, Int>? = null,
    val menu: Boolean = true,
)

/** The Settings on/off switch (green when on). */
@Composable
private fun SettingSwitch(colors: ZeronColors, checked: Boolean, tag: String, onChange: (Boolean) -> Unit) {
    androidx.compose.material3.Switch(
        checked = checked,
        onCheckedChange = onChange,
        modifier = Modifier.testTag(tag),
        colors = androidx.compose.material3.SwitchDefaults.colors(
            checkedThumbColor = Color.White,
            checkedTrackColor = colors.success,
            checkedBorderColor = colors.success,
            uncheckedThumbColor = Color.White,
            uncheckedTrackColor = colors.controlFill,
            uncheckedBorderColor = colors.controlFill,
        ),
    )
}

/**
 * The mark after a group header's count: a dot while one of its sessions
 * waits for input, else nothing. Running sessions show their spinner on
 * their own rows only; a second one on the header was noise.
 */
internal fun groupHeaderMark(rows: List<SessionRow>, input: androidx.compose.ui.graphics.Color): MarkKind? =
    if (rows.any { it.indicator == ChatIndicator.AWAITING_INPUT }) MarkKind.Dot(input) else null

@Composable
private fun GroupHeader(
    colors: ZeronColors,
    title: String,
    count: Int,
    collapsed: Boolean,
    live: MarkKind?,
    onToggle: () -> Unit,
    onLongPress: ((androidx.compose.ui.geometry.Rect) -> Unit)?,
    tile: Pair<String, Int>? = null,
) {
    var bounds by remember { mutableStateOf(androidx.compose.ui.geometry.Rect.Zero) }
    // SectionHeaderCell: 40pt, semibold title + count, chevron on the right
    // that turns sideways when folded.
    Row(
        Modifier
            .fillMaxWidth()
            .height(40.dp)
            .padding(horizontal = 8.dp)
            .clip(RoundedCornerShape(12.dp))
            .onGloballyPositioned { bounds = it.boundsInRoot() }
            .combinedClickable(onClick = onToggle, onLongClick = onLongPress?.let { cb -> { cb(bounds) } })
            .padding(start = 12.dp, end = 12.dp, bottom = 6.dp),
        verticalAlignment = Alignment.Bottom,
    ) {
        // Title, count and live mark share one weighted row so the chevron
        // always sits at the right edge (two weights split the space and
        // left it floating mid-row).
        Row(Modifier.weight(1f), verticalAlignment = Alignment.Bottom) {
            val titleText = @Composable {
                Text(title, color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
            if (tile != null) {
                // Tile centred on the title's x-height, like iOS.
                TileBesideLabel(
                    fontSize = 13.5.sp,
                    gap = 7.dp,
                    tile = { ProjectTile(tile.first, tile.second, colors, 14.dp) },
                    modifier = Modifier.weight(1f, fill = false),
                    label = titleText,
                )
            } else {
                Box(Modifier.weight(1f, fill = false)) { titleText() }
            }
            Spacer(Modifier.width(7.dp))
            Text("$count", color = colors.tertiary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.5.sp)
            if (live != null) {
                Spacer(Modifier.width(7.dp))
                StatusMark(live, colors, Modifier.padding(bottom = 3.dp).size(12.dp))
            }
        }
        ChevronMark(colors.tertiary, Modifier.padding(bottom = 4.dp).size(12.dp).graphicsLayer { rotationZ = if (collapsed) -90f else 0f }, expanded = true)
    }
}

@Composable
private fun SessionRowView(
    row: SessionRow,
    colors: ZeronColors,
    archived: Boolean,
    model: ZeronModel,
    tapGuard: TapGuard? = null,
    modifier: Modifier = Modifier,
    dragHandle: Modifier? = null,
    elevated: Boolean = false,
    showPin: Boolean = false,
) {
    val swipe = LocalSwipe.current
    val menus = LocalRowMenus.current
    val corner = cornerOf(row, colors)
    val backdrop = colors.shell
    var bounds by remember { mutableStateOf(androidx.compose.ui.geometry.Rect.Zero) }
    val latestRow by rememberUpdatedState(row)
    // The snapshot's `timeLabel` goes stale; re-derive it against LocalNow.
    val timeLabel = RelativeTime.label(row.lastActivityMs, LocalNow.current, LocalContext.current.resources)
    // iOS: leading Pin/Unpin (accent); trailing Archive + Move (Archive is
    // the outermost, full-swipe action) or a lone Unarchive when archived.
    val leading = if (archived) null else SwipeAction(
        title = stringResource(if (row.pinned) R.string.unpin else R.string.pin),
        color = colors.accent,
        icon = { tint -> if (row.pinned) PinSlashGlyph(20.dp, tint) else Glyph(Glyphs.Pin, 20.dp, tint) },
        // The gesture outlives recompositions: act on the row's current state.
        onAction = { model.pin(latestRow.id, !latestRow.pinned) },
    )
    val trailing = if (archived) {
        listOf(SwipeAction(stringResource(R.string.unarchive), colors.accent, { tint -> Glyph(Glyphs.Unarchive, 20.dp, tint) }) { model.unarchive(latestRow.id) })
    } else {
        listOf(
            SwipeAction(stringResource(R.string.archive), colors.secondary, { tint -> Glyph(Glyphs.Archive, 20.dp, tint) }) { model.archive(latestRow.id) },
            SwipeAction(stringResource(R.string.move), Color(0xFF5E6AD2), { tint -> Glyph(Glyphs.Folder, 20.dp, tint) }) {
                menus.move = RowMenuTarget(latestRow.id, archived = false, anchor = bounds)
            },
        )
    }
    SwipeRow(
        id = if (archived) "arch-${row.id}" else row.id,
        height = if (archived) 46.dp else 62.dp,
        coordinator = swipe,
        leading = leading,
        trailing = trailing,
        modifier = modifier,
    ) { scope ->
        Row(
            Modifier
                .fillMaxSize()
                .then(scope.offset)
                .background(if (scope.revealed) backdrop else if (elevated) colors.elevated else Color.Transparent)
                .onGloballyPositioned { bounds = it.boundsInRoot() }
                .combinedClickable(
                    onClick = {
                        if (!scope.closeIfOpen() && tapGuard?.allowsTap() != false) model.openSession(row.id)
                    },
                    onLongClick = {
                        scope.closeIfOpen()
                        menus.row = RowMenuTarget(row.id, archived, bounds)
                    },
                )
                .then(scope.gesture)
                .padding(horizontal = 20.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            if (archived) {
                Text(
                    row.title,
                    color = colors.text.copy(alpha = 0.88f),
                    fontFamily = ZeronType.Sans,
                    fontWeight = FontWeight.Normal,
                    fontSize = 16.5.sp,
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                    modifier = Modifier.weight(1f),
                )
                Text(timeLabel, color = colors.time, fontFamily = ZeronType.Sans, fontSize = 13.sp)
            } else {
                // iOS SessionCell: the agent mark beside the title line;
                // line 1 is title + status-or-time, line 2 is tile + project ·
                // branch + PR badge.
                // 18dp of ink in the 20dp column, centred between the title's
                // cap and x-height centres (the line box centre read high).
                TitleLineMark(
                    row.harness ?: "claude-code",
                    colors,
                    fontSize = 16.5.sp,
                    weight = FontWeight.Medium,
                    lineTop = 10.dp,
                    lineHeight = 22.dp,
                    modifier = Modifier.align(Alignment.Top),
                )
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f).align(Alignment.Top).padding(top = 10.dp)) {
                    Row(Modifier.height(22.dp), verticalAlignment = Alignment.CenterVertically) {
                        Text(
                            row.title,
                            color = if (row.unseen || row.indicator != ChatIndicator.IDLE || row.sendState == SendState.FAILED) colors.text else colors.text.copy(alpha = 0.88f),
                            fontFamily = ZeronType.Sans,
                            fontWeight = if (row.unseen) FontWeight.SemiBold else FontWeight.Medium,
                            fontSize = 16.5.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        Spacer(Modifier.width(10.dp))
                        if (showPin) {
                            // By Activity has no Pinned group: a quiet pin marks them.
                            Glyph(Glyphs.Pin, 12.dp, colors.tertiary, Modifier.graphicsLayer { rotationZ = 30f })
                            Spacer(Modifier.width(4.dp))
                        }
                        // Icon-only status (no word beside it): the glyph
                        // carries the state, TalkBack reads the word. Live
                        // states show the glyph alone; outcomes keep the time.
                        if (corner != null) {
                            val word = stringResource(corner.word)
                            StatusMark(corner.mark, colors, Modifier.size(12.dp).testTag("row-status").semantics { contentDescription = word })
                        }
                        if (corner == null || corner.showTime) {
                            if (corner != null) Spacer(Modifier.width(6.dp))
                            Text(timeLabel, color = colors.time, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp)
                        }
                    }
                    Row(Modifier.padding(top = 3.dp).height(18.dp), verticalAlignment = Alignment.CenterVertically) {
                        val project = SessionGrouping.rowProjectLabel(row)
                        // Project-less sessions tile as "H" in the home tone and
                        // read "~", like their By Project group header.
                        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                            // Tile centred on the name's x-height, like iOS.
                            TileBesideLabel(
                                fontSize = 13.5.sp,
                                gap = 7.dp,
                                tile = {
                                    ProjectTile(
                                        name = row.project?.name ?: "Home",
                                        colorIndex = row.project?.colorIndex?.toInt() ?: model.homeColorIndex(),
                                        colors = colors,
                                        size = 14.dp,
                                    )
                                },
                            ) {
                                Text(project, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                            val branch = row.branch?.takeIf { it.isNotEmpty() }
                            if (branch != null) {
                                Spacer(Modifier.width(10.dp))
                                AssetIcon("tool-git-branch", 12.dp, colors.subline)
                                Spacer(Modifier.width(4.dp))
                                Text(
                                    branch,
                                    color = colors.subline,
                                    fontFamily = ZeronType.Sans,
                                    fontSize = 12.5.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.weight(1f),
                                )
                            }
                        }
                        row.pullRequest?.let { pr ->
                            Spacer(Modifier.width(10.dp))
                            PrBadge(pr, colors)
                        }
                    }
                }
                if (dragHandle != null) {
                    Box(
                        Modifier.fillMaxHeight().width(36.dp).then(dragHandle),
                        contentAlignment = Alignment.Center,
                    ) {
                        ReorderMark(colors.tertiary, Modifier.size(20.dp))
                    }
                }
            }
        }
    }
}

/** iOS `PRBadgeView`: tone @ 0.08 pill, the PR glyph and the bare number @ 0.85. */
@Composable
internal fun PrBadge(pr: PullRequest, colors: ZeronColors) {
    val tone = when (pr.state) {
        PullRequestState.MERGED -> colors.accent
        PullRequestState.CLOSED -> colors.danger
        else -> colors.success
    }
    Row(
        Modifier
            .height(18.dp)
            .clip(RoundedCornerShape(5.dp))
            .background(tone.copy(alpha = 0.08f))
            .padding(horizontal = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        PullRequestIcon(tone.copy(alpha = 0.85f), 11.dp)
        Spacer(Modifier.width(3.dp))
        Text(
            "${pr.number}",
            color = tone.copy(alpha = 0.85f),
            fontFamily = ZeronType.Mono,
            fontWeight = FontWeight.Medium,
            fontSize = 11.sp,
        )
    }
}

/** A row's status glyph, its TalkBack word, and whether the time stays beside it. */
internal data class Corner(@androidx.annotation.StringRes val word: Int, val mark: MarkKind, val showTime: Boolean)

private fun cornerOf(row: SessionRow, colors: ZeronColors): Corner? = statusCorner(row.sendState, row.indicator, row.lastOutcome, colors)

/**
 * Row status, icon-only: live states (the running dot-matrix, the input dot)
 * from the live `indicator`; otherwise how the last run ended from
 * `lastOutcome`, which the seen marker does not clear (a green check for
 * done, a red dot for failed; unseen rows are told apart by the bold title).
 * A chat that never ran shows just its time.
 */
internal fun statusCorner(sendState: SendState?, indicator: ChatIndicator, outcome: ChatIndicator, colors: ZeronColors): Corner? {
    if (sendState == SendState.FAILED) return Corner(R.string.status_failed, MarkKind.Dot(colors.danger), showTime = true)
    when (indicator) {
        ChatIndicator.WORKING -> return Corner(R.string.status_working, MarkKind.Spinner, showTime = false)
        ChatIndicator.AWAITING_INPUT -> return Corner(R.string.status_input, MarkKind.Dot(colors.input), showTime = false)
        else -> {}
    }
    return when (if (indicator == ChatIndicator.ERRORED) ChatIndicator.ERRORED else outcome) {
        ChatIndicator.ERRORED -> Corner(R.string.status_failed, MarkKind.Dot(colors.failed), showTime = true)
        ChatIndicator.COMPLETED -> Corner(R.string.status_done, MarkKind.Check(colors.done), showTime = true)
        else -> null
    }
}

@Composable
private fun FolderScreen(model: ZeronModel, colors: ZeronColors, folder: ZeronModel.Route.Folder) {
    val rows = model.sessionsIn(folder.id)
    val archived = folder.id == "archived"
    NowProvider {
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp)) {
            BackButton(colors, onClick = { model.back() })
            Spacer(Modifier.width(10.dp))
            Text(
                when (folder.id) {
                    "archived" -> stringResource(R.string.archived)
                    "pinned" -> stringResource(R.string.pinned)
                    else -> folder.title
                },
                color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        }
        if (rows.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.nothing_here), color = colors.secondary, fontFamily = ZeronType.Sans)
            }
        } else if (folder.id == "pinned") {
            // Pins are an ordered list: drag the grip to reorder (synced).
            PinnedRows(model, colors, rows, Modifier.weight(1f))
        } else {
            LazyColumn(Modifier.weight(1f)) {
                items(rows, key = { it.id }) { row ->
                    SessionRowView(row, colors, archived, model)
                }
            }
        }
    }
    }
}

/**
 * The Pinned folder's rows. Each carries a reorder grip on its trailing edge
 * that drags the row up or down, iOS-style — the row follows the finger,
 * lifted; neighbours shift as it passes (animateItem). On drop the new order
 * goes to the core via [ZeronModel.movePin]. Between drags [order] mirrors
 * the workspace's pin list.
 */
@Composable
private fun PinnedRows(model: ZeronModel, colors: ZeronColors, rows: List<SessionRow>, modifier: Modifier = Modifier) {
    val order = remember { mutableStateListOf<SessionRow>() }
    var dragIndex by remember { mutableIntStateOf(-1) }
    var dragOffset by remember { mutableFloatStateOf(0f) }
    var rowPx by remember { mutableFloatStateOf(1f) }
    var preDrag by remember { mutableStateOf<List<SessionRow>?>(null) }
    val latestRows by rememberUpdatedState(rows)
    LaunchedEffect(rows) {
        if (dragIndex < 0 && order.toList() != rows) {
            order.clear()
            order.addAll(rows)
        }
    }

    fun drop() {
        val pre = preDrag
        val i = dragIndex
        dragIndex = -1
        preDrag = null
        dragOffset = 0f
        val id = order.getOrNull(i)?.id
        if (pre != null && id != null && order.toList() != pre) {
            // Like iOS pinsReordered: the rows now above/below the dropped one.
            if (!model.movePin(id, order.getOrNull(i - 1)?.id, order.getOrNull(i + 1)?.id)) {
                order.clear()
                order.addAll(latestRows)
            }
        } else if (order.toList() != latestRows) {
            order.clear()
            order.addAll(latestRows)
        }
    }

    LazyColumn(modifier) {
        items(order, key = { it.id }) { row ->
            val dragging = dragIndex >= 0 && order.getOrNull(dragIndex)?.id == row.id
            SessionRowView(
                row,
                colors,
                archived = false,
                model = model,
                modifier = Modifier
                    .then(if (dragging) Modifier else Modifier.animateItem())
                    .zIndex(if (dragging) 1f else 0f)
                    .onSizeChanged { rowPx = it.height.toFloat().coerceAtLeast(1f) }
                    .graphicsLayer {
                        translationY = if (dragging) dragOffset else 0f
                        shadowElevation = if (dragging) 10.dp.toPx() else 0f
                    },
                elevated = dragging,
                dragHandle = Modifier
                    .pointerInput(Unit) {
                        // Taps and long-presses on the grip stay on the grip.
                        awaitEachGesture { awaitFirstDown(requireUnconsumed = false).consume() }
                    }
                    .pointerInput(row.id) {
                        detectVerticalDragGestures(
                            onDragStart = {
                                preDrag = order.toList()
                                dragIndex = order.indexOfFirst { it.id == row.id }
                                dragOffset = 0f
                            },
                            onVerticalDrag = { change, amount ->
                                change.consume()
                                dragOffset += amount
                                while (dragOffset > rowPx / 2f && dragIndex in 0 until order.lastIndex) {
                                    order.add(dragIndex + 1, order.removeAt(dragIndex))
                                    dragIndex++
                                    dragOffset -= rowPx
                                }
                                while (dragOffset < -rowPx / 2f && dragIndex > 0) {
                                    order.add(dragIndex - 1, order.removeAt(dragIndex))
                                    dragIndex--
                                    dragOffset += rowPx
                                }
                            },
                            onDragEnd = { drop() },
                            onDragCancel = { drop() },
                        )
                    },
            )
        }
    }
}

@Composable
private fun SearchScreen(model: ZeronModel, colors: ZeronColors) {
    val results = model.search(model.searchQuery)
    NowProvider {
    Column(Modifier.fillMaxSize().statusBarsPadding().padding(horizontal = 16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 6.dp, bottom = 12.dp)) {
            BackButton(colors, onClick = { model.tab = ZeronModel.Tab.Sessions })
            Spacer(Modifier.width(10.dp))
            Text(stringResource(R.string.search), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        }
        BasicTextField(
            value = model.searchQuery,
            onValueChange = { model.searchQuery = it },
            textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp),
            cursorBrush = SolidColor(colors.accent),
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.controlFill).padding(horizontal = 14.dp, vertical = 12.dp),
            decorationBox = { inner ->
                Box {
                    if (model.searchQuery.isEmpty()) Text(stringResource(R.string.search_placeholder), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 16.sp)
                    inner()
                }
            },
        )
        if (model.searchQuery.isNotBlank() && results.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text(stringResource(R.string.no_matches), color = colors.secondary, fontFamily = ZeronType.Sans)
            }
        } else {
            LazyColumn(Modifier.weight(1f).padding(top = 8.dp)) {
                items(results, key = { it.id }) { row ->
                    SessionRowView(row, colors, archived = row.archived, model = model)
                }
            }
        }
    }
    }
}

@Composable
private fun SettingsScreen(model: ZeronModel, colors: ZeronColors) {
    val context = LocalContext.current
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.GetContent()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return@rememberLauncherForActivityResult
        model.setWallpaper(bytes, uri.lastPathSegment ?: context.getString(R.string.wallpaper))
    }
    var effects by remember { mutableStateOf(false) }
    var themePicker by remember { mutableStateOf<Boolean?>(null) }
    var confirmOut by remember { mutableStateOf(false) }
    var newProject by remember { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxSize().statusBarsPadding().padding(horizontal = 16.dp)) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 8.dp, bottom = 12.dp)) {
                BackButton(colors, onClick = { model.tab = ZeronModel.Tab.Sessions })
                Spacer(Modifier.width(10.dp))
                Text(stringResource(R.string.settings), color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
            }
        }
        item { GroupLabel(colors, stringResource(R.string.settings_account)) }
        item {
            val conn = model.connectivity
            val sub = when {
                model.client?.isDemo() == true -> stringResource(R.string.settings_demo_sub)
                model.client?.isDirect() == true -> directSummary(model.directStatus).replaceFirstChar { it.uppercase() }
                else -> stringResource(R.string.signed_in_org, model.client?.orgId().orEmpty())
            }
            SettingRow(colors, stringResource(R.string.accounts_computers), "${model.activeTitle()} · $sub", onClick = { model.showMachines = true }, trailing = {
                val online = model.client?.isDemo() == true ||
                    (if (model.client?.isDirect() == true) model.directStatus?.phase == uniffi.zeron_core.DirectPhase.LIVE else conn?.state == uniffi.zeron_core.ConnectivityState.CONNECTED)
                Box(Modifier.size(8.dp).clip(CircleShape).background(if (online) colors.success else colors.tertiary))
            })
        }
        item { SettingRow(colors, stringResource(R.string.add_computer_ssh), stringResource(R.string.add_computer_ssh_sub), onClick = { model.editMachine = Machine() }) }
        if (model.client?.isDirect() == true) {
            item { SettingRow(colors, stringResource(R.string.connection_details), stringResource(R.string.connection_details_sub), onClick = { model.showLinkDetails = true }) }
        }
        item {
            SettingRow(
                colors,
                stringResource(R.string.auto_route),
                stringResource(if (model.autoRoute) R.string.auto_route_sub_on else R.string.auto_route_sub_off),
                onClick = { model.applyAutoRoute(!model.autoRoute) },
                trailing = {
                    Spacer(Modifier.width(10.dp))
                    SettingSwitch(colors, model.autoRoute, "auto-route") { model.applyAutoRoute(it) }
                },
            )
        }
        item { GroupLabel(colors, stringResource(R.string.settings_devices)) }
        val devices = model.workspace?.devices.orEmpty()
        if (devices.isEmpty()) {
            item { SettingRow(colors, stringResource(R.string.this_device), stringResource(R.string.no_other_hosts)) }
        } else {
            items(devices, key = { it.id }) { device ->
                SettingRow(colors, device.name, stringResource(if (device.online) R.string.online else R.string.offline), trailing = {
                    Box(Modifier.size(8.dp).clip(CircleShape).background(if (device.online) colors.success else colors.tertiary))
                })
            }
        }
        item { GroupLabel(colors, stringResource(R.string.settings_appearance)) }
        item {
            listOf(0 to R.string.appearance_system, 1 to R.string.appearance_light, 2 to R.string.appearance_dark).forEach { (mode, label) ->
                SettingRow(colors, stringResource(label), null, onClick = { model.applyAppearance(mode) }, trailing = {
                    if (model.appearance == mode) Text("✓", color = colors.accent, fontSize = 16.sp)
                })
            }
        }
        item { ThemeSettingRows(model, colors, onPick = { themePicker = it }) }
        item { GroupLabel(colors, stringResource(R.string.settings_language)) }
        item {
            // Per-app locale, applied in place (no activity recreation).
            AppLanguage.changes.intValue
            val current = AppLanguage.current(context)
            listOf(
                AppLanguage.SYSTEM to R.string.language_system,
                AppLanguage.ENGLISH to R.string.language_english,
                AppLanguage.CHINESE to R.string.language_chinese,
            ).forEach { (tag, label) ->
                SettingRow(colors, stringResource(label), null, onClick = { if (tag != current) AppLanguage.apply(context, tag) }, trailing = {
                    if (current == tag) Text("✓", color = colors.accent, fontSize = 16.sp)
                })
            }
        }
        item { GroupLabel(colors, stringResource(R.string.wallpaper)) }
        item {
            SettingRow(colors, stringResource(if (model.wallpaper != null) R.string.change_wallpaper else R.string.choose_wallpaper), if (model.wallpaper != null) model.wallpaperName() else stringResource(R.string.wallpaper_sub), onClick = { picker.launch("image/*") })
        }
        if (model.wallpaper != null) {
            item { SettingRow(colors, stringResource(R.string.wallpaper_effect_row), stringResource(ZeronModel.effectLabel(model.wallpaperEffect)), onClick = { effects = true }) }
            item { SettingRow(colors, stringResource(R.string.remove_wallpaper), null, destructive = true, onClick = { model.clearWallpaper() }) }
        }
        item { GroupLabel(colors, stringResource(R.string.sessions)) }
        item { SettingRow(colors, stringResource(R.string.search), stringResource(R.string.search_placeholder), onClick = { model.tab = ZeronModel.Tab.Search }) }
        item { SettingRow(colors, stringResource(R.string.archived_sessions), null, onClick = { model.tab = ZeronModel.Tab.Sessions; model.openFolder("archived", context.getString(R.string.archived)) }) }
        item { SettingRow(colors, stringResource(R.string.new_project), stringResource(R.string.new_project_sub), onClick = { newProject = true }) }
        item { GroupLabel(colors, stringResource(R.string.settings_about)) }
        item {
            val newer = model.updateRelease?.takeIf { it.newer }
            SettingRow(colors, stringResource(R.string.check_for_updates), newer?.let { stringResource(R.string.update_available, it.name) } ?: stringResource(R.string.version_build, sh.zeron.android.BuildConfig.VERSION_NAME, sh.zeron.android.BuildConfig.VERSION_CODE), onClick = { model.checkForUpdates() }, trailing = {
                if (newer != null) Box(Modifier.size(9.dp).clip(CircleShape).background(colors.accent))
            })
        }
        item {
            SettingRow(
                colors,
                stringResource(R.string.auto_update),
                stringResource(if (model.autoUpdate) R.string.auto_update_sub_on else R.string.auto_update_sub_off),
                onClick = { model.applyAutoUpdate(!model.autoUpdate) },
                trailing = {
                    Spacer(Modifier.width(10.dp))
                    SettingSwitch(colors, model.autoUpdate, "auto-update") { model.applyAutoUpdate(it) }
                },
            )
        }
        item {
            val n = model.crashLogs.size
            SettingRow(
                colors,
                stringResource(R.string.crash_logs),
                if (n == 0) stringResource(R.string.crash_logs_none) else androidx.compose.ui.res.pluralStringResource(R.plurals.crash_logs_count, n, n),
                onClick = { model.openCrashLogs() },
            )
        }
        item { Spacer(Modifier.height(18.dp)) }
        if (model.activeMachine == "cloud") {
            item { SettingRow(colors, stringResource(R.string.sign_out), stringResource(R.string.sign_out_sub), destructive = true, onClick = { confirmOut = true }) }
        }
        item { Spacer(Modifier.height(32.dp)) }
    }
    if (effects) {
        AlertDialog(
            onDismissRequest = { effects = false },
            title = { Text(stringResource(R.string.wallpaper_effect_title)) },
            text = {
                Column {
                    WallpaperEffect.entries.forEach { effect ->
                        Text(
                            stringResource(ZeronModel.effectLabel(effect)),
                            color = if (effect == model.wallpaperEffect) colors.accent else colors.text,
                            modifier = Modifier.fillMaxWidth().clickable {
                                model.applyWallpaperEffect(effect)
                                effects = false
                            }.padding(vertical = 10.dp),
                            fontFamily = ZeronType.Sans,
                        )
                    }
                }
            },
            confirmButton = { TextButton(onClick = { effects = false }) { Text(stringResource(R.string.close)) } },
        )
    }
    if (confirmOut) {
        AlertDialog(
            onDismissRequest = { confirmOut = false },
            title = { Text(stringResource(R.string.sign_out_confirm)) },
            text = { Text(stringResource(R.string.sign_out_sub)) },
            confirmButton = { TextButton(onClick = { confirmOut = false; model.signOut() }) { Text(stringResource(R.string.sign_out)) } },
            dismissButton = { TextButton(onClick = { confirmOut = false }) { Text(stringResource(R.string.cancel)) } },
        )
    }
    themePicker?.let { dark -> ThemePickerSheet(model, colors, dark, onDismiss = { themePicker = null }) }
    if (newProject) {
        Box(Modifier.fillMaxSize().background(colors.background)) {
            NewProjectScreen(model, onClose = { newProject = false }, onCreated = { id, name ->
                newProject = false
                model.showToast(context.getString(R.string.project_added, name))
                model.newSessionProject = id
                model.showNewSession = true
            })
        }
    }
}

@Composable
internal fun GroupLabel(colors: ZeronColors, text: String) {
    Text(text, color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp, modifier = Modifier.padding(top = 18.dp, bottom = 6.dp, start = 4.dp))
}

@Composable
internal fun SettingRow(
    colors: ZeronColors,
    title: String,
    subtitle: String?,
    destructive: Boolean = false,
    onClick: (() -> Unit)? = null,
    trailing: @Composable () -> Unit = {},
) {
    Row(
        Modifier.fillMaxWidth().padding(vertical = 3.dp).clip(RoundedCornerShape(14.dp)).background(colors.elevated).then(if (onClick != null) Modifier.clickable(onClick = onClick) else Modifier).padding(horizontal = 14.dp, vertical = 12.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Column(Modifier.weight(1f)) {
            Text(title, color = if (destructive) colors.danger else colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp)
            if (!subtitle.isNullOrBlank()) Text(subtitle, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
        }
        trailing()
    }
}

