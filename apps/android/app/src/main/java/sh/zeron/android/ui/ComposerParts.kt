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
            AnimatedVisibility(model.attachments.isNotEmpty() && card) { StagedThumbnails(model) }
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

/** Staged attachments above the prompt, each removable: thumbnails for pictures, a name tile for the rest. */
@Composable
private fun StagedThumbnails(model: ComposerModel) {
    Row(
        Modifier.horizontalScroll(rememberScrollState()).padding(start = 12.dp, end = 12.dp, top = 12.dp),
        horizontalArrangement = Arrangement.spacedBy(8.dp),
    ) {
        for (item in model.attachments) {
            val thumb = item.thumb
            Box(Modifier.then(if (thumb != null) Modifier.size(72.dp) else Modifier.size(width = 120.dp, height = 72.dp))) {
                if (thumb != null) {
                    Image(thumb, "Attached image", Modifier.fillMaxSize().clip(RoundedCornerShape(16.dp)), contentScale = ContentScale.Crop)
                } else {
                    Surface(
                        shape = RoundedCornerShape(16.dp),
                        color = MaterialTheme.colorScheme.surfaceContainerHighest,
                        modifier = Modifier.fillMaxSize(),
                    ) {
                        Column(Modifier.padding(start = 10.dp, end = 10.dp, top = 10.dp), verticalArrangement = Arrangement.spacedBy(4.dp)) {
                            ZIcon(ZIcons.Text, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                            Text(item.name, style = MaterialTheme.typography.labelSmall, maxLines = 2, overflow = TextOverflow.Ellipsis)
                        }
                    }
                }
                Surface(
                    onClick = { model.attachments.remove(item) },
                    shape = CircleShape,
                    color = MaterialTheme.colorScheme.inverseSurface.copy(alpha = 0.85f),
                    contentColor = MaterialTheme.colorScheme.inverseOnSurface,
                    modifier = Modifier.align(Alignment.TopEnd).padding(4.dp).size(22.dp),
                ) {
                    Box(contentAlignment = Alignment.Center) { ZIcon(ZIcons.Close, "Remove ${item.name}", Modifier.size(14.dp)) }
                }
            }
        }
    }
}

/** [+] → Photos / Take photo / Files (any type) / Paste image, staged into the composer. */
@Composable
private fun AttachButton(model: ComposerModel, enabled: Boolean) {
    val context = LocalContext.current
    val scope = rememberCoroutineScope()
    var open by remember { mutableStateOf(false) }
    fun add(uris: List<Uri>) {
        scope.launch {
            var skipped = 0
            var problem: String? = null
            for (uri in uris) {
                if (model.attachments.size >= Staging.MAX_ATTACHMENTS) {
                    skipped++
                    continue
                }
                try {
                    val item = Staging.stage(context, uri)
                    if (model.attachments.sumOf { it.bytes.size } + item.bytes.size > Staging.MAX_TOTAL_BYTES) {
                        problem = "Attachments are limited to ${Staging.MAX_TOTAL_BYTES / 1024 / 1024} MB in total"
                    } else {
                        model.attachments.add(item)
                    }
                } catch (e: StagingException) {
                    problem = e.message
                }
            }
            if (skipped > 0) problem = "Up to ${Staging.MAX_ATTACHMENTS} attachments per message"
            problem?.let { android.widget.Toast.makeText(context, it, android.widget.Toast.LENGTH_LONG).show() }
        }
    }
    val photos = rememberLauncherForActivityResult(ActivityResultContracts.PickMultipleVisualMedia(Staging.MAX_ATTACHMENTS)) { add(it) }
    val files = rememberLauncherForActivityResult(ActivityResultContracts.OpenMultipleDocuments()) { add(it) }
    // The camera writes into a cache file the FileProvider shares with it.
    var capture by remember { mutableStateOf<Uri?>(null) }
    val camera = rememberLauncherForActivityResult(ActivityResultContracts.TakePicture()) { saved ->
        val uri = capture
        if (saved && uri != null) add(listOf(uri))
    }
    val hasCamera = remember { context.packageManager.hasSystemFeature(android.content.pm.PackageManager.FEATURE_CAMERA_ANY) }
    Box {
        IconButton(onClick = { open = true }, enabled = enabled && model.attachments.size < Staging.MAX_ATTACHMENTS) {
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
                MenuAction("Files", ZIcons.Folder) { files.launch(arrayOf("*/*")) },
                pasted?.let { uri -> MenuAction("Paste image", ZIcons.Copy) { add(listOf(uri)) } },
            ),
        )
    }
}

/** Files matching the `@query` being typed, shown above the composer. */
@Composable
fun MentionSuggestions(model: ComposerModel, search: suspend (String) -> List<FileMatch>, modifier: Modifier = Modifier) {
    val query = model.activeQuery()?.second
    var files by remember { mutableStateOf<List<FileMatch>>(emptyList()) }
    LaunchedEffect(query) {
        if (query == null) {
            files = emptyList()
            return@LaunchedEffect
        }
        delay(120)
        files = runCatching { search(query) }.getOrDefault(emptyList()).take(5)
    }
    AnimatedVisibility(query != null && files.isNotEmpty(), enter = fadeIn() + expandVertically(), exit = fadeOut() + shrinkVertically(), modifier = modifier) {
        Surface(
            shape = RoundedCornerShape(24.dp),
            color = composerContainer(),
            border = BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
            modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp),
        ) {
            Column(Modifier.padding(6.dp)) {
                files.forEach { file ->
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
