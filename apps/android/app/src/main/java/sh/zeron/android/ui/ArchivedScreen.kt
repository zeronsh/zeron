package sh.zeron.android.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.SwipeToDismissBox
import androidx.compose.material3.SwipeToDismissBoxValue
import androidx.compose.material3.Text
import androidx.compose.material3.rememberSwipeToDismissBoxState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.SessionRow

/** Archived sessions: open, unarchive (swipe or long press), rename. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ArchivedScreen(model: AppModel, onBack: () -> Unit, onOpen: (String) -> Unit) {
    val workspace by model.workspace.collectAsState()
    val rows = workspace?.archived.orEmpty()
    val list = rememberLazyListState()
    Box(Modifier.fillMaxSize()) {
        LazyColumn(state = list, contentPadding = PaddingValues(bottom = 32.dp), modifier = Modifier.fillMaxSize()) {
            item("header") {
                Column(Modifier.statusBarsPadding()) {
                    Row(Modifier.padding(start = 12.dp, top = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                        TonalCircleButton(ZIcons.Back, "Back", onClick = onBack)
                    }
                    ScreenHeader("Archived", if (rows.isEmpty()) null else "${rows.size} sessions")
                }
            }
            if (rows.isEmpty()) {
                item("empty") {
                    Column(Modifier.fillMaxWidth().padding(48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                        ZIcon(ZIcons.Archive, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.outline)
                        Spacer(Modifier.height(12.dp))
                        Text("Nothing archived", style = MaterialTheme.typography.titleLarge)
                    }
                }
            }
            itemsIndexed(rows, key = { _, r -> r.id }) { i, row ->
                Box(Modifier.animateItem().padding(horizontal = 16.dp).padding(bottom = ListItemDefaults.SegmentedGap)) {
                    ArchivedRow(model, row, i, rows.size, onOpen)
                }
            }
        }
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
}

@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun ArchivedRow(model: AppModel, row: SessionRow, index: Int, count: Int, onOpen: (String) -> Unit) {
    val state = rememberSwipeToDismissBoxState()
    val shape = segmentedShapes(index, count).shape
    var menu by remember { mutableStateOf(false) }
    var renaming by remember { mutableStateOf(false) }
    val haptics = LocalHapticFeedback.current
    SwipeToDismissBox(
        state = state,
        enableDismissFromStartToEnd = false,
        onDismiss = { if (it == SwipeToDismissBoxValue.EndToStart) model.unarchive(row.id) },
        backgroundContent = {
            Surface(Modifier.fillMaxSize(), shape = shape, color = MaterialTheme.colorScheme.secondaryContainer) {
                Box(Modifier.padding(end = 28.dp), contentAlignment = Alignment.CenterEnd) {
                    ZIcon(ZIcons.Unarchive, "Unarchive", tint = MaterialTheme.colorScheme.onSecondaryContainer)
                }
            }
        },
    ) {
        Box {
            SegmentedListItem(
                onClick = { onOpen(row.id) },
                onLongClick = {
                    haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                    menu = true
                },
                shapes = segmentedShapes(index, count),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { HarnessTile(row) },
                supportingContent = {
                    Text(row.project?.name ?: row.deviceName ?: "No project", maxLines = 1, overflow = TextOverflow.Ellipsis)
                },
                trailingContent = { Text(row.timeLabel, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.outline) },
            ) { Text(row.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
            ActionMenu(
                menu,
                { menu = false },
                listOf(
                    MenuAction("Unarchive", ZIcons.Unarchive) { model.unarchive(row.id) },
                    MenuAction("Rename…", ZIcons.Rename) { renaming = true },
                ),
            )
        }
    }
    if (renaming) RenameDialog(row.title, onDismiss = { renaming = false }) { model.rename(row.id, it) }
}
