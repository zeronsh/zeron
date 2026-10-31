package sh.zeron.android.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.expandHorizontally
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkHorizontally
import androidx.compose.animation.slideInHorizontally
import androidx.compose.animation.slideOutHorizontally
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.key
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import androidx.compose.ui.Modifier
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import uniffi.zeron_core.SubagentState

/** What the inspector column shows, a stack per session (iPad's inspector navigation). */
sealed interface InspectorPage {
    /** The companions list, scrolled to a section. */
    data class Companions(val section: CompanionSection?) : InspectorPage
    /** A terminal on `chat`'s host (a side chat's own; null: the inspector's session). */
    data class Terminal(val terminalId: String?, val chat: String? = null) : InspectorPage
    data class Subagent(val parent: String, val doc: String, val title: String, val running: Boolean) : InspectorPage
    data class SideChat(val id: String) : InspectorPage
}

/**
 * The large-screen shell's state (iOS `SplitRootController`): the open
 * session, the sidebar's mode, the Settings dialog, and the inspector beside
 * the session holding its companions.
 */
@Stable
class TabletShellState {
    /** The session in the main column (null: the new-session page). */
    var selected by mutableStateOf<String?>(null)
        private set
    /** The user's sidebar (Ctrl+B); it also steps aside for the inspector on narrower screens. */
    var sidebar by mutableStateOf(true)
    /** The sidebar shows search instead of the sessions list. */
    var searching by mutableStateOf(false)
    /** The Settings dialog's first page, while it's up. */
    var settings by mutableStateOf<String?>(null)
    /** Bumped to rebuild (and focus) the new-session page. */
    var draftToken by mutableIntStateOf(0)
        private set
    /** The session whose companions the inspector holds. */
    var inspectorChat by mutableStateOf<String?>(null)
        private set
    val inspector = mutableStateListOf<InspectorPage>()
    val inspectorShown: Boolean get() = inspectorChat != null && inspector.isNotEmpty()

    fun select(id: String?) {
        selected = id
        // Another session came up: the open panel follows it.
        if (inspectorShown && inspectorChat != id) {
            if (id == null) hideInspector() else show(id, listOf(InspectorPage.Companions(null)))
        }
    }

    fun newSession() {
        select(null)
        draftToken++
    }

    /** The companions panel beside the session, or put away when it's already showing it. */
    fun toggleCompanions(chatId: String, section: CompanionSection?) {
        if (inspectorShown && inspectorChat == chatId && inspector.size == 1 && section == null) return hideInspector()
        show(chatId, listOf(InspectorPage.Companions(section)))
    }

    /** A companion screen in the inspector, over the session's panel so Back returns to it. */
    fun push(chatId: String, page: InspectorPage) {
        if (inspectorChat != chatId || inspector.isEmpty()) show(chatId, listOf(InspectorPage.Companions(null)))
        inspector.add(page)
    }

    fun pop() {
        if (inspector.size > 1) inspector.removeAt(inspector.lastIndex) else hideInspector()
    }

    fun hideInspector() {
        inspector.clear()
        inspectorChat = null
    }

    private fun show(chatId: String, pages: List<InspectorPage>) {
        inspectorChat = chatId
        inspector.clear()
        inspector.addAll(pages)
    }
}

/**
 * Zeron on a large screen (iPad's split): the phone's Sessions page as a
 * sidebar (with its bottom bar), the open session — or the new-session page —
 * in the main column, and the session's companions in an inspector column
 * beside it. Narrower screens let the sidebar step aside while the inspector
 * shows.
 */
@Composable
fun TabletShell(
    model: AppModel,
    shell: TabletShellState,
    roomy: Boolean,
    sidebarWidth: androidx.compose.ui.unit.Dp,
    sidebar: @Composable () -> Unit,
    session: @Composable (String) -> Unit,
    draft: @Composable () -> Unit,
    page: @Composable (chatId: String, page: InspectorPage) -> Unit,
) {
    val sidebarShown = shell.sidebar && !(shell.inspectorShown && !roomy)
    // Back steps through the inspector's pages and the sidebar's search before leaving.
    androidx.activity.compose.BackHandler(shell.inspectorShown) { shell.pop() }
    androidx.activity.compose.BackHandler(!shell.inspectorShown && shell.searching) { shell.searching = false }
    Row(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        AnimatedVisibility(sidebarShown, enter = expandHorizontally() + fadeIn(), exit = shrinkHorizontally() + fadeOut()) {
            Row(Modifier.fillMaxHeight()) {
                // About a phone's width, so the rows read as they do on a phone.
                Box(Modifier.width(sidebarWidth).fillMaxHeight()) { sidebar() }
                VerticalDivider(color = MaterialTheme.colorScheme.outlineVariant)
            }
        }
        Box(Modifier.weight(1f).fillMaxHeight()) {
            val id = shell.selected
            if (id != null) {
                key(id) { session(id) }
            } else {
                key(shell.draftToken) { draft() }
            }
        }
        AnimatedVisibility(
            shell.inspectorShown,
            enter = expandHorizontally() + fadeIn(),
            exit = shrinkHorizontally() + fadeOut(),
        ) {
            Row(Modifier.fillMaxHeight()) {
                VerticalDivider(color = MaterialTheme.colorScheme.outlineVariant)
                Box(Modifier.width(400.dp).fillMaxHeight().background(MaterialTheme.colorScheme.surfaceContainerLow)) {
                    val chat = shell.inspectorChat
                    val top = shell.inspector.lastOrNull()
                    if (chat != null && top != null) {
                        // Pushed pages slide in from the trailing edge, popped ones back out.
                        AnimatedContent(
                            shell.inspector.size to top,
                            transitionSpec = {
                                val forward = targetState.first >= initialState.first
                                (slideInHorizontally { if (forward) it else -it / 3 } + fadeIn()) togetherWith
                                    (slideOutHorizontally { if (forward) -it / 3 else it } + fadeOut())
                            },
                            label = "inspector",
                        ) { (_, p) -> page(chat, p) }
                    }
                }
            }
        }
    }
}

/** One inspector page: the companions list, or a companion screen pushed over it. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun InspectorPageContent(model: AppModel, shell: TabletShellState, chatId: String, page: InspectorPage) {
    when (page) {
        is InspectorPage.Companions -> Column(Modifier.fillMaxSize()) {
            val c = rememberCompanions(model, chatId)
            val title = model.row(chatId)?.let { if (it.parentChatId != null) "Side chat" else it.title } ?: "Session"
            Row(Modifier.fillMaxWidth().statusBarsPadding().padding(start = 20.dp, end = 12.dp, top = 8.dp, bottom = 4.dp), verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(title, style = MaterialTheme.typography.titleLargeEmphasized, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    val running = c.runningAgents
                    Text(
                        if (running > 0) "$running agent${if (running == 1) "" else "s"} working" else "Agents, side chats & terminals",
                        style = MaterialTheme.typography.bodyMedium,
                        color = if (running > 0) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                TonalCircleButton(sh.zeron.android.design.ZIcons.Close, "Close", onClick = shell::hideInspector, size = 44.dp, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            }
            Spacer(Modifier.size(8.dp))
            CompanionsContent(
                model,
                chatId,
                title,
                page.section,
                dismissThen = { it() },
                onOpenSubagent = { a ->
                    a.docId?.let { doc -> shell.push(chatId, InspectorPage.Subagent(chatId, doc, a.title, a.state == SubagentState.RUNNING)) }
                },
                onOpenSideChat = { id -> shell.push(chatId, InspectorPage.SideChat(id)) },
                onOpenTerminal = { tab -> shell.push(chatId, InspectorPage.Terminal(tab?.ref?.id)) },
                showTitle = false,
                modifier = Modifier.weight(1f),
            )
        }
        is InspectorPage.Terminal -> TerminalScreen(model, page.chat ?: chatId, terminalId = page.terminalId, onBack = shell::pop, raiseKeyboard = false)
        is InspectorPage.Subagent -> SubagentScreen(
            model,
            parentId = page.parent,
            docId = page.doc,
            title = page.title,
            running = page.running,
            onBack = shell::pop,
            onOpenSubagent = { parent, doc, title, running -> shell.push(chatId, InspectorPage.Subagent(parent, doc, title, running)) },
        )
        // A side chat beside its parent; its own companions use the sheet.
        is InspectorPage.SideChat -> SessionScreen(
            model,
            page.id,
            onBack = shell::pop,
            onOpenSideChat = { id -> shell.push(chatId, InspectorPage.SideChat(id)) },
            onOpenChat = shell::select,
            onOpenTerminal = { chat, terminal -> shell.push(chatId, InspectorPage.Terminal(terminal, chat)) },
            onOpenSubagent = { parent, doc, title, running -> shell.push(chatId, InspectorPage.Subagent(parent, doc, title, running)) },
        )
    }
}
