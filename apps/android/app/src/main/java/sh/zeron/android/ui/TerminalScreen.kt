package sh.zeron.android.ui

import android.content.ClipData
import android.content.ClipboardManager
import android.view.HapticFeedbackConstants
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.slideInVertically
import androidx.compose.animation.slideOutVertically
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.isImeVisible
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FloatingActionButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.VerticalDivider
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.semantics.stateDescription
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.viewinterop.AndroidView
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.TerminalTab
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.terminal.TerminalView
import uniffi.zeron_core.CoreException
import uniffi.zeron_core.KeyModifiers
import uniffi.zeron_core.TerminalKey
import uniffi.zeron_core.TerminalPhase

/** Tap: for the next key. Tap again while armed: locked. Again: off. */
enum class Latch { Off, Once, Locked }

/** The terminal's page tone (iOS `TerminalViewController.background`). */
@Composable
fun terminalBackground(): Color = if (LocalDarkTheme.current) Color(0xFF090909) else Color(0xFFFAFAFB)

/**
 * A chat's terminal: a shell on the chat's host, in its folder (iOS
 * `TerminalViewController`). The title switches between the chat's
 * terminals (and opens another); the key bar rides the keyboard; connection,
 * exit and reconnect states show on a pill at the bottom.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class, androidx.compose.foundation.layout.ExperimentalLayoutApi::class)
@Composable
fun TerminalScreen(
    model: AppModel,
    chatId: String,
    terminalId: String? = null,
    onBack: () -> Unit,
    script: List<String> = emptyList(),
    /** The keyboard comes up with a new shell on a phone; beside other columns it waits for a tap. */
    raiseKeyboard: Boolean = true,
) {
    val client by model.client.collectAsState()
    val context = LocalContext.current
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    val density = LocalDensity.current.density
    val store = model.terminals
    // Re-read the chat's tabs when they come, go, or change phase.
    var storeRevision by remember { mutableIntStateOf(0) }
    LaunchedEffect(chatId) { store.changes.collect { if (it == chatId) storeRevision++ } }
    val tabs = remember(storeRevision, client) { store.tabs(chatId) }
    var current by remember {
        mutableStateOf(store.tabs(chatId).let { all -> all.firstOrNull { it.ref.id == terminalId } ?: if (terminalId == null) all.firstOrNull { it.isLive } else null })
    }
    var opening by remember { mutableStateOf(false) }
    var failure by remember { mutableStateOf<String?>(null) }
    var focused by remember { mutableStateOf(false) }
    // The key bar rides the software keyboard (a hardware keyboard needs neither).
    val imeUp = androidx.compose.foundation.layout.WindowInsets.isImeVisible
    var hasContent by remember { mutableStateOf(false) }
    var ctrl by remember { mutableStateOf(Latch.Off) }
    var alt by remember { mutableStateOf(Latch.Off) }
    var terminal by remember { mutableStateOf<TerminalView?>(null) }
    var pendingScript by remember { mutableStateOf(script) }

    // Pair with the host while the screen slides in.
    LaunchedEffect(chatId) { client?.warmDirect(chatId) }

    fun show(tab: TerminalTab) {
        val wasFocused = terminal?.hasFocus() == true
        current = tab
        hasContent = false
        failure = null
        if (wasFocused) view.post { terminal?.showKeyboard() }
    }

    fun openAnother() {
        if (opening) return
        opening = true
        failure = null
        val (cols, rows) = terminal?.grid ?: TerminalView.guessGrid(view.width, view.height, density)
        scope.launch {
            try {
                val tab = store.open(chatId, cols, rows)
                view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
                show(tab)
                if (raiseKeyboard) view.post { terminal?.showKeyboard() }
                // A screenshot script to type once the shell is up (debug launch extra).
                val lines = pendingScript
                pendingScript = emptyList()
                if (lines.isNotEmpty()) launch {
                    for (line in lines) {
                        delay(900)
                        for (ch in line) {
                            tab.session.typeText(ch.toString(), TerminalView.NO_MODS)
                            delay(45)
                        }
                        tab.session.press(TerminalKey.Enter, TerminalView.NO_MODS)
                        delay(2400)
                    }
                }
            } catch (e: Exception) {
                failure = describeTerminalError(e)
            } finally {
                opening = false
            }
        }
    }

    LaunchedEffect(Unit) { if (current == null) openAnother() }

    fun addToMessage(text: String) {
        val trimmed = text.trim('\n')
        if (trimmed.isEmpty()) return
        model.appendToComposer(chatId, "```\n$trimmed\n```\n")
        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
        model.toast("Added to message")
    }

    fun closeCurrent() {
        val tab = current ?: return
        store.close(tab)
        val next = store.tabs(chatId).firstOrNull()
        if (next != null) show(next) else onBack()
    }

    val tab = current
    val phase = tab?.phase
    val host = tab?.let { model.deviceName(it.ref.deviceId) }
    val fg = MaterialTheme.colorScheme.onSurface.toArgb()
    val accent = MaterialTheme.colorScheme.primary.toArgb()
    val dark = LocalDarkTheme.current
    LaunchedEffect(dark) { store.dark = dark }

    Column(Modifier.fillMaxSize().background(terminalBackground())) {
        // Header: back, the tab switcher, new terminal and the menu.
        Row(
            Modifier.fillMaxWidth().statusBarsPadding().padding(horizontal = 12.dp, vertical = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            var switcher by remember { mutableStateOf(false) }
            Box(Modifier.weight(1f).padding(horizontal = 8.dp), contentAlignment = Alignment.Center) {
                Column(
                    Modifier.clip(RoundedCornerShape(16.dp)).clickable(onClickLabel = "Switch terminal") { switcher = true }.padding(horizontal = 12.dp, vertical = 4.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        Text(tab?.name ?: "Terminal", maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMediumEmphasized, modifier = Modifier.weight(1f, fill = false))
                        Spacer(Modifier.width(4.dp))
                        ZIcon(ZIcons.ChevronDown, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    if (tab != null) {
                        Text("${tab.folder} · $host", maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                }
                ChoiceMenu(
                    switcher,
                    { switcher = false },
                    listOf(
                        MenuSection(null, tabs.map { t ->
                            MenuChoice(t.name, t === current, supporting = t.folder + if (t.isLive) "" else " · ended", leading = { ZIcon(ZIcons.Terminal, null, Modifier.size(20.dp)) }) { show(t) }
                        }),
                        MenuSection(null, listOf(MenuChoice("New terminal", leading = { ZIcon(ZIcons.Plus, null, Modifier.size(20.dp)) }) { openAnother() })),
                    ),
                )
            }
            TonalCircleButton(ZIcons.Plus, "New terminal", onClick = { openAnother() }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
            Spacer(Modifier.width(8.dp))
            var menu by remember { mutableStateOf(false) }
            Box {
                TonalCircleButton(ZIcons.More, "Terminal options", onClick = { menu = true }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                GroupedActionMenu(
                    menu,
                    { menu = false },
                    listOf(
                        listOf(
                            MenuAction("Bigger text", ZIcons.Plus) { terminal?.let { it.setFontSize(minOf(TerminalView.MAX_SIZE, it.fontSize + 1)) } },
                            MenuAction("Smaller text", ZIcons.Minus) { terminal?.let { it.setFontSize(maxOf(TerminalView.MIN_SIZE, it.fontSize - 1)) } },
                            MenuAction("Reset text size", ZIcons.Text) { terminal?.setFontSize(TerminalView.DEFAULT_SIZE) },
                        ),
                        listOfNotNull(
                            tab?.let { t -> MenuAction("Add output to message", ZIcons.Chat) { addToMessage(t.session.text(false)) } },
                            tab?.let { t ->
                                MenuAction("Copy all output", ZIcons.Copy) {
                                    context.getSystemService(ClipboardManager::class.java).setPrimaryClip(ClipData.newPlainText("Terminal", t.session.text(true)))
                                    model.toast("Copied")
                                }
                            },
                            tab?.let { t -> MenuAction("Clear screen", ZIcons.Restart) { t.session.typeText("l", KeyModifiers(ctrl = true, alt = false, shift = false)) } },
                        ),
                        listOfNotNull(tab?.let { MenuAction("Close terminal", ZIcons.Close, destructive = true) { closeCurrent() } }),
                    ),
                )
            }
        }

        Box(Modifier.weight(1f).fillMaxWidth()) {
            if (tab != null) {
                androidx.compose.runtime.key(tab) {
                    val terminalView = remember(tab) {
                        TerminalView(context, tab).also { t ->
                            t.onContent = { hasContent = true }
                            t.onFocusChanged = { focused = it }
                            t.onAddToMessage = { addToMessage(it) }
                        }
                    }
                    DisposableEffect(terminalView) {
                        terminal = terminalView
                        onDispose { if (terminal === terminalView) terminal = null }
                    }
                    terminalView.modifiers = { KeyModifiers(ctrl = ctrl != Latch.Off, alt = alt != Latch.Off, shift = false) }
                    terminalView.consumeModifiers = {
                        if (ctrl == Latch.Once) ctrl = Latch.Off
                        if (alt == Latch.Once) alt = Latch.Off
                    }
                    AndroidView(
                        factory = { terminalView },
                        update = {
                            it.foreground = fg
                            it.accent = accent
                        },
                        modifier = Modifier.fillMaxSize().semantics { contentDescription = "Terminal" },
                    )
                }
            }
            // A spinner until the shell paints something.
            val loading = opening || (phase is TerminalPhase.Connecting && !hasContent && tab?.hasContentNow() != true)
            androidx.compose.animation.AnimatedVisibility(loading && failure == null, enter = fadeIn(), exit = fadeOut(), modifier = Modifier.align(Alignment.Center)) {
                LoadingIndicator(Modifier.size(48.dp))
            }
            val status: Triple<String, Boolean, Pair<String, () -> Unit>?>? = when {
                failure != null -> Triple(failure!!, true, "Try again" to { openAnother() })
                phase is TerminalPhase.Reconnecting -> Triple("Reconnecting…", false, null)
                phase is TerminalPhase.Exited -> Triple(
                    phase.signal?.let { "Ended by $it" } ?: if (phase.code == 0) "Process exited" else "Exited with code ${phase.code}",
                    false,
                    "New terminal" to {
                        current?.let(store::forget)
                        openAnother()
                    },
                )
                phase is TerminalPhase.Gone -> Triple("This terminal ended on its host", false, "New terminal" to { openAnother() })
                else -> null
            }
            androidx.compose.animation.AnimatedVisibility(
                status != null,
                enter = fadeIn() + slideInVertically { it / 2 },
                exit = fadeOut() + slideOutVertically { it / 2 },
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 16.dp),
            ) {
                status?.let { (text, error, action) -> TerminalStatusPill(text, error, spinning = phase is TerminalPhase.Reconnecting, action = action) }
            }
            // Shows the keyboard again once it's been put away.
            val ended = tab?.isLive == false
            androidx.compose.animation.AnimatedVisibility(
                !imeUp && !ended && tab != null && status == null,
                enter = fadeIn() + scaleIn(initialScale = 0.6f),
                exit = fadeOut() + scaleOut(targetScale = 0.6f),
                modifier = Modifier.align(Alignment.BottomEnd).navigationBarsPadding().padding(16.dp),
            ) {
                FloatingActionButton(
                    onClick = { terminal?.showKeyboard() },
                    containerColor = MaterialTheme.colorScheme.secondaryContainer,
                    contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
                ) { ZIcon(ZIcons.Keyboard, "Show keyboard", Modifier.size(24.dp)) }
            }
        }
        // The keys a phone keyboard lacks, riding on top of it.
        androidx.compose.animation.AnimatedVisibility(focused && imeUp, enter = fadeIn() + slideInVertically { it }, exit = fadeOut() + slideOutVertically { it }) {
            TerminalKeyBar(
                ctrl = ctrl,
                alt = alt,
                onCtrl = { ctrl = it },
                onAlt = { alt = it },
                onKey = { terminal?.press(it) },
                onText = { terminal?.insertText(it) },
                onPaste = { terminal?.pasteFromClipboard() },
                onDismiss = { terminal?.hideKeyboard() },
            )
        }
        Spacer(Modifier.imePadding().navigationBarsPadding())
    }
}

private fun TerminalTab.hasContentNow(): Boolean = session.frame().lines.any { it.runs.isNotEmpty() }

fun describeTerminalError(e: Exception): String = when (e) {
    is CoreException.Unsupported -> "This computer needs a newer Zeron for terminals"
    is CoreException.HostUnavailable -> "The computer isn't reachable right now"
    else -> "Couldn't open a terminal"
}

/** Connection and exit state, with the one way forward. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun TerminalStatusPill(text: String, error: Boolean, spinning: Boolean, action: Pair<String, () -> Unit>?) {
    Surface(
        shape = RoundedCornerShape(50),
        color = MaterialTheme.colorScheme.surfaceContainerHighest,
        shadowElevation = 2.dp,
        modifier = Modifier.semantics { contentDescription = listOfNotNull(text, action?.first).joinToString(", ") },
    ) {
        Row(Modifier.heightIn(min = 48.dp).padding(start = 16.dp, end = if (action != null) 6.dp else 18.dp), verticalAlignment = Alignment.CenterVertically) {
            if (spinning) {
                LoadingIndicator(Modifier.size(22.dp))
            } else {
                Box(Modifier.size(8.dp).clip(CircleShape).background(if (error) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.outline))
            }
            Spacer(Modifier.width(10.dp))
            Text(text, style = MaterialTheme.typography.labelLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.widthIn(max = 220.dp))
            action?.let { (label, run) -> TextButton(onClick = run) { Text(label, style = MaterialTheme.typography.labelLargeEmphasized) } }
        }
    }
}

/**
 * The keys a phone keyboard lacks (iOS `TerminalKeyBar`): Esc, Tab, sticky
 * Ctrl and Alt (tap for the next key, tap again to lock), arrows that repeat
 * while held, the shell's usual punctuation, paste, and putting the keyboard
 * away.
 */
@Composable
fun TerminalKeyBar(
    ctrl: Latch,
    alt: Latch,
    onCtrl: (Latch) -> Unit,
    onAlt: (Latch) -> Unit,
    onKey: (TerminalKey) -> Unit,
    onText: (String) -> Unit,
    onPaste: () -> Unit,
    onDismiss: () -> Unit,
) {
    val view = LocalView.current
    fun tick() = view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
    fun next(l: Latch) = when (l) {
        Latch.Off -> Latch.Once
        Latch.Once -> Latch.Locked
        Latch.Locked -> Latch.Off
    }
    val edge = MaterialTheme.colorScheme.surfaceContainer
    Surface(color = edge, modifier = Modifier.fillMaxWidth()) {
        Row(Modifier.fillMaxWidth().height(52.dp), verticalAlignment = Alignment.CenterVertically) {
            // Keys slide under a soft edge instead of being cut.
            Row(
                Modifier
                    .weight(1f)
                    .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
                    .drawWithContent {
                        drawContent()
                        val fade = 14.dp.toPx() / size.width
                        drawRect(
                            Brush.horizontalGradient(0f to Color.Transparent, fade to Color.Black, 1f - fade to Color.Black, 1f to Color.Transparent),
                            blendMode = BlendMode.DstIn,
                        )
                    }
                    .horizontalScroll(rememberScrollState())
                    .padding(horizontal = 8.dp),
                horizontalArrangement = Arrangement.spacedBy(6.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                Key("esc", label = "Escape") { tick(); onKey(TerminalKey.Escape) }
                Key("ctrl", label = "Control", latch = ctrl) { view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK); onCtrl(next(ctrl)) }
                Key("tab", label = "Tab") { tick(); onKey(TerminalKey.Tab) }
                RepeatingKey(ZIcons.ArrowUp, "Up") { onKey(TerminalKey.Up) }
                RepeatingKey(ZIcons.ChevronDown, "Down") { onKey(TerminalKey.Down) }
                RepeatingKey(ZIcons.ArrowLeft, "Left") { onKey(TerminalKey.Left) }
                RepeatingKey(ZIcons.ArrowRight, "Right") { onKey(TerminalKey.Right) }
                Key("alt", label = "Alt", latch = alt) { view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK); onAlt(next(alt)) }
                KeyDivider()
                for (t in listOf("~", "/", "-", "|", "*", "$", "`")) Key(t, label = t) { tick(); onText(t) }
                KeyDivider()
                Key(icon = ZIcons.Paste, label = "Paste") { tick(); onPaste() }
            }
            Key(icon = ZIcons.KeyboardHide, label = "Hide keyboard", modifier = Modifier.padding(start = 2.dp, end = 8.dp)) { onDismiss() }
        }
    }
}

@Composable
private fun keyColors(latch: Latch): Pair<Color, Color> {
    val scheme = MaterialTheme.colorScheme
    val (bg, fg) = when (latch) {
        Latch.Off -> scheme.surfaceContainerHighest to scheme.onSurface
        Latch.Once -> scheme.primary.copy(alpha = 0.75f) to scheme.onPrimary
        Latch.Locked -> scheme.primary to scheme.onPrimary
    }
    val animatedBg by animateColorAsState(bg, MaterialTheme.motionScheme.fastEffectsSpec(), label = "key")
    return animatedBg to fg
}

@Composable
private fun Key(
    text: String? = null,
    @DrawableRes icon: Int? = null,
    label: String,
    latch: Latch = Latch.Off,
    modifier: Modifier = Modifier,
    onClick: () -> Unit,
) {
    val (bg, fg) = keyColors(latch)
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(12.dp),
        color = bg,
        contentColor = fg,
        modifier = modifier.height(38.dp).widthIn(min = 44.dp).semantics {
            contentDescription = label
            if (latch != Latch.Off) stateDescription = if (latch == Latch.Locked) "Locked" else "On"
        },
    ) {
        Box(Modifier.padding(horizontal = 12.dp), contentAlignment = Alignment.Center) {
            if (text != null) Text(text, fontFamily = GeistMono, style = MaterialTheme.typography.labelLarge)
            if (icon != null) ZIcon(icon, null, Modifier.size(20.dp))
        }
    }
}

/** Arrows: one step on press, then repeat while held. */
@Composable
private fun RepeatingKey(@DrawableRes icon: Int, label: String, onKey: () -> Unit) {
    val view = LocalView.current
    val scope = rememberCoroutineScope()
    var pressed by remember { mutableStateOf(false) }
    val scale by animateFloatAsState(if (pressed) 0.92f else 1f, MaterialTheme.motionScheme.fastSpatialSpec(), label = "press")
    Box(
        Modifier
            .height(38.dp)
            .widthIn(min = 44.dp)
            .graphicsLayer { scaleX = scale; scaleY = scale }
            .clip(RoundedCornerShape(12.dp))
            .background(if (pressed) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest)
            .semantics { contentDescription = label }
            .pointerInput(Unit) {
                detectTapGestures(onPress = {
                    pressed = true
                    view.performHapticFeedback(HapticFeedbackConstants.KEYBOARD_TAP)
                    onKey()
                    val repeat = scope.launch {
                        delay(380)
                        while (true) {
                            onKey()
                            delay(60)
                        }
                    }
                    tryAwaitRelease()
                    repeat.cancel()
                    pressed = false
                })
            }
            .padding(horizontal = 12.dp),
        contentAlignment = Alignment.Center,
    ) { ZIcon(icon, null, Modifier.size(20.dp)) }
}

@Composable
private fun KeyDivider() {
    VerticalDivider(Modifier.height(22.dp).padding(horizontal = 2.dp), color = MaterialTheme.colorScheme.outlineVariant)
}
