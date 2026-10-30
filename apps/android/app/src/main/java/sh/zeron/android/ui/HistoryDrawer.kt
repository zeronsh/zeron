package sh.zeron.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.animation.ExitTransition
import androidx.compose.animation.core.Spring
import androidx.compose.animation.core.VisibilityThreshold
import androidx.compose.animation.core.animate
import androidx.compose.animation.core.spring
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.foundation.MutatePriority
import androidx.compose.foundation.MutatorMutex
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.Orientation
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.draggable
import androidx.compose.foundation.gestures.rememberDraggableState
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.AbsoluteRoundedCornerShape
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.HorizontalDivider
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.State
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.AbsoluteAlignment
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChange
import androidx.compose.ui.input.pointer.util.VelocityTracker
import androidx.compose.ui.input.pointer.util.addPointerInputChange
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlin.math.abs
import kotlin.math.roundToInt
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.ProjectRef
import uniffi.zeron_core.SessionRow

private val DrawerWidth = 312.dp
/** How far the page behind slides while the drawer opens (parallax). */
private val ContentShift = 96.dp
/** Critically damped: carries the release velocity, never overshoots. */
private val DrawerSpring = spring<Float>(dampingRatio = Spring.DampingRatioNoBouncy, stiffness = 520f)
/** A fling faster than this (px/s) decides the swipe whatever the distance. */
private const val FlingVelocity = 600f

/** 0..1 driven by a finger, then settled on [DrawerSpring]. */
@Stable
open class SwipeProgress {
    var progress by mutableFloatStateOf(0f)
        private set
    private val mutex = MutatorMutex()

    /** [velocity] is in progress units per second. */
    internal suspend fun animateTo(target: Float, velocity: Float = 0f) = mutex.mutate {
        animate(progress, target, velocity, DrawerSpring) { v, _ -> progress = v.coerceIn(0f, 1f) }
    }

    /** A finger takes over: cancels any running animation. */
    internal suspend fun stop() = mutex.mutate(MutatePriority.UserInput) {}

    internal fun dragBy(fraction: Float) {
        progress = (progress + fraction).coerceIn(0f, 1f)
    }

    internal fun snapTo(value: Float) {
        progress = value
    }
}

/** 0 = closed, 1 = open; follows the finger mid-swipe. */
@Stable
class HistoryDrawerState : SwipeProgress() {
    val isOpen get() = progress > 0f
    /** The drawer's filters, kept from one opening to the next. */
    internal var filter by mutableStateOf(Filter.All)
    internal var projectId by mutableStateOf<String?>(null)
    suspend fun open() = animateTo(1f)
    suspend fun close() = animateTo(0f)

    /** Shut at once, stopping any swipe or animation in flight. */
    internal suspend fun dismiss() {
        stop()
        snapTo(0f)
    }
}

/**
 * A page sliding off to the left on its way home: 0 at rest, 1 gone. Only the
 * page that took the swipe ([owner], its back stack entry) moves. Once it's
 * let go for home the page is popped where it is, and its exit transition
 * ([homeSwipeExitOffset]) finishes the slide over Home.
 */
@Stable
class HomeSwipeState : SwipeProgress() {
    var owner by mutableStateOf<String?>(null)
        internal set

    /** A new swipe on [page]: one still springing back carries on from where it is. */
    internal fun begin(page: String?) {
        if (owner == page) return
        owner = page
        snapTo(0f)
    }

    /** Whether back stack entry [entry] is being popped by a swipe (it's already partly off screen). */
    fun leaving(entry: String): Boolean = owner == entry && progress > 0f
}

/** The rest of the slide for a page popped by a home swipe, from where the finger let it go. */
fun HomeSwipeState.exit(): ExitTransition =
    slideOutHorizontally(spring(Spring.DampingRatioNoBouncy, 520f, IntOffset.VisibilityThreshold)) { homeSwipeExitOffset(it, progress) }

/** How much further a page let go at [progress] slides to be fully off the left edge. */
fun homeSwipeExitOffset(fullWidth: Int, progress: Float): Int = -(fullWidth * (1f - progress.coerceIn(0f, 1f))).roundToInt()

/** Draws the page [owner] where the home swipe has it. */
fun Modifier.homeSwipeLayer(state: HomeSwipeState, owner: String): Modifier = graphicsLayer {
    if (state.owner == owner) {
        val p = state.progress
        translationX = -p * size.width
        alpha = 1f - 0.4f * p
    }
}

/**
 * Swipes anywhere on the page. Rightward opens chat history; leftward
 * ([onSwipeHome], when set) slides the page away and goes home. Children get
 * the first say: vertical scrolling, code blocks that scroll sideways, agent
 * visuals and row swipes keep their gestures; a mostly-horizontal drag nobody
 * claimed is ours. Both track the finger 1:1 and settle on a spring that
 * carries the release velocity. [enabled] gates the drawer; [drawer] is its
 * content. [page] is the current back stack entry's id (the home swipe's owner).
 */
@Composable
fun HistoryDrawerHost(
    state: HistoryDrawerState,
    enabled: Boolean,
    home: HomeSwipeState,
    page: String?,
    onSwipeHome: (() -> Unit)?,
    drawer: @Composable () -> Unit,
    content: @Composable () -> Unit,
) {
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current
    val widthPx = with(density) { DrawerWidth.toPx() }
    val settle: (Float) -> Unit = { velocity ->
        val target = if (drawerSettlesOpen(state.progress, velocity)) 1f else 0f
        scope.launch { state.animateTo(target, velocity / widthPx) }
    }
    val drag = rememberDraggableState { dx -> state.dragBy(dx / widthPx) }
    val dragModifier = Modifier.draggable(
        drag,
        Orientation.Horizontal,
        onDragStarted = { state.stop() },
        onDragStopped = { settle(it) },
    )
    // The keyboard (New session opens with it up) doesn't stay over the drawer,
    // but a drag that springs back leaves it where it was.
    val focus = LocalFocusManager.current
    val keyboard = LocalSoftwareKeyboardController.current
    LaunchedEffect(state) {
        snapshotFlow { drawerTakesKeyboard(state.progress) }.collect { open ->
            if (open) {
                focus.clearFocus(force = true)
                keyboard?.hide()
            }
        }
    }
    // Navigation the drawer didn't start (a notification, a toast) can land on a page without it.
    LaunchedEffect(enabled) { if (!enabled) state.dismiss() }
    val swipes = SwipeTargets(
        drawer = if (enabled) state else null,
        drawerWidthPx = widthPx,
        onDrawerRelease = settle,
        home = if (onSwipeHome != null && page != null) home else null,
        page = page,
        // A spring-back still running would fight the finger.
        onHomeStart = { scope.launch { home.stop() } },
        onHomeRelease = { pageWidth, velocity ->
            if (homeSwipeLeaves(home.progress, velocity)) {
                // Popped now, Home is there under the page as it finishes sliding off.
                onSwipeHome?.invoke()
            } else {
                // Leftward is negative: progress grows as the page leaves.
                scope.launch { home.animateTo(0f, -velocity / pageWidth) }
            }
        },
    )

    Box(Modifier.fillMaxSize()) {
        Box(
            Modifier
                .fillMaxSize()
                .graphicsLayer { translationX = state.progress * ContentShift.toPx() }
                .pageSwipes(rememberUpdatedState(swipes)),
        ) { content() }
        if (state.progress > 0f) {
            Box(
                Modifier
                    .fillMaxSize()
                    .graphicsLayer { alpha = state.progress }
                    .background(Color.Black.copy(alpha = 0.55f))
                    .pointerInput(Unit) { detectTapGestures { scope.launch { state.close() } } }
                    .then(dragModifier),
            )
            // Pinned to the left in right-to-left layouts too: it slides in from there.
            Box(
                Modifier
                    .align(AbsoluteAlignment.TopLeft)
                    .width(DrawerWidth)
                    .fillMaxHeight()
                    .graphicsLayer { translationX = (state.progress - 1f) * widthPx }
                    .then(dragModifier),
            ) { drawer() }
        }
    }
    // After the content: the last back handler registered runs first, and the
    // NavHost inside registers its own (it would pop the page under the drawer).
    BackHandler(enabled = state.isOpen) { scope.launch { state.close() } }
}

private class SwipeTargets(
    val drawer: HistoryDrawerState?,
    val drawerWidthPx: Float,
    val onDrawerRelease: (Float) -> Unit,
    val home: HomeSwipeState?,
    val page: String?,
    val onHomeStart: () -> Unit,
    /** (page width px, release velocity px/s) */
    val onHomeRelease: (Float, Float) -> Unit,
)

/**
 * Claims a mostly-horizontal drag that no child consumed (children see pointer
 * events first in the Main pass): rightward drives the drawer, leftward the
 * page's way home.
 */
private fun Modifier.pageSwipes(targets: State<SwipeTargets>): Modifier =
    pointerInput(targets) {
        val slop = viewConfiguration.touchSlop
        awaitEachGesture {
            val down = awaitFirstDown(requireUnconsumed = false)
            val t = targets.value
            if (t.drawer == null && t.home == null) return@awaitEachGesture
            if (t.drawer?.isOpen == true) return@awaitEachGesture
            var dx = 0f
            var dy = 0f
            // Decide: wait out the touch slop, backing off if anyone else claims it.
            val toDrawer: Boolean
            while (true) {
                val event = awaitPointerEvent()
                if (event.changes.size > 1) return@awaitEachGesture
                val change = event.changes.firstOrNull { it.id == down.id } ?: return@awaitEachGesture
                if (!change.pressed || change.isConsumed) return@awaitEachGesture
                val delta = change.positionChange()
                dx += delta.x
                dy += delta.y
                val claim = claimSwipe(dx, dy, slop, drawer = t.drawer != null, home = t.home != null)
                if (claim == SwipeClaim.Undecided) continue
                if (claim == SwipeClaim.Yield) return@awaitEachGesture
                toDrawer = claim == SwipeClaim.Drawer
                change.consume()
                break
            }
            val pageWidth = size.width.toFloat().coerceAtLeast(1f)
            val drawer = t.drawer
            val home = t.home
            if (!toDrawer && home != null) {
                t.onHomeStart()
                home.begin(t.page)
            }
            fun move(x: Float) {
                if (toDrawer) drawer?.dragBy(x / t.drawerWidthPx) else home?.dragBy(-x / pageWidth)
            }
            // Drive: the edge follows the finger from the touch-down point.
            val velocity = VelocityTracker()
            velocity.addPosition(down.uptimeMillis, down.position)
            move(dx)
            while (true) {
                val event = awaitPointerEvent()
                val change = event.changes.firstOrNull { it.id == down.id } ?: break
                velocity.addPointerInputChange(change)
                move(change.positionChange().x)
                change.consume()
                if (!change.pressed) break
            }
            val vx = velocity.calculateVelocity().x
            if (toDrawer) t.onDrawerRelease(vx) else t.onHomeRelease(pageWidth, vx)
        }
    }

internal enum class SwipeClaim { Undecided, Yield, Drawer, Home }

/**
 * A drag of ([dx], [dy]) px so far: still inside the touch slop, someone
 * else's (mostly vertical, or toward a side with nothing to open), or ours.
 * Horizontal has to win clearly, so a diagonal stays undecided a little longer.
 */
internal fun claimSwipe(dx: Float, dy: Float, slop: Float, drawer: Boolean, home: Boolean): SwipeClaim = when {
    abs(dy) > slop && abs(dy) >= abs(dx) -> SwipeClaim.Yield
    abs(dx) > slop && abs(dx) > abs(dy) * 1.2f -> when {
        dx > 0 -> if (drawer) SwipeClaim.Drawer else SwipeClaim.Yield
        else -> if (home) SwipeClaim.Home else SwipeClaim.Yield
    }
    else -> SwipeClaim.Undecided
}

/** A released drawer opens on a rightward fling, closes on a leftward one, else goes to the nearer side. */
internal fun drawerSettlesOpen(progress: Float, velocityX: Float): Boolean = settlesForward(progress, velocityX, past = 0.5f)

/** The drawer clears focus (and the keyboard) once it's all the way open, not at the first pixel of a drag. */
internal fun drawerTakesKeyboard(progress: Float): Boolean = progress >= 1f

/** A released page goes home on a leftward fling or once it's 40% gone; otherwise it springs back. */
internal fun homeSwipeLeaves(progress: Float, velocityX: Float): Boolean = settlesForward(progress, -velocityX, past = 0.4f)

/** [velocity] is px/s toward the end: a fling decides, whatever the distance. */
private fun settlesForward(progress: Float, velocity: Float, past: Float): Boolean = when {
    velocity > FlingVelocity -> true
    velocity < -FlingVelocity -> false
    else -> progress > past
}

/** Whether a chat passes the drawer's project and status filters ([projectId] null: any project). */
internal fun drawerShows(projectId: String?, filter: Filter, rowProjectId: String?, indicator: ChatIndicator, pinned: Boolean): Boolean =
    (projectId == null || rowProjectId == projectId) && when (filter) {
        Filter.All -> true
        Filter.NeedsYou -> indicator == ChatIndicator.AWAITING_INPUT
        Filter.Working -> indicator == ChatIndicator.WORKING
        Filter.Pinned -> pinned
    }

/** The drawer's project filter, or null (all projects) once that project has no chats left to show. */
internal fun drawerProject(projectId: String?, projects: List<ProjectRef>): String? =
    projectId?.takeIf { id -> projects.any { it.id == id } }

/**
 * Checkouts of one repository on several hosts share a name: the host tells
 * them apart. Project id to host, for same-named projects only.
 */
internal fun sameNameHosts(projects: List<ProjectRef>, hostOf: (String) -> String?): Map<String, String> {
    val counts = projects.groupingBy { it.name }.eachCount()
    return projects.filter { counts.getValue(it.name) > 1 }.mapNotNull { p -> hostOf(p.id)?.let { p.id to it } }.toMap()
}

/**
 * The drawer itself: New session, project + status filters, recent chats. Side
 * chats live with their parent: [activeId] in one marks the parent.
 */
@Composable
fun HistoryDrawerContent(
    model: AppModel,
    state: HistoryDrawerState,
    activeId: String?,
    /** Called with the project the drawer is filtered to, if any. */
    onNewSession: (projectId: String?) -> Unit,
    onPick: (String) -> Unit,
    onSearch: () -> Unit,
    onSettings: () -> Unit,
) {
    val workspace by model.workspace.collectAsState()
    var filter by state::filter
    var projectId by state::projectId
    var projectMenu by remember { mutableStateOf(false) }
    val all = remember(workspace) { workspace?.let { frontRows(it) }.orEmpty().sortedByDescending { it.lastActivityMs } }
    val projects = remember(all) { all.mapNotNull { it.project }.distinctBy { it.id } }
    val hosts = remember(workspace, projects) {
        val devices = workspace?.projects.orEmpty().associate { it.id to it.deviceName }
        sameNameHosts(projects) { devices[it] }
    }
    fun host(project: ProjectRef) = hosts[project.id]
    val project = drawerProject(projectId, projects)
    val shown = all.filter { row -> drawerShows(project, filter, row.project?.id, row.indicator, row.pinned) }
    val active = remember(activeId, workspace) { activeId?.let { id -> model.row(id)?.parentChatId ?: id } }
    val colors = MaterialTheme.colorScheme

    Surface(
        Modifier.fillMaxSize(),
        color = colors.surfaceContainerLow,
        shape = AbsoluteRoundedCornerShape(topRight = 28.dp, bottomRight = 28.dp),
        shadowElevation = 12.dp,
    ) {
        Column(
            Modifier.statusBarsPadding().navigationBarsPadding().padding(start = 14.dp, end = 14.dp, top = 24.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(14.dp),
        ) {
            Row(Modifier.fillMaxWidth().padding(horizontal = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                Text("Sessions", Modifier.weight(1f), fontSize = 22.sp, fontWeight = FontWeight.Bold, color = colors.onSurface)
                IconButton(onClick = onSearch) { ZIcon(ZIcons.Search, "Search sessions", Modifier.size(20.dp), tint = colors.onSurfaceVariant) }
            }
            Button(
                onClick = { onNewSession(project) },
                modifier = Modifier.fillMaxWidth().height(48.dp),
                colors = ButtonDefaults.buttonColors(containerColor = colors.primaryContainer, contentColor = colors.onPrimaryContainer),
            ) {
                ZIcon(ZIcons.NewSession, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("New session", fontWeight = FontWeight.SemiBold)
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
                        projects.firstOrNull { it.id == project }?.let { p -> listOfNotNull(p.name, host(p)).joinToString(" · ") } ?: "All projects",
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
                            listOf(MenuChoice("All projects", project == null, leading = { ZIcon(ZIcons.Home, null, Modifier.size(20.dp)) }) { projectId = null }) +
                                projects.map { p ->
                                    MenuChoice(p.name, project == p.id, supporting = host(p), leading = { ProjectTile(p.name, p.colorIndex.toInt(), 20.dp) }) { projectId = p.id }
                                },
                        ),
                    ),
                )
            }
            // Home's filter pills; they scroll sideways when they don't fit (large text).
            Row(Modifier.fillMaxWidth().horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                for (f in Filter.entries) Pill(f.label, filter == f, onClick = { filter = f })
            }
            LazyColumn(Modifier.weight(1f).fillMaxWidth(), contentPadding = PaddingValues(bottom = 8.dp)) {
                if (shown.isEmpty()) {
                    item {
                        Text(
                            "Nothing here right now",
                            Modifier.fillMaxWidth().padding(vertical = 24.dp, horizontal = 8.dp),
                            style = MaterialTheme.typography.bodyMedium,
                            color = colors.onSurfaceVariant,
                            textAlign = TextAlign.Center,
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
                    items(shown, key = { it.id }) { row -> HistoryRow(row, row.id == active) { onPick(row.id) } }
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
            ChatIndicator.AWAITING_INPUT -> colors.primary
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
