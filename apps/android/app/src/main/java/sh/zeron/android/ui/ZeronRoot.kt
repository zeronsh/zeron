@file:OptIn(
    androidx.compose.foundation.ExperimentalFoundationApi::class,
    androidx.compose.material3.ExperimentalMaterial3Api::class,
)

package sh.zeron.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.combinedClickable
import androidx.compose.ui.input.pointer.changedToUp
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
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
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
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
import androidx.compose.foundation.isSystemInDarkTheme
import androidx.compose.runtime.SideEffect
import androidx.compose.ui.platform.LocalView
import androidx.core.view.WindowCompat
import kotlinx.coroutines.launch
import sh.zeron.android.core.ZeronModel
import androidx.compose.material3.HorizontalDivider
import sh.zeron.android.design.AnchoredMenu
import sh.zeron.android.design.AssetIcon
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.PinSlashGlyph
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import sh.zeron.android.design.BackChevron
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.ChevronMark
import sh.zeron.android.design.FolderPlusMark
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.MarkKind
import sh.zeron.android.design.PlusMark
import sh.zeron.android.design.ProfileMark
import sh.zeron.android.design.StatusMark
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronDark
import sh.zeron.android.design.ZeronLight
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ProjectView
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
    val colors = if (dark) ZeronDark else ZeronLight
    val view = LocalView.current
    SideEffect {
        val window = (view.context as? android.app.Activity)?.window ?: return@SideEffect
        val controller = WindowCompat.getInsetsController(window, view)
        controller.isAppearanceLightStatusBars = !dark
        controller.isAppearanceLightNavigationBars = !dark
    }
    androidx.compose.runtime.CompositionLocalProvider(LocalZeronColors provides colors) {
        sh.zeron.android.design.ZeronMaterialTheme(colors) { AppContent(model, colors) }
    }
}

@Composable
private fun AppContent(model: ZeronModel, colors: ZeronColors) {
    val phase = model.phase
    when {
        phase is ZeronModel.Phase.Loading -> CenterMessage(colors, "Opening the workspace…")
        phase is ZeronModel.Phase.Failed -> Column(
            Modifier.fillMaxSize().background(colors.background).padding(24.dp),
            verticalArrangement = Arrangement.Center,
            horizontalAlignment = Alignment.CenterHorizontally,
        ) {
            Text(phase.message, color = colors.danger, fontFamily = ZeronType.Sans)
            Spacer(Modifier.height(16.dp))
            Text("Try the demo", color = colors.background, modifier = Modifier.clip(RoundedCornerShape(20.dp)).background(colors.text).clickable { model.enterDemo() }.padding(horizontal = 18.dp, vertical = 12.dp))
            Spacer(Modifier.height(10.dp))
            Text("Machines…", color = colors.text, modifier = Modifier.clip(RoundedCornerShape(20.dp)).background(colors.controlFill).clickable { model.showMachines = true }.padding(horizontal = 18.dp, vertical = 12.dp))
        }
        phase is ZeronModel.Phase.SignedOut || model.showSignIn -> SignInScreen(model)
        else -> Shell(model, colors)
    }
    if (model.showMachines) {
        BackHandler { if (!model.back()) model.showMachines = false }
        MachinesScreen(model)
    }
    model.editMachine?.let { machine ->
        BackHandler { model.editMachine = null }
        androidx.compose.runtime.key(machine.id) { MachineEditScreen(model, machine) }
    }
    if (model.showLinkDetails) {
        BackHandler { model.showLinkDetails = false }
        LinkDetailsScreen(model)
    }
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
                        "Undo",
                        color = if (colors.dark) Color(0xFF5B43E8) else Color(0xFFB4A8FF),
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
    val page = if (frontPage && colors.dark) SessionsBackdrop else colors.background
    BackHandler(enabled = model.showNewSession || model.showSignIn || stack.isNotEmpty() || model.tab != ZeronModel.Tab.Sessions) {
        if (model.showNewSession || model.showSignIn || stack.isNotEmpty()) model.back()
        else model.tab = ZeronModel.Tab.Sessions
    }
    var prompt by remember { mutableStateOf<Prompt?>(null) }
    var newProject by remember { mutableStateOf(false) }
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
        when {
            inSession -> {
                val id = (top as ZeronModel.Route.Session).id
                // A launch intent can swap chats without leaving composition; key the
                // screen so the transcript view is rebuilt on the new engine.
                androidx.compose.runtime.key(id) { SessionScreen(model, id) }
            }
            top is ZeronModel.Route.Folder -> FolderScreen(model, colors, top)
            model.tab == ZeronModel.Tab.Settings -> SettingsScreen(model, colors)
            model.tab == ZeronModel.Tab.Search -> SearchScreen(model, colors)
            else -> SessionsScreen(model, colors, onNewSpace = { newProject = true })
        }
        if (model.showNewSession) {
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.35f))) {
                NewSessionSheet(model, onDismiss = { model.showNewSession = false })
            }
        }
        if (newProject) {
            Box(Modifier.fillMaxSize().background(colors.background)) {
                NewProjectScreen(model, onClose = { newProject = false }, onCreated = { id, name ->
                    newProject = false
                    model.showToast("Added $name")
                    model.newSessionProject = id
                    model.showNewSession = true
                })
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
        val rename = MenuEntry("Rename…", icon = { c -> Glyph(Glyphs.Rename, 17.dp, c) }) {
            onPrompt(Prompt("Rename", row.title, "Rename") { value -> if (value.isNotEmpty()) model.rename(row.id, value) })
        }
        val entries = if (target.archived || row.archived) {
            listOf(
                MenuEntry("Unarchive", icon = { c -> Glyph(Glyphs.Unarchive, 18.dp, c) }) { model.unarchive(row.id) },
                rename,
            )
        } else {
            buildList {
                add(
                    MenuEntry(if (row.pinned) "Unpin" else "Pin", icon = { c -> if (row.pinned) PinSlashGlyph(17.dp, c) else Glyph(Glyphs.Pin, 17.dp, c) }) {
                        model.pin(row.id, !row.pinned)
                    },
                )
                sections.filter { it.id != row.sectionId }.forEach { section ->
                    add(MenuEntry("Move to ${section.name}", icon = { c -> Glyph(Glyphs.Folder, 17.dp, c) }) { model.move(row.id, section.id) })
                }
                if (row.sectionId != null) add(MenuEntry("No Section", icon = { c -> Glyph(Glyphs.Tray, 17.dp, c) }) { model.move(row.id, null) })
                add(
                    MenuEntry("New Section…", icon = { c -> Glyph(Glyphs.FolderPlus, 17.dp, c) }) {
                        onPrompt(Prompt("New Section", "", "Create") { value -> if (value.isNotEmpty()) model.createSection(value) })
                    },
                )
                add(rename)
                add(MenuEntry("Archive", destructive = true, icon = { c -> Glyph(Glyphs.Archive, 17.dp, c) }) { model.archive(row.id) })
            }
        }
        AnchoredMenu(colors, target.anchor, title = null, entries = entries, above = target.anchor.center.y > screenHeight * 0.55f) { menus.row = null }
    }
    menus.header?.let { (section, anchor) ->
        val entries = listOf(
            MenuEntry("Rename Section…", icon = { c -> Glyph(Glyphs.Rename, 17.dp, c) }) {
                onPrompt(Prompt("Rename Section", section.name, "Rename") { value -> if (value.isNotEmpty()) model.renameSection(section.id, value) })
            },
            MenuEntry("Delete Section", destructive = true, icon = { c -> Glyph(Glyphs.Archive, 17.dp, c) }) { model.deleteSection(section.id) },
        )
        AnchoredMenu(colors, anchor, title = section.name, entries = entries, above = anchor.center.y > screenHeight * 0.55f) { menus.header = null }
    }
}

private val SessionsBackdrop = Color(0xFF0D0D0D)

/** The list's floating header row: 6 + 44 (capsules) + 6. */
private val ListHeaderHeight = 56.dp

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
            BasicTextField(
                value = text,
                onValueChange = { text = it },
                textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp),
                cursorBrush = SolidColor(colors.accent),
                modifier = Modifier.fillMaxWidth().padding(vertical = 8.dp),
            )
        },
        confirmButton = { TextButton(onClick = { onConfirm(text.trim()) }) { Text(prompt.confirm) } },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

@Composable
private fun SessionsScreen(model: ZeronModel, colors: ZeronColors, onNewSpace: () -> Unit) {
    val workspace = model.workspace
    val front = workspace?.front
    var spaceId by remember { mutableStateOf<String?>(null) }
    var spaceMenu by remember { mutableStateOf(false) }
    var archivedOpen by remember { mutableStateOf(true) }
    var refreshing by remember { mutableStateOf(false) }
    LaunchedEffect(model.pendingSpaceMenu) {
        if (model.pendingSpaceMenu) {
            spaceMenu = true
            model.pendingSpaceMenu = false
        }
    }
    val projects = workspace?.projects.orEmpty()
    // iOS space filter: only spaces that have active sessions, most recent
    // activity first (the frame lists zeron, then edge). A space with no
    // live sessions (all archived, or never used) is left out; the selected
    // space always stays so the checkmark has a row.
    val menuProjects = remember(projects, spaceId) {
        projects
            .filter { it.sessions.isNotEmpty() || it.id == spaceId }
            .sortedByDescending { project -> project.sessions.maxOfOrNull { it.lastActivityMs } ?: 0L }
    }
    val space = projects.firstOrNull { it.id == spaceId }
    // Keyed on the snapshot itself: ZeronModel only swaps `workspace` when its
    // content changed, so streaming updates elsewhere do not rebuild the list.
    // iOS front page: Pinned and the user's sections as foldable groups, then
    // everything else under Recent (headed only when a group precedes it).
    val groups = remember(front, spaceId, projects) {
        val seen = HashSet<String>()
        fun fresh(list: List<SessionRow>) = list.filter { seen.add(it.id) }
        if (spaceId != null) {
            listOf(ListGroup("space", null, fresh(projects.firstOrNull { it.id == spaceId }?.sessions.orEmpty())))
        } else {
            val out = ArrayList<ListGroup>()
            val pinned = fresh(front?.pinned.orEmpty())
            if (pinned.isNotEmpty()) out.add(ListGroup("pinned", "Pinned", pinned))
            front?.sections.orEmpty().forEach { section -> out.add(ListGroup(section.id, section.name, fresh(section.sessions), section = section)) }
            val recent = fresh(front?.recent.orEmpty())
            out.add(ListGroup("recent", if (out.isEmpty()) null else "Recent", recent))
            out
        }
    }
    val rows = remember(groups) { groups.flatMap { it.rows } }
    val archived = remember(workspace, spaceId) {
        workspace?.archived.orEmpty().filter { spaceId == null || it.project?.id == spaceId }
    }
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
    Box(Modifier.fillMaxSize()) {
        val pullState = androidx.compose.material3.pulltorefresh.rememberPullToRefreshState()
        val headerBottom = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + ListHeaderHeight
        PullToRefreshBox(isRefreshing = refreshing, onRefresh = {
            refreshing = true
            model.refreshPull()
            refreshing = false
        }, modifier = Modifier.fillMaxSize(), state = pullState, indicator = {
            // Below the solid header band, or it would pull out of sight.
            androidx.compose.material3.pulltorefresh.PullToRefreshDefaults.Indicator(
                state = pullState,
                isRefreshing = refreshing,
                modifier = Modifier.align(Alignment.TopCenter).padding(top = headerBottom),
            )
        }) {
            if (rows.isEmpty() && archived.isEmpty()) {
                val topInset = WindowInsets.statusBars.asPaddingValues().calculateTopPadding()
                Column(Modifier.fillMaxSize().verticalScroll(rememberScrollState()).padding(top = topInset + ListHeaderHeight + ListHeaderGap)) {
                    DirectBanner(model, colors)
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
                    groups.forEach { group ->
                        val folded = group.title != null && model.isCollapsed(group.id)
                        if (group.title != null) {
                            item(key = "head-${group.id}", contentType = "head") {
                                GroupHeader(
                                    colors,
                                    title = group.title,
                                    count = group.rows.size,
                                    collapsed = folded,
                                    working = folded && group.rows.any { it.indicator == ChatIndicator.WORKING },
                                    onToggle = { model.toggleCollapsed(group.id) },
                                    onLongPress = group.section?.let { section -> { rect: androidx.compose.ui.geometry.Rect -> menus.header = section to rect } },
                                )
                            }
                        }
                        if (!folded) {
                            items(group.rows, key = { it.id }, contentType = { "row" }) { row ->
                                SessionRowView(row, colors, archived = false, model = model, tapGuard = guard)
                            }
                        }
                    }
                    if (archived.isNotEmpty()) {
                        item(key = "archived-head") {
                            Row(
                                Modifier.fillMaxWidth().clickable { archivedOpen = !archivedOpen }.padding(start = 20.dp, end = 16.dp, top = 22.dp, bottom = 8.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Text("Archived", color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 13.sp)
                                Spacer(Modifier.weight(1f))
                                ChevronMark(colors.tertiary, Modifier.size(12.dp), expanded = archivedOpen)
                            }
                        }
                        if (archivedOpen) {
                            items(archived, key = { "arch-${it.id}" }) { row ->
                                SessionRowView(row, colors, archived = true, model = model, tapGuard = guard)
                            }
                        }
                    }
                }
            }
        }
        // Solid band behind the status bar and the floating capsules, then a
        // fade: rows scrolling up dissolve a gap below the header instead of
        // running under "All" and the buttons (like the chat's top edge).
        val band = if (colors.dark) SessionsBackdrop else colors.background
        Column(Modifier.fillMaxWidth()) {
            Box(Modifier.fillMaxWidth().height(headerBottom).background(band))
            Box(Modifier.fillMaxWidth().height(ListHeaderGap).background(Brush.verticalGradient(listOf(band, band.copy(alpha = 0f)))))
        }
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 16.dp, vertical = 6.dp),
            verticalAlignment = Alignment.CenterVertically,
            horizontalArrangement = Arrangement.SpaceBetween,
        ) {
            Row(
                Modifier.height(44.dp).glassSurface(colors, 22.dp).clickable { spaceMenu = true }.padding(horizontal = 14.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(space?.name ?: "All", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.sp, maxLines = 1)
                Spacer(Modifier.width(6.dp))
                ChevronMark(colors.text, Modifier.size(11.dp), expanded = true)
            }
            Row(
                Modifier.height(44.dp).glassSurface(colors, 22.dp).padding(horizontal = 4.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Box(Modifier.size(40.dp).clickable { model.showNewSession = true }, contentAlignment = Alignment.Center) {
                    PlusMark(colors.text, Modifier.size(18.dp))
                }
                Box(Modifier.size(36.dp).clickable { model.tab = ZeronModel.Tab.Settings }, contentAlignment = Alignment.Center) {
                    ProfileMark(colors.text, Modifier.size(28.dp))
                }
            }
        }
        if (spaceMenu) {
            // iOS: no dimming scrim. The glass menu grows out of the "All"
            // capsule and covers it (same top, slightly further left).
            val none = remember { MutableInteractionSource() }
            Box(Modifier.fillMaxSize().clickable(interactionSource = none, indication = null) { spaceMenu = false }) {
                Column(
                    Modifier
                        .statusBarsPadding()
                        .padding(start = 9.dp, top = 6.dp)
                        .width(247.dp)
                        .glassSurface(colors, 26.dp)
                        // Measured from mobile-polish/project-menu.png (@3x): 247pt
                        // wide, check at 27pt, titles at 59pt inside the panel. Denser wash
                        // stands in for UIMenu's heavy blur.
                        .background(if (colors.dark) Color(0xFF232325).copy(alpha = 0.86f) else Color(0xFFF7F7F8).copy(alpha = 0.86f))
                        .padding(vertical = 8.dp),
                ) {
                    SpaceChoice(colors, "All", null, selected = spaceId == null) {
                        spaceId = null
                        spaceMenu = false
                    }
                    menuProjects.forEach { project ->
                        SpaceChoice(colors, project.name, projectSubtitle(project), selected = spaceId == project.id) {
                            spaceId = project.id
                            spaceMenu = false
                        }
                    }
                    HorizontalDivider(Modifier.padding(start = 23.dp, end = 21.dp, top = 4.dp, bottom = 4.dp), color = colors.text.copy(alpha = 0.14f))
                    Row(
                        Modifier.fillMaxWidth().height(52.dp).clickable {
                            spaceMenu = false
                            onNewSpace()
                        }.padding(start = 25.dp, end = 14.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        FolderPlusMark(colors.text, Modifier.size(22.dp))
                        Spacer(Modifier.width(12.dp))
                        Text("New space…", color = colors.text, fontFamily = ZeronType.Sans, fontSize = 17.sp)
                    }
                }
            }
        }
    }
}

@Composable
private fun SpaceChoice(
    colors: ZeronColors,
    title: String,
    subtitle: String?,
    selected: Boolean,
    onClick: () -> Unit,
) {
    val twoLine = !subtitle.isNullOrBlank()
    Row(
        Modifier.fillMaxWidth().height(if (twoLine) 58.dp else 48.dp).clickable(onClick = onClick).padding(start = 27.dp, end = 14.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.width(32.dp), contentAlignment = Alignment.CenterStart) {
            if (selected) sh.zeron.android.design.CheckGlyph(colors.text, Modifier.size(16.dp))
        }
        Column(Modifier.weight(1f)) {
            Text(title, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 17.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            if (twoLine) {
                Text(subtitle, color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
            }
        }
    }
}

private fun projectSubtitle(project: ProjectView): String {
    val host = project.deviceName ?: "This device"
    return if (project.deviceOnline) "@ $host" else "@ $host · offline"
}

/** One front-page group: Pinned, a user section, or Recent (headerless alone). */
private class ListGroup(val id: String, val title: String?, val rows: List<SessionRow>, val section: SectionView? = null)

@Composable
private fun GroupHeader(
    colors: ZeronColors,
    title: String,
    count: Int,
    collapsed: Boolean,
    working: Boolean,
    onToggle: () -> Unit,
    onLongPress: ((androidx.compose.ui.geometry.Rect) -> Unit)?,
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
        Text(title, color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 13.5.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
        Spacer(Modifier.width(7.dp))
        Text("$count", color = colors.tertiary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.5.sp)
        if (working) {
            Spacer(Modifier.width(7.dp))
            StatusMark(MarkKind.Spinner, colors, Modifier.padding(bottom = 3.dp).size(12.dp))
        }
        Spacer(Modifier.weight(1f))
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
) {
    val swipe = LocalSwipe.current
    val menus = LocalRowMenus.current
    val corner = cornerOf(row, colors)
    val backdrop = if (colors.dark) SessionsBackdrop else colors.background
    var bounds by remember { mutableStateOf(androidx.compose.ui.geometry.Rect.Zero) }
    val latestRow by rememberUpdatedState(row)
    // iOS: leading Pin/Unpin (accent), trailing Archive (red) or Unarchive.
    val leading = if (archived) null else SwipeAction(
        title = if (row.pinned) "Unpin" else "Pin",
        color = colors.accent,
        icon = { tint -> if (row.pinned) PinSlashGlyph(20.dp, tint) else Glyph(Glyphs.Pin, 20.dp, tint) },
        // The gesture outlives recompositions: act on the row's current state.
        onAction = { model.pin(latestRow.id, !latestRow.pinned) },
    )
    val trailing = if (archived) {
        SwipeAction("Unarchive", colors.accent, { tint -> Glyph(Glyphs.Unarchive, 20.dp, tint) }) { model.unarchive(latestRow.id) }
    } else {
        SwipeAction("Archive", colors.danger, { tint -> Glyph(Glyphs.Archive, 20.dp, tint) }) { model.archive(latestRow.id) }
    }
    SwipeRow(
        id = if (archived) "arch-${row.id}" else row.id,
        height = if (archived) 46.dp else 74.dp,
        coordinator = swipe,
        leading = leading,
        trailing = trailing,
    ) { scope ->
        Row(
            Modifier
                .fillMaxSize()
                .then(scope.offset)
                .background(if (scope.revealed) backdrop else Color.Transparent)
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
                Text(row.timeLabel, color = colors.time, fontFamily = ZeronType.Sans, fontSize = 13.sp)
            } else {
                Column(Modifier.weight(1f), verticalArrangement = Arrangement.Center) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        val project = row.project?.name ?: "No project"
                        val host = row.deviceName
                        Text(
                            if (host.isNullOrBlank()) project else "$project @ $host",
                            color = colors.secondary,
                            fontFamily = ZeronType.Sans,
                            fontSize = 13.5.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            modifier = Modifier.weight(1f),
                        )
                        if (row.pinned) {
                            // Pinned rows carry a quiet pin beside the time.
                            Glyph(Glyphs.Pin, 12.dp, colors.tertiary, Modifier.graphicsLayer { rotationZ = 30f })
                            Spacer(Modifier.width(4.dp))
                        }
                        if (corner != null) {
                            StatusMark(corner.mark, colors, Modifier.size(12.dp))
                            Spacer(Modifier.width(4.dp))
                            Text(corner.word, color = corner.color, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp)
                        } else {
                            Text(row.timeLabel, color = colors.time, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp)
                        }
                    }
                    Text(
                        row.title,
                        color = colors.text.copy(alpha = if (row.unseen || corner != null) 1f else 0.88f),
                        fontFamily = ZeronType.Sans,
                        fontWeight = FontWeight.Normal,
                        fontSize = 16.5.sp,
                        lineHeight = 20.sp,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        BrandMark(row.harness, colors, 14.dp)
                        val branch = row.branch?.takeIf { it.isNotEmpty() }
                        if (branch != null) {
                            Spacer(Modifier.width(6.dp))
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
                        } else {
                            Spacer(Modifier.weight(1f))
                        }
                        row.pullRequest?.let { pr ->
                            Spacer(Modifier.width(8.dp))
                            val tone = when (pr.state) {
                                PullRequestState.MERGED -> colors.accent
                                PullRequestState.CLOSED -> colors.danger
                                else -> colors.success
                            }
                            Text(
                                "#${pr.number}",
                                color = tone.copy(alpha = 0.9f),
                                fontFamily = ZeronType.Mono,
                                fontSize = 11.sp,
                                modifier = Modifier.clip(RoundedCornerShape(5.dp)).background(tone.copy(alpha = 0.14f)).padding(horizontal = 5.dp, vertical = 1.dp),
                            )
                        }
                    }
                }
            }
        }
    }
}

private data class Corner(val word: String, val color: Color, val mark: MarkKind)

private fun cornerOf(row: SessionRow, colors: ZeronColors): Corner? = statusCorner(row.sendState, row.indicator, colors, unseen = row.unseen)

private fun statusCorner(sendState: SendState?, indicator: ChatIndicator, colors: ZeronColors, unseen: Boolean = false): Corner? {
    if (sendState == SendState.FAILED) return Corner("Failed", colors.danger, MarkKind.Dot(colors.danger))
    return when (indicator) {
        ChatIndicator.WORKING -> Corner("Working", colors.working, MarkKind.Spinner)
        ChatIndicator.AWAITING_INPUT -> Corner("Input", colors.input, MarkKind.Dot(colors.input))
        ChatIndicator.ERRORED -> Corner("Failed", colors.failed, MarkKind.Dot(colors.failed))
        ChatIndicator.COMPLETED -> if (unseen) Corner("Done", colors.done, MarkKind.Check(colors.done)) else null
        ChatIndicator.IDLE -> null
    }
}

@Composable
private fun FolderScreen(model: ZeronModel, colors: ZeronColors, folder: ZeronModel.Route.Folder) {
    val rows = model.sessionsIn(folder.id)
    val archived = folder.id == "archived"
    Column(Modifier.fillMaxSize().statusBarsPadding()) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp)) {
            Box(
                Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.back() },
                contentAlignment = Alignment.Center,
            ) { BackChevron(colors.text, Modifier.size(18.dp)) }
            Spacer(Modifier.width(10.dp))
            Text(folder.title, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        }
        if (rows.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("Nothing here", color = colors.secondary, fontFamily = ZeronType.Sans)
            }
        } else {
            LazyColumn(Modifier.weight(1f)) {
                items(rows, key = { it.id }) { row ->
                    SessionRowView(row, colors, archived, model)
                }
            }
        }
    }
}

@Composable
private fun SearchScreen(model: ZeronModel, colors: ZeronColors) {
    val results = model.search(model.searchQuery)
    Column(Modifier.fillMaxSize().statusBarsPadding().padding(horizontal = 16.dp)) {
        Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 6.dp, bottom = 12.dp)) {
            Box(
                Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.tab = ZeronModel.Tab.Sessions },
                contentAlignment = Alignment.Center,
            ) { BackChevron(colors.text, Modifier.size(18.dp)) }
            Spacer(Modifier.width(10.dp))
            Text("Search", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        }
        BasicTextField(
            value = model.searchQuery,
            onValueChange = { model.searchQuery = it },
            textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp),
            cursorBrush = SolidColor(colors.accent),
            modifier = Modifier.fillMaxWidth().clip(RoundedCornerShape(14.dp)).background(colors.controlFill).padding(horizontal = 14.dp, vertical = 12.dp),
            decorationBox = { inner ->
                Box {
                    if (model.searchQuery.isEmpty()) Text("Sessions, projects, messages", color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 16.sp)
                    inner()
                }
            },
        )
        if (model.searchQuery.isNotBlank() && results.isEmpty()) {
            Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) {
                Text("No matches", color = colors.secondary, fontFamily = ZeronType.Sans)
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

@Composable
private fun SettingsScreen(model: ZeronModel, colors: ZeronColors) {
    val context = LocalContext.current
    val picker = androidx.activity.compose.rememberLauncherForActivityResult(androidx.activity.result.contract.ActivityResultContracts.GetContent()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return@rememberLauncherForActivityResult
        model.setWallpaper(bytes, uri.lastPathSegment ?: "Wallpaper")
    }
    var effects by remember { mutableStateOf(false) }
    var confirmOut by remember { mutableStateOf(false) }
    var newProject by remember { mutableStateOf(false) }
    LazyColumn(Modifier.fillMaxSize().statusBarsPadding().padding(horizontal = 16.dp)) {
        item {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.padding(top = 8.dp, bottom = 12.dp)) {
                Box(
                    Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.tab = ZeronModel.Tab.Sessions },
                    contentAlignment = Alignment.Center,
                ) { BackChevron(colors.text, Modifier.size(18.dp)) }
                Spacer(Modifier.width(10.dp))
                Text("Settings", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
            }
        }
        item { GroupLabel(colors, "Machine") }
        item {
            val conn = model.connectivity
            val sub = when {
                model.client?.isDemo() == true -> "Offline workspace · tap to switch"
                model.client?.isDirect() == true -> directSummary(model.directStatus).replaceFirstChar { it.uppercase() }
                else -> "Signed in · ${model.client?.orgId()}"
            }
            SettingRow(colors, model.activeTitle(), sub, onClick = { model.showMachines = true }, trailing = {
                val online = model.client?.isDemo() == true ||
                    (if (model.client?.isDirect() == true) model.directStatus?.phase == uniffi.zeron_core.DirectPhase.LIVE else conn?.state == uniffi.zeron_core.ConnectivityState.CONNECTED)
                Box(Modifier.size(8.dp).clip(CircleShape).background(if (online) colors.success else colors.tertiary))
            })
        }
        if (model.client?.isDirect() == true) {
            item { SettingRow(colors, "Connection Details", "Link state, engine version, streams and log", onClick = { model.showLinkDetails = true }) }
        }
        item { GroupLabel(colors, "Devices") }
        val devices = model.workspace?.devices.orEmpty()
        if (devices.isEmpty()) {
            item { SettingRow(colors, "This device", "No other hosts in the demo") }
        } else {
            items(devices, key = { it.id }) { device ->
                SettingRow(colors, device.name, if (device.online) "Online" else "Offline", trailing = {
                    Box(Modifier.size(8.dp).clip(CircleShape).background(if (device.online) colors.success else colors.tertiary))
                })
            }
        }
        item { GroupLabel(colors, "Appearance") }
        item {
            listOf(0 to "System", 1 to "Light", 2 to "Dark").forEach { (mode, label) ->
                SettingRow(colors, label, null, onClick = { model.applyAppearance(mode) }, trailing = {
                    if (model.appearance == mode) Text("✓", color = colors.accent, fontSize = 16.sp)
                })
            }
        }
        item { GroupLabel(colors, "Wallpaper") }
        item {
            SettingRow(colors, if (model.wallpaper != null) "Change Wallpaper…" else "Choose Wallpaper…", if (model.wallpaper != null) model.wallpaperName() else "Shown behind new chats and the sessions list", onClick = { picker.launch("image/*") })
        }
        if (model.wallpaper != null) {
            item { SettingRow(colors, "Effect", ZeronModel.effectLabel(model.wallpaperEffect), onClick = { effects = true }) }
            item { SettingRow(colors, "Remove Wallpaper", null, destructive = true, onClick = { model.clearWallpaper() }) }
        }
        item { GroupLabel(colors, "Sessions") }
        item { SettingRow(colors, "Search", "Sessions, projects, messages", onClick = { model.tab = ZeronModel.Tab.Search }) }
        item { SettingRow(colors, "Archived Sessions", null, onClick = { model.tab = ZeronModel.Tab.Sessions; model.openFolder("archived", "Archived") }) }
        item { SettingRow(colors, "New Project", "Browse a host folder", onClick = { newProject = true }) }
        item { GroupLabel(colors, "About") }
        item { SettingRow(colors, "Version", sh.zeron.android.BuildConfig.VERSION_NAME) }
        item { Spacer(Modifier.height(18.dp)) }
        if (model.activeMachine == "cloud") {
            item { SettingRow(colors, "Sign Out", "Local drafts stay on this device.", destructive = true, onClick = { confirmOut = true }) }
        }
        item { Spacer(Modifier.height(32.dp)) }
    }
    if (effects) {
        AlertDialog(
            onDismissRequest = { effects = false },
            title = { Text("Wallpaper Effect") },
            text = {
                Column {
                    WallpaperEffect.entries.forEach { effect ->
                        Text(
                            ZeronModel.effectLabel(effect),
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
            confirmButton = { TextButton(onClick = { effects = false }) { Text("Close") } },
        )
    }
    if (confirmOut) {
        AlertDialog(
            onDismissRequest = { confirmOut = false },
            title = { Text("Sign out?") },
            text = { Text("Local drafts stay on this device.") },
            confirmButton = { TextButton(onClick = { confirmOut = false; model.signOut() }) { Text("Sign Out") } },
            dismissButton = { TextButton(onClick = { confirmOut = false }) { Text("Cancel") } },
        )
    }
    if (newProject) {
        Box(Modifier.fillMaxSize().background(colors.background)) {
            NewProjectScreen(model, onClose = { newProject = false }, onCreated = { id, name ->
                newProject = false
                model.showToast("Added $name")
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

