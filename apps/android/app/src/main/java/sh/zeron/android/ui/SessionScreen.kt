package sh.zeron.android.ui

import sh.zeron.android.feedback.feedbackAction
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
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
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.ArrowBack
import androidx.compose.material.icons.filled.KeyboardArrowDown
import androidx.compose.material.icons.outlined.Archive
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.MoreVert
import androidx.compose.material.icons.outlined.PushPin
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
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.width
import sh.zeron.android.tools.Links
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
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
fun SessionScreen(model: AppModel, chatId: String, onBack: () -> Unit, onNavigate: (String) -> Unit = {}, showSubagents: Boolean = false) {
    val client by model.client.collectAsState()
    val core = client ?: return
    val handle: SessionHandle = remember(chatId) { runCatching { core.openSession(chatId) }.getOrNull() } ?: run {
        Text("This session isn't available.", Modifier.padding(32.dp))
        return
    }
    val transcript = remember(chatId) { TranscriptState() }
    DisposableEffect(chatId) {
        transcript.engine.attach(core, chatId)
        handle.setViewAttached(true)
        core.markSeen(chatId)
        onDispose {
            handle.setViewAttached(false)
            transcript.close()
        }
    }

    // Composer-facing state: re-pulled on this chat's events.
    var composer by remember { mutableStateOf(handle.composer()) }
    val workspace by model.workspace.collectAsState()
    val connectivity by model.connectivity.collectAsState()
    // The chat's subagents, read from its spawn chips (Rust groups them the
    // desktop's way); re-read with the transcript.
    var subagents by remember(chatId) { mutableStateOf(groupsOf(core, chatId)) }
    var subagentsOpen by androidx.compose.runtime.saveable.rememberSaveable(chatId) { mutableStateOf(showSubagents) }
    LaunchedEffect(chatId) {
        model.sessionEvents.collect {
            if (it == chatId) {
                composer = handle.composer()
                subagents = groupsOf(core, chatId)
            }
        }
    }
    // Upload rings on pending thumbnails follow the escort.
    LaunchedEffect(composer.transferProgress) { transcript.uploadProgress = composer.transferProgress }
    val row = remember(workspace, chatId) { model.row(chatId) }
    // What this chat's own subagent chips show is told to the model, so the Sessions list's covers say the same.
    val liveRunning = subagents.running.toInt()
    LaunchedEffect(chatId, liveRunning) { model.reportLiveSubagents(chatId, liveRunning) }
    DisposableEffect(chatId) { onDispose { model.releaseLiveSubagents(chatId) } }

    val context = LocalContext.current
    var sheet by remember { mutableStateOf<TextSheet?>(null) }
    val clipboard = LocalClipboardManager.current
    val actions = remember(chatId) {
        TranscriptActions(
            openUrl = { url ->
                val uri = Uri.parse(url)
                // A spawn card: open that subagent.
                val subagent = subagentDocOf(url)
                if (subagent != null) onNavigate(Routes.subagent(chatId, subagent))
                else when (val target = Links.classify(url, model.workspaceRef(chatId))) {
                    // Pages open in the in-app browser (localhost dev servers too).
                    is Links.Target.Web -> onNavigate(Routes.browser(chatId, target.url))
                    is Links.Target.File -> onNavigate(Routes.file(chatId, target.path))
                    is Links.Target.Outside -> sh.zeron.android.tools.toast(context, "${target.path} is outside this session's folder")
                    Links.Target.Other -> runCatching { context.startActivity(Intent(Intent.ACTION_VIEW, uri)) }
                        .onFailure { runCatching { CustomTabsIntent.Builder().build().launchUrl(context, uri) } }
                }
            },
            fileActions = { path ->
                when (val target = Links.classify(path, model.workspaceRef(chatId))) {
                    is Links.Target.File -> {
                        val ref = model.workspaceRef(chatId)
                        listOfNotNull(
                            MenuAction("Open", ZIcons.Text) { onNavigate(Routes.file(chatId, target.path)) },
                            if (ref != null && sh.zeron.android.tools.FileKind.of(target.path) == sh.zeron.android.tools.FileKind.Html) {
                                MenuAction("Open in browser", ZIcons.Globe) { onNavigate(Routes.browser(chatId, sh.zeron.android.tools.Browser.workspaceUrl(ref, target.path))) }
                            } else null,
                            ref?.let { MenuAction("Save to Downloads", ZIcons.Save) { model.downloads.saveFile(it, target.path) } },
                            MenuAction("Copy path", ZIcons.Copy, haptic = Haptic.Confirm, cue = Cue.Copy) { clipboard.setText(AnnotatedString(path)) },
                        )
                    }
                    else -> listOf(MenuAction("Copy path", ZIcons.Copy, haptic = Haptic.Confirm, cue = Cue.Copy) { clipboard.setText(AnnotatedString(path)) })
                }
            },
            openFile = { path ->
                when (val target = Links.classify(path, model.workspaceRef(chatId))) {
                    is Links.Target.File -> onNavigate(Routes.file(chatId, target.path))
                    is Links.Target.Outside -> sh.zeron.android.tools.toast(context, "${target.path} is outside this session's folder")
                    else -> Unit
                }
            },
            loadImage = { ref ->
                val device = handle.composer().host.deviceId
                runCatching {
                    val bytes = core.readAttachment(device, ref)
                    withContext(Dispatchers.Default) { BitmapFactory.decodeByteArray(bytes, 0, bytes.size)?.asImageBitmap() }
                }.getOrNull()
            },
            showText = { title, text, mono -> sheet = TextSheet(title, text, mono) },
        )
    }

    val project = row?.project?.name ?: "No project"
    val subtitle = composer.host.name?.let { "$project @ $it" } ?: project
    var overflow by remember { mutableStateOf(false) }

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
                    Text(composer.title, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.titleMediumEmphasized)
                    Text(subtitle, maxLines = 1, overflow = TextOverflow.Ellipsis, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                // Running count comes from the transcript when it has them,
                // else from the row (a count published before the chips synced).
                val running = maxOf(subagents.running.toInt(), row?.runningSubagents?.toInt() ?: 0)
                if (running > 0 || Subagents.total(subagents) > 0) {
                    SubagentsButton(running, onClick = { subagentsOpen = true })
                    Spacer(Modifier.width(8.dp))
                }
                TonalCircleButton(ZIcons.FileTree, "Files", onClick = { onNavigate(Routes.files(chatId)) }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                Spacer(Modifier.width(8.dp))
                Box {
                    TonalCircleButton(ZIcons.More, "More", onClick = { overflow = true }, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                    ActionMenu(
                        overflow,
                        { overflow = false },
                        listOfNotNull(
                            MenuAction("Files", ZIcons.FileTree) { onNavigate(Routes.files(chatId)) },
                            if (Subagents.total(subagents) > 0) MenuAction("Subagents", ZIcons.Bot) { subagentsOpen = true } else null,
                            MenuAction("Terminal", ZIcons.Terminal) { onNavigate(Routes.terminal(chatId)) },
                            MenuAction("Browser & previews", ZIcons.Globe) { onNavigate(Routes.browser(chatId, null)) },
                            MenuAction("Copy transcript", ZIcons.Copy, haptic = Haptic.Confirm, cue = Cue.Copy) { transcript.frame?.let { clipboard.setText(AnnotatedString(it.plainText())) } },
                            row?.let { r -> MenuAction(if (r.pinned) "Unpin" else "Pin", ZIcons.Pin, haptic = Haptic.Pop, cue = if (r.pinned) Cue.Unstar else Cue.Pin) { model.setPinned(chatId, !r.pinned) } },
                            row?.let { MenuAction("Archive", ZIcons.Archive, haptic = Haptic.Confirm, cue = Cue.Archive) { model.archive(chatId); onBack() } },
                        ),
                    )
                }
            }
        }
        Box(Modifier.weight(1f).fillMaxWidth()) {
            Transcript(transcript, actions, Modifier.fillMaxSize())
            val showJump by remember { derivedStateOf { !transcript.following && transcript.distanceFromBottom > 400f } }
            androidx.compose.animation.AnimatedVisibility(
                showJump,
                enter = fadeIn() + scaleIn(),
                exit = fadeOut() + scaleOut(),
                modifier = Modifier.align(Alignment.BottomCenter).padding(bottom = 12.dp),
            ) {
                SmallFloatingActionButton(
                    onClick = feedbackAction(Haptic.Select, Cue.Select) { transcript.scrollToBottom() },
                    shape = CircleShape,
                    containerColor = MaterialTheme.colorScheme.surfaceContainerHigh,
                    contentColor = MaterialTheme.colorScheme.onSurface,
                ) { ZIcon(ZIcons.ArrowDown, "Jump to latest", Modifier.size(22.dp)) }
            }
        }
        Column(Modifier.fillMaxWidth().imePadding().navigationBarsPadding()) {
            Banner(composer, connectivity?.state, onRetry = { runCatching { handle.retryDelivery() } })
            composer.openInput?.let { QuestionPanel(it) { answers -> runCatching { handle.respondInput(it.requestId, answers) } } }
            Composer(model, core, handle, composer, row, transcript)
        }
    }

    if (subagentsOpen) {
        SubagentsSheet(
            subagents,
            onOpen = { view ->
                subagentsOpen = false
                onNavigate(Routes.subagent(chatId, view.docId))
            },
            onDismiss = { subagentsOpen = false },
        )
    }

    sheet?.let { s ->
        ModalBottomSheet(onDismissRequest = { sheet = null }) {
            OpenCloseFeedback()
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

@Composable
private fun Banner(c: ComposerState, connectivity: ConnectivityState?, onRetry: () -> Unit) {
    val text = when {
        c.sendState == SendState.FAILED -> "Not delivered"
        c.sendState == SendState.QUEUED -> "${c.host.name ?: "Host"} is offline — will send when it's back"
        connectivity == ConnectivityState.OFFLINE -> "You're offline"
        !c.room.connected && c.room.retryAtMs != null -> "Reconnecting…"
        else -> null
    } ?: return
    val retry = feedbackAction(Haptic.Select, Cue.Refresh, onRetry)
    StatusBanner(text, if (c.sendState == SendState.FAILED) "Retry" to retry else null)
}
