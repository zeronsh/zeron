package sh.zeron.android.tools

import android.content.ClipData
import android.content.ClipboardManager
import android.view.HapticFeedbackConstants
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.waitForUpOrCancellation
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.coroutineScope
import kotlinx.coroutines.delay
import kotlinx.coroutines.flow.filterNotNull
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withTimeoutOrNull
import sh.zeron.android.R
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.ui.ActionMenu
import sh.zeron.android.ui.MenuAction
import sh.zeron.android.ui.StatusBanner
import sh.zeron.android.ui.TonalCircleButton
import uniffi.zeron_core.TerminalKey

/**
 * The workspace's terminals on its device: one engine PTY per tab, painted
 * by [TerminalView], with an extra-keys row above the keyboard. Leaving
 * detaches; the shells keep running and reattach with their scrollback.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun TerminalScreen(model: AppModel, ref: WorkspaceRef, onBack: () -> Unit) {
    val key = remember(ref) { Terminals.key(ref) }
    val tabs = remember(key) { Terminals.tabs(key) }
    val selection = remember(key) { Terminals.selection(key) }
    val selected = tabs.firstOrNull { it.id == selection.value } ?: tabs.firstOrNull()
    val client by model.client.collectAsState()
    val context = LocalContext.current
    val clipboard = remember { context.getSystemService(ClipboardManager::class.java) }
    val sticky = remember { StickyKeys() }
    val holder = remember { ViewHolder() }
    var grid by remember { mutableStateOf<Pair<Int, Int>?>(null) }
    var opening by remember { mutableStateOf(false) }
    var error by remember { mutableStateOf<String?>(null) }

    val background = MaterialTheme.colorScheme.surface.toArgb()
    val cursor = MaterialTheme.colorScheme.primary.toArgb()
    val dark = LocalDarkTheme.current
    val palette = remember(dark, background, cursor) { terminalPalette(dark, background, cursor) }

    /** A new terminal, as a new tab or in [replacing]'s place (Restart). */
    fun open(replacing: String? = null) {
        if (opening) return
        opening = true
        error = null
        Terminals.scope.launch {
            val (cols, rows) = grid ?: (80 to 24)
            runCatching { Terminals.open(model, ref, cols, rows) }
                .onSuccess { tab ->
                    if (replacing == null) {
                        Terminals.add(key, tab)
                    } else {
                        Terminals.replace(key, replacing, tab)
                        Terminals.forget(model, ref.deviceId, replacing)
                    }
                }
                .onFailure { error = it.userMessage() }
            opening = false
        }
    }

    fun kill(id: String) {
        val last = tabs.size <= 1
        Terminals.scope.launch { Terminals.kill(model, ref, id) }
        if (last) onBack()
    }

    // First visit: open one, sized to the view once it has measured.
    LaunchedEffect(key, client) {
        if (client == null || tabs.isNotEmpty()) return@LaunchedEffect
        withTimeoutOrNull(1000) { snapshotFlow { grid }.filterNotNull().first() }
        if (tabs.isEmpty()) open()
    }

    val session = remember(selected?.id, client) {
        val c = client
        val tab = selected
        if (c == null || tab == null) {
            null
        } else {
            val (cols, rows) = grid ?: (80 to 24)
            runCatching { TerminalSession(model, c, ref.deviceId, tab.id, cols, rows, palette) }.getOrNull()
        }
    }
    LaunchedEffect(session, palette) { session?.setPalette(palette) }
    val sessionTitle = session?.title
    LaunchedEffect(session, sessionTitle) { session?.let { Terminals.rename(key, it.id, sessionTitle) } }

    fun copy(text: String?) {
        if (!text.isNullOrEmpty()) clipboard?.setPrimaryClip(ClipData.newPlainText("Terminal", text))
    }

    fun paste() {
        val text = clipboard?.primaryClip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.coerceToText(context)?.toString()
        val s = session
        if (!text.isNullOrEmpty() && s != null && s.isAttached) s.screen.paste(text)
    }

    fun copySelection() {
        val s = session?.takeIf { it.isAttached } ?: return
        copy(s.screen.selectionText())
        s.screen.clearSelection()
    }

    var overflow by remember { mutableStateOf(false) }
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surface)) {
        Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
            Column(Modifier.statusBarsPadding()) {
                Row(
                    Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 8.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                    Column(Modifier.weight(1f).padding(horizontal = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        Text(
                            session?.title ?: selected?.shell ?: "Terminal",
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.titleMediumEmphasized,
                        )
                        Text(
                            "${ref.title} @ ${ref.deviceName ?: "device"}",
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                            style = MaterialTheme.typography.bodySmall,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                        )
                    }
                    AnimatedVisibility(session?.hasSelection == true, enter = fadeIn() + scaleIn(), exit = fadeOut() + scaleOut()) {
                        TonalCircleButton(
                            ZIcons.Copy,
                            "Copy",
                            onClick = ::copySelection,
                            modifier = Modifier.padding(end = 8.dp),
                            container = MaterialTheme.colorScheme.primaryContainer,
                            content = MaterialTheme.colorScheme.onPrimaryContainer,
                        )
                    }
                    Box {
                        TonalCircleButton(ZIcons.More, "More", onClick = { overflow = true }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                        ActionMenu(
                            overflow,
                            { overflow = false },
                            listOfNotNull(
                                session?.let { s -> MenuAction("Copy all", ZIcons.Copy) { if (s.isAttached) copy(s.screen.allText()) } },
                                session?.let { MenuAction("Paste", R.drawable.zi_document_add) { paste() } },
                                session?.takeIf { it.scrolledBack }?.let { s ->
                                    MenuAction("Scroll to bottom", ZIcons.ArrowDown) { if (s.isAttached) s.screen.scrollToBottom() }
                                },
                                MenuAction("New terminal", ZIcons.Plus) { open() },
                                selected?.let { t -> MenuAction("Close terminal", ZIcons.Close, destructive = true) { kill(t.id) } },
                            ),
                        )
                    }
                }
                if (tabs.size > 1) {
                    TabStrip(tabs, selected?.id, onSelect = { selection.value = it }, onClose = ::kill, onNew = { open() })
                }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            AndroidView(
                factory = { ctx ->
                    TerminalView(ctx, sticky).also { v ->
                        holder.view = v
                        v.onGrid = { cols, rows -> grid = cols to rows }
                        v.post { v.requestFocus() }
                    }
                },
                update = { v ->
                    v.setColors(background, cursor)
                    v.session = session
                },
                onRelease = { v ->
                    v.session = null
                    holder.view = null
                },
                modifier = Modifier.fillMaxSize(),
            )
            if (error == null && (opening || session?.connecting == true)) {
                LoadingIndicator(Modifier.align(Alignment.Center).size(48.dp))
            }
            // Qualified: the outer Column's scoped overload would shadow it.
            androidx.compose.animation.AnimatedVisibility(
                session?.scrolledBack == true,
                enter = fadeIn() + scaleIn(),
                exit = fadeOut() + scaleOut(),
                modifier = Modifier.align(Alignment.BottomEnd).padding(12.dp),
            ) {
                SmallFloatingActionButton(
                    onClick = { session?.takeIf { it.isAttached }?.screen?.scrollToBottom() },
                    shape = CircleShape,
                    containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                    contentColor = MaterialTheme.colorScheme.onSurface,
                ) { ZIcon(ZIcons.ArrowDown, "Scroll to bottom", Modifier.size(22.dp)) }
            }
        }
        Column(Modifier.fillMaxWidth().imePadding().navigationBarsPadding()) {
            val exit = session?.exitCode
            when {
                client == null -> StatusBanner("Not connected.", null)
                error != null -> StatusBanner(
                    error.orEmpty(),
                    if (selected == null) "Retry" to { open() } else "Dismiss" to { error = null },
                )
                exit != null && selected != null -> StatusBanner(
                    if (exit < 0) "Terminal is no longer running" else "Process exited ($exit)",
                    "Restart" to { open(replacing = selected.id) },
                )
            }
            ExtraKeys(
                sticky = sticky,
                onKey = { k -> session?.pressKey(k, sticky) },
                onText = { t -> session?.typeText(t, sticky) },
                onKeyboard = { holder.view?.toggleKeyboard() },
                onPaste = ::paste,
            )
        }
    }
}

private class ViewHolder {
    var view: TerminalView? = null
}

/** The open terminals as chips, with "+" for another. */
@Composable
private fun TabStrip(tabs: List<TerminalTab>, selected: String?, onSelect: (String) -> Unit, onClose: (String) -> Unit, onNew: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .horizontalScroll(rememberScrollState())
            .padding(start = 12.dp, end = 12.dp, bottom = 8.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        for (tab in tabs) {
            val active = tab.id == selected
            val container by animateColorAsState(
                if (active) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
                MaterialTheme.motionScheme.defaultEffectsSpec(),
                label = "tab",
            )
            Surface(
                onClick = { onSelect(tab.id) },
                shape = RoundedCornerShape(50),
                color = container,
                contentColor = if (active) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurfaceVariant,
            ) {
                Row(
                    Modifier.height(36.dp).padding(start = 12.dp, end = if (active) 4.dp else 14.dp),
                    verticalAlignment = Alignment.CenterVertically,
                    horizontalArrangement = Arrangement.spacedBy(6.dp),
                ) {
                    ZIcon(ZIcons.Terminal, null, Modifier.size(16.dp))
                    Text(
                        tab.label,
                        style = MaterialTheme.typography.labelLarge,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                        modifier = Modifier.widthIn(max = 160.dp),
                    )
                    if (active) {
                        Box(
                            Modifier.size(28.dp).clip(CircleShape).clickable { onClose(tab.id) },
                            contentAlignment = Alignment.Center,
                        ) { ZIcon(ZIcons.Close, "Close terminal", Modifier.size(14.dp)) }
                    }
                }
            }
        }
        TonalCircleButton(ZIcons.Plus, "New terminal", onClick = onNew, size = 36.dp, container = MaterialTheme.colorScheme.surfaceContainerHighest)
    }
}

/** Keys a phone keyboard lacks: Esc, Tab, sticky Ctrl/Alt, arrows (auto-repeating), symbols, paging. */
@Composable
private fun ExtraKeys(
    sticky: StickyKeys,
    onKey: (TerminalKey) -> Unit,
    onText: (String) -> Unit,
    onKeyboard: () -> Unit,
    onPaste: () -> Unit,
) {
    Surface(color = MaterialTheme.colorScheme.surfaceContainerHigh) {
        Row(
            Modifier
                .fillMaxWidth()
                .horizontalScroll(rememberScrollState())
                .padding(horizontal = 8.dp, vertical = 6.dp),
            horizontalArrangement = Arrangement.spacedBy(6.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            ExtraKey(icon = R.drawable.zi_keyboard, label = "Keyboard", onClick = onKeyboard)
            ExtraKey("Esc") { onKey(TerminalKey.ESCAPE) }
            ExtraKey("Tab") { onKey(TerminalKey.TAB) }
            ExtraKey("Ctrl", armed = sticky.ctrl) { sticky.ctrl = !sticky.ctrl }
            ExtraKey("Alt", armed = sticky.alt) { sticky.alt = !sticky.alt }
            RepeatKey(R.drawable.zi_arrow_left, "Left") { onKey(TerminalKey.LEFT) }
            RepeatKey(R.drawable.zi_arrow_up, "Up") { onKey(TerminalKey.UP) }
            RepeatKey(ZIcons.ArrowDown, "Down") { onKey(TerminalKey.DOWN) }
            RepeatKey(R.drawable.zi_arrow_right, "Right") { onKey(TerminalKey.RIGHT) }
            for (symbol in listOf("|", "~", "/", "-")) ExtraKey(symbol) { onText(symbol) }
            ExtraKey("Home") { onKey(TerminalKey.HOME) }
            ExtraKey("End") { onKey(TerminalKey.END) }
            ExtraKey("PgUp") { onKey(TerminalKey.PAGE_UP) }
            ExtraKey("PgDn") { onKey(TerminalKey.PAGE_DOWN) }
            ExtraKey("Paste", onClick = onPaste)
        }
    }
}

private val KeyShape = RoundedCornerShape(12.dp)

@Composable
private fun ExtraKey(
    label: String,
    armed: Boolean = false,
    @DrawableRes icon: Int? = null,
    onClick: () -> Unit,
) {
    val view = LocalView.current
    val container by animateColorAsState(
        if (armed) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.surfaceContainerHighest,
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "key",
    )
    Surface(
        onClick = {
            view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
            onClick()
        },
        shape = KeyShape,
        color = container,
        contentColor = if (armed) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.height(40.dp).widthIn(min = 44.dp),
    ) { KeyLabel(label, icon) }
}

/** A key that fires on press and repeats while held (the arrows). */
@Composable
private fun RepeatKey(@DrawableRes icon: Int, label: String, onPress: () -> Unit) {
    val view = LocalView.current
    val fire by rememberUpdatedState(onPress)
    var pressed by remember { mutableStateOf(false) }
    Surface(
        shape = KeyShape,
        color = if (pressed) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
        contentColor = MaterialTheme.colorScheme.onSurface,
        modifier = Modifier
            .height(40.dp)
            .widthIn(min = 44.dp)
            .semantics {
                role = Role.Button
                contentDescription = label
            }
            .pointerInput(Unit) {
                coroutineScope {
                    awaitEachGesture {
                        awaitFirstDown()
                        pressed = true
                        view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
                        fire()
                        val repeat = launch {
                            delay(400)
                            while (true) {
                                fire()
                                delay(50)
                            }
                        }
                        waitForUpOrCancellation()
                        repeat.cancel()
                        pressed = false
                    }
                }
            },
    ) { KeyLabel(label, icon) }
}

@Composable
private fun KeyLabel(label: String, @DrawableRes icon: Int?) {
    Box(Modifier.padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
        if (icon != null) {
            ZIcon(icon, label, Modifier.size(18.dp))
        } else {
            Text(label, fontFamily = GeistMono, style = MaterialTheme.typography.labelLarge, maxLines = 1)
        }
    }
}
