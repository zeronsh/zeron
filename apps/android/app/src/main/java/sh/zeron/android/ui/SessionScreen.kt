@file:OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)

package sh.zeron.android.ui

import android.graphics.Bitmap
import android.graphics.BitmapFactory
import androidx.compose.foundation.background
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.ime
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.union
import androidx.compose.foundation.layout.windowInsetsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.TextButton
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.layout.onSizeChanged
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.boundsInWindow
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.layout.offset
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.unit.IntOffset
import kotlin.math.roundToInt
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.PinSlashGlyph
import sh.zeron.android.design.PopupAnchoredMenu
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import androidx.compose.ui.viewinterop.AndroidView
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.contract.ActivityResultContracts
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AnchoredMenu
import sh.zeron.android.design.ArrowUpMark
import sh.zeron.android.design.AssetIcon
import sh.zeron.android.design.GaugeGlyph
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.PrGlyph
import sh.zeron.android.design.BackChevron
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.EllipsisMark
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.PlusMark
import sh.zeron.android.design.ProjectTile
import sh.zeron.android.design.StopMark
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.LayoutListener
import uniffi.zeron_core.NewSession
import uniffi.zeron_core.OutgoingAttachment
import uniffi.zeron_core.QueueEditAction
import uniffi.zeron_core.QueueEditFinish
import uniffi.zeron_core.QueueEditStart
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SendState
import uniffi.zeron_core.SessionTarget
import uniffi.zeron_core.TranscriptView
import uniffi.zeron_core.UserInputAnswer
import uniffi.zeron_core.UserInputQuestion
import uniffi.zeron_core.fileMentionLink
import uniffi.zeron_core.harnessLabel
import uniffi.zeron_core.modelLabel
import uniffi.zeron_core.reasoningLabel

private class FrameRelay : LayoutListener {
    var onReady: () -> Unit = {}
    override fun frameReady(revision: ULong) {
        onReady()
    }
}

internal class Staged(val name: String, val bytes: ByteArray, val preview: Bitmap?)

/** A composer context chip (iOS `ComposerChip`): model, effort, PR/branch, context. */
internal data class Chip(
    val id: String,
    val title: String,
    val harness: String? = null,
    val prState: uniffi.zeron_core.PullRequestState? = null,
    /** Project chips show the project's tile. */
    val colorIndex: Int? = null,
    /** Context chip past 85%: warning tint (iOS Palette.warning). */
    val warn: Boolean = false,
)

internal enum class Delivery { Send, Queue, Steer, Interrupt }

@Composable
fun SessionScreen(model: ZeronModel, chatId: String) {
    val colors = LocalZeronColors.current
    val client = model.client ?: return
    val text = model.text ?: return
    val context = LocalContext.current
    val focusManager = androidx.compose.ui.platform.LocalFocusManager.current
    val scope = rememberCoroutineScope()
    val handle = remember(chatId) { client.openSession(chatId) }
    var chrome by remember(chatId) { mutableStateOf(handle.composer()) }
    val row = client.sessionRow(chatId)
    LaunchedEffect(model.epoch) {
        // Decoding the composer (queue, questions) is not free; keep it off the
        // main thread so a busy turn does not stall scrolling.
        withContext(Dispatchers.Default) { runCatching { handle.composer() }.getOrNull() }?.let { chrome = it }
    }
    val relay = remember { FrameRelay() }
    val engine = remember(chatId) {
        TranscriptView(text, relay).also {
            it.attach(client, chatId)
            handle.setViewAttached(true)
        }
    }
    val images = remember { HashMap<String, Bitmap>() }
    var view by remember { mutableStateOf<TranscriptListView?>(null) }
    // Only whether the jump-to-bottom button shows, not the raw distance: a
    // state write per scrolled pixel recomposed the whole chat screen.
    var awayFromBottom by remember { mutableStateOf(false) }
    // Unsent text survives leaving the chat and app restarts (iOS Drafts).
    val drafts = remember { context.getSharedPreferences("drafts", android.content.Context.MODE_PRIVATE) }
    var draft by remember(chatId) { mutableStateOf(drafts.getString(chatId, "") ?: "") }
    var usageOpen by remember { mutableStateOf(false) }
    // Plan usage for the composer's ring (desktop AccountUsage): the engine's
    // cached probe paints at once, a forced probe follows, then every 5 min.
    var planAccounts by remember(chatId) { mutableStateOf<List<uniffi.zeron_core.AgentUsage>?>(null) }
    val usageDevice = row?.deviceId ?: chrome.host.deviceId
    val usageHarness = row?.harness
    LaunchedEffect(usageDevice, usageHarness) {
        if (!reportsPlanUsage(usageHarness)) return@LaunchedEffect
        runCatching { client.listAgentUsage(usageDevice, false) }.onSuccess { planAccounts = it }
        while (true) {
            runCatching { client.listAgentUsage(usageDevice, true) }.onSuccess { planAccounts = it }
            kotlinx.coroutines.delay(5 * 60_000L)
        }
    }
    var sendFailure by remember(chatId) { mutableStateOf<String?>(null) }
    var focused by remember { mutableStateOf(false) }
    var staged by remember { mutableStateOf(listOf<Staged>()) }
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    var renameText by remember { mutableStateOf("") }
    var editingQueue by remember { mutableStateOf<String?>(null) }
    // The user's own draft, parked while a queued message is in the composer.
    var stash by remember { mutableStateOf<String?>(null) }
    LaunchedEffect(chatId, draft, editingQueue) {
        if (editingQueue != null) return@LaunchedEffect
        kotlinx.coroutines.delay(400)
        drafts.edit().apply { if (draft.isBlank()) remove(chatId) else putString(chatId, draft) }.apply()
    }
    var lightbox by remember { mutableStateOf<Bitmap?>(null) }
    var detail by remember { mutableStateOf<Pair<String, String>?>(null) }
    // Root-relative top of the composer stack and the screen height: the
    // transcript stops a gap above the composer (keyboard included).
    var composerTop by remember { mutableIntStateOf(0) }
    var rootHeight by remember { mutableIntStateOf(0) }
    var menuAnchor by remember { mutableStateOf(Rect.Zero) }
    var chipMenu by remember { mutableStateOf<Pair<Chip, Rect>?>(null) }
    var headerPx by remember { mutableIntStateOf(0) }
    val picker = rememberLauncherForActivityResult(ActivityResultContracts.GetContent()) { uri ->
        if (uri == null) return@rememberLauncherForActivityResult
        val bytes = context.contentResolver.openInputStream(uri)?.use { it.readBytes() } ?: return@rememberLauncherForActivityResult
        val name = uri.lastPathSegment?.substringAfterLast('/') ?: "image.jpg"
        val preview = BitmapFactory.decodeByteArray(bytes, 0, bytes.size)
        staged = staged + Staged(name, bytes, preview)
    }
    DisposableEffect(chatId) {
        onDispose {
            model.scrolling = false
            runCatching { handle.setViewAttached(false) }
            runCatching { engine.closeEngine() }
            runCatching { engine.close() }
            runCatching { client.closeSession(chatId) }
            runCatching { handle.close() }
        }
    }
    val running = chrome.live.turnRunning
    val questions = chrome.openInput
    val density = LocalDensity.current
    val gapPx = with(density) { ComposerGap.roundToPx() }
    val headerGapPx = with(density) { HeaderGap.roundToPx() }
    val bottomInset = if (composerTop > 0 && rootHeight > 0) (rootHeight - composerTop + gapPx).coerceAtLeast(0) else 0
    Box(Modifier.fillMaxSize().background(colors.background).onSizeChanged { rootHeight = it.height }) {
        AndroidView(
            modifier = Modifier.fillMaxSize(),
            factory = { ctx ->
                TranscriptListView(ctx).also { created ->
                    created.engine = engine
                    created.faces = model.faces
                    created.colors = colors
                    view = created
                    relay.onReady = { created.requestFrame() }
                    created.post { created.onFrame() }
                }
            },
            update = { host ->
                view = host
                host.colors = colors
                host.faces = model.faces
                host.onToggle = { engine.toggle(it) }
                host.onToggleDetail = { rowKey, detail, open -> engine.toggleDetail(rowKey, detail, open) }
                host.onCopy = { textToCopy ->
                    val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                    cm.setPrimaryClip(android.content.ClipData.newPlainText("zeron", textToCopy))
                    model.showToast("Copied")
                }
                host.onLink = { url ->
                    if (url.startsWith("http")) {
                        runCatching {
                            context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(url)))
                        }
                    } else model.showToast(url)
                }
                host.onImage = { bmp -> lightbox = bmp }
                host.onDetail = { title, body -> detail = title to body }
                host.onTap = { focusManager.clearFocus() }
                host.bottomFadePx = gapPx
                host.bottomInsetPx = bottomInset
                host.topFadePx = headerGapPx
                host.topInsetPx = headerPx + headerGapPx
                host.imageFor = { images[it] }
                host.requestImage = req@{ ref ->
                    if (images.containsKey(ref) || ref.startsWith("pending:")) return@req
                    scope.launch {
                        val bytes = runCatching { client.readAttachment(chrome.host.deviceId, ref) }.getOrNull() ?: return@launch
                        val bmp = BitmapFactory.decodeByteArray(bytes, 0, bytes.size) ?: return@launch
                        images[ref] = bmp
                        host.postInvalidate()
                    }
                }
                host.onDistanceFromBottom = { d ->
                    val away = d > 140f
                    if (away != awayFromBottom) awayFromBottom = away
                }
                host.onScrollActive = { model.scrolling = it }
                relay.onReady = { host.requestFrame() }
            },
        )
        Column(
            Modifier
                .align(Alignment.TopCenter)
                .onSizeChanged {
                    headerPx = it.height
                    view?.topInsetPx = it.height + headerGapPx
                }
                .statusBarsPadding()
                .padding(horizontal = 12.dp, vertical = 4.dp),
        ) {
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.fillMaxWidth().height(52.dp)) {
                Box(
                    Modifier.size(44.dp).glassSurface(colors, 22.dp).clickable { model.back() },
                    contentAlignment = Alignment.Center,
                ) { BackChevron(colors.text, Modifier.size(18.dp)) }
                Row(Modifier.weight(1f), horizontalArrangement = Arrangement.Center, verticalAlignment = Alignment.CenterVertically) {
                    BrandMark(row?.harness, colors, 18.dp)
                    Spacer(Modifier.width(8.dp))
                    Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.widthIn(max = 230.dp)) {
                        Text(
                            chrome.title.ifBlank { row?.title ?: "Session" },
                            color = colors.text,
                            fontFamily = ZeronType.Sans,
                            fontWeight = FontWeight.SemiBold,
                            fontSize = 16.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                        val project = row?.project?.name ?: "No project"
                        val hostName = chrome.host.name
                        Text(
                            if (hostName != null) "$project @ $hostName" else project,
                            color = colors.secondary,
                            fontFamily = ZeronType.Sans,
                            fontSize = 12.sp,
                            maxLines = 1,
                            overflow = TextOverflow.Ellipsis,
                        )
                    }
                }
                Box {
                    Box(
                        Modifier.size(44.dp).onGloballyPositioned { menuAnchor = it.boundsInRoot() }.glassSurface(colors, 22.dp).clickable { menu = true },
                        contentAlignment = Alignment.Center,
                    ) { EllipsisMark(colors.text, Modifier.size(18.dp)) }
                }
            }
        }
        Column(
            Modifier.align(Alignment.BottomCenter).widthIn(max = 768.dp).fillMaxWidth().padding(horizontal = 16.dp).windowInsetsPadding(WindowInsets.ime.union(WindowInsets.navigationBars)).padding(bottom = 6.dp).onGloballyPositioned { composerTop = it.boundsInRoot().top.roundToInt() },
        ) {
            StatusPill(chrome, model.connectivity, sendFailure, editingQueue != null, colors) {
                if (editingQueue != null) {
                    editingQueue = null
                    draft = stash ?: ""
                    stash = null
                } else if (chrome.sendState == SendState.FAILED) {
                    runCatching { handle.retryDelivery() }
                }
            }
            if (chrome.queue.isNotEmpty()) {
                QueueCard(chrome.queue, colors, onMove = { id, delta -> runCatching { handle.moveQueuedBy(id, delta) } }, onNow = { id -> scope.launch { runCatching { handle.sendQueuedNow(id) } } }, onRemove = { id -> scope.launch { runCatching { handle.removeQueued(id) } } }, onEdit = { item ->
                    if (stash == null) stash = draft
                    editingQueue = item.id
                    draft = item.visibleText
                })
                Spacer(Modifier.height(8.dp))
            }
            if (questions != null) {
                QuestionCard(questions.questions, colors) { answers ->
                    runCatching { handle.respondInput(questions.requestId, answers) }
                }
            } else {
                ComposerBar(
                    colors = colors,
                    text = draft,
                    onText = { draft = it },
                    placeholder = if (editingQueue != null) "Edit queued message" else "Message ${row?.harness?.let { runCatching { harnessLabel(it) }.getOrNull() } ?: "the agent"}",
                    running = running,
                    canSteer = chrome.host.capabilities.midTurnSteering == true,
                    focused = focused,
                    onFocus = { focused = it },
                    chips = chipsFor(row, chrome),
                    onChip = { chip, rect -> chipMenu = chip to rect },
                    rings = {
                        UsageRings(colors, planFraction(planAccounts, row?.harness), chrome.contextUsage, onTap = { focusManager.clearFocus(); usageOpen = true })
                    },
                    images = staged,
                    onRemoveImage = { staged = staged.filterNot { s -> s === it } },
                    onAttach = { picker.launch("image/*") },
                    onSend = send@{ mode ->
                        val body = draft.trim()
                        if (body.isEmpty() && staged.isEmpty() && mode != Delivery.Send) return@send
                        if (mode == Delivery.Send && running && body.isEmpty() && staged.isEmpty()) {
                            runCatching { handle.interrupt() }
                            return@send
                        }
                        try {
                            if (mode == Delivery.Interrupt && running) handle.interrupt()
                            val editing = editingQueue
                            if (editing != null) {
                                scope.launch {
                                    try {
                                        when (val start = handle.beginQueuedEdit(editing, client.deviceId())) {
                                            is QueueEditStart.Acquired -> {
                                                val finish = handle.finishQueuedEdit(start.lease, QueueEditAction.COMMIT, body)
                                                if (finish != QueueEditFinish.FINISHED) model.showToast("Couldn't save the edit")
                                            }
                                            else -> model.showToast("That message isn't editable right now")
                                        }
                                    } catch (t: Throwable) {
                                        model.showToast(t.message ?: "Couldn't save the edit")
                                    }
                                }
                                editingQueue = null
                                draft = stash ?: ""
                                stash = null
                                return@send
                            } else {
                                handle.send(
                                    SendRequest(
                                        text = body,
                                        attachments = staged.map { OutgoingAttachment(it.name, "image/jpeg", it.bytes) },
                                        worktree = null,
                                        busy = if (mode == Delivery.Steer) BusyPolicy.STEER else BusyPolicy.QUEUE,
                                    ),
                                )
                            }
                            draft = ""
                            staged = emptyList()
                            sendFailure = null
                        } catch (t: Throwable) {
                            // Kept in the pill until the next send (iOS sendFailure).
                            sendFailure = "Couldn't send: ${t.message ?: "unknown error"}"
                        }
                    },
                    mentionSearch = { q ->
                        val device = row?.deviceId ?: chrome.host.deviceId
                        runCatching { client.searchFiles(device, chatId, row?.project?.id, q) }.getOrDefault(emptyList())
                    },
                    onMention = { path, dir ->
                        draft = draft.replace(Regex("@[^\\s]*$"), fileMentionLink(path, dir) + " ")
                    },
                )
            }
        }
        // iOS jump-to-latest: a 40pt glass circle 16 from the trailing edge,
        // 12 above the composer, popping in once you're away from the bottom.
        if (composerTop > 0) {
            val buttonPx = with(density) { 40.dp.roundToPx() }
            val lift = with(density) { 12.dp.roundToPx() }
            Box(Modifier.align(Alignment.TopEnd).padding(end = 16.dp).offset { IntOffset(0, composerTop - lift - buttonPx) }) {
                AnimatedVisibility(
                    visible = awayFromBottom,
                    enter = fadeIn(tween(180)) + scaleIn(tween(220), initialScale = 0.6f),
                    exit = fadeOut(tween(160)) + scaleOut(tween(160), targetScale = 0.6f),
                ) {
                    Box(
                        Modifier.size(40.dp).glassSurface(colors, 20.dp).clickable { view?.jumpToBottom() },
                        contentAlignment = Alignment.Center,
                    ) { ArrowUpMark(colors.text, Modifier.size(16.dp).graphicsLayer { rotationZ = 180f }) }
                }
            }
        }
        if (menu) {
            // iOS sessionMenu(), read when it opens: Pin/Unpin, Rename…, Copy
            // Transcript, Archive (destructive).
            val current = client.sessionRow(chatId)
            val pinned = current?.pinned == true
            AnchoredMenu(
                colors,
                menuAnchor,
                title = null,
                entries = listOf(
                    MenuEntry(if (pinned) "Unpin" else "Pin", icon = { c -> if (pinned) PinSlashGlyph(17.dp, c) else Glyph(Glyphs.Pin, 17.dp, c) }) {
                        model.pin(chatId, !pinned)
                    },
                    MenuEntry("Rename…", icon = { c -> Glyph(Glyphs.Rename, 17.dp, c) }) {
                        renameText = chrome.title.ifBlank { current?.title ?: "" }
                        renaming = true
                    },
                    MenuEntry("Copy Transcript", icon = { c -> Glyph(Glyphs.Copy, 17.dp, c) }) {
                        val text = runCatching { engine.frame().let { f -> try { f.plainText() } finally { f.close() } } }.getOrDefault("")
                        if (text.isBlank()) {
                            model.showToast("Nothing to copy yet")
                        } else {
                            val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                            cm.setPrimaryClip(android.content.ClipData.newPlainText("transcript", text))
                            model.showToast("Transcript copied")
                        }
                    },
                    MenuEntry("Usage", icon = { c -> GaugeGlyph(c, Modifier.size(17.dp)) }) { focusManager.clearFocus(); usageOpen = true },
                    MenuEntry("Archive", destructive = true, icon = { c -> Glyph(Glyphs.Archive, 17.dp, c) }) {
                        model.archive(chatId)
                        model.back()
                    },
                ),
                above = false,
            ) { menu = false }
        }
        if (usageOpen) {
            UsageSheet(colors, client, row?.deviceId ?: chrome.host.deviceId, row?.harness, chrome.contextUsage, onAccounts = { planAccounts = it }) { usageOpen = false }
        }
        chipMenu?.let { (chip, anchor) ->
            ChipMenu(chip, anchor, row, chrome, client, chatId, colors, model) { chipMenu = null }
        }
        lightbox?.let { bmp ->
            Box(
                Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.92f)).clickable { lightbox = null },
                contentAlignment = Alignment.Center,
            ) {
                Image(bmp.asImageBitmap(), contentDescription = "Attachment", modifier = Modifier.fillMaxWidth().padding(16.dp), contentScale = ContentScale.Fit)
            }
        }
        detail?.let { (title, body) ->
            AlertDialog(
                onDismissRequest = { detail = null },
                title = { Text(title) },
                text = { Text(body, modifier = Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) },
                confirmButton = { TextButton(onClick = { detail = null }) { Text("Close") } },
            )
        }
        if (renaming) {
            AlertDialog(
                onDismissRequest = { renaming = false },
                title = { Text("Rename") },
                text = {
                    BasicTextField(
                        value = renameText,
                        onValueChange = { renameText = it },
                        textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp),
                        modifier = Modifier.fillMaxWidth(),
                    )
                },
                confirmButton = {
                    TextButton(onClick = {
                        val name = renameText.trim()
                        if (name.isNotEmpty()) model.rename(chatId, name)
                        renaming = false
                    }) { Text("Rename") }
                },
                dismissButton = { TextButton(onClick = { renaming = false }) { Text("Cancel") } },
            )
        }
    }
}

/** Gap between the last message and the composer; the fade edge lives in it. */
private val ComposerGap = 18.dp

/** Gap between the header and the first visible row; the top fade lives in it. */
private val HeaderGap = 14.dp

/** The chip menus the iOS session composer shows (CoreSessionSource.chipMenu). */
@Composable
private fun ChipMenu(
    chip: Chip,
    anchor: Rect,
    row: uniffi.zeron_core.SessionRow?,
    chrome: uniffi.zeron_core.ComposerState,
    client: uniffi.zeron_core.CoreClient,
    chatId: String,
    colors: ZeronColors,
    model: ZeronModel,
    onDismiss: () -> Unit,
) {
    val context = LocalContext.current
    val harness = row?.harness ?: "claude-code"
    var models by remember(chip.id) { mutableStateOf<List<uniffi.zeron_core.ModelInfo>?>(null) }
    if (chip.id == "model" || chip.id == "effort") {
        LaunchedEffect(chip.id, harness) {
            models = runCatching { client.listModels(chrome.host.deviceId, harness) }.getOrNull()?.takeIf { it.isNotEmpty() }
                ?: runCatching { uniffi.zeron_core.fallbackModels(harness) }.getOrDefault(emptyList())
        }
    }
    fun setConfig(change: (uniffi.zeron_core.ChatConfig) -> uniffi.zeron_core.ChatConfig) {
        val current = runCatching { client.sessionConfig(chatId) }.getOrNull()
            ?: uniffi.zeron_core.ChatConfig(harness = harness, model = row?.model, reasoning = row?.reasoning, modelOptions = emptyMap(), sandbox = uniffi.zeron_core.SandboxLevel.WORKSPACE_WRITE)
        try {
            client.setSessionConfig(chatId, change(current))
            model.refreshPull()
        } catch (t: Throwable) {
            model.showToast(t.message ?: "Couldn't change the session")
        }
    }
    val pr = row?.pullRequest
    val (title, entries) = when (chip.id) {
        "model" -> "Model" to models.orEmpty().map { m ->
            MenuEntry(m.label, subtitle = m.description, checked = m.id == row?.model) { setConfig { it.copy(model = m.id) } }
        }
        "effort" -> {
            val all = models.orEmpty()
            val levels = all.firstOrNull { it.id == row?.model }?.reasoningLevels ?: all.firstOrNull()?.reasoningLevels ?: emptyList()
            "Reasoning effort" to levels.map { l ->
                MenuEntry(reasoningLabel(l), checked = l == row?.reasoning) { setConfig { it.copy(reasoning = l) } }
            }
        }
        "pr" -> (pr?.title ?: "") to listOfNotNull(
            pr?.let { p ->
                MenuEntry("Open Pull Request", icon = { c -> AssetIcon("tool-global", 16.dp, c) }) {
                    runCatching { context.startActivity(android.content.Intent(android.content.Intent.ACTION_VIEW, android.net.Uri.parse(p.url))) }
                }
            },
            pr?.let { p ->
                MenuEntry("Copy Link", icon = { c -> AssetIcon("fileicon-files-link", 16.dp, c) }) {
                    val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                    cm.setPrimaryClip(android.content.ClipData.newPlainText("pull request", p.url))
                    model.showToast("Copied")
                }
            },
        )
        "branch" -> (row?.branch ?: "") to listOf(
            MenuEntry("Copy Branch Name", icon = { c -> AssetIcon("tool-git-branch", 16.dp, c) }) {
                val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                cm.setPrimaryClip(android.content.ClipData.newPlainText("branch", row?.branch ?: ""))
                model.showToast("Copied")
            },
        )
        else -> null to emptyList()
    }
    AnchoredMenu(colors, anchor, title, entries, loading = (chip.id == "model" || chip.id == "effort") && models == null, onDismiss = onDismiss)
}

/**
 * iOS StatusPill: a small glass capsule above the composer with a status dot
 * — offline, reconnecting countdown, not delivered (tap to retry), a failed
 * send, editing a queued message (tap to cancel), upload progress.
 */
@Composable
private fun StatusPill(
    chrome: uniffi.zeron_core.ComposerState,
    connectivity: uniffi.zeron_core.Connectivity?,
    sendFailure: String?,
    editing: Boolean,
    colors: ZeronColors,
    onTap: () -> Unit,
) {
    var now by remember { mutableStateOf(System.currentTimeMillis()) }
    val retryAt = chrome.room.retryAtMs.takeIf { !chrome.room.connected }
    LaunchedEffect(retryAt) {
        while (retryAt != null) {
            now = System.currentTimeMillis()
            kotlinx.coroutines.delay(1000)
        }
    }
    val progress = chrome.transferProgress
    val (dot, text) = when {
        editing -> colors.input to "Editing queued message · Tap to cancel"
        sendFailure != null -> colors.danger to sendFailure
        chrome.sendState == SendState.FAILED -> colors.danger to "Not delivered · Tap to retry"
        chrome.sendState == SendState.QUEUED -> colors.tertiary to "${chrome.host.name ?: "Host"} is offline — will send when it's back"
        progress != null && progress < 1.0 -> colors.accent to "Uploading · ${Math.round(progress * 100)}%"
        connectivity?.state == uniffi.zeron_core.ConnectivityState.OFFLINE -> colors.tertiary to "Offline — sends are saved"
        retryAt != null -> colors.tertiary to "Reconnecting in ${((retryAt - now) / 1000).coerceAtLeast(1)}s"
        chrome.queueError != null -> colors.danger to chrome.queueError!!
        else -> return
    }
    Row(Modifier.padding(bottom = 8.dp)) {
        Row(
            Modifier.height(30.dp).glassSurface(colors, 15.dp).clickable(onClick = onTap).padding(start = 11.dp, end = 12.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(8.dp).clip(RoundedCornerShape(4.dp)).background(dot))
            Spacer(Modifier.width(8.dp))
            Text(text, color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 13.sp, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
private fun QueueCard(
    items: List<uniffi.zeron_core.QueueItem>,
    colors: ZeronColors,
    onMove: (String, Int) -> Unit,
    onNow: (String) -> Unit,
    onRemove: (String) -> Unit,
    onEdit: (uniffi.zeron_core.QueueItem) -> Unit,
) {
    Column(Modifier.fillMaxWidth().glassSurface(colors, 22.dp).padding(12.dp)) {
        Text("Queued", color = colors.secondary, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 12.sp)
        items.forEachIndexed { index, item ->
            // iOS QueuePanel gate labels: who holds the row, or why it waits.
            val gate = when (val g = item.gate) {
                is uniffi.zeron_core.QueueGate.Editing -> if (g.mine) "Editing" else "Being edited"
                is uniffi.zeron_core.QueueGate.ReviewRequired -> "Needs review"
                null -> if (item.actionPending) "Updating" else null
            }
            Row(Modifier.fillMaxWidth().padding(top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                Column(Modifier.weight(1f)) {
                    Text(item.visibleText, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 14.sp, maxLines = 2, overflow = TextOverflow.Ellipsis)
                    if (gate != null) Text(gate, color = colors.input, fontFamily = ZeronType.Sans, fontSize = 12.sp)
                }
                if (items.size > 1) {
                    Text("↑", color = if (index > 0) colors.secondary else colors.tertiary.copy(alpha = 0.4f), fontSize = 15.sp, modifier = Modifier.clickable(enabled = index > 0) { onMove(item.id, -1) }.padding(6.dp))
                    Text("↓", color = if (index < items.size - 1) colors.secondary else colors.tertiary.copy(alpha = 0.4f), fontSize = 15.sp, modifier = Modifier.clickable(enabled = index < items.size - 1) { onMove(item.id, 1) }.padding(6.dp))
                }
                Text("Edit", color = colors.accent, fontSize = 13.sp, modifier = Modifier.clickable { onEdit(item) }.padding(6.dp))
                Text("Now", color = colors.text, fontSize = 13.sp, modifier = Modifier.clickable { onNow(item.id) }.padding(6.dp))
                Text("Remove", color = colors.danger, fontSize = 13.sp, modifier = Modifier.clickable { onRemove(item.id) }.padding(6.dp))
            }
        }
    }
}

@Composable
private fun QuestionCard(questions: List<UserInputQuestion>, colors: ZeronColors, onSubmit: (List<UserInputAnswer>) -> Unit) {
    var page by remember(questions) { mutableStateOf(0) }
    val picks = remember(questions) { mutableMapOf<String, MutableSet<String>>() }
    val q = questions.getOrNull(page) ?: return
    Column(Modifier.fillMaxWidth().glassSurface(colors, 26.dp).padding(16.dp)) {
        Text("${page + 1} of ${questions.size} · ${q.header}", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 12.5.sp)
        Spacer(Modifier.height(8.dp))
        Text(q.question, color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
        Spacer(Modifier.height(12.dp))
        q.options.forEach { option ->
            val selected = picks[q.id]?.contains(option) == true
            Text(
                option,
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontSize = 16.sp,
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(vertical = 4.dp)
                    .clip(RoundedCornerShape(12.dp))
                    .background(if (selected) colors.accentSoft else colors.chip.copy(alpha = 0.7f))
                    .clickable {
                        val set = picks.getOrPut(q.id) { mutableSetOf() }
                        if (q.multiSelect) {
                            if (!set.add(option)) set.remove(option)
                        } else {
                            set.clear(); set.add(option)
                            if (page < questions.lastIndex) page++ else onSubmit(finish(questions, picks))
                        }
                    }
                    .padding(horizontal = 14.dp, vertical = 12.dp),
            )
        }
        Row(Modifier.fillMaxWidth().padding(top = 8.dp), horizontalArrangement = Arrangement.SpaceBetween) {
            Text("Back", color = colors.secondary, modifier = Modifier.clickable { if (page > 0) page-- }.padding(8.dp))
            Text(if (page == questions.lastIndex) "Send" else "Next", color = colors.text, fontWeight = FontWeight.SemiBold, modifier = Modifier.clickable {
                if (page < questions.lastIndex) page++ else onSubmit(finish(questions, picks))
            }.padding(8.dp))
        }
    }
}

private fun finish(questions: List<UserInputQuestion>, picks: Map<String, Set<String>>) =
    questions.map { UserInputAnswer(it.id, picks[it.id]?.toList() ?: emptyList()) }

private fun chipsFor(row: uniffi.zeron_core.SessionRow?, chrome: uniffi.zeron_core.ComposerState): List<Chip> {
    val chips = mutableListOf<Chip>()
    val model = row?.modelLabel ?: row?.model?.let { modelLabel(row.harness ?: "", it) } ?: row?.harness?.let { harnessLabel(it) }
    if (model != null) chips.add(Chip("model", model, harness = row?.harness ?: "claude-code"))
    row?.reasoning?.takeIf { it.isNotEmpty() }?.let { chips.add(Chip("effort", reasoningLabel(it))) }
    val pr = row?.pullRequest
    if (pr != null) {
        chips.add(Chip("pr", "${pr.number}", prState = pr.state))
    } else {
        row?.branch?.takeIf { it.isNotEmpty() }?.let { chips.add(Chip("branch", it)) }
    }
    // Context (and plan) usage live in the ring cluster beside the send button.
    return chips
}

@Composable
private fun ChipView(chip: Chip, colors: ZeronColors, onTap: (Rect) -> Unit) {
    var bounds by remember { mutableStateOf(Rect.Zero) }
    val tone = when (chip.prState) {
        null -> if (chip.warn) colors.warning else null
        uniffi.zeron_core.PullRequestState.MERGED -> colors.accent
        uniffi.zeron_core.PullRequestState.CLOSED -> colors.danger
        else -> colors.success
    }
    val mono = chip.id == "branch" || chip.id == "pr"
    Row(
        Modifier
            .onGloballyPositioned { bounds = it.boundsInRoot() }
            .clip(RoundedCornerShape(14.dp))
            .background(tone?.copy(alpha = 0.1f) ?: colors.controlFill)
            .clickable { onTap(bounds) }
            .padding(horizontal = 10.dp, vertical = 6.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        val fg = tone?.copy(alpha = 0.85f) ?: colors.text
        when (chip.id) {
            "project" -> if (chip.colorIndex != null) ProjectTile(chip.title, chip.colorIndex, colors, 14.dp) else Glyph(Glyphs.Tray, 14.dp, fg)
            "host" -> Glyph(Glyphs.Computer, 14.dp, fg)
            "model" -> BrandMark(chip.harness, colors, 13.dp)
            "effort" -> GaugeGlyph(fg, Modifier.size(13.dp))
            "pr" -> PrGlyph(fg, Modifier.size(12.dp))
            "branch" -> AssetIcon("tool-git-branch", 12.dp, fg)
            else -> Unit
        }
        if (chip.id in setOf("project", "host", "model", "effort", "pr", "branch")) Spacer(Modifier.width(if (chip.id == "pr") 5.dp else 6.dp))
        Text(
            chip.title,
            color = fg,
            fontFamily = if (mono) ZeronType.Mono else ZeronType.Sans,
            fontWeight = FontWeight.Medium,
            fontSize = if (mono) 13.sp else 14.sp,
            maxLines = 1,
        )
    }
}

@Composable
internal fun ComposerBar(
    colors: ZeronColors,
    text: String,
    onText: (String) -> Unit,
    placeholder: String,
    running: Boolean,
    canSteer: Boolean,
    focused: Boolean,
    onFocus: (Boolean) -> Unit,
    chips: List<Chip>,
    onChip: (Chip, Rect) -> Unit = { _, _ -> },
    images: List<Staged>,
    onRemoveImage: (Staged) -> Unit,
    onAttach: () -> Unit = {},
    onSend: (Delivery) -> Unit,
    mentionSearch: suspend (String) -> List<uniffi.zeron_core.FileMatch>,
    onMention: (String, Boolean) -> Unit,
    /** Usage rings (desktop footer ring cluster), just before the send button. */
    rings: (@Composable () -> Unit)? = null,
) {
    var trailingPx by remember { mutableIntStateOf(0) }
    val trailingDp = with(androidx.compose.ui.platform.LocalDensity.current) { trailingPx.toDp() }
    val resting = !focused && text.isEmpty() && images.isEmpty()
    val has = text.isNotBlank() || images.isNotEmpty()
    val stop = running && !has
    var deliveryMenu by remember { mutableStateOf(false) }
    var mentions by remember { mutableStateOf(listOf<uniffi.zeron_core.FileMatch>()) }
    val scope = rememberCoroutineScope()
    val keyboard = LocalSoftwareKeyboardController.current
    val card = !resting
    fun onTextChange(value: String) {
        onText(value)
        val query = Regex("@([^\\s]*)$").find(value)?.groupValues?.getOrNull(1)
        if (query != null) {
            scope.launch { mentions = mentionSearch(query).take(8) }
        } else mentions = emptyList()
    }
    Column(
        Modifier
            .fillMaxWidth()
            .glassSurface(colors, if (resting) 25.dp else 26.dp)
            .then(if (resting) Modifier.height(50.dp) else Modifier)
            .padding(horizontal = if (resting) 14.dp else 8.dp, vertical = if (resting) 0.dp else 6.dp),
        verticalArrangement = if (resting) Arrangement.Center else Arrangement.Top,
    ) {
        if (mentions.isNotEmpty()) {
            Column(Modifier.heightIn(max = 180.dp).verticalScroll(rememberScrollState())) {
                mentions.forEach { match ->
                    Text(
                        match.path,
                        color = colors.text,
                        fontFamily = ZeronType.Sans,
                        fontSize = 14.sp,
                        maxLines = 1,
                        modifier = Modifier.fillMaxWidth().clickable {
                            onMention(match.path, match.isDir)
                            mentions = emptyList()
                        }.padding(8.dp),
                    )
                }
            }
        }
        if (images.isNotEmpty()) {
            Row(Modifier.padding(bottom = 6.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                images.forEach { staged ->
                    Box {
                        if (staged.preview != null) {
                            Image(
                                staged.preview.asImageBitmap(),
                                contentDescription = staged.name,
                                modifier = Modifier.size(56.dp).clip(RoundedCornerShape(10.dp)),
                                contentScale = ContentScale.Crop,
                            )
                        }
                        Text("×", color = Color.White, modifier = Modifier.align(Alignment.TopEnd).clickable { onRemoveImage(staged) }.padding(2.dp))
                    }
                }
            }
        }
        // One field for both states. Swapping two fields on focus dropped the
        // IME, because the focused instance left the composition.
        Box(Modifier.fillMaxWidth()) {
            BasicTextField(
                value = text,
                onValueChange = { onTextChange(it) },
                textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.5.sp),
                cursorBrush = SolidColor(colors.accent),
                modifier = Modifier
                    .fillMaxWidth()
                    .padding(
                        start = 8.dp,
                        end = if (card) 8.dp else maxOf(42.dp, trailingDp + 8.dp),
                        top = if (card) 6.dp else 0.dp,
                        bottom = if (card) 48.dp else 0.dp,
                    )
                    .onFocusChanged {
                        onFocus(it.isFocused)
                        if (it.isFocused) keyboard?.show()
                    },
                decorationBox = { inner ->
                    Box {
                        if (text.isEmpty()) Text(placeholder, color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 16.5.sp)
                        inner()
                    }
                },
                maxLines = if (card) 8 else 1,
            )
            if (card) {
                Row(
                    Modifier.align(Alignment.BottomCenter).fillMaxWidth().height(44.dp),
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    Box(
                        Modifier.size(34.dp).clip(RoundedCornerShape(17.dp)).background(colors.controlFill).clickable(onClick = onAttach),
                        contentAlignment = Alignment.Center,
                    ) {
                        PlusMark(colors.text, Modifier.size(16.dp))
                    }
                    val chipScroll = rememberScrollState()
                    Row(
                        Modifier.weight(1f).padding(horizontal = 8.dp).fadeEdges(chipScroll).horizontalScroll(chipScroll),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        chips.forEach { chip ->
                            ChipView(chip, colors) { rect -> onChip(chip, rect) }
                        }
                    }
                    rings?.let { Box(Modifier.padding(end = 4.dp)) { it() } }
                    SendButton(colors, stop, has, running, canSteer, deliveryMenu, { deliveryMenu = it }, onSend)
                }
            } else {
                Row(
                    Modifier.align(Alignment.CenterEnd).onSizeChanged { trailingPx = it.width },
                    verticalAlignment = Alignment.CenterVertically,
                ) {
                    rings?.let { Box(Modifier.padding(end = 4.dp)) { it() } }
                    SendButton(colors, stop, has, running, canSteer, deliveryMenu, { deliveryMenu = it }, onSend)
                }
            }
        }
    }
}

@Composable
private fun SendButton(
    colors: ZeronColors,
    stop: Boolean,
    has: Boolean,
    running: Boolean,
    canSteer: Boolean,
    deliveryMenu: Boolean,
    onMenu: (Boolean) -> Unit,
    onSend: (Delivery) -> Unit,
) {
    var anchor by remember { mutableStateOf(Rect.Zero) }
    Box {
        Box(
            Modifier
                .size(34.dp)
                .onGloballyPositioned { anchor = it.boundsInWindow() }
                .clip(RoundedCornerShape(17.dp))
                .background(if (stop) colors.text else if (has) colors.accent else colors.text.copy(alpha = 0.10f))
                .combinedClickable(
                    onClick = { onSend(if (stop) Delivery.Send else if (running && canSteer) Delivery.Steer else Delivery.Queue) },
                    onLongClick = { if (running && has) onMenu(true) },
                ),
            contentAlignment = Alignment.Center,
        ) {
            if (stop) StopMark(colors.background, Modifier.size(16.dp)) else ArrowUpMark(if (has) Color.White else colors.tertiary, Modifier.size(16.dp))
        }
        if (running && has && deliveryMenu) {
            PopupAnchoredMenu(
                colors,
                anchor,
                title = null,
                entries = listOfNotNull(
                    MenuEntry("Queue for next turn") { onSend(Delivery.Queue) },
                    if (canSteer) MenuEntry("Steer now") { onSend(Delivery.Steer) } else null,
                    MenuEntry("Stop and send", destructive = true) { onSend(Delivery.Interrupt) },
                ),
                above = true,
            ) { onMenu(false) }
        }
    }
}

@Composable
fun SignInScreen(model: ZeronModel) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    Column(
        Modifier.fillMaxSize().background(colors.background).statusBarsPadding().navigationBarsPadding().padding(24.dp),
        verticalArrangement = Arrangement.SpaceBetween,
    ) {
        Spacer(Modifier.height(1.dp))
        Column(horizontalAlignment = Alignment.CenterHorizontally, modifier = Modifier.fillMaxWidth()) {
            Text("✦", color = colors.text, fontSize = 44.sp)
            Spacer(Modifier.height(12.dp))
            Text("Zeron", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 34.sp)
            Spacer(Modifier.height(8.dp))
            Text("Your coding agents, from anywhere.", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 17.sp)
        }
        Column {
            model.signInError?.let { Text(it, color = colors.danger, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(bottom = 12.dp)) }
            model.authOrgs?.let { orgs ->
                Text("Choose an organization", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, modifier = Modifier.padding(bottom = 8.dp))
                orgs.forEach { org ->
                    Text(org.name, color = colors.text, modifier = Modifier.fillMaxWidth().clickable { model.chooseOrg(org) }.padding(vertical = 10.dp), fontFamily = ZeronType.Sans, fontSize = 16.sp)
                }
            }
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(colors.text).clickable {
                    val url = model.authorizeUrl()
                    val tabs = androidx.browser.customtabs.CustomTabsIntent.Builder().build()
                    tabs.launchUrl(context, android.net.Uri.parse(url))
                }.padding(vertical = 15.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text(if (model.signInBusy) "Signing in…" else "Sign In", color = colors.background, fontFamily = ZeronType.Sans, fontWeight = FontWeight.SemiBold, fontSize = 17.sp)
            }
            Spacer(Modifier.height(12.dp))
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(colors.controlFill).clickable { model.enterDemo() }.padding(vertical = 15.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Explore the demo", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 17.sp)
            }
            Spacer(Modifier.height(12.dp))
            Box(
                Modifier.fillMaxWidth().clip(RoundedCornerShape(28.dp)).background(colors.controlFill).clickable { model.showMachines = true }.padding(vertical = 15.dp),
                contentAlignment = Alignment.Center,
            ) {
                Text("Connect to your computer (SSH)", color = colors.text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 17.sp)
            }
        }
    }
}

private val ChatIndicator.word: String?
    get() = when (this) {
        ChatIndicator.WORKING -> "Working"
        ChatIndicator.AWAITING_INPUT -> "Input"
        ChatIndicator.ERRORED -> "Failed"
        ChatIndicator.COMPLETED -> "Done"
        ChatIndicator.IDLE -> null
    }

/**
 * Soft edges on a horizontally scrolling row (like iOS's scroll edge effect):
 * content fades out over [width] at a side only when there is more to scroll
 * that way, instead of being cut off hard.
 */
internal fun Modifier.fadeEdges(state: androidx.compose.foundation.ScrollState, width: androidx.compose.ui.unit.Dp = 16.dp): Modifier =
    this
        .graphicsLayer { compositingStrategy = androidx.compose.ui.graphics.CompositingStrategy.Offscreen }
        .drawWithContent {
            drawContent()
            val w = width.toPx().coerceAtMost(size.width / 2f)
            if (state.value > 0) {
                drawRect(
                    androidx.compose.ui.graphics.Brush.horizontalGradient(listOf(Color.Transparent, Color.Black), startX = 0f, endX = w),
                    size = androidx.compose.ui.geometry.Size(w, size.height),
                    blendMode = androidx.compose.ui.graphics.BlendMode.DstIn,
                )
            }
            if (state.value < state.maxValue) {
                drawRect(
                    androidx.compose.ui.graphics.Brush.horizontalGradient(listOf(Color.Black, Color.Transparent), startX = size.width - w, endX = size.width),
                    topLeft = androidx.compose.ui.geometry.Offset(size.width - w, 0f),
                    size = androidx.compose.ui.geometry.Size(w, size.height),
                    blendMode = androidx.compose.ui.graphics.BlendMode.DstIn,
                )
            }
        }
