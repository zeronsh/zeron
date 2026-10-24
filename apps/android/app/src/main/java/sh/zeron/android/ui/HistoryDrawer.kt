package sh.zeron.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.selection.selectable
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.unit.Dp
import androidx.compose.animation.core.CubicBezierEasing
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.tween
import androidx.compose.foundation.MutatePriority
import androidx.compose.foundation.MutatorMutex
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.systemGestureExclusion
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow

private val DrawerWidth = 312.dp
/** How far the page behind slides while the drawer opens. */
private val ContentShift = 36.dp
/** The left-edge strip that starts the swipe. */
private val EdgeWidth = 20.dp
/** Android exempts at most 200dp per edge from the system back gesture. */
private val ExclusionHeight = 200.dp
private val NeedsYou = Color(0xFFF5B963)

/** 0 = closed, 1 = open; follows the finger mid-swipe. */
@Stable
class HistoryDrawerState {
    var progress by mutableFloatStateOf(0f)
        private set
    val isOpen get() = progress > 0f
    private val mutex = MutatorMutex()

    suspend fun open() = animateTo(1f)
    suspend fun close() = animateTo(0f)

    internal suspend fun animateTo(target: Float, velocity: Float = 0f) = mutex.mutate {
        animate(progress, target, velocity, tween(260, easing = CubicBezierEasing(.2f, .8f, .2f, 1f))) { v, _ -> progress = v }
    }

    /** A finger takes over: cancels any running open/close animation. */
    internal suspend fun stop() = mutex.mutate(MutatePriority.UserInput) {}

    internal fun dragBy(fraction: Float) {
        progress = (progress + fraction).coerceIn(0f, 1f)
    }
}

/**
 * Swipe right from the left edge for chat history. The drawer tracks the
 * finger, the page behind slides and dims, and a release past 40% (or a
 * flick) opens it. [enabled] gates the edge swipe; [drawer] is its content.
 */
@Composable
fun HistoryDrawerHost(
    state: HistoryDrawerState,
    enabled: Boolean,
    drawer: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val widthPx = with(density) { DrawerWidth.toPx() }
    val settle: (Float) -> Unit = { velocity ->
        val target = when {
            velocity > 800f -> 1f
            velocity < -800f -> 0f
            state.progress > 0.4f -> 1f
            else -> 0f
        }
        scope.launch { state.animateTo(target, velocity / widthPx) }
    }
    val drag = rememberDraggableState { dx -> state.dragBy(dx / widthPx) }
    val dragModifier = Modifier.draggable(
        drag,
        Orientation.Horizontal,
        onDragStarted = { state.stop() },
        onDragStopped = { settle(it) },
    )
    BackHandler(enabled = state.isOpen) { scope.launch { state.close() } }

    Box(Modifier.fillMaxSize()) {
        val p = state.progress
        Box(Modifier.fillMaxSize().graphicsLayer { translationX = state.progress * ContentShift.toPx() }) { content() }
        if (enabled && !state.isOpen) {
            // The grip hints the gesture.
            Box(
                Modifier
                    .align(Alignment.CenterStart)
                    .padding(start = 3.dp)
                    .size(4.dp, 56.dp)
                    .clip(RoundedCornerShape(2.dp))
                    .background(MaterialTheme.colorScheme.outlineVariant),
            )
        }
        if (enabled) {
            // The strip that catches the swipe stays put while the drawer opens:
            // removing it mid-gesture would drop the drag.
            Box(
                Modifier
                    .align(Alignment.CenterStart)
                    .width(EdgeWidth)
                    .fillMaxHeight()
                    // Keep the system back swipe off the middle band, where the grip is.
                    .systemGestureExclusion { coords ->
                        val band = with(density) { ExclusionHeight.toPx() }
                        val mid = coords.size.height / 2f
                        Rect(0f, mid - band / 2, coords.size.width.toFloat(), mid + band / 2)
                    }
                    .then(dragModifier),
            )
        }
        if (p > 0f) {
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer { alpha = state.progress }
                    .background(Color.Black.copy(alpha = 0.62f))
                    .pointerInput(Unit) { detectTapGestures { scope.launch { state.close() } } }
                    .then(dragModifier),
            )
            Box(
                Modifier
                    .width(DrawerWidth)
                    .fillMaxHeight()
                    .graphicsLayer { translationX = (state.progress - 1f) * (widthPx + 24.dp.toPx()) }
                    .then(dragModifier),
            ) { drawer() }
        }
    }
}

/** One track, one pill: the selection slides between the four filters. */
@Composable
internal fun FilterSegments(
    selected: Filter,
    onSelect: (Filter) -> Unit,
    modifier: Modifier = Modifier,
    count: (Filter) -> Int = { 0 },
) {
    val colors = MaterialTheme.colorScheme
    val segments = Filter.entries
    // A live count widens its segment so the number fits beside the label.
    fun share(f: Filter) = f.share + if (count(f) > 0) 18f else 0f
    val total = segments.sumOf { share(it).toDouble() }.toFloat()
    BoxWithConstraints(
        modifier
            .fillMaxWidth()
            .height(40.dp)
            .clip(RoundedCornerShape(20.dp))
            .background(colors.surfaceContainerHigh)
            .border(1.dp, colors.outlineVariant.copy(alpha = 0.5f), RoundedCornerShape(20.dp))
            .padding(4.dp),
    ) {
        val unit = maxWidth / total
        val before = segments.takeWhile { it != selected }.sumOf { share(it).toDouble() }.toFloat()
        val motion = tween<Dp>(280, easing = CubicBezierEasing(.2f, .8f, .2f, 1f))
        val left by animateDpAsState(unit * before, motion, label = "filter-left")
        val width by animateDpAsState(unit * share(selected), motion, label = "filter-width")
        Box(
            Modifier
                .offset(x = left)
                .width(width)
                .fillMaxHeight()
                .shadow(4.dp, RoundedCornerShape(16.dp), ambientColor = colors.primary, spotColor = colors.primary)
                .clip(RoundedCornerShape(16.dp))
                .background(colors.primary),
        )
        Row(Modifier.fillMaxSize()) {
            segments.forEach { f ->
                val on = f == selected
                val text by animateColorAsState(if (on) colors.onPrimary else colors.onSurfaceVariant, tween(200), label = "filter-text")
                Box(
                    Modifier
                        .weight(share(f))
                        .fillMaxHeight()
                        .clip(RoundedCornerShape(16.dp))
                        .selectable(selected = on, role = Role.Tab, onClick = { onSelect(f) }),
                    contentAlignment = Alignment.Center,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(f.label, fontSize = 13.sp, fontWeight = FontWeight.Medium, color = text, maxLines = 1, softWrap = false)
                        val n = count(f)
                        if (n > 0) {
                            Spacer(Modifier.width(5.dp))
                            Text("$n", fontSize = 11.sp, fontWeight = FontWeight.SemiBold, color = text.copy(alpha = 0.75f), maxLines = 1)
                        }
                    }
                }
            }
        }
    }
}

/** The drawer itself: new chat, project + status filters, recent chats. */
@Composable
fun HistoryDrawerContent(
    model: AppModel,
    activeId: String?,
    onNewChat: () -> Unit,
    onPick: (String) -> Unit,
    onSearch: () -> Unit,
    onSettings: () -> Unit,
) {
    val workspace by model.workspace.collectAsState()
    var filter by rememberSaveable { mutableStateOf(Filter.All) }
    var projectId by rememberSaveable { mutableStateOf<String?>(null) }
    var projectMenu by remember { mutableStateOf(false) }
    val all = remember(workspace) { workspace?.let { frontRows(it) }.orEmpty().sortedByDescending { it.lastActivityMs } }
    val projects = remember(all) { all.mapNotNull { it.project }.distinctBy { it.id } }
    val shown = all.filter { row ->
        (projectId == null || row.project?.id == projectId) && when (filter) {
            Filter.All -> true
            Filter.NeedsYou -> row.indicator == ChatIndicator.AWAITING_INPUT
            Filter.Working -> row.indicator == ChatIndicator.WORKING
            Filter.Pinned -> row.pinned
        }
    }
    val colors = MaterialTheme.colorScheme

    Surface(
        Modifier.fillMaxSize(),
        color = colors.surfaceContainerLow,
        shape = RoundedCornerShape(topEnd = 28.dp, bottomEnd = 28.dp),
        shadowElevation = 12.dp,
    ) {
        Column(
            Modifier.statusBarsPadding().navigationBarsPadding().padding(start = 14.dp, end = 14.dp, top = 24.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Chats", Modifier.weight(1f), fontSize = 22.sp, fontWeight = FontWeight.Bold, color = colors.onSurface)
                IconButton(onClick = onSearch) { ZIcon(ZIcons.Search, "Search chats", Modifier.size(20.dp), tint = colors.onSurfaceVariant) }
            }
            Button(
                onClick = onNewChat,
                modifier = Modifier.fillMaxWidth().height(48.dp),
                colors = ButtonDefaults.buttonColors(containerColor = colors.primaryContainer, contentColor = colors.onPrimaryContainer),
            ) {
                ZIcon(ZIcons.NewSession, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("New chat", fontWeight = FontWeight.SemiBold)
            }
            Box {
                Row(
                    Modifier
                        .fillMaxWidth()
                        .height(44.dp)
                        .clip(RoundedCornerShape(12.dp))
                        .background(colors.surfaceContainerHigh)
                        .clickable { projectMenu = true }
                        .padding(horizontal = 14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    ZIcon(ZIcons.Folder, null, Modifier.size(16.dp), tint = colors.onSurface)
                    Spacer(Modifier.width(10.dp))
                    Text(
                        projects.firstOrNull { it.id == projectId }?.name ?: "All projects",
                        Modifier.weight(1f),
                        style = MaterialTheme.typography.bodyMedium,
                        fontWeight = FontWeight.Medium,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                    ZIcon(ZIcons.ChevronDown, null, Modifier.size(16.dp), tint = colors.onSurfaceVariant)
                }
                ChoiceMenu(
                    projectMenu,
                    { projectMenu = false },
                    listOf(
                        MenuSection(
                            "Project",
                            listOf(MenuChoice("All projects", projectId == null, leading = { ZIcon(ZIcons.Home, null, Modifier.size(20.dp)) }) { projectId = null }) +
                                projects.map { project ->
                                    MenuChoice(project.name, projectId == project.id, leading = { ProjectTile(project.name, project.colorIndex.toInt(), 20.dp) }) { projectId = project.id }
                                },
                        ),
                    ),
                )
            }
            FilterSegments(filter, onSelect = { filter = it })
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(bottom = 8.dp)) {
                if (shown.isEmpty()) {
                    item {
                        Text(
                            "Nothing here right now",
                            Modifier.fillMaxWidth().padding(vertical = 24.dp, horizontal = 8.dp),
                            style = MaterialTheme.typography.bodyMedium,
                            color = colors.onSurfaceVariant,
                            textAlign = androidx.compose.ui.text.style.TextAlign.Center,
                        )
                    }
                } else {
                    item {
                        Text(
                            "RECENT",
                            Modifier.padding(start = 8.dp, top = 4.dp, bottom = 6.dp),
                            style = MaterialTheme.typography.labelMedium,
                            fontWeight = FontWeight.SemiBold,
                            letterSpacing = 0.7.sp,
                            color = colors.onSurfaceVariant,
                        )
                    }
                    items(shown, key = { it.id }) { row -> HistoryRow(row, row.id == activeId) { onPick(row.id) } }
                }
            }
            HorizontalDivider(color = colors.outlineVariant)
            Row(
                Modifier
                    .fillMaxWidth()
                    .height(44.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .clickable(onClick = onSettings)
                    .padding(horizontal = 10.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                ZIcon(ZIcons.TabSettings, null, Modifier.size(18.dp), tint = colors.onSurfaceVariant)
                Spacer(Modifier.width(10.dp))
                Text("Settings", style = MaterialTheme.typography.bodyMedium, color = colors.onSurfaceVariant)
            }
        }
    }
}

/** Status dot, project monogram, title, age. */
@Composable
private fun HistoryRow(row: SessionRow, active: Boolean, onClick: () -> Unit) {
    val colors = MaterialTheme.colorScheme
    Row(
        Modifier
            .fillMaxWidth()
            .heightIn(min = 44.dp)
            .clip(RoundedCornerShape(12.dp))
            .background(if (active) colors.surfaceContainerHighest else Color.Transparent)
            .clickable(onClick = onClick)
            .padding(horizontal = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
        horizontalArrangement = Arrangement.spacedBy(10.dp),
    ) {
        val dot = when (row.indicator) {
            ChatIndicator.WORKING -> colors.primary
            ChatIndicator.AWAITING_INPUT -> NeedsYou
            ChatIndicator.ERRORED -> colors.error
            else -> colors.outlineVariant
        }
        Box(Modifier.size(14.dp), contentAlignment = Alignment.Center) {
            if (row.indicator == ChatIndicator.WORKING) Box(Modifier.size(14.dp).clip(CircleShape).background(dot.copy(alpha = 0.25f)))
            Box(Modifier.size(8.dp).clip(CircleShape).background(dot))
        }
        ProjectTile(row.project?.name, row.project?.colorIndex?.toInt() ?: 0, 18.dp)
        Text(
            row.title,
            Modifier.weight(1f),
            style = MaterialTheme.typography.bodyMedium,
            fontWeight = if (row.unseen) FontWeight.SemiBold else FontWeight.Normal,
            color = colors.onSurface,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
        Text(row.timeLabel, style = MaterialTheme.typography.labelMedium, color = colors.onSurfaceVariant)
    }
}
