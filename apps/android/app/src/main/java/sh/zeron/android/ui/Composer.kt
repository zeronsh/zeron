package sh.zeron.android.ui

import android.net.Uri
import androidx.browser.customtabs.CustomTabsIntent
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.ExperimentalLayoutApi
import androidx.compose.foundation.layout.FlowRow
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.outlined.CallSplit
import androidx.compose.material.icons.filled.ArrowUpward
import androidx.compose.material.icons.filled.Stop
import androidx.compose.material.icons.outlined.AutoAwesome
import androidx.compose.material.icons.outlined.DataUsage
import androidx.compose.material.icons.outlined.Delete
import androidx.compose.material.icons.outlined.KeyboardArrowDown
import androidx.compose.material.icons.outlined.KeyboardArrowUp
import androidx.compose.material.icons.outlined.MoreHoriz
import androidx.compose.material.icons.outlined.Send
import androidx.compose.material.icons.outlined.Speed
import androidx.compose.material.icons.outlined.Schedule
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.transcript.TranscriptState
import androidx.compose.runtime.DisposableEffect
import androidx.compose.ui.focus.FocusRequester
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import uniffi.zeron_core.QueueEditAction
import uniffi.zeron_core.QueueEditLease
import uniffi.zeron_core.QueueEditStart
import java.util.UUID
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import androidx.compose.material3.AssistChip
import androidx.compose.material3.AssistChipDefaults
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.DropdownMenu
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.ToggleButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.saveable.rememberSaveable
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import uniffi.zeron_core.BusyPolicy
import uniffi.zeron_core.ChatConfig
import uniffi.zeron_core.ComposerState
import uniffi.zeron_core.CoreClient
import uniffi.zeron_core.InputRequest
import uniffi.zeron_core.ModelInfo
import uniffi.zeron_core.QueueGate
import uniffi.zeron_core.QueueItem
import uniffi.zeron_core.SandboxLevel
import uniffi.zeron_core.SendRequest
import uniffi.zeron_core.SessionHandle
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.UserInputAnswer
import uniffi.zeron_core.fallbackModels
import uniffi.zeron_core.harnessLabel
import uniffi.zeron_core.reasoningLabel

private enum class Delivery(val label: String) { Queue("Queue"), Steer("Steer"), Interrupt("Stop & send") }

/**
 * The session composer: the shared surface plus this chat's context chips,
 * staged photos, `@` mentions, and editing of messages queued behind the
 * live turn (a host lease, renewed while the edit is open).
 */
@Composable
fun Composer(
    model: AppModel,
    client: CoreClient,
    handle: SessionHandle,
    c: ComposerState,
    row: SessionRow?,
    transcript: TranscriptState,
) {
    val draft = remember(c.chatId) { ComposerModel() }
    val scope = rememberCoroutineScope()
    val focus = remember { FocusRequester() }
    var error by remember { mutableStateOf<String?>(null) }
    var delivery by remember { mutableStateOf(Delivery.Queue) }
    val running = c.live.turnRunning
    val canSteer = c.host.capabilities.midTurnSteering == true

    // Queue editing: the row being edited, its lease, and the user's own draft parked meanwhile.
    var editingId by remember { mutableStateOf<String?>(null) }
    var lease by remember { mutableStateOf<QueueEditLease?>(null) }
    var editBase by remember { mutableStateOf<Pair<String, String>?>(null) }
    var renewal by remember { mutableStateOf<Job?>(null) }
    var stash by remember { mutableStateOf<Pair<String, List<StagedImage>>?>(null) }
    var editPending by remember { mutableStateOf(false) }

    fun restoreStash() {
        stash?.let { (text, images) ->
            draft.clear()
            draft.setText(text)
            draft.images.addAll(images)
        }
        stash = null
    }

    fun finishEdit(text: String?) {
        val held = lease ?: return
        val base = editBase
        renewal?.cancel()
        lease = null
        editBase = null
        editingId = null
        // Unchanged: release the row as it was (a commit would drop what the composer never showed).
        val body = if (text != null && base != null) {
            if (text == base.second) null else replacingVisible(base.first, base.second, text)
        } else {
            text
        }
        scope.launch { runCatching { handle.finishQueuedEdit(held, if (body == null) QueueEditAction.CANCEL else QueueEditAction.COMMIT, body) } }
    }

    fun beginEdit(id: String) {
        if (editPending) return
        editPending = true
        scope.launch {
            try {
                if (editingId != null) {
                    finishEdit(null)
                    restoreStash()
                }
                val start = runCatching { handle.beginQueuedEdit(id, UUID.randomUUID().toString()) }.getOrNull()
                if (start !is QueueEditStart.Acquired) {
                    error = "Can't edit right now — another device is editing it, or it was just sent."
                    return@launch
                }
                lease = start.lease
                renewal = scope.launch {
                    while (true) {
                        delay(20_000)
                        val held = lease ?: break
                        if (!runCatching { handle.renewQueuedEdit(held) }.getOrDefault(false)) break
                    }
                }
                val item = handle.composer().queue.firstOrNull { it.id == id }
                editBase = item?.let { it.text to it.visibleText }
                if (stash == null) stash = draft.text to draft.images.toList()
                draft.clear()
                draft.setText(item?.visibleText ?: "")
                editingId = id
                error = null
                focus.requestFocus()
            } finally {
                editPending = false
            }
        }
    }

    fun send() {
        if (!draft.hasContent) return
        if (editingId != null) {
            finishEdit(draft.encoded())
            restoreStash()
            return
        }
        val body = draft.encoded()
        try {
            val queued = running && delivery == Delivery.Queue
            if (delivery == Delivery.Interrupt && running) handle.interrupt()
            handle.send(
                SendRequest(
                    body,
                    draft.images.map { it.outgoing },
                    null,
                    if (running && delivery == Delivery.Steer) BusyPolicy.STEER else BusyPolicy.QUEUE,
                ),
            )
            draft.clear()
            error = null
            delivery = Delivery.Queue
            // An immediate send gets the runway; one queued behind a live turn
            // takes it over once its bubble lands.
            if (queued) transcript.expectQueuedTurn() else transcript.beginOwnTurn()
        } catch (e: Exception) {
            error = "Couldn't send: ${e.message}"
        }
    }

    // Leaving the session gives an edited row straight back.
    DisposableEffect(c.chatId) { onDispose { finishEdit(null) } }

    Column(Modifier.padding(horizontal = 12.dp).padding(bottom = 8.dp)) {
        if (c.queue.isNotEmpty()) QueuePanel(c.queue, handle, editingId, onEdit = ::beginEdit)
        MentionSuggestions(draft, search = { q ->
            client.searchFiles(c.host.deviceId, c.chatId, null, q)
        })
        error?.let {
            Text(it, color = MaterialTheme.colorScheme.error, style = MaterialTheme.typography.bodySmall, modifier = Modifier.padding(start = 16.dp, bottom = 6.dp))
        }
        if (editingId != null) {
            StatusBanner("Editing queued message", "Cancel" to {
                finishEdit(null)
                restoreStash()
            })
        }
        ComposerSurface(
            model = draft,
            placeholder = if (editingId != null) "Edit queued message" else "Message ${row?.harnessLabel ?: "the agent"}",
            action = when {
                editingId != null -> ComposerAction.Send
                running && !draft.hasContent -> ComposerAction.Stop
                running -> ComposerAction.Queue
                else -> ComposerAction.Send
            },
            onAction = { if (editingId == null && running && !draft.hasContent) runCatching { handle.interrupt() } else send() },
            attach = editingId == null,
            focusRequester = focus,
        ) {
            if (running && editingId == null) DeliveryChip(delivery, canSteer) { delivery = it }
            SessionChips(model, client, c, row)
        }
    }
}

/** The row's raw text with its visible part replaced, keeping the hidden context after it. */
private fun replacingVisible(raw: String, visible: String, edited: String): String {
    if (edited.isBlank() || raw == visible) return edited
    val body = raw.trimStart()
    if (body.startsWith(visible)) return edited + body.substring(visible.length)
    // Attachment-only rows show a placeholder: all of the raw text is context.
    return if (body.isEmpty()) edited else edited + "\n\n" + body
}

@Composable
private fun DeliveryChip(current: Delivery, canSteer: Boolean, onChange: (Delivery) -> Unit) {
    var open by remember { mutableStateOf(false) }
    ContextChip(
        current.label,
        leading = { ZIcon(ZIcons.Queue, null, Modifier.size(16.dp)) },
        onClick = { open = true },
        tint = MaterialTheme.colorScheme.primary,
    ) {
        ChoiceMenu(
            open,
            { open = false },
            listOf(
                MenuSection(
                    "While the agent works",
                    listOfNotNull(
                        MenuChoice("Queue", current == Delivery.Queue, "Send when this turn ends") { onChange(Delivery.Queue) },
                        if (canSteer) MenuChoice("Steer", current == Delivery.Steer, "Add to the running turn") { onChange(Delivery.Steer) } else null,
                        MenuChoice("Stop & send", current == Delivery.Interrupt, "Stop the turn, then send") { onChange(Delivery.Interrupt) },
                    ),
                ),
            ),
        )
    }
}

@Composable
private fun SessionChips(app: AppModel, client: CoreClient, c: ComposerState, row: SessionRow?) {
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val harness = row?.harness ?: "claude-code"
    val deviceId = c.host.deviceId
    val favorites by app.favorites.favorites.collectAsState()
    val workspace by app.workspace.collectAsState()
    // The harness's models: the device's list (the picker shows a loader until it lands).
    var models by remember(deviceId, harness) { mutableStateOf<List<ModelInfo>?>(null) }
    var menu by remember { mutableStateOf<String?>(null) }

    fun open(which: String) {
        menu = which
        if (models == null) scope.launch {
            models = runCatching { client.listModels(deviceId, harness) }.getOrNull() ?: fallbackModels(harness)
        }
    }

    fun setConfig(change: (ChatConfig) -> ChatConfig) {
        val current = client.sessionConfig(c.chatId) ?: ChatConfig(harness, row?.model, row?.reasoning, emptyMap(), SandboxLevel.WORKSPACE_WRITE)
        runCatching { client.setSessionConfig(c.chatId, change(current)) }
    }

    val modelLabel = row?.modelLabel ?: row?.harnessLabel
    if (modelLabel != null) {
        val label = row?.harnessLabel ?: harnessLabel(harness)
        val catalog = models.orEmpty().map { ModelChoice(harness, label, it) }
        val current = row?.model?.let { id ->
            catalog.firstOrNull { it.model.id == id }
                ?: ModelChoice(harness, label, ModelInfo(id, row.modelLabel ?: id, "Selected in this session; not in the device's model list", emptyList(), emptyList(), null))
        }
        ContextChip(modelLabel, leading = { HarnessMark(harness, 14.dp) }, onClick = { open("model") }) {
            // A session keeps its harness (as on the desktop): its own provider and its favorites.
            ModelPickerPopover(
                expanded = menu == "model",
                onDismiss = { menu = null },
                catalog = catalog,
                current = current,
                favorites = favorites,
                onToggleFavorite = app.favorites::toggle,
                onPick = { m -> setConfig { it.copy(model = m.model.id) } },
                locked = true,
                loading = models == null,
                labelFor = { harnessLabel(it) },
            )
        }
    }
    // Effort: the device's ladder once loaded, the built-in catalog's until then
    // (never an empty menu); shown whenever the model has one.
    val builtIn = remember(harness) { fallbackModels(harness) }
    val ladder = (models ?: builtIn).let { list -> list.firstOrNull { it.id == row?.model } ?: list.firstOrNull() }
    val levels = ladder?.reasoningLevels.orEmpty()
    val level = row?.reasoning?.takeIf { it.isNotEmpty() } ?: ladder?.defaultReasoning?.takeIf { it in levels }
    if (row != null && level != null && levels.isNotEmpty()) {
        ContextChip(reasoningLabel(level), leading = { ZIcon(ZIcons.Effort, null, Modifier.size(16.dp)) }, onClick = { open("effort") }) {
            ChoiceMenu(menu == "effort", { menu = null }, listOf(MenuSection("Reasoning effort", levels.map { l ->
                MenuChoice(reasoningLabel(l), l == level) { setConfig { it.copy(reasoning = l) } }
            })))
        }
    }
    val pr = row?.pullRequest
    if (pr != null) {
        ContextChip("#${pr.number}", leading = { ZIcon(ZIcons.PullRequest, null, Modifier.size(16.dp)) }, onClick = {
            CustomTabsIntent.Builder().build().launchUrl(context, Uri.parse(pr.url))
        })
    }
    val project = workspace?.projects?.firstOrNull { it.id == row?.project?.id }
    if (project?.gitDetected == true || !row?.branch.isNullOrEmpty()) {
        SessionBranchChip(row?.branch?.takeIf { it.isNotEmpty() }, row?.cwd, project?.path)
    }
    ContextUsageChip(c.contextUsage)
}

@Composable
fun StatusBanner(text: String, action: Pair<String, () -> Unit>?) {
    Surface(
        shape = RoundedCornerShape(50),
        color = MaterialTheme.colorScheme.secondaryContainer,
        contentColor = MaterialTheme.colorScheme.onSecondaryContainer,
        modifier = Modifier.padding(horizontal = 16.dp, vertical = 6.dp),
    ) {
        Row(Modifier.padding(start = 16.dp, end = if (action != null) 4.dp else 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Text(text, style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(vertical = 10.dp).weight(1f, fill = false))
            action?.let { (label, run) -> TextButton(onClick = run) { Text(label) } }
        }
    }
}

/** The agent asked something: answer with its options instead of typing. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class, ExperimentalLayoutApi::class)
@Composable
fun QuestionPanel(input: InputRequest, onSubmit: (List<UserInputAnswer>) -> Unit) {
    val picked = remember(input.requestId) { input.questions.associate { it.id to mutableStateListOf<String>() } }
    Surface(
        shape = RoundedCornerShape(28.dp),
        color = MaterialTheme.colorScheme.surfaceContainerHigh,
        modifier = Modifier.fillMaxWidth().padding(horizontal = 10.dp, vertical = 4.dp),
    ) {
        Column(Modifier.padding(20.dp), verticalArrangement = Arrangement.spacedBy(12.dp)) {
            for (q in input.questions) {
                Column(verticalArrangement = Arrangement.spacedBy(8.dp)) {
                    if (q.header.isNotEmpty()) Text(q.header, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.primary)
                    Text(q.question, style = MaterialTheme.typography.bodyLarge)
                    FlowRow(horizontalArrangement = Arrangement.spacedBy(6.dp), verticalArrangement = Arrangement.spacedBy(6.dp)) {
                        val selection = picked.getValue(q.id)
                        for (option in q.options) {
                            ToggleButton(
                                checked = option in selection,
                                onCheckedChange = { on ->
                                    if (!q.multiSelect) selection.clear()
                                    if (on) selection.add(option) else selection.remove(option)
                                },
                            ) { Text(option) }
                        }
                    }
                }
            }
            val ready = input.questions.all { picked.getValue(it.id).isNotEmpty() }
            Button(
                onClick = { onSubmit(input.questions.map { UserInputAnswer(it.id, picked.getValue(it.id).toList()) }) },
                enabled = ready,
                shapes = ButtonDefaults.shapes(),
                modifier = Modifier.align(Alignment.End),
            ) {
                ZIcon(ZIcons.Send, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Answer")
            }
        }
    }
}

/** Messages queued behind the live turn (shared across devices). */
@Composable
fun QueuePanel(queue: List<QueueItem>, handle: SessionHandle, editingId: String?, onEdit: (String) -> Unit) {
    val scope = rememberCoroutineScope()
    Column(Modifier.fillMaxWidth().padding(bottom = 8.dp), verticalArrangement = Arrangement.spacedBy(2.dp)) {
        queue.forEachIndexed { i, item ->
            val editing = item.id == editingId
            Surface(
                shape = segmentShape(i, queue.size, outer = 20.dp, inner = 6.dp),
                color = if (editing) MaterialTheme.colorScheme.secondaryContainer else composerContainer(),
                border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
                modifier = Modifier.fillMaxWidth(),
            ) {
                Row(Modifier.padding(start = 14.dp, end = 4.dp, top = 6.dp, bottom = 6.dp), verticalAlignment = Alignment.CenterVertically) {
                    ZIcon(ZIcons.Queue, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.primary)
                    Spacer(Modifier.width(12.dp))
                    Column(Modifier.weight(1f)) {
                        Text(
                            item.visibleText.ifEmpty { "Attachment" },
                            style = MaterialTheme.typography.bodyMedium,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                        )
                        (if (editing) "Editing" else gate(item))?.let {
                            Text(it, style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                        }
                    }
                    var menu by remember { mutableStateOf(false) }
                    Box {
                        IconButton(onClick = { menu = true }) { ZIcon(ZIcons.More, "Queued message actions", Modifier.size(20.dp)) }
                        ActionMenu(
                            menu,
                            { menu = false },
                            listOfNotNull(
                                MenuAction("Send now", ZIcons.Send) { scope.launch { runCatching { handle.deliverQueuedNow(item.id) } } },
                                if (!editing && item.gate == null) MenuAction("Edit", ZIcons.Rename) { onEdit(item.id) } else null,
                                if (i > 0) MenuAction("Move up", ZIcons.ChevronUp) { runCatching { handle.moveQueuedBy(item.id, -1) } } else null,
                                if (i < queue.size - 1) MenuAction("Move down", ZIcons.ChevronDown) { runCatching { handle.moveQueuedBy(item.id, 1) } } else null,
                                MenuAction("Remove", ZIcons.Delete, destructive = true) { scope.launch { runCatching { handle.removeQueued(item.id) } } },
                            ),
                        )
                    }
                }
            }
        }
    }
}

private fun gate(item: QueueItem): String? = when (val g = item.gate) {
    is QueueGate.Editing -> if (g.mine) "Editing" else "Being edited"
    is QueueGate.ReviewRequired -> "Needs review"
    null -> if (item.actionPending) "Updating" else null
}
