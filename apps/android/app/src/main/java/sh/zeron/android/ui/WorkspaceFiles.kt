@file:OptIn(androidx.compose.foundation.ExperimentalFoundationApi::class)

package sh.zeron.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.boundsInRoot
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.CancellationException
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AnchoredMenu
import sh.zeron.android.design.AssetIcon
import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.design.Glyph
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.MenuEntry
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface
import uniffi.zeron_core.FolderEntry
import uniffi.zeron_core.FolderListing

private sealed interface FileTreeRow {
    data class Node(val path: String, val name: String, val isDir: Boolean, val isRepo: Boolean, val depth: Int) : FileTreeRow

    /** A dim hint row under a dir: empty folder or a failed load (tap retries). */
    data class Note(val depth: Int, val text: String, val retryDir: String?) : FileTreeRow
}

/**
 * The session's workspace file tree (desktop FileTree): directories expand in
 * place, files preview on tap, long-press on a file or folder opens the row's
 * action menu (preview / @-mention / copy path). Rooted at the session's
 * working directory, falling back to the computer's home.
 */
@Composable
internal fun WorkspaceFilesScreen(
    model: ZeronModel,
    deviceId: String,
    cwd: String?,
    onClose: () -> Unit,
    onPreview: (path: String) -> Unit,
    onMention: (path: String, isDir: Boolean) -> Unit,
) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    val haptics = LocalHapticFeedback.current
    val client = model.client ?: return
    var root by remember { mutableStateOf(PcFileRefs.usableCwd(cwd)) }
    val listings = remember { mutableStateMapOf<String, FolderListing>() }
    var loading by remember { mutableStateOf(setOf<String>()) }
    val failed = remember { mutableStateMapOf<String, String>() }
    var expanded by remember { mutableStateOf(setOf<String>()) }
    var tick by remember { mutableIntStateOf(0) }
    var menuTarget by remember { mutableStateOf<Pair<Rect, FileTreeRow.Node>?>(null) }
    BackHandler(onBack = onClose)
    LaunchedEffect(deviceId, expanded, tick) {
        val pending = LinkedHashSet<String>()
        root?.let { pending += it }
        pending += expanded
        for (p in pending) {
            if (listings.containsKey(p) || loading.contains(p) || failed.containsKey(p)) continue
            loading += p
            try {
                val next = client.listFolders(deviceId, p)
                listings[p] = next
                if (root == null) root = next.path
            } catch (t: Throwable) {
                if (t is CancellationException) throw t
                failed[p] = t.message ?: context.getString(R.string.read_folder_failed)
            }
            loading -= p
        }
    }
    val emptyText = stringResource(R.string.no_files)
    val retryText = stringResource(R.string.folder_load_retry)
    val rows = ArrayList<FileTreeRow>()
    fun emit(dir: String, depth: Int): Unit {
        val listing = listings[dir]
        when {
            listing == null && failed.containsKey(dir) -> rows += FileTreeRow.Note(depth, retryText, dir)
            listing == null -> Unit
            listing.entries.isEmpty() -> rows += FileTreeRow.Note(depth, emptyText, null)
            else -> for (e in listing.entries.sortedWith(compareByDescending<FolderEntry> { it.isDir }.thenBy(String.CASE_INSENSITIVE_ORDER) { it.name })) {
                val p = HostPaths.join(dir, e.name)
                rows += FileTreeRow.Node(p, e.name, e.isDir, e.isRepo, depth)
                if (e.isDir && p in expanded) emit(p, depth + 1)
            }
        }
    }
    root?.let { emit(it, 0) }
    Column(Modifier.fillMaxSize().background(colors.background).consumeBlankTaps().statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(colors, onClick = onClose)
            Text(
                root?.let { HostPaths.name(it) } ?: stringResource(R.string.browse_files),
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 17.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                textAlign = TextAlign.Center,
            )
            Box(
                Modifier.size(44.dp).glassSurface(colors, 22.dp).combinedClickable(onClick = {
                    listings.clear()
                    failed.clear()
                    tick++
                }),
                contentAlignment = Alignment.Center,
            ) { Glyph(Glyphs.Refresh, 17.dp, colors.text) }
        }
        Box(Modifier.fillMaxSize()) {
            LazyColumn(Modifier.fillMaxSize()) {
                items(rows.size, key = { i -> rows[i].let { if (it is FileTreeRow.Node) it.path else "note$i" } }) { i ->
                    when (val r = rows[i]) {
                        is FileTreeRow.Note -> Text(
                            r.text,
                            color = if (r.retryDir != null) colors.accent else colors.secondary,
                            fontFamily = ZeronType.Sans,
                            fontSize = 13.sp,
                            modifier = Modifier
                                .fillMaxWidth()
                                .then(if (r.retryDir != null) Modifier.combinedClickable(onClick = {
                                    failed.remove(r.retryDir)
                                    tick++
                                }) else Modifier)
                                .padding(start = (46 + r.depth * 16).dp, end = 16.dp, top = 9.dp, bottom = 9.dp),
                        )
                        is FileTreeRow.Node -> {
                            var bounds by remember { mutableStateOf(Rect.Zero) }
                            Row(
                                Modifier
                                    .fillMaxWidth()
                                    .onGloballyPositioned { bounds = it.boundsInRoot() }
                                    .combinedClickable(
                                        onLongClick = {
                                            haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                                            menuTarget = bounds to r
                                        },
                                        onClick = {
                                            if (r.isDir) {
                                                expanded = if (r.path in expanded) expanded - r.path else expanded + r.path
                                            } else {
                                                onPreview(r.path)
                                            }
                                        },
                                    )
                                    .padding(start = (12 + r.depth * 16).dp, end = 16.dp)
                                    .height(38.dp),
                                verticalAlignment = Alignment.CenterVertically,
                            ) {
                                Box(Modifier.size(14.dp), contentAlignment = Alignment.Center) {
                                    if (r.isDir) {
                                        Text(
                                            if (r.path in expanded) "▾" else "▸",
                                            color = colors.secondary,
                                            fontSize = 12.sp,
                                        )
                                    }
                                }
                                Spacer(Modifier.width(6.dp))
                                if (r.isDir) {
                                    Glyph(Glyphs.Folder, 17.dp, if (r.isRepo) colors.accent else colors.secondary)
                                } else {
                                    AssetIcon(fileIconName(r.name), 17.dp, colors.secondary)
                                }
                                Spacer(Modifier.width(8.dp))
                                Text(
                                    r.name,
                                    color = colors.text,
                                    fontFamily = ZeronType.Sans,
                                    fontSize = 15.sp,
                                    maxLines = 1,
                                    overflow = TextOverflow.Ellipsis,
                                    modifier = Modifier.weight(1f),
                                )
                                if (r.isRepo) {
                                    Text("git", color = colors.accent, fontFamily = ZeronType.Sans, fontSize = 11.sp)
                                } else if (loading.contains(r.path)) {
                                    Text("…", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
                                }
                            }
                        }
                    }
                }
            }
            menuTarget?.let { (anchor, node) ->
                val rel = root?.let { HostPaths.relativeTo(it, node.path) }
                val screenMid = with(LocalDensity.current) { (LocalConfiguration.current.screenHeightDp.dp * 0.55f).toPx() }
                AnchoredMenu(
                    colors,
                    anchor,
                    title = node.name,
                    entries = buildList {
                        if (!node.isDir) {
                            add(MenuEntry(stringResource(R.string.file_preview_open), icon = { c -> AssetIcon(fileIconName(node.name), 16.dp, c) }) { onPreview(node.path) })
                        }
                        add(MenuEntry(stringResource(R.string.mention), icon = { c -> AssetIcon("fileicon-files-link", 16.dp, c) }) { onMention(node.path, node.isDir) })
                        add(MenuEntry(stringResource(R.string.copy_path), icon = { c -> Glyph(Glyphs.Copy, 16.dp, c) }) {
                            val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                            cm.setPrimaryClip(android.content.ClipData.newPlainText("path", node.path))
                            model.showToast(context.getString(R.string.copied))
                        })
                        if (rel != null) {
                            add(MenuEntry(stringResource(R.string.copy_rel_path), icon = { c -> Glyph(Glyphs.Copy, 16.dp, c) }) {
                                val cm = context.getSystemService(android.content.ClipboardManager::class.java)
                                cm.setPrimaryClip(android.content.ClipData.newPlainText("path", rel))
                                model.showToast(context.getString(R.string.copied))
                            })
                        }
                    },
                    above = anchor.center.y > screenMid,
                ) { menuTarget = null }
            }
        }
    }
}
