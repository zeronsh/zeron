package sh.zeron.android.ui

import android.view.HapticFeedbackConstants
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.TerminalTab
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.SubagentRow
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.TerminalPhase

/** The companions sheet's sections (each chip opens its own). */
enum class CompanionSection(val title: String) { Agents("Agents"), SideChats("Side chats"), Terminals("Terminals") }

/** What's working alongside one session: its subagents, side chats and terminals. */
@Stable
class Companions(val agents: List<SubagentRow>, val sideChats: List<SessionRow>, val terminals: List<TerminalTab>) {
    val runningAgents: Int get() = agents.count { it.state == SubagentState.RUNNING }
    val liveTerminals: Int get() = terminals.count { it.isLive }
    val isEmpty: Boolean get() = agents.isEmpty() && sideChats.isEmpty() && terminals.isEmpty()
}

/**
 * A session's companions, re-read as it changes. Streaming changes the
 * session every frame; this settles a few times a second (a subagent count
 * doesn't need 120 Hz). A side chat's own siblings live with its parent.
 */
@Composable
fun rememberCompanions(model: AppModel, chatId: String, includeSideChats: Boolean = true): Companions {
    fun load() = Companions(
        model.sessionSubagents(chatId),
        if (includeSideChats) model.sideChats(chatId) else emptyList(),
        model.terminals.tabs(chatId),
    )
    var value by remember(chatId) { mutableStateOf(load()) }
    LaunchedEffect(chatId, includeSideChats) {
        val dirty = Channel<Unit>(Channel.CONFLATED)
        launch { model.sessionEvents.collect { if (it == chatId) dirty.trySend(Unit) } }
        launch { model.workspace.collect { dirty.trySend(Unit) } }
        launch { model.terminals.changes.collect { if (it == chatId) dirty.trySend(Unit) } }
        for (unit in dirty) {
            value = load()
            delay(250)
        }
    }
    return value
}

/**
 * Chips above the composer for what's working alongside the session — "2
 * working" while subagents run, side chats, live terminals — each a tap from
 * its section of the companions sheet. Nothing shows when there's nothing.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CompanionStrip(c: Companions, onTap: (CompanionSection) -> Unit, modifier: Modifier = Modifier) {
    val view = LocalView.current
    fun tap(s: CompanionSection) {
        view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
        onTap(s)
    }
    Row(modifier.animateContentSize(MaterialTheme.motionScheme.defaultSpatialSpec()), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        AnimatedVisibility(c.agents.isNotEmpty(), enter = fadeIn() + expandHorizontally(), exit = fadeOut() + shrinkHorizontally()) {
            val running = c.runningAgents
            CompanionChip(
                icon = if (running > 0) null else ZIcons.Bot,
                text = if (running > 0) "$running working" else "${c.agents.size}",
                label = if (running > 0) "$running subagents working" else "${c.agents.size} subagents",
                spinning = running > 0,
            ) { tap(CompanionSection.Agents) }
        }
        AnimatedVisibility(c.sideChats.isNotEmpty(), enter = fadeIn() + expandHorizontally(), exit = fadeOut() + shrinkHorizontally()) {
            CompanionChip(ZIcons.SideChats, "${c.sideChats.size}", "${c.sideChats.size} side chats") { tap(CompanionSection.SideChats) }
        }
        AnimatedVisibility(c.terminals.isNotEmpty(), enter = fadeIn() + expandHorizontally(), exit = fadeOut() + shrinkHorizontally()) {
            CompanionChip(ZIcons.Terminal, "${c.terminals.size}", "${c.terminals.size} terminals", dot = if (c.liveTerminals > 0) successColor() else null) { tap(CompanionSection.Terminals) }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CompanionChip(@DrawableRes icon: Int?, text: String, label: String, spinning: Boolean = false, dot: Color? = null, onClick: () -> Unit) {
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(50),
        color = if (spinning) MaterialTheme.colorScheme.primaryContainer else MaterialTheme.colorScheme.surfaceContainerHigh,
        contentColor = if (spinning) MaterialTheme.colorScheme.onPrimaryContainer else MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.height(32.dp).semantics { contentDescription = label },
    ) {
        Row(Modifier.padding(start = if (spinning) 6.dp else 11.dp, end = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            if (spinning) LoadingIndicator(Modifier.size(22.dp), color = MaterialTheme.colorScheme.primary)
            icon?.let { ZIcon(it, null, Modifier.size(16.dp)) }
            Spacer(Modifier.width(if (spinning) 3.dp else 6.dp))
            Text(text, style = MaterialTheme.typography.labelLargeEmphasized)
            dot?.let {
                Spacer(Modifier.width(6.dp))
                Box(Modifier.size(7.dp).clip(CircleShape).background(it))
            }
        }
    }
}

/**
 * Everything working alongside one session (iOS `CompanionsViewController`):
 * the subagents its agent spawned (live first), its side chats, and its
 * terminals — each opens in place, and each section can start another.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CompanionsSheet(
    model: AppModel,
    chatId: String,
    title: String,
    initialSection: CompanionSection?,
    onDismiss: () -> Unit,
    onOpenSubagent: (SubagentRow) -> Unit,
    onOpenSideChat: (String) -> Unit,
    onOpenTerminal: (TerminalTab?) -> Unit,
) {
    val sheet = rememberModalBottomSheetState()
    val scope = rememberCoroutineScope()
    ModalBottomSheet(onDismissRequest = onDismiss, sheetState = sheet, containerColor = MaterialTheme.colorScheme.surfaceContainerLow) {
        CompanionsContent(
            model, chatId, title, initialSection,
            dismissThen = { run ->
                scope.launch { sheet.hide() }.invokeOnCompletion {
                    onDismiss()
                    run()
                }
            },
            onOpenSubagent = onOpenSubagent,
            onOpenSideChat = onOpenSideChat,
            onOpenTerminal = onOpenTerminal,
        )
    }
}

/**
 * The companions list itself: in the sheet on phones, in the inspector beside
 * the session on large screens. `dismissThen` puts the sheet away (if any)
 * before opening what was picked.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun CompanionsContent(
    model: AppModel,
    chatId: String,
    title: String,
    initialSection: CompanionSection?,
    dismissThen: (() -> Unit) -> Unit,
    onOpenSubagent: (SubagentRow) -> Unit,
    onOpenSideChat: (String) -> Unit,
    onOpenTerminal: (TerminalTab?) -> Unit,
    modifier: Modifier = Modifier,
    /** Shown in place of the big title (the inspector's own header). */
    showTitle: Boolean = true,
) {
    val c = rememberCompanions(model, chatId)
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    var forking by remember { mutableStateOf(false) }
    // Running agents' elapsed time ticks on screen only.
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            now = System.currentTimeMillis()
        }
    }
    fun newSideChat() {
        val id = model.createSideChat(chatId) ?: return model.toast("Couldn't start a side chat")
        dismissThen { onOpenSideChat(id) }
    }
    fun fork() {
        if (forking) return
        forking = true
        scope.launch {
            try {
                val id = model.forkSideChat(chatId)
                view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                dismissThen { onOpenSideChat(id) }
            } catch (e: Exception) {
                model.toast(forkError(e))
            } finally {
                forking = false
            }
        }
    }

    val agents = remember(c.agents) {
        // Live first, then newest.
        c.agents.withIndex().sortedWith(compareByDescending<IndexedValue<SubagentRow>> { it.value.state == SubagentState.RUNNING }.thenByDescending { it.index }).map { it.value }
    }
    val list = rememberLazyListState()
    LaunchedEffect(initialSection) {
        val section = initialSection ?: return@LaunchedEffect
        // Items: title, [agents header + rows], side chats header + rows (or empty text + 2 actions), terminals…
        val first = if (showTitle) 1 else 0
        val sides = first + if (agents.isNotEmpty()) agents.size + 1 else 0
        val index = when (section) {
            CompanionSection.Agents -> first
            CompanionSection.SideChats -> sides
            CompanionSection.Terminals -> sides + 1 + if (c.sideChats.isEmpty()) 3 else c.sideChats.size
        }
        delay(150)
        list.animateScrollToItem(index.coerceAtLeast(0))
    }

    run {
        LazyColumn(state = list, contentPadding = PaddingValues(start = 16.dp, end = 16.dp, bottom = 32.dp), modifier = modifier.fillMaxWidth()) {
            if (showTitle) item("title") {
                Column(Modifier.padding(start = 8.dp, bottom = 4.dp)) {
                    Text(title, style = MaterialTheme.typography.headlineSmallEmphasized, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val running = c.runningAgents
                    Text(
                        if (running > 0) "$running agent${if (running == 1) "" else "s"} working" else "Agents, side chats & terminals",
                        style = MaterialTheme.typography.bodyMedium,
                        color = if (running > 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
            if (agents.isNotEmpty()) {
                header(CompanionSection.Agents)
                itemsIndexed(agents, key = { _, a -> "agent/${a.id}" }) { i, a ->
                    AgentRow(a, i, agents.size, now, Modifier.animateItem()) { onOpen -> if (onOpen) dismissThen { onOpenSubagent(a) } }
                }
            }
            header(CompanionSection.SideChats) {
                var menu by remember { mutableStateOf(false) }
                Box {
                    TonalCircleButton(ZIcons.Plus, "Add side chat", onClick = { menu = true }, size = 36.dp)
                    ActionMenu(
                        menu,
                        { menu = false },
                        listOf(
                            MenuAction("New side chat", ZIcons.SideChats) { newSideChat() },
                            MenuAction("Fork this chat", ZIcons.Fork) { fork() },
                        ),
                    )
                }
            }
            if (c.sideChats.isEmpty()) {
                empty("side-empty", "Ask something on the side without derailing the main thread.")
                item("side-new") { ActionRow(ZIcons.SideChats, "New side chat", 0, 2, Modifier.animateItem()) { newSideChat() } }
                item("side-fork") { ActionRow(ZIcons.Fork, if (forking) "Forking…" else "Fork this chat", 1, 2, Modifier.animateItem(), busy = forking) { fork() } }
            } else {
                itemsIndexed(c.sideChats, key = { _, r -> "side/${r.id}" }) { i, row ->
                    Swipeable(ZIcons.Archive, "Archive", Modifier.animateItem(), onSwipe = { model.archive(row.id) }, index = i, count = c.sideChats.size) {
                        SideChatRow(row, i, c.sideChats.size) { dismissThen { onOpenSideChat(row.id) } }
                    }
                }
            }
            header(CompanionSection.Terminals) {
                TonalCircleButton(ZIcons.Plus, "New terminal", onClick = { dismissThen { onOpenTerminal(null) } }, size = 36.dp)
            }
            if (c.terminals.isEmpty()) {
                empty("term-empty", "A shell on this session's computer, in its folder.")
                item("term-new") { ActionRow(ZIcons.Terminal, "New terminal", 0, 1, Modifier.animateItem()) { dismissThen { onOpenTerminal(null) } } }
            } else {
                itemsIndexed(c.terminals, key = { _, t -> "term/${t.ref.id}" }) { i, tab ->
                    Swipeable(ZIcons.Close, "Close", Modifier.animateItem(), onSwipe = { model.terminals.close(tab) }, index = i, count = c.terminals.size, destructive = true) {
                        TerminalRow(tab, model.deviceName(tab.ref.deviceId), i, c.terminals.size) { dismissThen { onOpenTerminal(tab) } }
                    }
                }
            }
        }
    }
}

fun forkError(e: Exception): String = when (e) {
    is CoreException.HostUnavailable -> "Its computer isn't reachable right now"
    is CoreException.Unsupported -> "Its computer needs a newer Zeron to fork"
    else -> "Couldn't fork this chat"
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
private fun LazyListScope.header(section: CompanionSection, action: (@Composable () -> Unit)? = null) {
    item("h-${section.name}") {
        Row(
            Modifier.fillMaxWidth().animateItem().padding(start = 12.dp, top = 20.dp, bottom = 8.dp).heightIn(min = 36.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text(section.title, style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.primary, modifier = Modifier.weight(1f))
            action?.invoke()
        }
    }
}

private fun LazyListScope.empty(key: String, text: String) {
    item(key) {
        Text(
            text,
            style = MaterialTheme.typography.bodyMedium,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.animateItem().padding(start = 12.dp, end = 12.dp, bottom = 10.dp),
        )
    }
}

/** A status tile: the session row's harness tile, toned for what it shows. */
@Composable
private fun Tile(container: Color, content: @Composable () -> Unit) {
    Box(Modifier.size(44.dp).clip(RoundedCornerShape(14.dp)).background(container), contentAlignment = Alignment.Center) { content() }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun AgentRow(a: SubagentRow, index: Int, count: Int, now: Long, modifier: Modifier, onClick: (Boolean) -> Unit) {
    val detail = SubagentFormat.detail(a)
    val scheme = MaterialTheme.colorScheme
    SegmentedListItem(
        onClick = { onClick(a.docId != null) },
        enabled = a.docId != null,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor(), disabledContainerColor = cardColor()),
        modifier = modifier.padding(bottom = ListItemDefaults.SegmentedGap).semantics {
            contentDescription = "${a.title}, " + when (a.state) {
                SubagentState.RUNNING -> "working"
                SubagentState.FAILED -> "failed"
                SubagentState.DONE -> "done"
            }
        },
        leadingContent = {
            when (a.state) {
                SubagentState.RUNNING -> Tile(scheme.primaryContainer) { LoadingIndicator(Modifier.size(30.dp)) }
                SubagentState.DONE -> Tile(successColor().copy(alpha = 0.14f)) { ZIcon(ZIcons.Check, null, Modifier.size(20.dp), tint = successColor()) }
                SubagentState.FAILED -> Tile(scheme.errorContainer) { ZIcon(ZIcons.Close, null, Modifier.size(20.dp), tint = scheme.onErrorContainer) }
            }
        },
        supportingContent = {
            Text(
                when (a.state) {
                    SubagentState.RUNNING -> a.tail ?: detail.ifEmpty { "Working" }
                    SubagentState.DONE -> detail.ifEmpty { "Finished" }
                    SubagentState.FAILED -> if (detail.isEmpty()) "Didn't finish" else "Failed · $detail"
                },
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        },
        trailingContent = if (a.state == SubagentState.RUNNING) {
            { Text(SubagentFormat.elapsedSince(a.startedAtMs, now), style = MaterialTheme.typography.labelLarge, color = scheme.primary) }
        } else if (a.docId != null) {
            { ZIcon(ZIcons.ChevronRight, null, Modifier.size(18.dp), tint = scheme.outline) }
        } else {
            null
        },
    ) { Text(a.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SideChatRow(row: SessionRow, index: Int, count: Int, onClick: () -> Unit) {
    SegmentedListItem(
        onClick = onClick,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = { HarnessTile(row) },
        supportingContent = { Text(row.preview ?: "No messages yet", maxLines = 1, overflow = TextOverflow.Ellipsis) },
        trailingContent = { StatusLabel(row) },
    ) {
        Text(
            if (row.hasTitle) row.title else "Side chat",
            style = if (row.unseen) MaterialTheme.typography.titleMediumEmphasized else MaterialTheme.typography.titleMedium,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun TerminalRow(tab: TerminalTab, host: String, index: Int, count: Int, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val (status, tone) = when (tab.phase) {
        is TerminalPhase.Live, is TerminalPhase.Connecting -> "Live" to successColor()
        is TerminalPhase.Reconnecting -> "Reconnecting" to scheme.onSurfaceVariant
        is TerminalPhase.Exited, is TerminalPhase.Gone -> "Ended" to scheme.outline
    }
    SegmentedListItem(
        onClick = onClick,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = { Tile(chipContainer()) { ZIcon(ZIcons.Terminal, null, Modifier.size(22.dp), tint = scheme.onSurfaceVariant) } },
        supportingContent = { Text("${tab.folder} · $host", maxLines = 1, overflow = TextOverflow.Ellipsis) },
        trailingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                if (tab.isLive) {
                    Box(Modifier.size(7.dp).clip(CircleShape).background(tone))
                    Spacer(Modifier.width(5.dp))
                }
                Text(status, style = MaterialTheme.typography.labelLarge, color = tone)
            }
        },
        modifier = Modifier.semantics { contentDescription = "${tab.name}, $status" },
    ) { Text(tab.name, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ActionRow(@DrawableRes icon: Int, text: String, index: Int, count: Int, modifier: Modifier, busy: Boolean = false, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    SegmentedListItem(
        onClick = onClick,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = {
            Tile(scheme.primaryContainer) {
                if (busy) LoadingIndicator(Modifier.size(28.dp)) else ZIcon(icon, null, Modifier.size(22.dp), tint = scheme.onPrimaryContainer)
            }
        },
        modifier = modifier.padding(bottom = ListItemDefaults.SegmentedGap),
    ) { Text(text, style = MaterialTheme.typography.titleMediumEmphasized, color = scheme.primary) }
}

/** Swipe left for the row's one action (archive a side chat, close a terminal). */
@OptIn(ExperimentalMaterial3Api::class)
@Composable
private fun Swipeable(
    @DrawableRes icon: Int,
    label: String,
    modifier: Modifier,
    onSwipe: () -> Unit,
    index: Int,
    count: Int,
    destructive: Boolean = false,
    content: @Composable () -> Unit,
) {
    val state = rememberSwipeToDismissBoxState()
    Box(modifier.padding(bottom = ListItemDefaults.SegmentedGap)) {
        SwipeToDismissBox(
            state = state,
            enableDismissFromStartToEnd = false,
            onDismiss = { if (it == SwipeToDismissBoxValue.EndToStart) onSwipe() },
            backgroundContent = {
                val scheme = MaterialTheme.colorScheme
                Surface(
                    Modifier.fillMaxSize(),
                    shape = segmentedShapes(index, count).shape,
                    color = if (destructive) scheme.errorContainer else scheme.tertiaryContainer,
                    contentColor = if (destructive) scheme.onErrorContainer else scheme.onTertiaryContainer,
                ) {
                    Box(Modifier.padding(horizontal = 28.dp), contentAlignment = Alignment.CenterEnd) { ZIcon(icon, label) }
                }
            },
        ) { content() }
    }
}
