package sh.zeron.android.ui

import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.itemsIndexed
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Scaffold
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.material3.TopAppBar
import androidx.compose.material3.TopAppBarDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.FolderListing

/**
 * Create a project (a device + folder pair): pick a host, browse its folders
 * over the device relay (git repos badged), and use the current folder.
 * [onCreated] gets the new project's id (the new-session page selects it).
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun NewProjectScreen(model: AppModel, onDismiss: () -> Unit, onCreated: (String) -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        DialogSystemBars()
        val workspace by model.workspace.collectAsState()
        val client by model.client.collectAsState()
        val scope = rememberCoroutineScope()
        val hosts = workspace?.devices.orEmpty().filter { it.isExecutionHost }
        var device by remember { mutableStateOf<DeviceView?>(null) }
        LaunchedEffect(hosts.isNotEmpty()) { if (device == null) device = hosts.firstOrNull { it.online } ?: hosts.firstOrNull() }
        var listing by remember { mutableStateOf<FolderListing?>(null) }
        // The device `listing` came from: the path must be one this device listed.
        var listingDevice by remember { mutableStateOf<String?>(null) }
        var path by remember { mutableStateOf<String?>(null) }
        var loading by remember { mutableStateOf(false) }
        var status by remember { mutableStateOf<String?>(null) }
        var creating by remember { mutableStateOf(false) }
        var generation by remember { mutableIntStateOf(0) }
        var hostMenu by remember { mutableStateOf(false) }

        fun load(at: String?) {
            val d = device ?: return
            val c = client ?: return
            loading = true
            status = "Loading…"
            val mine = ++generation
            scope.launch {
                val result = runCatching { c.listFolders(d.id, at) }.getOrNull()
                // A newer load (another device or folder) superseded this one.
                if (mine != generation) return@launch
                loading = false
                if (result == null) {
                    status = if (d.online) "Couldn't read that folder." else "${d.name} is offline."
                    return@launch
                }
                listing = result
                listingDevice = d.id
                path = result.path
                status = result.path
            }
        }
        LaunchedEffect(device?.id) { if (device != null) load(null) }

        fun create() {
            val d = device ?: return
            val p = path ?: return
            if (listingDevice != d.id) return
            val git = listing?.entries?.any { it.name == ".git" } == true
            creating = true
            scope.launch {
                val id = runCatching { client?.createProject(d.id, p, git) }.getOrNull()
                creating = false
                if (id != null) {
                    onCreated(id)
                    onDismiss()
                } else {
                    status = "Couldn't create the project on ${d.name}."
                }
            }
        }

        val current = listing
        val name = current?.path?.trimEnd('/')?.substringAfterLast('/').orEmpty()
        Scaffold(
            topBar = {
                TopAppBar(
                    title = { Text(name.ifEmpty { "New project" }, maxLines = 1, overflow = TextOverflow.Ellipsis) },
                    navigationIcon = { TonalCircleButton(ZIcons.Close, "Close", onClick = onDismiss, modifier = Modifier.padding(start = 8.dp), size = 44.dp) },
                    actions = {
                        Box(Modifier.padding(end = 8.dp)) {
                            ContextChip(
                                device?.name ?: "No hosts",
                                leading = { ZIcon(ZIcons.Laptop, null, Modifier.size(16.dp)) },
                                onClick = { hostMenu = true },
                            ) {
                                ChoiceMenu(hostMenu, { hostMenu = false }, listOf(MenuSection("Device", hosts.map { h ->
                                    MenuChoice(h.name, h.id == device?.id, if (h.online) "Online" else "Offline") { device = h }
                                })))
                            }
                        }
                    },
                    colors = TopAppBarDefaults.topAppBarColors(containerColor = MaterialTheme.colorScheme.background),
                )
            },
            containerColor = MaterialTheme.colorScheme.background,
        ) { padding ->
            Box(Modifier.fillMaxSize().padding(padding)) {
                if (hosts.isEmpty()) {
                    Text(
                        "No desktop devices yet — open Zeron on a computer to add one.",
                        textAlign = TextAlign.Center,
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.align(Alignment.Center).padding(32.dp),
                    )
                    return@Box
                }
                val parent = current?.path?.let(::parentOf)
                val folders = current?.entries.orEmpty().filter { it.isDir }
                val count = folders.size + if (parent != null) 1 else 0
                LazyColumn(contentPadding = PaddingValues(start = 16.dp, end = 16.dp, top = 8.dp, bottom = 160.dp)) {
                    if (parent != null) {
                        item("..") {
                            FolderRow(0, count, "Parent folder", ZIcons.Back, false, enabled = !loading) { load(parent) }
                        }
                    }
                    itemsIndexed(folders, key = { _, e -> e.name }) { i, entry ->
                        val index = i + if (parent != null) 1 else 0
                        FolderRow(index, count, entry.name, if (entry.isRepo) ZIcons.Branch else ZIcons.Folder, entry.isRepo, enabled = !loading) {
                            load(current!!.path.trimEnd('/') + "/" + entry.name)
                        }
                    }
                }
                if (loading && current == null) LoadingIndicator(Modifier.align(Alignment.Center))
                Column(
                    Modifier.align(Alignment.BottomCenter).fillMaxWidth().navigationBarsPadding().padding(horizontal = 20.dp, vertical = 12.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.spacedBy(8.dp),
                ) {
                    status?.let {
                        Text(
                            it,
                            style = MaterialTheme.typography.bodySmall,
                            fontFamily = if (it == current?.path) GeistMono else null,
                            color = MaterialTheme.colorScheme.onSurfaceVariant,
                            maxLines = 2,
                            overflow = TextOverflow.Ellipsis,
                            textAlign = TextAlign.Center,
                        )
                    }
                    Button(
                        onClick = ::create,
                        enabled = !loading && !creating && current != null && listingDevice == device?.id,
                        shapes = ButtonDefaults.shapes(),
                        contentPadding = PaddingValues(horizontal = 24.dp, vertical = 16.dp),
                        modifier = Modifier.fillMaxWidth().testTag("use-folder"),
                    ) {
                        if (creating) LoadingIndicator(Modifier.size(20.dp)) else Text("Use “${name.ifEmpty { current?.path ?: "folder" }}”")
                    }
                }
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun FolderRow(index: Int, count: Int, title: String, icon: Int, repo: Boolean, enabled: Boolean, onClick: () -> Unit) {
    SegmentedListItem(
        onClick = onClick,
        enabled = enabled,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = {
            ZIcon(icon, null, Modifier.size(22.dp), tint = if (repo) MaterialTheme.colorScheme.primary else MaterialTheme.colorScheme.onSurfaceVariant)
        },
        trailingContent = if (icon != ZIcons.Back) {
            { ZIcon(ZIcons.ChevronRight, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.outline) }
        } else {
            null
        },
        modifier = Modifier.padding(bottom = ListItemDefaults.SegmentedGap),
    ) { Text(title, style = MaterialTheme.typography.titleMedium, maxLines = 1, overflow = TextOverflow.Ellipsis) }
}

/** The folder above [path], or null at the root. */
private fun parentOf(path: String): String? {
    val trimmed = if (path.endsWith("/") && path.length > 1) path.dropLast(1) else path
    if (trimmed == "/" || !trimmed.contains('/')) return null
    return trimmed.substringBeforeLast('/').ifEmpty { "/" }
}
