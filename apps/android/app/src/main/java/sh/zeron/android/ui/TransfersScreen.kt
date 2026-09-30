package sh.zeron.android.ui

import android.app.DownloadManager
import android.content.Intent
import androidx.compose.animation.animateContentSize
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedButton
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.SnackbarHost
import androidx.compose.material3.SnackbarHostState
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.TransferCenter
import sh.zeron.android.core.Transfers
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons

/**
 * Settings → Transfers: files between this phone's engine and your other
 * devices (docs/file-transfer.md § Clients). Live rows show progress,
 * throughput and the transport, with Cancel; an incoming transfer that
 * waits for you gets Accept / Decline; received items open from their copy
 * in Download/Zeron.
 */
@Composable
fun TransfersScreen(model: AppModel, onBack: () -> Unit) {
    val center = model.transfers
    val list by center.list.collectAsState()
    val settings by center.settings.collectAsState()
    val exports by center.exports.collectAsState()
    val error by center.error.collectAsState()
    val context = LocalContext.current
    val snackbar = remember { SnackbarHostState() }
    val scope = rememberCoroutineScope()
    DisposableEffect(center) {
        val release = center.watch()
        onDispose { release() }
    }

    fun attempt(what: String, body: suspend () -> Unit) {
        scope.launch {
            runCatching { body() }.onFailure { snackbar.showSnackbar("Couldn't $what: ${it.userMessage()}") }
        }
    }

    val live = list.filter { it.state.live }
    val done = list.filter { it.state.terminal }
    SubPage(
        title = "Transfers",
        subtitle = error ?: "Files between this phone and your devices",
        onBack = onBack,
        overlay = { SnackbarHost(snackbar, Modifier.align(Alignment.BottomCenter).navigationBarsPadding()) },
    ) {
        sectionTitle("Receiving")
        item {
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                val ask = settings?.requireConfirmation == true
                SegmentedListItem(
                    onClick = { attempt("change the setting") { center.setRequireConfirmation(!ask) } },
                    shapes = segmentedShapes(0, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Bell) },
                    supportingContent = { Text("Files from your other devices wait until you accept them") },
                    trailingContent = {
                        Switch(ask, { on -> attempt("change the setting") { center.setRequireConfirmation(on) } }, enabled = settings != null)
                    },
                ) { Text("Ask before accepting") }
                SegmentedListItem(
                    onClick = { runCatching { context.startActivity(Intent(DownloadManager.ACTION_VIEW_DOWNLOADS).addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) } },
                    shapes = segmentedShapes(1, 2),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Folder) },
                    supportingContent = { Text("Received files are copied to Download/Zeron") },
                    trailingContent = { ZIcon(ZIcons.ChevronRight, null, Modifier.size(20.dp)) },
                ) { Text("Downloads") }
            }
        }
        if (list.isEmpty()) {
            item {
                Column(Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 48.dp), horizontalAlignment = Alignment.CenterHorizontally) {
                    Box(
                        Modifier.size(88.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.secondaryContainer),
                        contentAlignment = Alignment.Center,
                    ) { ZIcon(ZIcons.ArrowDown, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSecondaryContainer) }
                    Spacer(Modifier.height(16.dp))
                    Text("No transfers yet", style = MaterialTheme.typography.titleLarge)
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "Send files here from Zeron on a computer, or share them from any app with Zeron.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                    )
                }
            }
            return@SubPage
        }
        if (live.isNotEmpty()) {
            sectionTitle("In progress")
            for (t in live) {
                item(t.id) {
                    TransferCard(
                        t,
                        exported = null,
                        onCancel = { attempt("cancel") { center.cancel(t.id) } },
                        onAccept = { attempt("accept") { center.accept(t.id) } },
                        onDecline = { attempt("decline") { center.decline(t.id) } },
                        onOpen = {},
                        onClear = {},
                    )
                }
            }
        }
        if (done.isNotEmpty()) {
            item {
                Row(Modifier.fillMaxWidth().padding(start = 28.dp, end = 16.dp, top = 16.dp), verticalAlignment = Alignment.CenterVertically) {
                    Text("Earlier", style = MaterialTheme.typography.titleSmallEmphasized, color = MaterialTheme.colorScheme.primary, modifier = Modifier.weight(1f))
                    TextButton(onClick = { attempt("clear") { center.clear() } }) { Text("Clear") }
                }
            }
            for (t in done) {
                item(t.id) {
                    TransferCard(
                        t,
                        exported = exports[t.id],
                        onCancel = {},
                        onAccept = {},
                        onDecline = {},
                        onOpen = { item ->
                            if (!center.open(context, t, item)) scope.launch { snackbar.showSnackbar("Nothing on this phone can open ${item.name}.") }
                        },
                        onClear = { attempt("clear") { center.clear(t.id) } },
                    )
                }
            }
        }
    }
}

@Composable
private fun TransferCard(
    t: Transfers.Transfer,
    exported: Map<String, String>?,
    onCancel: () -> Unit,
    onAccept: () -> Unit,
    onDecline: () -> Unit,
    onOpen: (Transfers.Item) -> Unit,
    onClear: (() -> Unit)?,
) {
    val liveTone = t.state.live
    Surface(
        shape = RoundedCornerShape(28.dp),
        color = cardColor(),
        modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp),
    ) {
        Column(Modifier.animateContentSize().padding(16.dp)) {
            Row(verticalAlignment = Alignment.CenterVertically) {
                val (container, content) = when {
                    t.state == Transfers.State.Failed -> MaterialTheme.colorScheme.errorContainer to MaterialTheme.colorScheme.onErrorContainer
                    liveTone -> MaterialTheme.colorScheme.primaryContainer to MaterialTheme.colorScheme.onPrimaryContainer
                    else -> MaterialTheme.colorScheme.secondaryContainer to MaterialTheme.colorScheme.onSecondaryContainer
                }
                IconTile(if (t.incoming) ZIcons.ArrowDown else ZIcons.Send, container, content)
                Spacer(Modifier.width(14.dp))
                Column(Modifier.weight(1f)) {
                    Text(t.title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis)
                    Text(
                        "${if (t.incoming) "From" else "To"} ${t.peerDeviceName} · ${Transfers.stateLabel(t)}",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
                if (liveTone) {
                    TonalCircleButton(ZIcons.Close, "Cancel", onClick = onCancel, size = 40.dp)
                } else if (onClear != null) {
                    TonalCircleButton(ZIcons.Close, "Remove from list", onClick = onClear, size = 40.dp, container = androidx.compose.ui.graphics.Color.Transparent)
                }
            }
            if (liveTone) {
                Spacer(Modifier.height(14.dp))
                if (t.state == Transfers.State.Transferring || t.state == Transfers.State.Verifying) {
                    LinearWavyProgressIndicator(progress = { t.fraction }, modifier = Modifier.fillMaxWidth())
                } else if (t.state != Transfers.State.AwaitingAcceptance) {
                    LinearWavyProgressIndicator(Modifier.fillMaxWidth())
                }
            }
            Spacer(Modifier.height(8.dp))
            Text(
                Transfers.detail(t),
                style = MaterialTheme.typography.bodySmall,
                color = if (t.state == Transfers.State.Failed) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 3,
                overflow = TextOverflow.Ellipsis,
            )
            if (t.incoming && t.state == Transfers.State.AwaitingAcceptance) {
                Spacer(Modifier.height(12.dp))
                Row(horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                    Button(onClick = onAccept, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                        ZIcon(ZIcons.Check, null, Modifier.size(ButtonDefaults.IconSize))
                        Spacer(Modifier.size(ButtonDefaults.IconSpacing))
                        Text("Accept")
                    }
                    OutlinedButton(onClick = onDecline, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) { Text("Decline") }
                }
            }
            if (t.incoming && t.state == Transfers.State.Completed) {
                Spacer(Modifier.height(10.dp))
                if (exported == null) {
                    Text("Copying to Downloads…", style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
                } else {
                    Column(verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                        t.items.forEachIndexed { i, item ->
                            SegmentedListItem(
                                onClick = { onOpen(item) },
                                shapes = segmentedShapes(i, t.items.size),
                                colors = ListItemDefaults.segmentedColors(containerColor = MaterialTheme.colorScheme.surfaceContainerHigh),
                                leadingContent = { ZIcon(if (item.kind == Transfers.Kind.Folder) ZIcons.Folder else ZIcons.Text, null, Modifier.size(22.dp)) },
                                supportingContent = {
                                    Text(
                                        if (item.kind == Transfers.Kind.Folder) "${Transfers.files(item.fileCount)} · ${Transfers.bytes(item.size)}" else Transfers.bytes(item.size),
                                    )
                                },
                                trailingContent = {
                                    FilledTonalButton(onClick = { onOpen(item) }, shapes = ButtonDefaults.shapes()) {
                                        Text(if (item.kind == Transfers.Kind.Folder) "Show" else "Open")
                                    }
                                },
                            ) { Text(item.name, maxLines = 1, overflow = TextOverflow.Ellipsis) }
                        }
                    }
                }
            }
        }
    }
}

/** A compact live row for the share sheet: progress of one transfer. */
@Composable
fun TransferProgressCard(t: Transfers.Transfer, center: TransferCenter, onCancel: () -> Unit) {
    TransferCard(t, center.exports.collectAsState().value[t.id], onCancel, {}, {}, {}, null)
}
