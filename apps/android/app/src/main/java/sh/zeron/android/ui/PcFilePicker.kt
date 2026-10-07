package sh.zeron.android.ui

import androidx.activity.compose.BackHandler
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.res.pluralStringResource
import androidx.compose.ui.res.stringResource
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.LocalZeronColors
import uniffi.zeron_core.fileMentionLink

/**
 * References to files on the session's computer, the way the desktop
 * composer writes them: a file inside the session's folder becomes the
 * canonical workspace-relative mention link (`[name](zeron-file:rel/path)`,
 * what an `@` pick or a file-tree drop inserts; the host hands the agent a
 * plain relative link). Nothing is uploaded: the agent runs on that
 * computer. A file outside the session's folder can't be a mention (they are
 * workspace-relative by design), so it goes in as its absolute path in
 * backticks, which the agent can read just the same.
 */
object PcFileRefs {
    fun reference(cwd: String?, path: String, link: (String, Boolean) -> String = ::fileMentionLink, isDir: Boolean = false): String {
        val rel = usableCwd(cwd)?.let { HostPaths.relativeTo(it, path) }
        if (rel != null && !rel.contains('\\')) return link(rel, isDir)
        return if (path.contains('`')) "`` $path ``" else "`$path`"
    }

    /** The session folder to start in and resolve against; `~` / blank mean none. */
    fun usableCwd(cwd: String?): String? = cwd?.trim()?.takeIf { it.isNotEmpty() && it != "~" && !it.startsWith("~/") }

    /** Append [refs] to the draft, space-separated, leaving the caret after a trailing space. */
    fun insert(draft: String, refs: List<String>): String {
        if (refs.isEmpty()) return draft
        val lead = if (draft.isEmpty() || draft.last().isWhitespace()) "" else " "
        return draft + lead + refs.joinToString(" ") + " "
    }
}

/**
 * The composer's "Computer files" picker: the New Project folder browser
 * (same roots, same navigation) locked to the session's computer, starting
 * in the session's folder, listing files as well; tap files to select
 * several, then Insert.
 */
@Composable
internal fun PcFilePicker(model: ZeronModel, deviceId: String, cwd: String?, onClose: () -> Unit, onPick: (List<String>) -> Unit) {
    val colors = LocalZeronColors.current
    var selected by remember { mutableStateOf(listOf<String>()) }
    BackHandler(onBack = onClose)
    HostBrowser(
        model,
        initialDeviceId = deviceId,
        title = stringResource(R.string.pc_files_title),
        onClose = onClose,
        startPath = PcFileRefs.usableCwd(cwd),
        lockDevice = true,
        pickFiles = true,
        selected = selected.toSet(),
        onToggleFile = { p -> selected = if (p in selected) selected - p else selected + p },
    ) { _ ->
        BrowserActionButton(
            colors,
            if (selected.isEmpty()) stringResource(R.string.pc_files_pick) else pluralStringResource(R.plurals.pc_files_insert, selected.size, selected.size),
            enabled = selected.isNotEmpty(),
        ) { onPick(selected) }
    }
}
