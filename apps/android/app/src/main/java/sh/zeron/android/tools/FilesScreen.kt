package sh.zeron.android.tools

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.imePadding
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.material3.HorizontalFloatingToolbar
import androidx.compose.material3.FloatingToolbarDefaults
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateListOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.Job
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.userMessage
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.ui.ActionMenu
import sh.zeron.android.ui.MenuAction
import uniffi.zeron_core.HostStream

/**
 * The desktop files panel on a phone: the workspace's tree with lazily
 * loaded folders, fuzzy find, git status markers, live updates from the
 * device's file watcher, and per-file actions (open, open in the browser,
 * save to Downloads, copy path). Works on any device's workspace.
 */
@Composable
fun FilesScreen(
    model: AppModel,
    ref: WorkspaceRef,
    onBack: () -> Unit,
    onOpenFile: (String) -> Unit,
    onTerminal: () -> Unit,
    onBrowser: (String?) -> Unit,
) {
    val api = model.workspaceApi
    val scope = rememberCoroutineScope()
    val context = LocalContext.current
    val clipboard = LocalClipboardManager.current

    // The tree outlives the screen (opening a file and coming back keeps it).
    val tree = remember(ref) { TreeState.of(ref) }
    val children = tree.children
    val expanded = tree.expanded
    val loading = remember(ref) { mutableStateMapOf<String, Boolean>() }
    var error by remember(ref) { mutableStateOf<String?>(null) }
    var includeIgnored by tree.includeIgnored
    var marks by remember(ref) { mutableStateOf<Map<String, GitMark>>(emptyMap()) }
    val folderMarks = remember(marks) { WorkspaceApi.folderMarks(marks) }

    fun load(dir: String) {
        loading[dir] = true
        scope.launch {
            try {
                children[dir] = api.list(ref, dir, includeIgnored)
                if (dir.isEmpty()) error = null
            } catch (e: Exception) {
                if (dir.isEmpty()) error = e.userMessage()
                else children[dir] = emptyList()
            } finally {
                loading.remove(dir)
            }
        }
    }

    fun reloadLoaded() {
        for (dir in children.keys.toList()) load(dir)
    }

    LaunchedEffect(ref, includeIgnored) {
        if (tree.listedIgnored != includeIgnored) children.clear()
        tree.listedIgnored = includeIgnored
        load("")
        for (dir in expanded) load(dir)
    }

    // Live: the device's file watcher and git status.
    DisposableEffect(ref) {
        var files: HostStream? = null
        var git: HostStream? = null
        var pending: Job? = null
        val dirty = HashSet<String>()
        var baseline = true
        val job = scope.launch {
            files = runCatching {
                api.watchFiles(ref, scope, onItem = { frame ->
                    if (frame.optBoolean("resyncRequired")) {
                        if (!baseline) reloadLoaded()
                        baseline = false
                        return@watchFiles
                    }
                    val changes = frame.optJSONArray("changes") ?: return@watchFiles
                    for (i in 0 until changes.length()) {
                        val c = changes.getJSONObject(i)
                        for (p in listOf(c.optString("path"), c.optString("oldPath"))) {
                            if (p.isNotEmpty()) dirty += p.substringBeforeLast('/', "")
                        }
                    }
                    pending?.cancel()
                    pending = scope.launch {
                        delay(250)
                        for (dir in dirty.toList()) if (children.containsKey(dir)) load(dir)
                        dirty.clear()
                    }
                }, onEnd = {})
            }.getOrNull()
            git = runCatching { api.watchGit(ref, scope, onItem = { marks = it ?: emptyMap() }, onEnd = {}) }.getOrNull()
        }
        onDispose {
            job.cancel()
            files?.cancel()
            git?.cancel()
            files?.destroy()
            git?.destroy()
        }
    }

    // Search.
    var searching by remember { mutableStateOf(false) }
    var query by remember { mutableStateOf("") }
    var results by remember { mutableStateOf<List<Entry>?>(null) }
    LaunchedEffect(query, searching, includeIgnored) {
        if (!searching || query.isBlank()) {
            results = null
            return@LaunchedEffect
        }
        delay(200)
        results = runCatching { api.search(ref, query.trim(), includeIgnored) }.getOrElse { emptyList() }
    }

    fun toggle(dir: String) {
        if (dir in expanded) {
            expanded.remove(dir)
        } else {
            expanded.add(dir)
            if (!children.containsKey(dir)) load(dir)
        }
    }

    fun reveal(path: String) {
        var dir = path.substringBeforeLast('/', "")
        val chain = ArrayList<String>()
        while (dir.isNotEmpty()) {
            chain.add(0, dir)
            dir = dir.substringBeforeLast('/', "")
        }
        for (d in chain) if (d !in expanded) {
            expanded.add(d)
            if (!children.containsKey(d)) load(d)
        }
    }

    var menuFor by remember { mutableStateOf<Entry?>(null) }
    var overflow by remember { mutableStateOf(false) }

    fun actionsFor(e: Entry): List<MenuAction> = buildList {
        if (!e.isDir) add(MenuAction("Open", ZIcons.Text) { onOpenFile(e.path) })
        if (!e.isDir && FileKind.of(e.path) == FileKind.Html) add(MenuAction("Open in browser", ZIcons.Globe) { onBrowser(Browser.workspaceUrl(ref, e.path)) })
        if (e.isDir) add(MenuAction("Save folder to Downloads", ZIcons.Save) { model.downloads.saveFolder(ref, e.path, includeIgnored) })
        else add(MenuAction("Save to Downloads", ZIcons.Save) { model.downloads.saveFile(ref, e.path) })
        add(MenuAction("Copy path", ZIcons.Copy) { clipboard.setText(AnnotatedString(ref.absolute(e.path) ?: e.path)) })
        add(MenuAction("Copy relative path", ZIcons.Copy) { clipboard.setText(AnnotatedString(e.path)) })
    }

    Column(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        ToolHeader(
            title = ref.title,
            subtitle = listOfNotNull("Files", ref.deviceName).joinToString(" · "),
            onBack = onBack,
        ) {
            HeaderAction(ZIcons.Search, "Find file", onClick = {
                searching = !searching
                if (!searching) query = ""
            }, selected = searching)
            Box {
                HeaderAction(ZIcons.More, "More", onClick = { overflow = true })
                ActionMenu(
                    overflow,
                    { overflow = false },
                    listOf(
                        MenuAction(if (includeIgnored) "Hide ignored files" else "Show ignored files", if (includeIgnored) ZIcons.EyeClosed else ZIcons.Eye) { includeIgnored = !includeIgnored },
                        MenuAction("Collapse all", ZIcons.Collapse) { expanded.clear() },
                        MenuAction("Refresh", ZIcons.Refresh) { reloadLoaded() },
                        MenuAction("Copy folder path", ZIcons.Copy) { clipboard.setText(AnnotatedString(ref.root ?: "")) },
                    ),
                )
            }
        }
        if (searching) SearchField(query, { query = it }, Modifier.padding(horizontal = 16.dp, vertical = 4.dp))

        Box(Modifier.weight(1f).fillMaxWidth()) {
            val rows = remember(children.toMap(), expanded.toList()) { visibleRows(children, expanded) }
            val found = results
            val list = rememberLazyListState()
            when {
                searching && found != null -> LazyColumn(Modifier.fillMaxSize(), contentPadding = PaddingValues(bottom = 120.dp)) {
                    if (found.isEmpty()) item { Hint("No files match “$query”") }
                    items(found, key = { "s:" + it.path }) { e ->
                        SearchRow(e, marks[e.path], onClick = {
                            if (e.isDir) {
                                searching = false
                                query = ""
                                reveal(e.path + "/x")
                            } else {
                                onOpenFile(e.path)
                            }
                        }, onLongClick = { menuFor = e })
                    }
                }
                error != null && children[""] == null -> Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
                    Text("Couldn't open the files", style = MaterialTheme.typography.titleMedium)
                    Spacer(Modifier.height(6.dp))
                    Text(error ?: "", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.height(12.dp))
                    TextButton(onClick = { load("") }) { Text("Try again") }
                }
                children[""] == null -> Box(Modifier.fillMaxSize(), contentAlignment = Alignment.Center) { LoadingIndicator() }
                else -> LazyColumn(Modifier.fillMaxSize(), state = list, contentPadding = PaddingValues(top = 4.dp, bottom = 120.dp)) {
                    if (rows.isEmpty()) item { Hint("This folder is empty") }
                    items(rows, key = { it.entry.path }) { row ->
                        val e = row.entry
                        TreeRow(
                            row = row,
                            open = e.path in expanded,
                            busy = loading[e.path] == true,
                            mark = if (e.isDir) folderMarks[e.path] else marks[e.path],
                            onClick = { if (e.isDir) toggle(e.path) else onOpenFile(e.path) },
                            onLongClick = { menuFor = e },
                        )
                    }
                }
            }
            menuFor?.let { e ->
                Box(Modifier.align(Alignment.Center)) {
                    ActionMenu(true, { menuFor = null }, actionsFor(e))
                }
            }

            Column(Modifier.align(Alignment.BottomCenter).fillMaxWidth().imePadding().navigationBarsPadding()) {
                DownloadsStrip(model)
                HorizontalFloatingToolbar(
                    expanded = true,
                    modifier = Modifier.align(Alignment.CenterHorizontally).padding(bottom = 12.dp),
                    colors = FloatingToolbarDefaults.vibrantFloatingToolbarColors(),
                ) {
                    ToolbarButton(ZIcons.Terminal, "Terminal", onTerminal)
                    ToolbarButton(ZIcons.Globe, "Browser") { onBrowser(null) }
                    ToolbarButton(ZIcons.Save, "Save project to Downloads") { model.downloads.saveFolder(ref, "", includeIgnored) }
                    ToolbarButton(ZIcons.Refresh, "Refresh") { reloadLoaded() }
                }
            }
        }
    }
}

@Composable
private fun ToolbarButton(icon: Int, label: String, onClick: () -> Unit) {
    IconButton(onClick = onClick) { ZIcon(icon, label, Modifier.size(22.dp)) }
}

/** Per-workspace tree state, kept for the app's lifetime. */
private class TreeState {
    val children = mutableStateMapOf<String, List<Entry>>()
    val expanded = mutableStateListOf<String>()
    val includeIgnored = mutableStateOf(false)
    var listedIgnored = false

    companion object {
        private val all = HashMap<String, TreeState>()

        fun of(ref: WorkspaceRef): TreeState = all.getOrPut("${ref.deviceId}|${ref.chatId ?: ref.spaceId}") { TreeState() }
    }
}

data class TreeRowModel(val entry: Entry, val depth: Int)

/** Depth-first rows under the expanded folders. */
fun visibleRows(children: Map<String, List<Entry>>, expanded: Collection<String>): List<TreeRowModel> {
    val out = ArrayList<TreeRowModel>()
    val open = expanded.toHashSet()
    fun walk(dir: String, depth: Int) {
        for (e in children[dir].orEmpty()) {
            out += TreeRowModel(e, depth)
            if (e.isDir && e.path in open) walk(e.path, depth + 1)
        }
    }
    walk("", 0)
    return out
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun TreeRow(row: TreeRowModel, open: Boolean, busy: Boolean, mark: GitMark?, onClick: () -> Unit, onLongClick: () -> Unit) {
    val e = row.entry
    val turn by animateFloatAsState(if (open) 90f else 0f, MaterialTheme.motionScheme.fastSpatialSpec(), label = "chevron")
    Row(
        Modifier
            .fillMaxWidth()
            .combinedClickable(onClick = onClick, onLongClick = onLongClick)
            .padding(start = 12.dp + 18.dp * row.depth, end = 16.dp)
            .height(40.dp)
            .alpha(if (e.ignored) 0.5f else 1f),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        Box(Modifier.size(20.dp), contentAlignment = Alignment.Center) {
            if (e.isDir) {
                if (busy) LoadingIndicator(Modifier.size(18.dp))
                else ZIcon(ZIcons.ChevronRight, null, Modifier.size(16.dp).rotate(turn), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            }
        }
        Spacer(Modifier.width(4.dp))
        FileIcon(e.path, e.isDir)
        Spacer(Modifier.width(10.dp))
        Text(
            e.name,
            style = MaterialTheme.typography.bodyLarge,
            color = mark?.let { gitColor(it.kind) } ?: MaterialTheme.colorScheme.onSurface,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.weight(1f),
        )
        if (mark != null) {
            if (e.isDir) {
                Box(Modifier.size(7.dp).background(gitColor(mark.kind), RoundedCornerShape(50)))
            } else {
                Text(mark.letter.toString(), fontFamily = GeistMono, fontWeight = FontWeight.SemiBold, style = MaterialTheme.typography.labelMedium, color = gitColor(mark.kind))
            }
        } else if (!e.isDir && e.size != null) {
            Text(formatBytes(e.size), style = MaterialTheme.typography.labelSmall, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

@OptIn(ExperimentalFoundationApi::class)
@Composable
private fun SearchRow(e: Entry, mark: GitMark?, onClick: () -> Unit, onLongClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().combinedClickable(onClick = onClick, onLongClick = onLongClick).padding(horizontal = 20.dp, vertical = 10.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        FileIcon(e.path, e.isDir)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(e.name, style = MaterialTheme.typography.bodyLarge, maxLines = 1, overflow = TextOverflow.Ellipsis, color = mark?.let { gitColor(it.kind) } ?: MaterialTheme.colorScheme.onSurface)
            val parent = e.path.substringBeforeLast('/', "")
            if (parent.isNotEmpty()) Text(parent, style = MaterialTheme.typography.bodySmall, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
    }
}

@Composable
fun SearchField(value: String, onChange: (String) -> Unit, modifier: Modifier = Modifier, placeholder: String = "Find a file") {
    val focus = remember { androidx.compose.ui.focus.FocusRequester() }
    LaunchedEffect(Unit) { runCatching { focus.requestFocus() } }
    Surface(shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.surfaceContainerHigh, modifier = modifier.fillMaxWidth()) {
        Row(Modifier.padding(horizontal = 16.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            ZIcon(ZIcons.Search, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(10.dp))
            Box(Modifier.weight(1f)) {
                if (value.isEmpty()) Text(placeholder, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                BasicTextField(
                    value,
                    onChange,
                    singleLine = true,
                    textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                    cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                    modifier = Modifier.fillMaxWidth().focusRequester(focus),
                )
            }
            if (value.isNotEmpty()) {
                IconButton(onClick = { onChange("") }, modifier = Modifier.size(24.dp)) { ZIcon(ZIcons.Close, "Clear", Modifier.size(18.dp)) }
            }
        }
    }
}

@Composable
fun Hint(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodyMedium,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.fillMaxWidth().padding(32.dp),
    )
}
