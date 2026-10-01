package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.pullFeedback
import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import androidx.compose.foundation.background
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
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.snapshotFlow
import kotlinx.coroutines.flow.distinctUntilChanged
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

/** "1 needs you" / "2 working" — the live summary beside New session. */
fun liveSummary(ws: WorkspaceSnapshot): String? {
    val (working, awaiting) = liveCounts(ws)
    return when {
        awaiting > 0 -> "$awaiting needs you"
        working > 0 -> "$working working"
        else -> null
    }
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
fun SessionsScreen(model: AppModel, onOpen: (String) -> Unit) {
    val workspace by model.workspace.collectAsState()
    val connectivity by model.connectivity.collectAsState()
    var filter by rememberSaveable { mutableStateOf(Filter.All) }
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
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

    val fb = LocalFeedback.current
    val archive: (SessionRow) -> Unit = { row ->
        fb.both(Haptic.Confirm, Cue.Archive)
        model.archive(row.id)
        scope.launch {
            if (snackbar.showSnackbar("Archived", actionLabel = "Undo", withDismissAction = true) == SnackbarResult.ActionPerformed) {
                fb.both(Haptic.Select, Cue.Undo)
                model.unarchive(row.id)
            }
        }
    }

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
            onRefresh = pullFeedback(pull) {
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
                                ZIcons.Offline, "Offline", onClick = feedbackAction(Haptic.Select, Cue.Refresh) { scope.launch { model.refresh() } },
                                container = MaterialTheme.colorScheme.errorContainer, content = MaterialTheme.colorScheme.onErrorContainer,
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
                        if (front.pinned.isNotEmpty()) group("pinned", "Pinned", front.pinned, model, onOpen, archive)
                        for (section in front.sections) {
                            group(section.id, section.name, section.sessions, model, onOpen, archive, collapsed = section.collapsed) {
                                model.setSectionCollapsed(section.id, !section.collapsed)
                            }
                        }
                        if (front.recent.isNotEmpty()) group("recent", "Recent", front.recent, model, onOpen, archive)
                    }
                    Filter.NeedsYou -> group("f", null, frontRows(ws).filter { it.indicator == ChatIndicator.AWAITING_INPUT }, model, onOpen, archive)
                    Filter.Working -> group("f", null, frontRows(ws).filter { it.indicator == ChatIndicator.WORKING }, model, onOpen, archive)
                    Filter.Pinned -> group("f", null, front.pinned, model, onOpen, archive)
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
        SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).padding(bottom = 180.dp))
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
}

private fun LazyListScope.group(
    id: String,
    title: String?,
    rows: List<SessionRow>,
    model: AppModel,
    onOpen: (String) -> Unit,
    archive: (SessionRow) -> Unit,
    collapsed: Boolean = false,
    onToggle: (() -> Unit)? = null,
) {
    if (title != null) {
        item("h-$id") {
            Row(
                Modifier.animateItem().fillMaxWidth().padding(start = 28.dp, end = 12.dp, top = 20.dp, bottom = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Text(title, style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.primary)
                Spacer(Modifier.width(8.dp))
                Text("${rows.size}", style = MaterialTheme.typography.titleSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.weight(1f))
                if (onToggle != null) {
                    IconButton(onClick = feedbackAction(Haptic.Select, if (collapsed) Cue.Open else Cue.Close, onToggle), modifier = Modifier.size(32.dp)) {
                        ZIcon(if (collapsed) ZIcons.ChevronDown else ZIcons.ChevronUp, if (collapsed) "Expand" else "Collapse", Modifier.size(18.dp))
                    }
                }
            }
        }
    } else {
        item("h-$id") { Spacer(Modifier.height(8.dp)) }
    }
    if (collapsed) return
    itemsIndexed(rows, key = { _, r -> "$id/${r.id}" }) { i, row ->
        Box(Modifier.animateItem().padding(horizontal = 16.dp).padding(bottom = ListItemDefaults.SegmentedGap)) {
            SwipeableSessionRow(row, i, rows.size, model, onOpen, archive)
        }
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SwipeableSessionRow(
    row: SessionRow,
    index: Int,
    count: Int,
    model: AppModel,
    onOpen: (String) -> Unit,
    archive: (SessionRow) -> Unit,
) {
    val state = rememberSwipeToDismissBoxState()
    val shape = segmentedShapes(index, count).shape
    // A tick as the swipe crosses the point where letting go archives.
    val fb = LocalFeedback.current
    LaunchedEffect(state, fb) {
        snapshotFlow { state.targetValue }.distinctUntilChanged().collect { if (it == SwipeToDismissBoxValue.EndToStart) fb.haptic(Haptic.Threshold) }
    }
    SwipeToDismissBox(
        state = state,
        enableDismissFromStartToEnd = false,
        onDismiss = { if (it == SwipeToDismissBoxValue.EndToStart) archive(row) },
        backgroundContent = {
            Surface(Modifier.fillMaxSize(), shape = shape, color = MaterialTheme.colorScheme.tertiaryContainer) {
                Box(Modifier.padding(end = 28.dp), contentAlignment = Alignment.CenterEnd) {
                    ZIcon(ZIcons.Archive, "Archive", tint = MaterialTheme.colorScheme.onTertiaryContainer)
                }
            }
        },
    ) {
        SessionItem(row, index, count, model, onOpen, archive)
    }
}

/** The list card colour: paper-white in light, raised in dark. */
@Composable
fun cardColor() = if (LocalDarkTheme.current) MaterialTheme.colorScheme.surfaceContainer else MaterialTheme.colorScheme.surfaceContainerLowest

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SessionItem(
    row: SessionRow,
    index: Int,
    count: Int,
    model: AppModel,
    onOpen: (String) -> Unit,
    archive: (SessionRow) -> Unit,
) {
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    val fb = LocalFeedback.current
    Box {
        SegmentedListItem(
            onClick = tapAction { onOpen(row.id) },
            onLongClick = {
                fb.haptic(Haptic.LongPress)
                menu = true
            },
            shapes = segmentedShapes(index, count),
            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
            leadingContent = { HarnessTile(row) },
            supportingContent = { Subline(row) },
            trailingContent = {
                Column(horizontalAlignment = Alignment.End, verticalArrangement = Arrangement.spacedBy(6.dp)) {
                    // Running subagents lead the row's own status (never
                    // replace it) — also once the parent's turn is done.
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        if (row.runningSubagents > 0u) {
                            RunningPill(row.runningSubagents.toInt())
                            Spacer(Modifier.width(8.dp))
                        }
                        StatusLabel(row)
                    }
                    row.pullRequest?.let { PrBadge(it.number, it.state) }
                }
            },
        ) {
            Text(row.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        ActionMenu(
            menu,
            { menu = false },
            listOf(
                MenuAction(if (row.pinned) "Unpin" else "Pin", ZIcons.Pin, haptic = Haptic.Pop, cue = if (row.pinned) Cue.Unstar else Cue.Pin) { model.setPinned(row.id, !row.pinned) },
                MenuAction("Rename", ZIcons.Rename) { renaming = true },
                MenuAction("Archive", ZIcons.Archive) { archive(row) }, // archive() answers itself
            ),
        )
    }
    if (renaming) RenameDialog(row.title, onDismiss = { renaming = false }) { model.rename(row.id, it) }
}

/** The harness mark on a tile toned by the session's project. */
@Composable
private fun HarnessTile(row: SessionRow) {
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
fun RenameDialog(current: String, onDismiss: () -> Unit, onRename: (String) -> Unit) {
    var text by remember { mutableStateOf(current) }
    AlertDialog(
        onDismissRequest = onDismiss,
        icon = { ZIcon(ZIcons.Rename, null) },
        title = { Text("Rename session") },
        text = {
            OpenCloseFeedback()
            OutlinedTextField(text, { text = it }, singleLine = true, shape = RoundedCornerShape(16.dp))
        },
        confirmButton = {
            TextButton(onClick = feedbackAction(Haptic.Confirm, Cue.Select) { onRename(text.trim()); onDismiss() }, enabled = text.isNotBlank()) { Text("Rename") }
        },
        dismissButton = { TextButton(onClick = tapAction(action = onDismiss)) { Text("Cancel") } },
    )
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
