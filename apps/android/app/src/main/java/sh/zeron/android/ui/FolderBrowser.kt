package sh.zeron.android.ui

import sh.zeron.android.design.MenuDivider
import sh.zeron.android.design.BackButton
import sh.zeron.android.design.consumeBlankTaps
import sh.zeron.android.R
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.res.stringResource
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
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
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
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
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.launch
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.AssetIcon
import sh.zeron.android.design.CheckGlyph
import sh.zeron.android.design.Glyph
import androidx.compose.ui.platform.testTag
import sh.zeron.android.design.Glyphs
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import uniffi.zeron_core.DriveEntry
import uniffi.zeron_core.FolderListing

/**
 * Host paths as the engine reports them. A Windows engine answers with
 * `C:\Users\me`-style paths (drive letter, backslashes); everything else is
 * POSIX. Joining and going up must keep the host's own style, or the paths
 * sent back (and stored on new projects) come out mixed.
 */
object HostPaths {
    private val drive = Regex("^[A-Za-z]:([\\\\/].*)?$")

    fun isWindows(path: String): Boolean = drive.matches(path) || path.startsWith("\\\\")

    private fun isSep(c: Char, windows: Boolean) = c == '/' || (windows && c == '\\')

    /** `C:\` or `C:` (a drive root), `\\server\share`, or `/`. */
    fun isRoot(path: String): Boolean {
        if (isWindows(path)) {
            val t = path.trimEnd('\\', '/')
            if (t.length == 2 && t[1] == ':') return true
            if (path.startsWith("\\\\")) return t.drop(2).count { it == '\\' || it == '/' } <= 1
            return false
        }
        return path.trimEnd('/').isEmpty()
    }

    fun join(base: String, name: String): String {
        val windows = isWindows(base)
        if (base.isEmpty()) return name
        return if (isSep(base.last(), windows)) base + name else base + (if (windows) '\\' else '/') + name
    }

    /** One level up, or null at a root. `C:\Users` goes to `C:\`, `/home` to `/`. */
    fun parent(path: String): String? {
        if (path.isEmpty() || isRoot(path)) return null
        val windows = isWindows(path)
        var end = path.length
        while (end > 1 && isSep(path[end - 1], windows)) end--
        val trimmed = path.substring(0, end)
        val cut = trimmed.indexOfLast { isSep(it, windows) }
        if (cut < 0) return null
        val up = trimmed.substring(0, cut)
        return when {
            windows && up.length == 2 && up[1] == ':' -> "$up\\"
            windows && up.isEmpty() -> null
            !windows && up.isEmpty() -> "/"
            else -> up
        }
    }

    /**
     * [path] relative to [base] with `/` separators (the form a
     * `zeron-file:` mention carries), or null when it is not inside [base].
     * Windows paths compare case-insensitively and accept either separator.
     */
    fun relativeTo(base: String, path: String): String? {
        val windows = isWindows(base) || isWindows(path)
        fun norm(p: String) = (if (windows) p.replace('\\', '/') else p).trimEnd('/')
        val b = norm(base)
        val t = norm(path)
        if (b.isEmpty() && !windows) return t.trimStart('/').ifEmpty { null }
        if (t.length <= b.length + 1) return null
        if (!t.startsWith(b, ignoreCase = windows) || t[b.length] != '/') return null
        val rel = t.substring(b.length + 1)
        if (rel.isEmpty() || rel.split('/').any { it.isEmpty() || it == "." || it == ".." }) return null
        return rel
    }

    /** Last component, or the path itself for a root (`C:\`, `/`). */
    fun name(path: String): String {
        if (isRoot(path)) return path
        val windows = isWindows(path)
        val t = path.trimEnd { isSep(it, windows) }
        return t.substring(t.indexOfLast { isSep(it, windows) } + 1)
    }
}

/** What a [HostBrowser]'s bottom action sees. */
internal data class BrowserState(
    val deviceId: String,
    val current: String?,
    val busy: Boolean,
    val error: String?,
    /** Repo flags from the listing we came from, keyed by child path. */
    val repoHint: Map<String, Boolean>,
)

/**
 * Pick a folder on a host and register it as a project (iOS
 * NewProjectViewController): home first, then any drive (ListDrives), folders
 * only, `..` to go up. The project is created with the path exactly as the
 * host listed it; git is taken from the folder's repo flag in its parent.
 */
@Composable
fun NewProjectScreen(model: ZeronModel, initialDeviceId: String? = null, onClose: () -> Unit, onCreated: (String, String) -> Unit) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    val client = model.client ?: return
    var creating by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
    HostBrowser(model, initialDeviceId, title = stringResource(R.string.new_project), onClose = onClose) { state ->
        val current = state.current
        val usable = current != null && !state.busy && state.error == null && !creating
        BrowserActionButton(
            colors,
            if (creating) stringResource(R.string.adding) else current?.let { stringResource(R.string.use_folder_named, HostPaths.name(it)) } ?: stringResource(R.string.use_this_folder),
            usable,
        ) {
            val folder = current ?: return@BrowserActionButton
            creating = true
            scope.launch {
                try {
                    val git = state.repoHint[folder] ?: false
                    val id = client.createProject(state.deviceId, folder, git)
                    model.refreshPull()
                    onCreated(id, HostPaths.name(folder))
                } catch (t: Throwable) {
                    model.showToast(t.message ?: context.getString(R.string.create_project_failed))
                } finally {
                    creating = false
                }
            }
        }
    }
}

/**
 * The host folder browser shared by New Project and the composer's "Computer
 * files" picker: a device row (unless [lockDevice]), browse roots (home,
 * [startPath] when given, every drive the host reports), the current path,
 * `..` then folders (repo-tagged), and with [pickFiles] the folder's files
 * too, each tap toggling it in [selected]. [action] draws the bottom button.
 */
@Composable
internal fun HostBrowser(
    model: ZeronModel,
    initialDeviceId: String?,
    title: String,
    onClose: () -> Unit,
    startPath: String? = null,
    lockDevice: Boolean = false,
    pickFiles: Boolean = false,
    selected: Set<String> = emptySet(),
    onToggleFile: (String) -> Unit = {},
    action: @Composable (BrowserState) -> Unit,
) {
    val colors = LocalZeronColors.current
    val context = LocalContext.current
    val client = model.client ?: return
    val devices = remember(model.workspace) { client.executionDevices().ifEmpty { client.devices() } }
    var deviceId by remember { mutableStateOf(initialDeviceId ?: devices.firstOrNull { it.online }?.id ?: devices.firstOrNull()?.id ?: client.deviceId()) }
    var path by remember { mutableStateOf(startPath) }
    var listing by remember { mutableStateOf<FolderListing?>(null) }
    var drives by remember { mutableStateOf<List<DriveEntry>>(emptyList()) }
    var home by remember { mutableStateOf<String?>(null) }
    var repoHint by remember { mutableStateOf<Map<String, Boolean>>(emptyMap()) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    LaunchedEffect(deviceId) {
        drives = runCatching { client.listDrives(deviceId) }.getOrDefault(emptyList())
    }
    // Re-read when the host comes back online, not on every workspace tick:
    // the epoch moves whenever any session updates, and reloading then made
    // the list flicker and ignore taps while it was busy.
    val online = devices.firstOrNull { it.id == deviceId }?.online
    LaunchedEffect(deviceId, path, online) {
        busy = true
        error = null
        try {
            val next = client.listFolders(deviceId, path)
            listing = next
            if (path == null) home = next.path
            repoHint = repoHint + next.entries.filter { it.isDir }.associate { HostPaths.join(next.path, it.name) to it.isRepo }
        } catch (t: Throwable) {
            error = t.message ?: context.getString(R.string.read_folder_failed)
        } finally {
            busy = false
        }
    }
    val device = devices.firstOrNull { it.id == deviceId }
    val current = listing?.path
    Column(Modifier.fillMaxSize().background(colors.background).consumeBlankTaps().statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            BackButton(colors, onClick = onClose)
            Text(
                current?.let { HostPaths.name(it) } ?: title,
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 17.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
            Spacer(Modifier.width(44.dp))
        }
        if (devices.size > 1 && !lockDevice) {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 4.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                devices.forEach { d ->
                    LocationChip(colors, if (d.online) d.name else stringResource(R.string.host_offline_suffix, d.name), selected = d.id == deviceId, icon = { c -> Glyph(Glyphs.Computer, 15.dp, c) }) {
                        if (d.id != deviceId) {
                            deviceId = d.id
                            path = null
                            listing = null
                            home = null
                        }
                    }
                }
            }
        }
        // Browse roots: home, the start folder (the session's), then every
        // drive / volume the host reports.
        Row(Modifier.padding(horizontal = 16.dp, vertical = 6.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            LocationChip(colors, stringResource(R.string.home_folder), selected = current != null && current == home, icon = { c -> Glyph(Glyphs.Home, 15.dp, c) }) { path = null }
            if (startPath != null) {
                LocationChip(colors, HostPaths.name(startPath), selected = current != null && current == startPath, icon = { c -> Glyph(Glyphs.Folder, 15.dp, c) }) { path = startPath }
            }
            drives.forEach { d ->
                val onDrive = current != null && current != home && HostPaths.isWindows(d.path) && current.startsWith(d.path.take(2), ignoreCase = true)
                LocationChip(colors, d.name, selected = onDrive, icon = { c -> Glyph(Glyphs.Drive, 15.dp, c) }) { path = d.path }
            }
        }
        Text(
            current ?: if (busy) stringResource(R.string.loading) else "",
            color = colors.secondary,
            fontFamily = ZeronType.Mono,
            fontSize = 12.5.sp,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
        )
        error?.let {
            Text(if (device?.online == false) stringResource(R.string.device_offline, device.name) else it, color = colors.danger, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp))
        }
        MenuDivider(colors, Modifier.padding(top = 6.dp))
        val folders = listing?.entries.orEmpty().filter { it.isDir }
        val files = if (pickFiles) listing?.entries.orEmpty().filter { !it.isDir } else emptyList()
        LazyColumn(Modifier.weight(1f)) {
            val up = current?.let { HostPaths.parent(it) }
            if (up != null) {
                item(key = "..") {
                    FolderRow(colors, "..", repo = false, icon = { c -> Glyph(Glyphs.ArrowUp, 18.dp, c) }) { if (!busy) path = up }
                }
            }
            items(folders, key = { it.name }) { entry ->
                FolderRow(colors, entry.name, repo = entry.isRepo, icon = { c -> Glyph(Glyphs.Folder, 18.dp, c) }) {
                    val base = current ?: return@FolderRow
                    if (!busy) path = HostPaths.join(base, entry.name)
                }
            }
            items(files, key = { "file:" + it.name }) { entry ->
                val full = current?.let { HostPaths.join(it, entry.name) }
                FileRow(colors, entry.name, checked = full != null && full in selected) {
                    if (full != null && !busy) onToggleFile(full)
                }
            }
            if (listing?.truncated == true) {
                item(key = "truncated") {
                    Text(stringResource(R.string.folders_truncated), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp, modifier = Modifier.padding(20.dp))
                }
            }
            if (listing != null && folders.isEmpty() && files.isEmpty()) {
                item(key = "empty") {
                    Text(stringResource(if (pickFiles) R.string.no_files else R.string.no_folders), color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                }
            }
        }
        action(BrowserState(deviceId, current, busy, error, repoHint))
    }
}

/** The browser's full-width bottom button (Use folder / Insert). */
@Composable
internal fun BrowserActionButton(colors: ZeronColors, title: String, enabled: Boolean, onClick: () -> Unit) {
    Box(
        Modifier
            .padding(horizontal = 16.dp, vertical = 12.dp)
            .fillMaxWidth()
            .height(50.dp)
            .clip(RoundedCornerShape(25.dp))
            .background(if (enabled) colors.text else colors.controlFill)
            .clickable(enabled = enabled, onClick = onClick),
        contentAlignment = Alignment.Center,
    ) {
        Text(
            title,
            color = if (enabled) colors.background else colors.tertiary,
            fontFamily = ZeronType.Sans,
            fontWeight = FontWeight.SemiBold,
            fontSize = 16.sp,
            maxLines = 1,
            overflow = TextOverflow.Ellipsis,
        )
    }
}

@Composable
private fun LocationChip(colors: ZeronColors, title: String, selected: Boolean, icon: @Composable (androidx.compose.ui.graphics.Color) -> Unit, onClick: () -> Unit) {
    val fg = if (selected) colors.background else colors.text
    Row(
        Modifier.clip(RoundedCornerShape(16.dp)).background(if (selected) colors.text else colors.controlFill).clickable(onClick = onClick).padding(horizontal = 12.dp, vertical = 7.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        icon(fg)
        Spacer(Modifier.width(6.dp))
        Text(title, color = fg, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 14.sp, maxLines = 1)
    }
}

@Composable
private fun FolderRow(colors: ZeronColors, name: String, repo: Boolean, icon: @Composable (androidx.compose.ui.graphics.Color) -> Unit, onClick: () -> Unit) {
    Row(Modifier.fillMaxWidth().clickable(onClick = onClick).padding(horizontal = 20.dp, vertical = 13.dp), verticalAlignment = Alignment.CenterVertically) {
        icon(if (repo) colors.accent else colors.secondary)
        Spacer(Modifier.width(14.dp))
        Text(name, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (repo) {
            Text("git", color = colors.accent, fontFamily = ZeronType.Mono, fontSize = 11.sp, modifier = Modifier.clip(RoundedCornerShape(5.dp)).background(colors.accentSoft).padding(horizontal = 6.dp, vertical = 2.dp))
            Spacer(Modifier.width(8.dp))
        }
        Text("›", color = colors.tertiary, fontSize = 18.sp)
    }
}

/** A file in the picker: its type icon, the name, a check when selected. */
@Composable
private fun FileRow(colors: ZeronColors, name: String, checked: Boolean, onClick: () -> Unit) {
    Row(
        Modifier.fillMaxWidth().testTag("browser-file").clickable(onClick = onClick).background(if (checked) colors.accentSoft else androidx.compose.ui.graphics.Color.Transparent).padding(horizontal = 20.dp, vertical = 13.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        AssetIcon(fileIconName(name), 18.dp, colors.secondary)
        Spacer(Modifier.width(14.dp))
        Text(name, color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp, maxLines = 1, overflow = TextOverflow.Ellipsis, modifier = Modifier.weight(1f))
        if (checked) CheckGlyph(colors.accent, Modifier.size(16.dp))
    }
}

/** A generic file-type icon from the bundled set. */
internal fun fileIconName(name: String): String = when (name.substringAfterLast('.', "").lowercase()) {
    "md", "markdown", "mdx" -> "fileicon-files-markdown"
    "rs" -> "fileicon-files-rust"
    "png", "jpg", "jpeg", "gif", "webp", "svg", "heic" -> "fileicon-files-image"
    "txt", "log", "csv", "toml", "yaml", "yml", "json", "lock" -> "fileicon-files-text"
    else -> "fileicon-files-document"
}
