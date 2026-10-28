package sh.zeron.android.ui

import android.net.Uri
import androidx.activity.compose.rememberLauncherForActivityResult
import androidx.activity.result.PickVisualMediaRequest
import androidx.activity.result.contract.ActivityResultContracts
import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.expandVertically
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.shrinkVertically
import androidx.compose.foundation.BorderStroke
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.verticalScroll
import androidx.compose.foundation.background
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.DropdownMenuGroup
import androidx.compose.material3.DropdownMenuItem
import androidx.compose.material3.DropdownMenuPopup
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.MenuDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.focus.onFocusChanged
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.input.key.isCtrlPressed
import androidx.compose.ui.input.key.isMetaPressed
import androidx.compose.ui.input.key.key
import androidx.compose.ui.input.key.onPreviewKeyEvent
import androidx.compose.ui.input.key.type
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.FileMatch

/**
 * The composer, shared by sessions and the new-session page. One tonal
 * surface with two states: a resting one-line capsule — [+] Message… [↑] —
 * and, while focused or holding a draft, a card with staged photos and the
 * full-width prompt above a toolbar of context chips.
 */
@Composable
fun ComposerSurface(
    model: ComposerModel,
    placeholder: String,
    action: ComposerAction,
    onAction: () -> Unit,
    modifier: Modifier = Modifier,
    alwaysCard: Boolean = false,
    attach: Boolean = true,
    focusRequester: FocusRequester = remember { FocusRequester() },
    chips: @Composable RowScope.() -> Unit = {},
) {
    var focused by remember { mutableStateOf(false) }
    val card = alwaysCard || focused || model.hasContent
    Surface(
        shape = RoundedCornerShape(28.dp),
        color = composerContainer(),
        border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = modifier.fillMaxWidth(),
    ) {
        Column(Modifier.animateContentSize(MaterialTheme.motionScheme.defaultSpatialSpec())) {
            AnimatedVisibility(model.images.isNotEmpty() && card) { StagedThumbnails(model) }
            Row(verticalAlignment = Alignment.CenterVertically, modifier = Modifier.heightIn(min = 56.dp)) {
                if (!card) {
                    Spacer(Modifier.width(4.dp))
                    AttachButton(model, attach)
                } else {
                    Spacer(Modifier.width(20.dp))
                }
                Box(Modifier.weight(1f).padding(vertical = if (card) 16.dp else 8.dp)) {
                    if (model.text.isEmpty()) {
                        Text(placeholder, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                    }
                    BasicTextField(
                        model.value,
                        { model.value = it },
                        textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                        cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                        maxLines = if (card) 8 else 2,
                        modifier = Modifier
                            .fillMaxWidth()
                            .focusRequester(focusRequester)
                            .onFocusChanged { focused = it.isFocused }
                            // Hardware keyboards: Ctrl/⌘+Enter sends (plain Enter is a newline).
                            .onPreviewKeyEvent { e ->
                                val send = e.type == androidx.compose.ui.input.key.KeyEventType.KeyDown &&
                                    e.key == androidx.compose.ui.input.key.Key.Enter && (e.isCtrlPressed || e.isMetaPressed)
                                if (send && action != ComposerAction.Stop && model.hasContent) onAction()
                                send
                            },
                    )
                }
                if (!card) {
                    Spacer(Modifier.width(8.dp))
                    ActionButton(action, model.hasContent, onAction)
                    Spacer(Modifier.width(8.dp))
                } else {
                    Spacer(Modifier.width(16.dp))
                }
            }
            AnimatedVisibility(card, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically()) {
                Row(
                    verticalAlignment = Alignment.CenterVertically,
                    modifier = Modifier.padding(start = 4.dp, end = 8.dp, bottom = 8.dp),
                ) {
                    AttachButton(model, attach)
                    Row(
                        Modifier
                            .weight(1f)
                            .graphicsLayer { compositingStrategy = CompositingStrategy.Offscreen }
                            .drawWithContent {
                                drawContent()
                                // Chips scrolling under the send button fade out.
                                val fade = 24.dp.toPx()
                                drawRect(
                                    Brush.horizontalGradient(listOf(Color.Black, Color.Transparent), startX = size.width - fade, endX = size.width),
                                    topLeft = Offset(size.width - fade, 0f),
                                    blendMode = BlendMode.DstIn,
                                )
                            }
                            .horizontalScroll(rememberScrollState()),
                        horizontalArrangement = Arrangement.spacedBy(6.dp),
                        verticalAlignment = Alignment.CenterVertically,
                        content = chips,
                    )
                    Spacer(Modifier.width(8.dp))
                    ActionButton(action, model.hasContent, onAction)
                }
            }
        }
    }
}

/** Staged photos above the prompt, each removable. */
@Composable
private fun StagedThumbnails(model: ComposerModel) {
    Row(
        Modifier.horizontalScroll(rememberScrollState()).padding(start = 12.dp, end = 12.dp, top = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (image in model.images) {
            Box(Modifier.size(72.dp)) {
                Image(image.thumb, "Attached image", Modifier.fillMaxSize().clip(RoundedCornerShape(16.dp)), contentScale = ContentScale.Crop)
                Surface(
                    onClick = { model.images.remove(image) },
                    shape = CircleShape,
                    color = MaterialTheme.colorScheme.inverseSurface.copy(alpha = 0.85f),
                    contentColor = MaterialTheme.colorScheme.inverseOnSurface,
                    modifier = Modifier.align(Alignment.TopEnd).padding(4.dp).size(22.dp),
                ) {
                    Box(contentAlignment = Alignment.Center) { ZIcon(ZIcons.Close, "Remove image", Modifier.size(14.dp)) }
                }
            }
        }
    }
}

/** [+] → Photos / Take photo / Files / Paste image, staged into the composer. */
@Composable
private fun AttachButton(model: ComposerModel, enabled: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var open by remember { mutableStateOf(false) }
    fun add(uris: List<Uri>) {
        scope.launch {
            for (uri in uris.take(Staging.MAX_IMAGES - model.images.size)) Staging.stage(context, uri)?.let { model.images.add(it) }
        }
    }
    val photos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Staging.MAX_IMAGES)) { add(it) }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { add(it) }
    // The camera writes into a cache file the FileProvider shares with it.
    var capture by remember { mutableStateOf<Uri?>(null) }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { saved ->
        val uri = capture
        if (saved && uri != null) add(listOf(uri))
    }
    val hasCamera = remember { context.packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_CAMERA_ANY) }
    Box {
        IconButton(onClick = { open = true }, enabled = enabled && model.images.size < Staging.MAX_IMAGES) {
            ZIcon(ZIcons.Plus, "Attach", Modifier.size(22.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        }
        val clip = if (open) context.getSystemService(android.content.ClipboardManager::class.java)?.primaryClip else null
        val pasted = clip?.takeIf { it.itemCount > 0 }?.getItemAt(0)?.uri
            ?.takeIf { context.contentResolver.getType(it)?.startsWith("image/") == true }
        ActionMenu(
            open,
            { open = false },
            listOfNotNull(
                MenuAction("Photos", ZIcons.Gallery) { photos.launch(PickVisualMediaRequest(ActivityResultContracts.PickVisualMedia.ImageOnly)) },
                if (hasCamera) {
                    MenuAction("Take photo", ZIcons.Camera) {
                        val dir = java.io.File(context.cacheDir, "camera").apply { mkdirs() }
                        val file = java.io.File(dir, "capture-${System.currentTimeMillis()}.jpg")
                        val uri = androidx.core.content.FileProvider.getUriForFile(context, "${context.packageName}.files", file)
                        capture = uri
                        camera.launch(uri)
                    }
                } else {
                    null
                },
                MenuAction("Files", ZIcons.Folder) { files.launch(arrayOf("image/*")) },
                pasted?.let { uri -> MenuAction("Paste image", ZIcons.Copy) { add(listOf(uri)) } },
            ),
        )
    }
}

/** Files matching the `@query` being typed, shown above the composer. */
/** A chat the `@` menu offers: a task, one of its agents, or a side chat. */
class MentionChat(
    val id: String,
    val title: String,
    /** An agent's task: its row reads `Task › Agent`. */
    val task: String?,
    val harness: String?,
    val kind: Kind,
    val indicator: uniffi.zeron_core.ChatIndicator,
    /** A task's agents' providers, for its marks. */
    val agents: List<String?> = emptyList(),
) {
    enum class Kind { Task, Agent, SideChat }

    val label: String get() = if (task != null) "$task › $title" else title
}

/** The `@` menu's tabs, like the desktop's. */
enum class MentionTab(val title: String) { All("All"), Tasks("Tasks"), SideChats("Side chats"), Files("Files") }

/**
 * The chats `chatId` started that the `@` menu offers, in its order: each task
 * followed by its agents (most recent task first), then side chats. In a
 * task's coordinator, its own agents.
 */
fun mentionChats(model: sh.zeron.android.core.AppModel, chatId: String): List<MentionChat> {
    fun title(row: uniffi.zeron_core.SessionRow) = if (row.hasTitle) row.title else "Untitled chat"
    fun agentsOf(task: uniffi.zeron_core.SessionRow) = model.sideChats(task.id).sortedBy { it.createdAtMs }.map {
        MentionChat(it.id, title(it), title(task), it.harness, MentionChat.Kind.Agent, it.indicator)
    }
    val me = model.row(chatId)
    if (me?.isTask == true) return agentsOf(me)
    val (tasks, sides) = model.sideChats(chatId).partition { it.isTask }
    return tasks.sortedByDescending { it.createdAtMs }.flatMap { task ->
        val agents = agentsOf(task)
        // A task reads as what needs someone first, as on its card.
        val indicator = when {
            task.indicator == uniffi.zeron_core.ChatIndicator.AWAITING_INPUT || agents.any { it.indicator == uniffi.zeron_core.ChatIndicator.AWAITING_INPUT } -> uniffi.zeron_core.ChatIndicator.AWAITING_INPUT
            task.indicator == uniffi.zeron_core.ChatIndicator.WORKING || agents.any { it.indicator == uniffi.zeron_core.ChatIndicator.WORKING } -> uniffi.zeron_core.ChatIndicator.WORKING
            else -> task.indicator
        }
        listOf(MentionChat(task.id, title(task), null, task.harness, MentionChat.Kind.Task, indicator, agents.map { it.harness })) + agents
    } + sides.sortedByDescending { it.lastActivityMs }.map { MentionChat(it.id, title(it), null, it.harness, MentionChat.Kind.SideChat, it.indicator) }
}

/** A squircle with a chat's provider mark. */
@Composable
private fun MentionMark(harness: String?, size: androidx.compose.ui.unit.Dp) {
    Box(
        Modifier.size(size).clip(RoundedCornerShape(size * 0.3f)).background(MaterialTheme.colorScheme.surfaceContainerHighest),
        contentAlignment = Alignment.Center,
    ) { sh.zeron.android.design.HarnessMark(harness, size * 0.6f, tint = MaterialTheme.colorScheme.onSurfaceVariant) }
}

/** How a mentioned chat is doing: working, waiting on a question, failed, or done. */
@OptIn(androidx.compose.material3.ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun MentionStatus(indicator: uniffi.zeron_core.ChatIndicator) {
    when (indicator) {
        uniffi.zeron_core.ChatIndicator.WORKING -> androidx.compose.material3.LoadingIndicator(Modifier.size(20.dp))
        uniffi.zeron_core.ChatIndicator.AWAITING_INPUT -> Box(Modifier.size(7.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
        uniffi.zeron_core.ChatIndicator.ERRORED -> Box(Modifier.size(7.dp).clip(CircleShape).background(MaterialTheme.colorScheme.error))
        else -> ZIcon(ZIcons.Check, null, Modifier.size(15.dp), tint = successColor())
    }
}

/**
 * The `@` menu over the composer: this chat's tasks (each followed by its
 * agents), side chats, and files, with tabs once there's a chat to mention.
 */
@Composable
fun MentionSuggestions(
    model: ComposerModel,
    search: suspend (String) -> List<FileMatch>,
    modifier: Modifier = Modifier,
    chats: List<MentionChat> = emptyList(),
) {
    val query = model.activeQuery()?.second
    var files by remember { mutableStateOf<List<FileMatch>>(emptyList()) }
    var tab by remember { mutableStateOf(MentionTab.All) }
    LaunchedEffect(query == null) { if (query == null) tab = MentionTab.All }
    LaunchedEffect(query) {
        if (query == null) {
            files = emptyList()
            return@LaunchedEffect
        }
        delay(120)
        files = runCatching { search(query) }.getOrDefault(emptyList()).take(5)
    }
    fun norm(s: String) = s.filterNot { it.isWhitespace() }.lowercase()
    val wanted = norm(query.orEmpty())
    val matching = chats.filter { wanted.isEmpty() || norm(it.label).contains(wanted) }
    val shownChats = when (tab) {
        MentionTab.All -> matching
        MentionTab.Tasks -> matching.filter { it.kind != MentionChat.Kind.SideChat }
        MentionTab.SideChats -> matching.filter { it.kind == MentionChat.Kind.SideChat }
        MentionTab.Files -> emptyList()
    }
    val shownFiles = if (tab == MentionTab.All || tab == MentionTab.Files) files else emptyList()
    val visible = query != null && (files.isNotEmpty() || matching.isNotEmpty())
    AnimatedVisibility(visible, enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically(), modifier = modifier) {
        Surface(
            shape = RoundedCornerShape(24.dp),
            color = composerContainer(),
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp),
        ) {
            Column(Modifier.padding(6.dp).heightIn(max = 360.dp).verticalScroll(rememberScrollState())) {
                if (chats.isNotEmpty()) {
                    Row(Modifier.horizontalScroll(rememberScrollState()).padding(horizontal = 6.dp, vertical = 4.dp), horizontalArrangement = Arrangement.spacedBy(6.dp)) {
                        MentionTab.entries.forEach { t ->
                            androidx.compose.material3.FilterChip(
                                selected = t == tab,
                                onClick = { tab = t },
                                label = { Text(t.title) },
                            )
                        }
                    }
                }
                if (shownChats.isEmpty() && shownFiles.isEmpty()) {
                    Text(
                        when (tab) {
                            MentionTab.Tasks -> if (wanted.isEmpty()) "No tasks in this chat" else "No matching tasks"
                            MentionTab.SideChats -> if (wanted.isEmpty()) "No side chats in this chat" else "No matching side chats"
                            else -> "No matching files"
                        },
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = 10.dp),
                    )
                }
                shownChats.forEach { chat ->
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(18.dp))
                            .clickable { model.insertChatMention(chat.id, chat.title) }
                            .padding(horizontal = 10.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        MentionMark(chat.harness, 26.dp)
                        Spacer(Modifier.width(10.dp))
                        Row(Modifier.weight(1f), verticalAlignment = Alignment.CenterVertically) {
                            chat.task?.let { task ->
                                Text(task, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                                Text(" › ", style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.outline)
                            }
                            Text(chat.title, style = MaterialTheme.typography.bodyLargeEmphasized, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f, fill = false))
                            val kind = when (chat.kind) {
                                MentionChat.Kind.Task -> "Task"
                                MentionChat.Kind.SideChat -> "Side chat"
                                MentionChat.Kind.Agent -> null
                            }
                            kind?.let {
                                Spacer(Modifier.width(8.dp))
                                Text(it, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                            }
                        }
                        if (chat.agents.isNotEmpty()) {
                            Spacer(Modifier.width(8.dp))
                            val shown = if (chat.agents.size > 3) 2 else chat.agents.size
                            Row(horizontalArrangement = Arrangement.spacedBy(3.dp)) {
                                chat.agents.take(shown).forEach { MentionMark(it, 18.dp) }
                                if (chat.agents.size > shown) {
                                    Box(Modifier.size(18.dp).clip(RoundedCornerShape(6.dp)).background(MaterialTheme.colorScheme.surfaceContainerHighest), contentAlignment = Alignment.Center) {
                                        Text("+${chat.agents.size - shown}", style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                                    }
                                }
                            }
                        }
                        Spacer(Modifier.width(10.dp))
                        Box(Modifier.size(20.dp), contentAlignment = Alignment.Center) { MentionStatus(chat.indicator) }
                    }
                }
                shownFiles.forEach { file ->
                    val name = file.path.trimEnd('/').substringAfterLast('/')
                    val dir = file.path.trimEnd('/').substringBeforeLast('/', "")
                    Row(
                        Modifier
                            .fillMaxWidth()
                            .clip(RoundedCornerShape(18.dp))
                            .clickable { model.insertMention(file) }
                            .padding(horizontal = 14.dp, vertical = 8.dp),
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        ZIcon(if (file.isDir) ZIcons.Folder else ZIcons.Text, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                        Spacer(Modifier.width(12.dp))
                        Column {
                            Text(name, style = MaterialTheme.typography.bodyLargeEmphasized, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            if (dir.isNotEmpty()) {
                                Text(dir, style = MaterialTheme.typography.bodySmall, fontFamily = GeistMono, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
                            }
                        }
                    }
                }
            }
        }
    }
}

/** An expressive choice menu: one group, inline section labels, a trailing check on the choice. */
@Composable
fun ChoiceMenu(expanded: Boolean, onDismiss: () -> Unit, sections: List<MenuSection>) {
    DropdownMenuPopup(expanded = expanded, onDismissRequest = onDismiss) {
        val groups = sections.filter { it.choices.isNotEmpty() }
        val all = groups.sumOf { it.choices.size }
        DropdownMenuGroup(shapes = MenuDefaults.groupShape(0, 1)) {
            var index = 0
            groups.forEach { section ->
                section.title?.let { title -> MenuDefaults.Label { Text(title, style = MaterialTheme.typography.labelMedium) } }
                section.choices.forEach { choice ->
                    DropdownMenuItem(
                        selected = choice.selected,
                        onClick = {
                            choice.onClick()
                            onDismiss()
                        },
                        text = { Text(choice.label) },
                        supportingText = choice.supporting?.let { { Text(it) } },
                        shapes = MenuDefaults.itemShape(index++, all),
                        leadingIcon = choice.leading,
                        selectedLeadingIcon = choice.leading,
                        trailingIcon = if (choice.selected) {
                            { ZIcon(ZIcons.Check, null, Modifier.size(20.dp)) }
                        } else {
                            null
                        },
                        colors = MenuDefaults.selectableItemColors(
                            selectedContainerColor = MaterialTheme.colorScheme.secondaryContainer,
                            selectedTextColor = MaterialTheme.colorScheme.onSecondaryContainer,
                            selectedLeadingIconColor = MaterialTheme.colorScheme.onSecondaryContainer,
                            selectedTrailingIconColor = MaterialTheme.colorScheme.onSecondaryContainer,
                        ),
                    )
                }
            }
        }
    }
}
