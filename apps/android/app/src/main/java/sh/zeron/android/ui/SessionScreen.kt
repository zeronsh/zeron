package sh.zeron.android.ui

import android.content.Intent
import android.graphics.BitmapFactory
import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.animation.animateColorAsState
import sh.zeron.android.design.ZIcon
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.statusBarsPadding
import sh.zeron.android.design.ZIcons
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.text.selection.SelectionContainer
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SmallFloatingActionButton
import androidx.compose.material3.Surface
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.height
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.graphics.ImageBitmap
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.foundation.background
import androidx.compose.ui.draw.drawBehind
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.withContext
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.Geist
import sh.zeron.android.design.GeistMono
import sh.zeron.android.transcript.Transcript
import sh.zeron.android.transcript.TranscriptActions
import sh.zeron.android.transcript.TranscriptState
import uniffi.zeron_core.ComposerState
import uniffi.zeron_core.ConnectivityState
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionHandle

private data class TextSheet(val title: String, val text: String, val mono: Boolean)

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SessionScreen(
    model: AppModel,
    chatId: String,
    onBack: () -> Unit,
    onOpenVoice: () -> Unit = {},
    onVoiceSettings: () -> Unit = {},
    onOpenSideChat: (String) -> Unit = {},
    onOpenTerminal: (chatId: String, terminalId: String?) -> Unit = { _, _ -> },
    onOpenSubagent: (parentId: String, docId: String, title: String, running: Boolean) -> Unit = { _, _, _, _ -> },
    /** Debug launch extra: open a companion once the session is up (`companions`, `terminal`, `side-chat:<id>`, `subagent:<tool id>`). */
    openCompanion: String? = null,
    /** An agent's logins (an error card's "Log in": `signIn` starts one at once). */
    onOpenAccounts: (AccountsTarget, signIn: Boolean) -> Unit = { _, _ -> },
    onOpenBilling: () -> Unit = {},
    /** Companions open beside the session (the large-screen inspector) instead of over it. */
    onShowCompanions: ((CompanionSection?) -> Unit)? = null,
) {
    val client by model.client.collectAsState()
    val core = client ?: return
    val handle: SessionHandle = remember(chatId) { runCatching { core.openSession(chatId) }.getOrNull() } ?: run {
        // Never a stand-in transcript: back to Sessions with a note.
        LaunchedEffect(chatId) {
            model.toast("Couldn't open session. Try again.")
            onBack()
        }
        return
    }
    val transcript = remember(chatId) { TranscriptState() }
    val density = LocalDensity.current.density
    DisposableEffect(chatId) {
        transcript.engine.attach(core, chatId)
        // Agent visuals run their scripts from the chat's host.
        model.row(chatId)?.deviceId?.let { transcript.setVisualSource(core, it, maxOf(2f, density)) }
        handle.setViewAttached(true)
        model.markSeen(chatId)
        model.openChats.add(chatId)
        onDispose {
            model.openChats.remove(chatId)
            handle.setViewAttached(false)
            transcript.close()
            // What arrived while it was open has been seen too.
            model.markSeen(chatId)
            // A side chat started here and left without a word: gone.
            val unused = runCatching { handle.transcriptStatus().let { it.entryCount == 0u && it.pendingCount == 0u } && handle.composer().queue.isEmpty() }.getOrDefault(false)
            if (model.isFreshSideChat(chatId) && unused && model.drafts.load(chatId).isEmpty() && model.row(chatId)?.preview == null) {
                model.forgetFreshSideChat(chatId)
                model.delete(chatId)
            }
        }
    }

    // Composer-facing state: re-pulled on this chat's events.
    var composer by remember { mutableStateOf(handle.composer()) }
    val workspace by model.workspace.collectAsState()
    val connectivity by model.connectivity.collectAsState()
    LaunchedEffect(chatId) {
        model.sessionEvents.collect { if (it == chatId) composer = handle.composer() }
    }
    // Upload rings on pending thumbnails follow the escort.
    LaunchedEffect(composer.transferProgress) { transcript.uploadProgress = composer.transferProgress }
    val row = remember(workspace, chatId) { model.row(chatId) }

    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var sheet by remember { mutableStateOf<TextSheet?>(null) }
    var viewing by remember { mutableStateOf<ImageBitmap?>(null) }
    val actions = remember(chatId) {
        suspend fun load(ref: String): ImageBitmap? {
            val device = handle.composer().host.deviceId
            return runCatching {
                val bytes = core.readAttachment(device, ref)
                withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap() }
            }.getOrNull()
        }
        val open: (String) -> Unit = { ref -> scope.launch { load(ref)?.let { viewing = it } } }
        TranscriptActions(
            openUrl = { url ->
                val uri = Uri.parse(url)
                when (uri.scheme) {
                    "http", "https" -> CustomTabsIntent.Builder().build().launchUrl(context, uri)
                    // An image chip opens its upload full size, as its thumbnail did.
                    "zeron-preview" -> uri.getQueryParameter("ref")?.let(open)
                    else -> runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, uri)) }
                }
            },
            loadImage = ::load,
            showText = { title, text, mono -> sheet = TextSheet(title, text, mono) },
            openImage = open,
            openSubagent = { doc, title, running -> onOpenSubagent(chatId, doc, title, running) },
            // An error card's "Log in": that agent's sign-in, started at once.
            act = { action -> if (action == "sign-in") signInTarget(model, chatId)?.let { onOpenAccounts(it, true) } },
        )
    }
    // The composer's draft outlives the panel that replaces it while the agent asks.
    val draft = remember(chatId) { ComposerModel(model.drafts.load(chatId)) }
    // Text handed to this chat (a terminal's output) lands after what's typed.
    DisposableEffect(chatId) {
        model.composerSinks[chatId] = { text ->
            val current = draft.text
            draft.setText(if (current.isEmpty()) text else current + (if (current.endsWith("\n")) "" else "\n") + text)
            model.drafts.save(chatId, draft.text)
        }
        onDispose { model.composerSinks.remove(chatId) }
    }

    // Companions: subagents, side chats (a side chat's siblings live with its parent), terminals.
    val parentId = row?.parentChatId
    // `companion model-picker` (screenshots): the model picker opens once the session is up.
    val openPicker = remember(chatId) { openCompanion == "model-picker" }
    val isCloud = row?.isCloud == true
    val cloudOutOfCredits = isCloud && model.cloud.outOfCredits
    var nudges by remember { mutableIntStateOf(0) }
    LaunchedEffect(isCloud) { if (isCloud && model.cloud.info == null) model.cloud.refresh() }
    val companions = rememberCompanions(model, chatId, includeSideChats = parentId == null)
    var companionsSheet by remember { mutableStateOf<CompanionSection?>(null) }
    var showCompanions by remember { mutableStateOf(false) }
    var confirmDelete by remember { mutableStateOf(false) }
    val view = androidx.compose.ui.platform.LocalView.current
    fun newSideChat() {
        val id = model.createSideChat(parentId ?: chatId) ?: return model.toast("Couldn't start a side chat")
        onOpenSideChat(id)
    }
    fun fork() {
        model.toast("Forking…")
        scope.launch {
            try {
                val id = model.forkSideChat(chatId)
                view.performHapticFeedback(android.view.HapticFeedbackConstants.CONFIRM)
                onOpenSideChat(id)
            } catch (e: Exception) {
                model.toast(forkError(e))
            }
        }
    }
    fun openTerminal() = onOpenTerminal(chatId, model.terminals.tabs(chatId).firstOrNull { it.isLive }?.ref?.id)
    LaunchedEffect(chatId) {
        val which = openCompanion ?: return@LaunchedEffect
        delay(800)
        when {
            which == "companions" -> onShowCompanions?.invoke(null) ?: run { showCompanions = true }
            which == "terminal" -> onOpenTerminal(chatId, null)
            which.startsWith("side-chat:") -> onOpenSideChat(which.removePrefix("side-chat:"))
            which.startsWith("subagent:") -> {
                val tool = which.removePrefix("subagent:")
                model.sessionSubagents(chatId).firstOrNull { it.id == tool || it.docId?.endsWith(tool) == true }?.let { a ->
                    a.docId?.let { onOpenSubagent(chatId, it, a.title, a.state == uniffi.zeron_core.SubagentState.RUNNING) }
                }
            }
        }
    }

    val project = row?.project?.name ?: "No project"
    val parentTitle = remember(workspace, parentId) { parentId?.let { model.row(it)?.title } }
    // A side chat heads itself as one, under its parent's title.
    val headerTitle = if (parentId != null) "Side chat" else composer.title
    val subtitle = parentTitle ?: composer.host.name?.let { "$project @ $it" } ?: project
    var overflow by remember { mutableStateOf(false) }
    val clipboard = LocalClipboardManager.current

    // Flat Material chrome: the header sits on the page and takes the
    // container tone once the transcript scrolls under it; the transcript
    // ends where the composer begins (nothing streams behind it).
    val scrolled by remember { derivedStateOf { transcript.offset > 1f } }
    val headerColor by animateColorAsState(
        if (scrolled) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.background,
        MaterialTheme.motionScheme.defaultEffectsSpec(),
        label = "header",
    )
    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        Surface(color = headerColor) {
            Row(
                Modifier
                    .fillMaxWidth()
                    .statusBarsPadding()
                    .padding(horizontal = 12.dp, vertical = 8.dp),
                verticalAlignment = Alignment.CenterVertically,
            ) {
                TonalCircleButton(ZIcons.Back, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                Column(Modifier.weight(1f).padding(horizontal = 12.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Text(headerTitle, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMediumEmphasized)
                    Text(subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                // The session has its own composer; the call stays a tap away up here.
                if (sh.zeron.android.voice.rememberVoiceVisible(model.voice)) {
                    sh.zeron.android.voice.VoiceHeaderButton(model.voice, onOpenStage = onOpenVoice, onOpenSettings = onVoiceSettings)
                    androidx.compose.foundation.layout.Spacer(Modifier.size(8.dp))
                }
                // Agents, side chats and terminals; a badge while agents work.
                androidx.compose.material3.BadgedBox(
                    badge = {
                        if (companions.runningAgents > 0) {
                            androidx.compose.material3.Badge(containerColor = MaterialTheme.colorScheme.primary) { Text("${companions.runningAgents}") }
                        }
                    },
                ) {
                    TonalCircleButton(ZIcons.Companions, "Agents, side chats and terminals", onClick = { onShowCompanions?.invoke(null) ?: run { showCompanions = true } }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                }
                androidx.compose.foundation.layout.Spacer(Modifier.size(8.dp))
                Box {
                    TonalCircleButton(ZIcons.More, "More", onClick = { overflow = true }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                    GroupedActionMenu(
                        overflow,
                        { overflow = false },
                        listOf(
                            listOf(
                                MenuAction("New side chat", ZIcons.SideChats) { newSideChat() },
                                MenuAction("Fork into side chat", ZIcons.Fork) { fork() },
                                MenuAction("Terminal", ZIcons.Terminal) { openTerminal() },
                            ),
                            listOfNotNull(
                                row?.let { r -> MenuAction(if (r.pinned) "Unpin" else "Pin", ZIcons.Pin) { model.setPinned(chatId, !r.pinned) } },
                                MenuAction("Copy transcript", ZIcons.Copy) { transcript.frame?.let { clipboard.setText(AnnotatedString(it.plainText())) } },
                                row?.let {
                                    MenuAction("Archive", ZIcons.Archive) {
                                        model.archive(chatId)
                                        onBack()
                                    }
                                },
                            ),
                            listOfNotNull(row?.let { MenuAction("Delete", ZIcons.Delete, destructive = true) { confirmDelete = true } }),
                        ),
                    )
                }
            }
        }
        // The transcript runs on under the chips and composer and fades out
        // behind them (iOS's edge fade), instead of stopping at a hard line.
        Box(Modifier.weight(1f).fillMaxWidth()) {
            val density = LocalDensity.current
            var stack by remember { mutableStateOf(0.dp) }
            LaunchedEffect(stack) { transcript.bottomInset = stack.value + 12f }
            Transcript(transcript, actions, Modifier.fillMaxSize())
            val page = MaterialTheme.colorScheme.background
            Box(
                Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    .height(stack + FADE)
                    .background(Brush.verticalGradient(0f to page.copy(alpha = 0f), FADE / (stack + FADE) to page.copy(alpha = 0.94f), 1f to page)),
            )
            val showJump by remember { derivedStateOf { !transcript.following && transcript.distanceFromBottom > 400f } }
            androidx.compose.animation.AnimatedVisibility(
                showJump,
                enter = fadeIn() + scaleIn(),
                exit = fadeOut() + scaleOut(),
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = stack + 12.dp),
            ) {
                SmallFloatingActionButton(
                    onClick = { transcript.scrollToBottom() },
                    shape = CircleShape,
                    containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                    contentColor = MaterialTheme.colorScheme.onSurface,
                ) { ZIcon(ZIcons.ArrowDown, "Jump to latest", Modifier.size(22.dp)) }
            }
            Column(
                Modifier
                    .align(Alignment.BottomCenter)
                    .fillMaxWidth()
                    // Measured with the keyboard and navigation bar: the last row scrolls clear of all of it.
                    .onSizeChanged { stack = with(density) { it.height.toDp() } }
                    .imePadding()
                    .navigationBarsPadding(),
            ) {
                // A Cloud chat out of credits says so once, above the composer.
                androidx.compose.animation.AnimatedVisibility(cloudOutOfCredits) {
                    CloudCreditsNotice(nudges, onAddCredits = onOpenBilling)
                }
                Row(Modifier.fillMaxWidth().padding(end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Box(Modifier.weight(1f)) { Banner(composer, connectivity?.state, onRetry = { runCatching { handle.retryDelivery() } }, cloud = if (isCloud) model.cloud else null, chatId = chatId, deviceId = row?.deviceId) }
                    CompanionStrip(companions, onTap = { section -> onShowCompanions?.invoke(section) ?: run { companionsSheet = section; showCompanions = true } }, modifier = if (companions.isEmpty) Modifier else Modifier.padding(vertical = 6.dp))
                }
                val input = composer.openInput
                // The panel replaces the composer while the agent asks.
                if (input != null) {
                    QuestionPanel(input) { answers -> runCatching { handle.respondInput(input.requestId, answers) } }
                } else {
                    Composer(model, core, handle, composer, row, transcript, draft, openModelPicker = openPicker, beforeSend = {
                        // Out of credits, a Cloud send can't wake its machine: say so instead of a send that never lands.
                        if (cloudOutOfCredits) nudges++
                        !cloudOutOfCredits
                    })
                }
            }
        }
    }

    viewing?.let { ImageViewer(it) { viewing = null } }

    if (showCompanions) {
        CompanionsSheet(
            model,
            chatId,
            title = if (parentId != null) "Side chat" else composer.title,
            initialSection = companionsSheet,
            onDismiss = {
                showCompanions = false
                companionsSheet = null
            },
            onOpenSubagent = { a -> a.docId?.let { onOpenSubagent(chatId, it, a.title, a.state == uniffi.zeron_core.SubagentState.RUNNING) } },
            onOpenSideChat = onOpenSideChat,
            onOpenTerminal = { tab -> onOpenTerminal(chatId, tab?.ref?.id) },
        )
    }

    if (confirmDelete) {
        // Permanent, like the desktop's dialog; a Cloud session's machine goes with it.
        val name = row?.takeIf { it.hasTitle }?.title ?: "New session"
        androidx.compose.material3.AlertDialog(
            onDismissRequest = { confirmDelete = false },
            icon = { ZIcon(ZIcons.Delete, null) },
            title = { Text("Delete session?") },
            text = {
                Text(
                    if (row?.isCloud == true) "\u201C$name\u201D and its Cloud machine will be permanently deleted, with anything not pushed from it. This can\u2019t be undone."
                    else "\u201C$name\u201D will be permanently deleted. This can\u2019t be undone.",
                )
            },
            confirmButton = {
                androidx.compose.material3.TextButton(onClick = {
                    confirmDelete = false
                    model.delete(chatId)
                    onBack()
                }) { Text("Delete", color = MaterialTheme.colorScheme.error) }
            },
            dismissButton = { androidx.compose.material3.TextButton(onClick = { confirmDelete = false }) { Text("Cancel") } },
        )
    }

    sheet?.let { s ->
        ModalBottomSheet(onDismissRequest = { sheet = null }) {
            Text(s.title, style = MaterialTheme.typography.titleMedium, modifier = Modifier.padding(horizontal = 24.dp))
            SelectionContainer {
                Text(
                    s.text,
                    fontFamily = if (s.mono) GeistMono else Geist,
                    style = if (s.mono) MaterialTheme.typography.bodySmall else MaterialTheme.typography.bodyLarge,
                    modifier = Modifier
                        .fillMaxWidth()
                        .verticalScroll(rememberScrollState())
                        .padding(24.dp),
                )
            }
        }
    }
}

/** How far above the chips and composer the transcript starts to fade. */
private val FADE = 44.dp

/** The status pill above the composer (iOS SessionChrome banner). */
@Composable
private fun Banner(c: ComposerState, connectivity: ConnectivityState?, onRetry: () -> Unit, cloud: sh.zeron.android.core.CloudStore? = null, chatId: String = "", deviceId: String? = null) {
    // Reconnect countdowns tick each second.
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    val retryAt = c.room.retryAtMs
    if (!c.room.connected && retryAt != null) {
        LaunchedEffect(retryAt) {
            while (true) {
                now = System.currentTimeMillis()
                delay(1_000)
            }
        }
    }
    // A Cloud chat whose send can't land says why: out of credits the notice
    // above already does; a failed machine is named.
    if (cloud != null && c.sendState == SendState.FAILED) {
        LaunchedEffect(Unit) { cloud.refreshSessions() }
        if (cloud.outOfCredits) return
        cloud.sessionError(chatId, deviceId)?.let { return StatusBanner("Cloud machine: $it", "Retry" to onRetry) }
    }
    val text = when {
        c.sendState == SendState.FAILED -> "Not delivered"
        c.sendState == SendState.QUEUED -> "${c.host.name ?: "Host"} is offline — will send when it's back"
        c.sendState == SendState.SENDING -> "Sending to host…"
        connectivity == ConnectivityState.OFFLINE -> "Offline — sends are saved"
        !c.room.connected && retryAt != null -> "Reconnecting in ${maxOf(1L, (retryAt - now) / 1000)}s"
        else -> null
    } ?: return
    StatusBanner(text, if (c.sendState == SendState.FAILED) "Retry" to onRetry else null)
}

/** Whose logins an error card's "Log in" opens: Cloud's for a Cloud chat, else the host's. */
fun signInTarget(model: AppModel, chatId: String): AccountsTarget? {
    val row = model.row(chatId) ?: return null
    val harness = row.harness ?: "claude-code"
    if (row.isCloud) return sh.zeron.android.core.CloudStore.agents.firstOrNull { it.harness == harness }?.let { AccountsTarget.Cloud(it) }
    val label = row.harnessLabel ?: uniffi.zeron_core.agentLoginRules(harness).name.ifEmpty { harness }
    return AccountsTarget.Device(row.deviceId, model.deviceName(row.deviceId), harness, label)
}

/** "Out of Cloud credits" with its one way forward; a refused send shakes it. */
@Composable
private fun CloudCreditsNotice(nudges: Int, onAddCredits: () -> Unit) {
    val shake = remember { androidx.compose.animation.core.Animatable(0f) }
    val view = androidx.compose.ui.platform.LocalView.current
    LaunchedEffect(nudges) {
        if (nudges == 0) return@LaunchedEffect
        view.performHapticFeedback(android.view.HapticFeedbackConstants.REJECT)
        for (x in listOf(-8f, 8f, -5f, 5f, -2f, 0f)) shake.animateTo(x, androidx.compose.animation.core.tween(50))
    }
    Surface(
        shape = androidx.compose.foundation.shape.RoundedCornerShape(50),
        color = MaterialTheme.colorScheme.errorContainer,
        contentColor = MaterialTheme.colorScheme.onErrorContainer,
        modifier = Modifier
            .padding(horizontal = 16.dp, vertical = 6.dp)
            .graphicsLayer { translationX = shake.value * density },
    ) {
        Row(Modifier.padding(start = 16.dp, end = 6.dp, top = 4.dp, bottom = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            ZIcon(ZIcons.Warning, null, Modifier.size(18.dp))
            androidx.compose.foundation.layout.Spacer(Modifier.size(10.dp))
            Text(sh.zeron.android.core.CloudStore.OUT_OF_CREDITS, style = MaterialTheme.typography.labelLarge, modifier = Modifier.weight(1f))
            androidx.compose.material3.Button(onClick = onAddCredits, contentPadding = androidx.compose.foundation.layout.PaddingValues(horizontal = 14.dp)) { Text("Add credits") }
        }
    }
}
