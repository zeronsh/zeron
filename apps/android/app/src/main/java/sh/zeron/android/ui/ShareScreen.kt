package sh.zeron.android.ui

import android.net.Uri
import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.FilledTonalButton
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.PhoneEngine
import sh.zeron.android.core.TransferCenter
import sh.zeron.android.core.Transfers
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.runtime.RuntimeState

/**
 * The share sheet's page: stage what was shared into the phone engine's
 * outbox, pick a device, send, and follow the transfer. Leaving before
 * sending drops the staged copy; after sending, the outbox batch goes once
 * the transfer ends (TransferCenter).
 */
@Composable
fun ShareScreen(model: AppModel, uris: List<Uri>, text: String?, onClose: () -> Unit, onOpenApp: (String?) -> Unit) {
    val center = model.transfers
    val onboarded by model.onboarded.collectAsState()
    val client by model.client.collectAsState()
    val engine by model.phone.state.collectAsState()
    val workspace by model.workspace.collectAsState()
    val transfers by center.list.collectAsState()
    val scope = rememberCoroutineScope()

    var staged by remember { mutableStateOf<Pair<String, List<TransferCenter.Staged>>?>(null) }
    var stageError by remember { mutableStateOf<String?>(null) }
    var sending by remember { mutableStateOf<String?>(null) }
    var sendError by remember { mutableStateOf<String?>(null) }
    var transferId by remember { mutableStateOf<String?>(null) }

    DisposableEffect(center) {
        val release = center.watch()
        onDispose { release() }
    }
    // Stage as soon as the guest exists: copying can take a while for big files.
    val guestReady = engine !is RuntimeState.NotInstalled && engine !is RuntimeState.Bootstrapping
    LaunchedEffect(guestReady) {
        if (!guestReady || staged != null) return@LaunchedEffect
        if (uris.isEmpty() && text.isNullOrEmpty()) {
            stageError = "Nothing to send."
            return@LaunchedEffect
        }
        runCatching { center.stage(uris, text) }
            .onSuccess { staged = it }
            .onFailure {
                stageError = if (it is SecurityException) "The app you shared from didn't give Zeron access to the file." else it.message ?: "Couldn't read what was shared."
            }
    }
    // Abandoned before sending: drop the staged copy.
    DisposableEffect(Unit) {
        onDispose { if (transferId == null) staged?.let { center.discard(it.first) } }
    }

    val recipients = remember(client, workspace?.devices) { center.recipients() }
    val transfer = transfers.firstOrNull { it.id == transferId }

    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        LazyColumn(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            item {
                Box(Modifier.padding(start = 12.dp, top = 8.dp)) {
                    TonalCircleButton(ZIcons.Close, "Close", onClick = onClose)
                }
            }
            val files = staged?.second
            item {
                ScreenHeader(
                    "Send with Zeron",
                    when {
                        files != null -> "${Transfers.files(files.size.toLong())} · ${Transfers.bytes(files.sumOf { it.size })}"
                        else -> "${Transfers.files(maxOf(uris.size, 1).toLong())} from another app"
                    },
                )
            }
            when {
                !model.phone.isSupportedAbi -> item {
                    Notice(
                        ZIcons.Phone,
                        "This phone can't run Zeron's engine",
                        "Zeron sends shared files from the engine on this phone, which isn't available for this device.",
                    ) {}
                }
                !onboarded -> item {
                    Notice(
                        ZIcons.Phone,
                        "Finish setting up Zeron",
                        "Zeron sends shared files from the engine on this phone. Open Zeron to sign in or continue without an account.",
                    ) { Button(onClick = { onOpenApp(null) }, shapes = ButtonDefaults.shapes()) { Text("Open Zeron") } }
                }
                stageError != null -> item { Notice(ZIcons.Warning, "Couldn't prepare the files", stageError!!) {} }
                client == null -> item {
                    when (engine) {
                        RuntimeState.Starting, is RuntimeState.Bootstrapping, is RuntimeState.Running ->
                            Notice(ZIcons.Terminal, "Connecting to the engine…", PhoneEngine.stateLabel(engine), busy = true) {}
                        else -> Notice(ZIcons.Terminal, "The engine isn't running", "Start it to send these files.") {
                            Button(onClick = { model.startEngine() }, shapes = ButtonDefaults.shapes()) { Text("Start engine") }
                        }
                    }
                }
                files == null -> item { Notice(ZIcons.Text, "Preparing…", "Copying the files into Zeron", busy = true) {} }
                transferId != null -> {
                    item {
                        if (transfer != null) {
                            TransferProgressCard(transfer, center, onCancel = { scope.launch { runCatching { center.cancel(transfer.id) } } })
                        } else {
                            Notice(ZIcons.Send, "Starting…", "Handing the files to the engine", busy = true) {}
                        }
                    }
                    item {
                        Row(Modifier.fillMaxWidth().padding(16.dp), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                            FilledTonalButton(onClick = { onOpenApp("transfers") }, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) { Text("All transfers") }
                            Button(onClick = onClose, modifier = Modifier.weight(1f), shapes = ButtonDefaults.shapes()) {
                                Text(if (transfer?.state?.live != false) "Send in background" else "Done")
                            }
                        }
                    }
                }
                else -> {
                    item { FileList(files) }
                    sectionTitle("Send to")
                    if (recipients.isEmpty()) {
                        item {
                            Notice(
                                ZIcons.Laptop,
                                "No device can receive yet",
                                "Your computers appear here while Zeron runs on them (version with file transfer).",
                            ) {}
                        }
                    }
                    item {
                        Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                            recipients.forEachIndexed { i, d ->
                                SegmentedListItem(
                                    onClick = {
                                        if (sending != null) return@SegmentedListItem
                                        sending = d.id
                                        sendError = null
                                        scope.launch {
                                            runCatching { center.send(d.id, files.map { it.guestPath }) }
                                                .onSuccess { transferId = it }
                                                .onFailure { sendError = it.userMessage() }
                                            sending = null
                                        }
                                    },
                                    enabled = d.online,
                                    shapes = segmentedShapes(i, recipients.size),
                                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                                    leadingContent = {
                                        IconTile(
                                            when (d.platform) {
                                                "linux" -> ZIcons.Server
                                                "android", "ios" -> ZIcons.Phone
                                                else -> ZIcons.Laptop
                                            },
                                        )
                                    },
                                    supportingContent = { Text(if (d.online) "Online" else "Offline") },
                                    trailingContent = {
                                        if (sending == d.id) {
                                            LoadingIndicator(Modifier.size(28.dp))
                                        } else {
                                            Box(Modifier.size(10.dp).clip(CircleShape).background(if (d.online) successColor() else MaterialTheme.colorScheme.outlineVariant))
                                        }
                                    },
                                ) { Text(d.name) }
                            }
                        }
                    }
                    if (sendError != null) {
                        item {
                            Text(
                                sendError!!,
                                style = MaterialTheme.typography.bodyMedium,
                                color = MaterialTheme.colorScheme.error,
                                modifier = Modifier.padding(horizontal = 28.dp, vertical = 12.dp),
                            )
                        }
                    }
                }
            }
        }
    }
}

@Composable
private fun FileList(files: List<TransferCenter.Staged>) {
    Surface(shape = RoundedCornerShape(28.dp), color = cardColor(), modifier = Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 4.dp)) {
        Column(Modifier.padding(vertical = 8.dp)) {
            for (f in files.take(20)) {
                Row(Modifier.padding(horizontal = 16.dp, vertical = 8.dp), verticalAlignment = Alignment.CenterVertically) {
                    ZIcon(ZIcons.Text, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.width(12.dp))
                    Text(f.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
                    Spacer(Modifier.width(8.dp))
                    Text(Transfers.bytes(f.size), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
            }
            if (files.size > 20) {
                Text("and ${files.size - 20} more", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, modifier = Modifier.padding(horizontal = 48.dp, vertical = 8.dp))
            }
        }
    }
}

@Composable
private fun Notice(icon: Int, title: String, detail: String, busy: Boolean = false, action: @Composable () -> Unit) {
    Column(Modifier.fillMaxWidth().padding(horizontal = 32.dp, vertical = 40.dp), horizontalAlignment = Alignment.CenterHorizontally) {
        Box(Modifier.size(88.dp), contentAlignment = Alignment.Center) {
            if (busy) {
                LoadingIndicator(Modifier.size(72.dp))
            } else {
                Box(
                    Modifier.size(88.dp).clip(MaterialShapes.Cookie9Sided.toShape()).background(MaterialTheme.colorScheme.secondaryContainer),
                    contentAlignment = Alignment.Center,
                ) { ZIcon(icon, null, Modifier.size(40.dp), tint = MaterialTheme.colorScheme.onSecondaryContainer) }
            }
        }
        Spacer(Modifier.height(16.dp))
        Text(title, style = MaterialTheme.typography.titleLarge, textAlign = TextAlign.Center)
        Spacer(Modifier.height(6.dp))
        Text(detail, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant, textAlign = TextAlign.Center)
        Spacer(Modifier.heightIn(min = 16.dp))
        action()
    }
}
