package sh.zeron.android.ui

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
import androidx.compose.material3.HorizontalDivider
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
import sh.zeron.android.design.Glyph
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

    /** Last component, or the path itself for a root (`C:\`, `/`). */
    fun name(path: String): String {
        if (isRoot(path)) return path
        val windows = isWindows(path)
        val t = path.trimEnd { isSep(it, windows) }
        return t.substring(t.indexOfLast { isSep(it, windows) } + 1)
    }
}

/**
 * Pick a folder on a host and register it as a project (iOS
 * NewProjectViewController): home first, then any drive (ListDrives), folders
 * only, `..` to go up. The project is created with the path exactly as the
 * host listed it; git is taken from the folder's repo flag in its parent.
 */
@Composable
fun NewProjectScreen(model: ZeronModel, initialDeviceId: String? = null, onClose: () -> Unit, onCreated: (String, String) -> Unit) {
    val colors = LocalZeronColors.current
    val client = model.client ?: return
    val devices = remember(model.workspace) { client.executionDevices().ifEmpty { client.devices() } }
    var deviceId by remember { mutableStateOf(initialDeviceId ?: devices.firstOrNull { it.online }?.id ?: devices.firstOrNull()?.id ?: client.deviceId()) }
    var path by remember { mutableStateOf<String?>(null) }
    var listing by remember { mutableStateOf<FolderListing?>(null) }
    var drives by remember { mutableStateOf<List<DriveEntry>>(emptyList()) }
    var home by remember { mutableStateOf<String?>(null) }
    // Repo flags from the listing we came from, keyed by child path.
    var repoHint by remember { mutableStateOf<Map<String, Boolean>>(emptyMap()) }
    var error by remember { mutableStateOf<String?>(null) }
    var busy by remember { mutableStateOf(false) }
    var creating by remember { mutableStateOf(false) }
    val scope = rememberCoroutineScope()
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
            error = t.message ?: "Couldn't read that folder"
        } finally {
            busy = false
        }
    }
    val device = devices.firstOrNull { it.id == deviceId }
    val current = listing?.path
    Column(Modifier.fillMaxSize().background(colors.background).statusBarsPadding().navigationBarsPadding()) {
        Row(Modifier.fillMaxWidth().padding(horizontal = 8.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
            Text("Cancel", color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp, modifier = Modifier.clip(RoundedCornerShape(10.dp)).clickable(onClick = onClose).padding(10.dp))
            Text(
                current?.let { HostPaths.name(it) } ?: "New Project",
                color = colors.text,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 17.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.weight(1f).padding(horizontal = 8.dp),
                textAlign = androidx.compose.ui.text.style.TextAlign.Center,
            )
            Spacer(Modifier.width(64.dp))
        }
        if (devices.size > 1) {
            Row(Modifier.padding(horizontal = 16.dp, vertical = 4.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
                devices.forEach { d ->
                    LocationChip(colors, d.name + if (d.online) "" else " · offline", selected = d.id == deviceId, icon = { c -> Glyph(Glyphs.Computer, 15.dp, c) }) {
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
        // Browse roots: home, then every drive / volume the host reports.
        Row(Modifier.padding(horizontal = 16.dp, vertical = 6.dp).horizontalScroll(rememberScrollState()), horizontalArrangement = Arrangement.spacedBy(8.dp)) {
            LocationChip(colors, "Home", selected = current != null && current == home, icon = { c -> Glyph(Glyphs.Home, 15.dp, c) }) { path = null }
            drives.forEach { d ->
                val onDrive = current != null && current != home && HostPaths.isWindows(d.path) && current.startsWith(d.path.take(2), ignoreCase = true)
                LocationChip(colors, d.name, selected = onDrive, icon = { c -> Glyph(Glyphs.Drive, 15.dp, c) }) { path = d.path }
            }
        }
        Text(
            current ?: if (busy) "Loading…" else "",
            color = colors.secondary,
            fontFamily = ZeronType.Mono,
            fontSize = 12.5.sp,
            maxLines = 2,
            overflow = TextOverflow.Ellipsis,
            modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp),
        )
        error?.let {
            Text(if (device?.online == false) "${device.name} is offline." else it, color = colors.danger, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(horizontal = 20.dp, vertical = 4.dp))
        }
        HorizontalDivider(color = colors.hairline, modifier = Modifier.padding(top = 6.dp))
        val folders = listing?.entries.orEmpty().filter { it.isDir }
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
            if (listing?.truncated == true) {
                item(key = "truncated") {
                    Text("Only the first folders are shown.", color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 13.sp, modifier = Modifier.padding(20.dp))
                }
            }
            if (listing != null && folders.isEmpty()) {
                item(key = "empty") {
                    Text("No folders here.", color = colors.tertiary, fontFamily = ZeronType.Sans, fontSize = 14.sp, modifier = Modifier.padding(20.dp))
                }
            }
        }
        val usable = current != null && !busy && error == null && !creating
        Box(
            Modifier
                .padding(horizontal = 16.dp, vertical = 12.dp)
                .fillMaxWidth()
                .height(50.dp)
                .clip(RoundedCornerShape(25.dp))
                .background(if (usable) colors.text else colors.controlFill)
                .clickable(enabled = usable) {
                    val folder = current ?: return@clickable
                    creating = true
                    scope.launch {
                        try {
                            val git = repoHint[folder] ?: false
                            val id = client.createProject(deviceId, folder, git)
                            model.refreshPull()
                            onCreated(id, HostPaths.name(folder))
                        } catch (t: Throwable) {
                            model.showToast(t.message ?: "Couldn't create the project")
                        } finally {
                            creating = false
                        }
                    }
                },
            contentAlignment = Alignment.Center,
        ) {
            Text(
                if (creating) "Adding…" else current?.let { "Use “${HostPaths.name(it)}”" } ?: "Use this folder",
                color = if (usable) colors.background else colors.tertiary,
                fontFamily = ZeronType.Sans,
                fontWeight = FontWeight.SemiBold,
                fontSize = 16.sp,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
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
