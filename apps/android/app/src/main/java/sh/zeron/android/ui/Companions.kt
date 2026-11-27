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
import androidx.compose.material3.IconButton
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
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.key
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
import uniffi.zeron_core.InputRequest
import uniffi.zeron_core.SessionHandle
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.SubagentRow
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.TerminalPhase
import uniffi.zeron_core.UserInputAnswer

/** The companions sheet's sections (each chip opens its own). */
enum class CompanionSection(val title: String) { Agents("Agents"), SideChats("Side chats"), Terminals("Terminals") }

/** What's working alongside one session: its subagents, side chats and terminals. */
@Stable
class Companions(val agents: List<SubagentRow>, val sideChats: List<SessionRow>, val terminals: List<TerminalTab>) {
    val runningAgents: Int get() = agents.count { it.state == SubagentState.RUNNING }
    /** Chats it delegated to that ask the person something (answered on their rows). */
    val askingAgents: Int get() = agents.count { it.asks }
    val liveTerminals: Int get() = terminals.count { it.isLive }
    val isEmpty: Boolean get() = agents.isEmpty() && sideChats.isEmpty() && terminals.isEmpty()
}

/** A chat its parent's agent delegated to (a chat of its own, not a spawn in the parent's runtime). */
val SubagentRow.isDelegated: Boolean get() = chatId != null

/** A session's side chats among its children: the chats it delegated to are its subagents instead. */
internal fun sideChatsOf(children: List<SessionRow>): List<SessionRow> = children.filter { !it.archived && !it.delegated }

/**
 * The Agents section's order: its own spawns (live first, then newest), then
 * the chats it delegated to (any that ask first, then live ones, oldest first
 * as the core lists them).
 */
internal fun orderedAgents(agents: List<SubagentRow>): List<SubagentRow> {
    val (delegated, native) = agents.partition { it.isDelegated }
    val live = compareByDescending<IndexedValue<SubagentRow>> { it.value.state == SubagentState.RUNNING }
    return native.withIndex().sortedWith(live.thenByDescending { it.index }).map { it.value } +
        delegated.withIndex().sortedWith(compareByDescending<IndexedValue<SubagentRow>> { it.value.asks }.then(live).thenBy { it.index }).map { it.value }
}

/** A delegated row's word on it: its agent and, when it isn't the parent's, its device ("Codex · Build box"). */
internal fun delegatedDetail(a: SubagentRow): String = listOfNotNull(a.kind, a.deviceName).joinToString(" · ")

/** What an approval would run: its fenced or backticked part, if it has one (the desktop's ask card shows it as code). */
internal fun askCommand(question: String): String? {
    Regex("```[^\\n]*\\n([\\s\\S]*?)```").find(question)?.let { return it.groupValues[1].trim().ifEmpty { null } }
    return Regex("`([^`]+)`").find(question)?.groupValues?.get(1)?.trim()?.ifEmpty { null }
}

/** A question answered in place: one question, a single pick, a handful of options. */
internal fun answersInPlace(input: InputRequest): Boolean =
    !input.isApproval && input.questions.size == 1 && input.questions[0].let { !it.multiSelect && it.options.size in 1..5 }

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
            val asking = c.askingAgents
            when {
                // A subagent's ask outranks the work: it waits on the person, and its row answers it.
                asking > 0 -> CompanionChip(
                    icon = ZIcons.StatusInput,
                    text = "$asking needs you",
                    label = if (asking == 1) "A subagent needs you" else "$asking subagents need you",
                    attention = true,
                ) { tap(CompanionSection.Agents) }
                else -> CompanionChip(
                    icon = if (running > 0) null else ZIcons.Bot,
                    text = if (running > 0) "$running working" else "${c.agents.size}",
                    label = if (running > 0) "$running subagents working" else "${c.agents.size} subagents",
                    spinning = running > 0,
                ) { tap(CompanionSection.Agents) }
            }
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
private fun CompanionChip(@DrawableRes icon: Int?, text: String, label: String, spinning: Boolean = false, attention: Boolean = false, dot: Color? = null, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(50),
        // Needs you: the transcript's "Needs you" ink, as a tint.
        color = when {
            attention -> attentionContainer()
            spinning -> scheme.primaryContainer
            else -> scheme.surfaceContainerHigh
        },
        contentColor = when {
            attention -> warningColor()
            spinning -> scheme.onPrimaryContainer
            else -> scheme.onSurfaceVariant
        },
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

    val agents = remember(c.agents) { orderedAgents(c.agents) }
    // What delegated chats ask, read from their own composers while they ask.
    val asks = rememberChildAsks(model, remember(c.agents) { c.agents.filter { it.asks }.mapNotNull { it.chatId } })
    // The section's segments: each agent's row, and under one that asks, its ask, joined to it.
    val segments = remember(agents, asks.keys) { agents.flatMap { a -> if (a.chatId in asks) listOf(a to false, a to true) else listOf(a to false) } }
    fun openAgent(a: SubagentRow) {
        val chat = a.chatId
        if (chat != null) dismissThen { onOpenSideChat(chat) } else if (a.docId != null) dismissThen { onOpenSubagent(a) }
    }
    val list = rememberLazyListState()
    LaunchedEffect(initialSection) {
        val section = initialSection ?: return@LaunchedEffect
        // Items: title, [agents header + rows], side chats header + rows (or empty text + 2 actions), terminals…
        val first = if (showTitle) 1 else 0
        val sides = first + if (segments.isNotEmpty()) segments.size + 1 else 0
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
                    CompanionsSubtitle(c)
                }
            }
            if (segments.isNotEmpty()) {
                header(CompanionSection.Agents)
                itemsIndexed(segments, key = { _, (a, ask) -> (if (ask) "ask/" else "agent/") + a.id }) { i, (a, ask) ->
                    val chat = a.chatId
                    when {
                        ask && chat != null -> asks[chat]?.let { AskCard(it, i, segments.size, Modifier.animateItem()) { openAgent(a) } }
                        chat != null -> DelegatedRow(a, asks[chat]?.input, i, segments.size, now, Modifier.animateItem(), onStop = { model.interruptChat(chat) }) { openAgent(a) }
                        else -> AgentRow(a, i, segments.size, now, Modifier.animateItem()) { onOpen -> if (onOpen) openAgent(a) }
                    }
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

/** The companions title's second line: who needs the person, else how many work. */
@Composable
fun CompanionsSubtitle(c: Companions) {
    val running = c.runningAgents
    val asking = c.askingAgents
    val scheme = MaterialTheme.colorScheme
    Text(
        when {
            asking > 0 -> if (asking == 1) "A subagent needs you" else "$asking subagents need you"
            running > 0 -> "$running agent${if (running == 1) "" else "s"} working"
            else -> "Agents, side chats & terminals"
        },
        style = MaterialTheme.typography.bodyMedium,
        color = when {
            asking > 0 -> warningColor()
            running > 0 -> scheme.primary
            else -> scheme.onSurfaceVariant
        },
    )
}

/** Behind something that needs the person: the transcript's "Needs you" ink as a wash. */
@Composable
fun attentionContainer(): Color = warningColor().copy(alpha = if (sh.zeron.android.design.LocalDarkTheme.current) 0.18f else 0.13f)

/**
 * What one delegated chat asks: its own session, held only while it asks,
 * and its composer's open input, re-read as that chat changes.
 */
@Stable
class ChildAsk(val chatId: String, private val handle: SessionHandle) {
    /** Null until its composer has the question (or once it's answered). */
    var input by mutableStateOf(runCatching { handle.composer().openInput }.getOrNull())
        private set

    fun refresh() {
        input = runCatching { handle.composer().openInput }.getOrNull()
    }

    fun answer(questionId: String, label: String): Boolean =
        input?.let { i -> runCatching { handle.respondInput(i.requestId, listOf(UserInputAnswer(questionId, listOf(label)))) }.isSuccess } ?: false
}

/**
 * The asks of the delegated chats in `chatIds` (those whose rows say they
 * ask). Each chat's session opens while it's listed and is let go after;
 * nothing opens on its own (the person answers on the row, or opens it).
 */
@Composable
fun rememberChildAsks(model: AppModel, chatIds: List<String>): Map<String, ChildAsk> {
    val client by model.client.collectAsState()
    val core = client ?: return emptyMap()
    val out = LinkedHashMap<String, ChildAsk>()
    for (id in chatIds) key(id) {
        val handle = remember(core) { runCatching { core.session(id) ?: core.openSession(id) }.getOrNull() }
        if (handle != null) {
            val ask = remember(handle) { ChildAsk(id, handle) }
            DisposableEffect(handle) {
                onDispose {
                    handle.close()
                    // Warm only while someone looks at it (its own screen keeps it).
                    if (id !in model.openChats) runCatching { core.closeSession(id) }
                }
            }
            LaunchedEffect(handle) {
                ask.refresh()
                val dirty = Channel<Unit>(Channel.CONFLATED)
                launch { model.sessionEvents.collect { if (it == id) dirty.trySend(Unit) } }
                launch { model.workspace.collect { dirty.trySend(Unit) } }
                for (unit in dirty) ask.refresh()
            }
            out[id] = ask
        }
    }
    return out
}

/**
 * A chat the session's agent delegated to (the desktop's subagent tray row):
 * how it stands, its agent's mark, its title, its agent and device, and while
 * it runs how long and Stop. The whole row opens it, a chat of its own.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun DelegatedRow(a: SubagentRow, input: InputRequest?, index: Int, count: Int, now: Long, modifier: Modifier, onStop: () -> Unit, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val running = a.state == SubagentState.RUNNING
    val detail = delegatedDetail(a)
    val approval = input?.isApproval == true
    val view = LocalView.current
    SegmentedListItem(
        onClick = onOpen,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        modifier = modifier.padding(bottom = ListItemDefaults.SegmentedGap).semantics {
            contentDescription = "${a.title}, " + listOf(
                when {
                    a.asks -> if (approval) "needs approval" else "has a question"
                    running -> "working"
                    a.state == SubagentState.FAILED -> "failed"
                    else -> "done"
                },
                detail,
            ).filter { it.isNotEmpty() }.joinToString(", ")
        },
        leadingContent = {
            when {
                // Its ask, as the desktop marks it: an approval in the "Needs you" ink, a question in the accent.
                a.asks && approval -> Tile(attentionContainer()) { ZIcon(ZIcons.StatusApproval, null, Modifier.size(22.dp), tint = warningColor()) }
                a.asks -> Tile(scheme.primaryContainer) { ZIcon(ZIcons.StatusInput, null, Modifier.size(22.dp), tint = scheme.primary) }
                running -> Tile(scheme.primaryContainer) { LoadingIndicator(Modifier.size(30.dp)) }
                a.state == SubagentState.DONE -> Tile(successColor().copy(alpha = 0.14f)) { ZIcon(ZIcons.Check, null, Modifier.size(20.dp), tint = successColor()) }
                else -> Tile(scheme.errorContainer) { ZIcon(ZIcons.Close, null, Modifier.size(20.dp), tint = scheme.onErrorContainer) }
            }
        },
        supportingContent = {
            Row(verticalAlignment = Alignment.CenterVertically) {
                sh.zeron.android.design.HarnessMark(a.harness, 14.dp, tint = scheme.onSurfaceVariant)
                Spacer(Modifier.width(6.dp))
                Text(
                    when {
                        a.state == SubagentState.FAILED -> if (detail.isEmpty()) "Didn't finish" else "Failed · $detail"
                        else -> detail.ifEmpty { a.harness ?: "Subagent" }
                    },
                    maxLines = 1,
                    overflow = TextOverflow.Ellipsis,
                )
            }
        },
        trailingContent = if (running) {
            {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text(SubagentFormat.elapsedSince(a.startedAtMs, now), style = MaterialTheme.typography.labelLarge, color = if (a.asks) scheme.onSurfaceVariant else scheme.primary)
                    // A chat of its own: its host stops it (the desktop row's Stop).
                    IconButton(onClick = {
                        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                        onStop()
                    }, modifier = Modifier.size(40.dp)) {
                        ZIcon(ZIcons.Stop, "Stop ${a.title}", Modifier.size(18.dp), tint = scheme.onSurfaceVariant)
                    }
                }
            }
        } else {
            { ZIcon(ZIcons.ChevronRight, null, Modifier.size(18.dp), tint = scheme.outline) }
        },
    ) { Text(a.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
}

/** How long a pressed answer holds (the others out of reach) when its chat hasn't moved on (the desktop's ASK_HOLD). */
private const val ASK_HOLD_MS = 4_000L

/**
 * What a delegated chat asks, joined under its row (the desktop's ask card):
 * an approval's command as code, then Deny and Allow once at the end; a
 * question's words and its options, and its chat a tap away.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class, androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
private fun AskCard(ask: ChildAsk, index: Int, count: Int, modifier: Modifier, onOpen: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val view = LocalView.current
    val input = ask.input
    // An answer pressed holds until the ask clears (a new one is a new card) or a moment passes.
    var held by remember(input?.requestId) { mutableStateOf<String?>(null) }
    LaunchedEffect(held) {
        if (held == null) return@LaunchedEffect
        delay(ASK_HOLD_MS)
        held = null
    }
    fun answer(questionId: String, label: String) {
        if (held != null) return
        held = label
        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
        if (!ask.answer(questionId, label)) {
            held = null
            view.performHapticFeedback(HapticFeedbackConstants.REJECT)
        }
    }
    Surface(
        shape = segmentedShapes(index, count).shape,
        color = cardColor(),
        modifier = modifier.fillMaxWidth().padding(bottom = ListItemDefaults.SegmentedGap),
    ) {
        Column(
            // Under the row's words: past its tile.
            Modifier.padding(start = ASK_INSET, end = 16.dp, top = 12.dp, bottom = 12.dp),
            verticalArrangement = Arrangement.spacedBy(10.dp),
        ) {
            val q = input?.questions?.firstOrNull()
            if (input == null || q == null) {
                Text("Needs you", style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant)
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.End) {
                    androidx.compose.material3.TextButton(onClick = onOpen) { Text("Answer in its chat") }
                }
            } else if (input.isApproval) {
                val command = askCommand(q.question)
                if (command != null) {
                    Surface(shape = RoundedCornerShape(12.dp), color = chipContainer(), modifier = Modifier.fillMaxWidth()) {
                        androidx.compose.foundation.text.selection.SelectionContainer {
                            Text(
                                command,
                                fontFamily = sh.zeron.android.design.GeistMono,
                                style = MaterialTheme.typography.bodyMedium,
                                color = scheme.onSurface,
                                maxLines = 8,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.padding(horizontal = 12.dp, vertical = 10.dp),
                            )
                        }
                    }
                } else {
                    Text(q.question, style = MaterialTheme.typography.bodyMedium, maxLines = 6, overflow = TextOverflow.Ellipsis)
                }
                Row(Modifier.fillMaxWidth(), horizontalArrangement = Arrangement.spacedBy(8.dp, Alignment.End), verticalAlignment = Alignment.CenterVertically) {
                    androidx.compose.material3.TextButton(onClick = { answer(q.id, "No") }, enabled = held == null || held == "No") { Busy(held == "No", "Deny") }
                    androidx.compose.material3.Button(onClick = { answer(q.id, "Yes") }, enabled = held == null || held == "Yes") { Busy(held == "Yes", "Allow once") }
                }
            } else {
                Text(q.question, style = MaterialTheme.typography.bodyMedium, maxLines = 6, overflow = TextOverflow.Ellipsis)
                androidx.compose.foundation.layout.FlowRow(
                    horizontalArrangement = Arrangement.spacedBy(8.dp),
                    verticalArrangement = Arrangement.spacedBy(4.dp),
                    itemVerticalAlignment = Alignment.CenterVertically,
                ) {
                    if (answersInPlace(input)) {
                        for (option in q.options) {
                            androidx.compose.material3.FilledTonalButton(onClick = { answer(q.id, option) }, enabled = held == null || held == option) {
                                Busy(held == option, option)
                            }
                        }
                    }
                    androidx.compose.material3.TextButton(onClick = onOpen, enabled = held == null) { Text("Answer in its chat") }
                }
            }
        }
    }
}

/**
 * An answer's label, with a spinner while it's the one pressed (the
 * desktop's busy answer): it keeps its look while the others step back.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Busy(busy: Boolean, label: String) {
    if (busy) {
        LoadingIndicator(Modifier.size(20.dp), color = androidx.compose.material3.LocalContentColor.current)
        Spacer(Modifier.width(6.dp))
    }
    Text(label, maxLines = 1, overflow = TextOverflow.Ellipsis)
}

/** A list item's leading inset to its words: its padding, its 44 dp tile and the gap. */
private val ASK_INSET = 76.dp

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
    val state = rememberSwipeRowState()
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
