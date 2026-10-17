package sh.zeron.android.ui

import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.ui.focus.focusRequester
import sh.zeron.android.core.projectGroups
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.IconButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.SnackbarResult
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.pulltorefresh.PullToRefreshBox
import androidx.compose.material3.pulltorefresh.PullToRefreshDefaults
import androidx.compose.material3.pulltorefresh.rememberPullToRefreshState
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ProjectColors
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.PullRequestState
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.WorkspaceSnapshot

private enum class Filter(val label: String) { All("All"), NeedsYou("Needs you"), Working("Working"), Pinned("Pinned") }

/** "2 working · 1 needs you" — the live summary beside New session. */
fun liveSummary(ws: WorkspaceSnapshot): String? {
    val (working, awaiting) = liveCounts(ws)
    val parts = listOfNotNull(
        if (working > 0) "$working working" else null,
        if (awaiting > 0) "$awaiting need${if (awaiting == 1) "s" else ""} you" else null,
    )
    return parts.joinToString(" · ").ifEmpty { null }
}

private fun frontRows(ws: WorkspaceSnapshot): List<SessionRow> {
    val seen = HashSet<String>()
    return (ws.front.pinned + ws.front.sections.flatMap { it.sessions } + ws.front.recent).filter { seen.add(it.id) }
}

private fun liveCounts(ws: WorkspaceSnapshot): Pair<Int, Int> {
    val rows = frontRows(ws)
    return rows.count { it.indicator == ChatIndicator.WORKING } to rows.count { it.indicator == ChatIndicator.AWAITING_INPUT }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SessionsScreen(model: AppModel, onOpen: (String) -> Unit, onArchived: () -> Unit, selected: String? = null) {
    val workspace by model.workspace.collectAsState()
    val connectivity by model.connectivity.collectAsState()
    val groupByProject by model.groupByProject.collectAsState()
    val collapsedGroups by model.collapsed.collectAsState()
    var filter by rememberSaveable { mutableStateOf(Filter.All) }
    val scope = rememberCoroutineScope()
    var options by remember { mutableStateOf(false) }
    var newSection by remember { mutableStateOf<String?>(null) }
    var renameSection by remember { mutableStateOf<uniffi.zeron_core.SectionView?>(null) }
    var reorder by remember { mutableStateOf(false) }
    var refreshing by remember { mutableStateOf(false) }
    val pull = rememberPullToRefreshState()
    val ws = workspace
    val counts = remember(ws) { ws?.let { liveCounts(it) } ?: (0 to 0) }

    val subtitle = when {
        connectivity?.state == ConnectivityState.OFFLINE -> "Offline"
        connectivity?.state == ConnectivityState.RECONNECTING -> "Reconnecting…"
        model.isDemo -> "Demo workspace"
        else -> model.accountDetail.takeIf { it.isNotEmpty() }
    }

    val archive: (SessionRow) -> Unit = { row -> model.archive(row.id) }
    // "New section…" from a row's Move menu files that row into it.
    val actions = RowActions(
        model = model,
        onOpen = onOpen,
        archive = archive,
        sections = ws?.front?.sections.orEmpty().map { it.id to it.name },
        newSection = { chatId -> newSection = chatId ?: "" },
        selected = selected,
    )

    val list = androidx.compose.foundation.lazy.rememberLazyListState()
    val density = androidx.compose.ui.platform.LocalDensity.current
    Box(Modifier.fillMaxSize()) {
        sh.zeron.android.design.WallpaperHero(
            model.wallpaper,
            scrollFade = {
                if (list.firstVisibleItemIndex > 0) 0f
                else 1f - list.firstVisibleItemScrollOffset / (with(density) { 400.dp.toPx() })
            },
        )
        PullToRefreshBox(
            isRefreshing = refreshing,
            onRefresh = {
                refreshing = true
                scope.launch {
                    model.refresh()
                    refreshing = false
                }
            },
            state = pull,
            modifier = Modifier.fillMaxSize(),
            indicator = {
                PullToRefreshDefaults.LoadingIndicator(
                    state = pull,
                    isRefreshing = refreshing,
                    modifier = Modifier.align(Alignment.TopCenter).padding(WindowInsets.statusBars.asPaddingValues()),
                )
            },
        ) {
            LazyColumn(
                state = list,
                contentPadding = PaddingValues(top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp, bottom = 200.dp),
                modifier = Modifier.fillMaxSize(),
            ) {
                item("header") {
                    ScreenHeader("Sessions", subtitle) {
                        if (connectivity?.state == ConnectivityState.OFFLINE) {
                            TonalCircleButton(
                                ZIcons.Offline, "Offline", onClick = { scope.launch { model.refresh() } },
                                container = MaterialTheme.colorScheme.errorContainer, content = MaterialTheme.colorScheme.onErrorContainer,
                            )
                        }
                        Box {
                            TonalCircleButton(ZIcons.More, "Session options", onClick = { options = true })
                            ActionMenu(
                                options,
                                { options = false },
                                listOf(
                                    MenuAction("New section…", ZIcons.Folder) { newSection = "" },
                                    MenuAction("Collapse all", ZIcons.Collapse) {
                                        val front = ws?.front ?: return@MenuAction
                                        for (section in front.sections) if (!section.collapsed) model.setSectionCollapsed(section.id, true)
                                        val local = listOf("pinned", "recent") +
                                            if (groupByProject) projectGroups(front.recent, ws.projects).map { it.id } else emptyList()
                                        local.forEach { model.setGroupCollapsed(it, true) }
                                    },
                                    MenuAction("Group by project", ZIcons.Project, checked = groupByProject) { model.setGroupByProject(!groupByProject) },
                                    MenuAction("Archived", ZIcons.Archive, onClick = onArchived),
                                ),
                            )
                        }
                    }
                }
                item("filters") {
                    Row(
                        Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()).padding(horizontal = 16.dp, vertical = 8.dp),
                        horizontalArrangement = Arrangement.spacedBy(8.dp),
                    ) {
                        for (f in Filter.entries) {
                            Pill(
                                f.label,
                                filter == f,
                                onClick = { filter = f },
                                count = when (f) {
                                    Filter.NeedsYou -> counts.second
                                    Filter.Working -> counts.first
                                    else -> null
                                },
                            )
                        }
                    }
                }
                if (ws == null) return@LazyColumn
                val front = ws.front
                when (filter) {
                    Filter.All -> {
                        if (front.pinned.isNotEmpty()) {
                            group(
                                "pinned", "Pinned", front.pinned, actions,
                                collapsed = "pinned" in collapsedGroups,
                                onToggle = { model.setGroupCollapsed("pinned", "pinned" !in collapsedGroups) },
                                menu = listOf(MenuAction("Reorder…", ZIcons.Sort) { reorder = true }),
                            )
                        }
                        for (section in front.sections) {
                            group(
                                section.id, section.name, section.sessions, actions,
                                collapsed = section.collapsed,
                                onToggle = { model.setSectionCollapsed(section.id, !section.collapsed) },
                                menu = listOf(
                                    MenuAction("Rename section…", ZIcons.Rename) { renameSection = section },
                                    MenuAction("Delete section", ZIcons.Delete, destructive = true) { model.deleteSection(section.id) },
                                ),
                            )
                        }
                        if (groupByProject) {
                            // Checkouts of one repository together; project-less sessions per device.
                            for (g in projectGroups(front.recent, ws.projects)) {
                                group(
                                    g.id, g.name, g.sessions, actions,
                                    collapsed = g.id in collapsedGroups,
                                    onToggle = { model.setGroupCollapsed(g.id, g.id !in collapsedGroups) },
                                )
                            }
                        } else if (front.recent.isNotEmpty()) {
                            group(
                                "recent", "Recent", front.recent, actions,
                                collapsed = "recent" in collapsedGroups,
                                onToggle = { model.setGroupCollapsed("recent", "recent" !in collapsedGroups) },
                            )
                        }
                    }
                    Filter.NeedsYou -> group("f", null, frontRows(ws).filter { it.indicator == ChatIndicator.AWAITING_INPUT }, actions)
                    Filter.Working -> group("f", null, frontRows(ws).filter { it.indicator == ChatIndicator.WORKING }, actions)
                    Filter.Pinned -> group("f", null, front.pinned, actions)
                }
                val empty = when (filter) {
                    Filter.All -> front.pinned.isEmpty() && front.sections.isEmpty() && front.recent.isEmpty()
                    Filter.NeedsYou -> counts.second == 0
                    Filter.Working -> counts.first == 0
                    Filter.Pinned -> front.pinned.isEmpty()
                }
                if (empty) item("empty") { EmptyState(filter) }
            }
        }
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }

    newSection?.let { chatId ->
        TextPromptDialog("New section", "", "Create", ZIcons.Folder, onDismiss = { newSection = null }) { name ->
            val id = model.createSection(name)
            if (id != null && chatId.isNotEmpty()) model.move(chatId, id)
        }
    }
    renameSection?.let { section ->
        TextPromptDialog("Rename section", section.name, "Rename", ZIcons.Rename, onDismiss = { renameSection = null }) { name ->
            model.renameSection(section.id, name)
        }
    }
    if (reorder && ws != null) ReorderPinsSheet(model, ws.front.pinned) { reorder = false }
}

/** What a session row can do, shared by every group on the page. */
class RowActions(
    val model: AppModel,
    val onOpen: (String) -> Unit,
    val archive: (SessionRow) -> Unit,
    /** User sections a row can move to (id, name). */
    val sections: List<Pair<String, String>>,
    /** Prompt for a new section, filing `chatId` into it (null: just create). */
    val newSection: (String?) -> Unit,
    /** The chat open beside the list (large screens). */
    val selected: String? = null,
)

private fun LazyListScope.group(
    id: String,
    title: String?,
    rows: List<SessionRow>,
    actions: RowActions,
    collapsed: Boolean = false,
    onToggle: (() -> Unit)? = null,
    menu: List<MenuAction> = emptyList(),
) {
    if (title != null) {
        item("h-$id") {
            var open by remember { mutableStateOf(false) }
            Box(Modifier.animateItem()) {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .combinedClickable(
                            onClick = { onToggle?.invoke() },
                            onLongClick = if (menu.isNotEmpty()) ({ open = true }) else null,
                        )
                        .padding(start = 28.dp, end = 12.dp, top = 20.dp, bottom = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                        Text(title, style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.primary, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                        Spacer(Modifier.width(8.dp))
                        Text("${rows.size}", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (menu.isNotEmpty()) {
                        IconButton(onClick = { open = true }, modifier = Modifier.size(32.dp)) {
                            ZIcon(ZIcons.More, "$title options", Modifier.size(18.dp))
                        }
                    }
                    if (onToggle != null) {
                        IconButton(onClick = onToggle, modifier = Modifier.size(32.dp)) {
                            ZIcon(if (collapsed) ZIcons.ChevronDown else ZIcons.ChevronUp, if (collapsed) "Expand" else "Collapse", Modifier.size(18.dp))
                        }
                    }
                }
                Box(Modifier.align(Alignment.TopEnd).padding(end = 12.dp, top = 20.dp)) { ActionMenu(open, { open = false }, menu) }
            }
        }
    } else {
        item("h-$id") { Spacer(Modifier.height(8.dp)) }
    }
    if (collapsed) return
    itemsIndexed(rows, key = { _, r -> "$id/${r.id}" }) { i, row ->
        Box(Modifier.animateItem().padding(horizontal = 16.dp).padding(bottom = ListItemDefaults.SegmentedGap)) {
            SwipeableSessionRow(row, i, rows.size, actions)
        }
    }
}

/** Swipe right to pin (or unpin), left to archive. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SwipeableSessionRow(row: SessionRow, index: Int, count: Int, actions: RowActions) {
    val state = rememberSwipeToDismissBoxState()
    val scope = rememberCoroutineScope()
    val shape = segmentedShapes(index, count).shape
    SwipeToDismissBox(
        state = state,
        onDismiss = {
            when (it) {
                SwipeToDismissBoxValue.EndToStart -> actions.archive(row)
                SwipeToDismissBoxValue.StartToEnd -> {
                    actions.model.setPinned(row.id, !row.pinned)
                    scope.launch { state.reset() }
                }
                else -> Unit
            }
        },
        backgroundContent = {
            val pinning = state.dismissDirection == SwipeToDismissBoxValue.StartToEnd
            Surface(
                Modifier.fillMaxSize(),
                shape = shape,
                color = if (pinning) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.tertiaryContainer,
            ) {
                Box(
                    Modifier.padding(horizontal = 28.dp),
                    contentAlignment = if (pinning) Alignment.CenterStart else Alignment.CenterEnd,
                ) {
                    if (pinning) {
                        ZIcon(ZIcons.Pin, if (row.pinned) "Unpin" else "Pin", tint = MaterialTheme.colorScheme.onSecondaryContainer)
                    } else {
                        ZIcon(ZIcons.Archive, "Archive", tint = MaterialTheme.colorScheme.onTertiaryContainer)
                    }
                }
            }
        },
    ) {
        SessionItem(row, index, count, actions)
    }
}

/** The list card colour: paper-white in light, raised in dark. */
@Composable
fun cardColor() = if (LocalDarkTheme.current) MaterialTheme.colorScheme.surfaceContainer else MaterialTheme.colorScheme.surfaceContainerLowest

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SessionItem(row: SessionRow, index: Int, count: Int, actions: RowActions) {
    var menu by remember { mutableStateOf(false) }
    var moving by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    val haptics = LocalHapticFeedback.current
    val model = actions.model
    Box {
        SegmentedListItem(
            onClick = { actions.onOpen(row.id) },
            onLongClick = {
                haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                menu = true
            },
            shapes = segmentedShapes(index, count),
            colors = ListItemDefaults.segmentedColors(
                containerColor = if (row.id == actions.selected) MaterialTheme.colorScheme.secondaryContainer else cardColor(),
            ),
            leadingContent = { HarnessTile(row) },
            supportingContent = { Subline(row) },
            trailingContent = {
                Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    StatusLabel(row)
                    row.pullRequest?.let { PrBadge(it.number, it.state) }
                }
            },
        ) {
            Text(
                row.title,
                style = if (row.unseen) MaterialTheme.typography.titleMediumEmphasized else MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        ActionMenu(
            menu,
            { menu = false },
            listOf(
                MenuAction(if (row.pinned) "Unpin" else "Pin", ZIcons.Pin) { model.setPinned(row.id, !row.pinned) },
                MenuAction("Move to section…", ZIcons.Folder) { moving = true },
                MenuAction("Rename…", ZIcons.Rename) { renaming = true },
                MenuAction("Archive", ZIcons.Archive, destructive = true) { actions.archive(row) },
            ),
        )
        ChoiceMenu(
            moving,
            { moving = false },
            listOf(
                MenuSection("Move to section", actions.sections.map { (id, name) ->
                    MenuChoice(name, row.sectionId == id, leading = { ZIcon(ZIcons.Folder, null, Modifier.size(20.dp)) }) { model.move(row.id, id) }
                } + listOf(
                    MenuChoice("No section", row.sectionId == null, leading = { ZIcon(ZIcons.Home, null, Modifier.size(20.dp)) }) { model.move(row.id, null) },
                    MenuChoice("New section…", false, leading = { ZIcon(ZIcons.Plus, null, Modifier.size(20.dp)) }) { actions.newSection(row.id) },
                )),
            ),
        )
    }
    if (renaming) RenameDialog(row.title, onDismiss = { renaming = false }) { model.rename(row.id, it) }
}

/** The harness mark on a tile toned by the session's project. */
@Composable
fun HarnessTile(row: SessionRow) {
    val tone = ProjectColors.color(row.colorIndex(), LocalDarkTheme.current)
    Box(
        Modifier.size(48.dp).clip(RoundedCornerShape(16.dp)).background(tone.copy(alpha = if (LocalDarkTheme.current) 0.18f else 0.12f)),
        contentAlignment = Alignment.Center,
    ) { HarnessMark(row.harness, 24.dp, tint = MaterialTheme.colorScheme.onSurface) }
}

/** Project monogram + name, then the branch — the desktop sidebar subline. */
@Composable
private fun Subline(row: SessionRow) {
    val muted = MaterialTheme.colorScheme.onSurfaceVariant
    Row(verticalAlignment = Alignment.CenterVertically) {
        val project = row.project
        if (project != null) {
            ProjectTile(project.name, project.colorIndex.toInt(), 16.dp)
            Spacer(Modifier.width(6.dp))
        }
        Text(
            project?.name ?: row.deviceName ?: "No project",
            style = MaterialTheme.typography.bodyMedium,
            color = muted,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f, fill = false),
        )
        row.branch?.takeIf { it.isNotEmpty() }?.let { branch ->
            Spacer(Modifier.width(10.dp))
            ZIcon(ZIcons.Branch, null, Modifier.size(14.dp), tint = muted.copy(alpha = 0.7f))
            Spacer(Modifier.width(3.dp))
            Text(branch, style = MaterialTheme.typography.bodyMedium, color = muted.copy(alpha = 0.7f), maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
fun PrBadge(number: ULong, state: PullRequestState) {
    val tone = when (state) {
        PullRequestState.OPEN -> successColor()
        PullRequestState.MERGED -> MaterialTheme.colorScheme.primary
        PullRequestState.CLOSED -> MaterialTheme.colorScheme.error
    }
    Surface(shape = RoundedCornerShape(8.dp), color = tone.copy(alpha = 0.12f), contentColor = tone) {
        Row(Modifier.padding(horizontal = 6.dp, vertical = 2.dp), verticalAlignment = Alignment.CenterVertically) {
            ZIcon(ZIcons.PullRequest, null, Modifier.size(13.dp))
            Spacer(Modifier.width(3.dp))
            Text("$number", style = MaterialTheme.typography.labelMediumEmphasized)
        }
    }
}

@Composable
fun RenameDialog(current: String, onDismiss: () -> Unit, onRename: (String) -> Unit) =
    TextPromptDialog("Rename session", current, "Rename", ZIcons.Rename, onDismiss, onRename)

/** A one-field dialog (rename a session, name or rename a section). */
@Composable
fun TextPromptDialog(title: String, current: String, confirm: String, icon: Int, onDismiss: () -> Unit, onConfirm: (String) -> Unit) {
    var text by remember { mutableStateOf(current) }
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    androidx.compose.runtime.LaunchedEffect(Unit) { focus.requestFocus() }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(icon, null) },
        title = { Text(title) },
        text = {
            OutlinedTextField(
                text, { text = it },
                singleLine = true,
                shape = RoundedCornerShape(16.dp),
                modifier = Modifier.focusRequester(focus),
            )
        },
        confirmButton = {
            TextButton(onClick = { onConfirm(text.trim()); onDismiss() }, enabled = text.isNotBlank()) { Text(confirm) }
        },
        dismissButton = { TextButton(onClick = onDismiss) { Text("Cancel") } },
    )
}

/** Pins are an ordered list (synced to the desktop): move them up and down. */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ReorderPinsSheet(model: AppModel, pinned: List<SessionRow>, onDismiss: () -> Unit) {
    val sheet = androidx.compose.material3.rememberModalBottomSheetState(skipPartiallyExpanded = true)
    androidx.compose.material3.ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        Text("Reorder pinned", style = MaterialTheme.typography.headlineSmallEmphasized, modifier = Modifier.padding(start = 24.dp, bottom = 16.dp))
        LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 24.dp)) {
            itemsIndexed(pinned, key = { _, r -> r.id }) { i, row ->
                SegmentedListItem(
                    onClick = {},
                    shapes = segmentedShapes(i, pinned.size),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { HarnessTile(row) },
                    trailingContent = {
                        Row {
                            IconButton(onClick = { model.movePin(row.id, pinned.getOrNull(i - 2)?.id, pinned[i - 1].id) }, enabled = i > 0) {
                                ZIcon(ZIcons.ChevronUp, "Move up", Modifier.size(20.dp))
                            }
                            IconButton(onClick = { model.movePin(row.id, pinned[i + 1].id, pinned.getOrNull(i + 2)?.id) }, enabled = i < pinned.size - 1) {
                                ZIcon(ZIcons.ChevronDown, "Move down", Modifier.size(20.dp))
                            }
                        }
                    },
                    modifier = Modifier.animateItem().padding(bottom = ListItemDefaults.SegmentedGap),
                ) { Text(row.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            }
        }
    }
}

@Composable
private fun EmptyState(filter: Filter) {
    Column(Modifier.fillMaxWidth().padding(48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        ZIcon(ZIcons.Chat, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.outline)
        Spacer(Modifier.height(12.dp))
        Text(
            when (filter) {
                Filter.All -> "No sessions yet"
                Filter.NeedsYou -> "Nothing needs you"
                Filter.Working -> "No agents working"
                Filter.Pinned -> "Nothing pinned"
            },
            style = MaterialTheme.typography.titleLarge,
        )
    }
}
