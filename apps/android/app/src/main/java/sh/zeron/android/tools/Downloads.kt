package sh.zeron.android.tools

import android.app.Application
import android.app.DownloadManager
import android.content.ContentValues
import android.content.Intent
import android.net.Uri
import android.provider.MediaStore
import android.util.Log
import kotlinx.coroutines.CoroutineScope
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.SupervisorJob
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import org.json.JSONArray
import org.json.JSONObject
import sh.zeron.android.core.AppModel
import sh.zeron.android.core.Transfers
import sh.zeron.android.core.userMessage
import java.io.OutputStream
import java.util.zip.ZipEntry
import java.util.zip.ZipOutputStream

/**
 * Save to Downloads: a file as itself, a folder or a whole project as a
 * streamed `.zip`, into Download/Zeron through MediaStore (no storage
 * permission). Bytes come chunk by chunk over `ReadWorkspaceBytes` from
 * whichever device owns the workspace and go straight into the MediaStore
 * stream — never whole in memory. A device whose engine predates that method
 * sends the path to this phone with file transfer instead (it lands in
 * Download/Zeron the same way).
 */
class Downloads(private val app: Application, private val model: AppModel) {
    sealed interface State {
        /** [fraction] is null while the size is unknown (listing a folder). */
        data class Running(val fraction: Float?, val detail: String) : State
        data class Done(val uri: Uri?, val detail: String) : State
        data class Failed(val message: String) : State
    }

    data class Job(val id: String, val name: String, val state: State)

    private val scope = CoroutineScope(SupervisorJob() + Dispatchers.Main)
    private val _jobs = MutableStateFlow<List<Job>>(emptyList())
    val jobs: StateFlow<List<Job>> = _jobs.asStateFlow()
    private var next = 0

    private fun update(id: String, state: State) {
        _jobs.value = _jobs.value.map { if (it.id == id) it.copy(state = state) else it }
        val job = _jobs.value.firstOrNull { it.id == id } ?: return
        when (state) {
            is State.Running -> model.notifier.download(id, "Saving ${job.name}", state.detail, state.fraction, done = false)
            is State.Done -> model.notifier.download(id, "Saved ${job.name}", state.detail, 1f, done = true, open = openIntent(job.name, state.uri))
            is State.Failed -> model.notifier.download(id, "Couldn't save ${job.name}", state.message, null, done = true)
        }
    }

    fun dismiss(id: String) {
        _jobs.value = _jobs.value.filterNot { it.id == id }
    }

    /** One file, as itself. */
    fun saveFile(ref: WorkspaceRef, path: String) = start(path.substringAfterLast('/').ifEmpty { "file" }) { id, name ->
        val api = model.workspaceApi
        val uri = insert(name, Transfers.relativePath(emptyList()))
        try {
            write(uri) { out ->
                api.readBytes(ref, path) { chunk, size ->
                    out.write(chunk)
                    done += chunk.size
                    progress(id, size, "${formatBytes(done)} of ${formatBytes(size)}")
                }
            }
            publish(uri)
        } catch (e: Exception) {
            discard(uri)
            throw e
        }
        State.Done(uri, "In Downloads/Zeron · ${formatBytes(done)}")
    }

    /**
     * A folder (`""` = the whole workspace) as `<name>.zip`. Git-ignored
     * files (build outputs, `node_modules`) are left out unless [includeIgnored].
     */
    fun saveFolder(ref: WorkspaceRef, dir: String, includeIgnored: Boolean) =
        start((dir.substringAfterLast('/').ifEmpty { ref.title }).let { Transfers.safeSegment(it) } + ".zip") { id, name ->
            val api = model.workspaceApi
            update(id, State.Running(null, "Listing files…"))
            val files = ArrayList<Entry>()
            val pending = ArrayDeque(listOf(dir))
            while (pending.isNotEmpty()) {
                for (e in api.list(ref, pending.removeFirst(), includeIgnored)) {
                    when {
                        e.symlink -> Unit
                        e.isDir -> pending.addLast(e.path)
                        else -> files += e
                    }
                }
            }
            val total = files.sumOf { it.size ?: 0L }
            val base = if (dir.isEmpty()) "" else "$dir/"
            val root = name.removeSuffix(".zip")
            val uri = insert(name, Transfers.relativePath(emptyList()))
            var skipped = 0
            try {
                write(uri) { out ->
                    ZipOutputStream(out).use { zip ->
                        for ((index, f) in files.withIndex()) {
                            zip.putNextEntry(ZipEntry("$root/" + f.path.removePrefix(base)))
                            try {
                                api.readBytes(ref, f.path) { chunk, _ ->
                                    zip.write(chunk)
                                    done += chunk.size
                                    progress(id, total, "${index + 1} of ${files.size} files · ${formatBytes(done)}")
                                }
                            } catch (e: Exception) {
                                // A file that vanished or changed mid-save doesn't sink the archive.
                                if (WorkspaceApi.isUnsupported(e)) throw e
                                skipped++
                            }
                            zip.closeEntry()
                        }
                        zip.finish()
                    }
                }
                publish(uri)
            } catch (e: Exception) {
                discard(uri)
                throw e
            }
            val skippedNote = if (skipped > 0) " · $skipped skipped" else ""
            State.Done(uri, "In Downloads/Zeron · ${files.size} files, ${formatBytes(done)}$skippedNote")
        }

    // ── plumbing ────────────────────────────────────────────────────────

    private inner class Run(val id: String) {
        var done = 0L
        private var last = 0L

        fun progress(id: String, total: Long, detail: String) {
            val now = System.currentTimeMillis()
            if (now - last < 250) return
            last = now
            scope.launch { update(id, State.Running(if (total > 0) (done.toFloat() / total).coerceIn(0f, 1f) else null, detail)) }
        }
    }

    private fun start(name: String, body: suspend Run.(id: String, name: String) -> State): String {
        val id = "dl-${System.currentTimeMillis()}-${next++}"
        _jobs.value = _jobs.value + Job(id, name, State.Running(null, "Starting…"))
        update(id, State.Running(null, "Starting…"))
        scope.launch {
            val result = try {
                withContext(Dispatchers.IO) { Run(id).body(id, name) }
            } catch (e: Exception) {
                Log.w("Zeron", "save to Downloads failed: $name", e)
                if (WorkspaceApi.isUnsupported(e)) State.Failed("This device's engine can't send files yet — update Zeron on it.")
                else State.Failed(e.userMessage())
            }
            update(id, result)
        }
        return id
    }

    /**
     * Older engines lack `ReadWorkspaceBytes`: have the owning device send the
     * path to this phone over file transfer, which lands in Download/Zeron.
     */
    fun sendToPhone(ref: WorkspaceRef, path: String) = start(path.substringAfterLast('/').ifEmpty { ref.title }) { _, _ ->
        val phone = model.engineDeviceId.value ?: error("This phone's engine isn't running.")
        val abs = ref.absolute(path) ?: error("The project's folder is unknown.")
        model.hostCall(ref.deviceId, WorkspaceApi.SEND_FILES, JSONObject().put("toDeviceId", phone).put("paths", JSONArray().put(abs)))
        State.Done(null, "Sending with file transfer — it lands in Downloads/Zeron")
    }

    private fun insert(name: String, relativePath: String): Uri {
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, name)
            put(MediaStore.MediaColumns.MIME_TYPE, mime(name))
            put(MediaStore.MediaColumns.RELATIVE_PATH, relativePath)
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        return app.contentResolver.insert(collection, values) ?: error("Downloads refused $name")
    }

    private inline fun write(uri: Uri, body: (OutputStream) -> Unit) {
        (app.contentResolver.openOutputStream(uri) ?: error("Couldn't open Downloads")).buffered(1 shl 16).use { body(it) }
    }

    private fun publish(uri: Uri) {
        app.contentResolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
    }

    private fun discard(uri: Uri) {
        runCatching { app.contentResolver.delete(uri, null, null) }
    }

    fun mime(name: String): String = model.transfers.mimeOf(name).let { if (name.endsWith(".zip")) "application/zip" else it }

    /** What tapping a saved item does: open it, or the Downloads app for archives. */
    fun openIntent(name: String, uri: Uri?): Intent = if (uri == null || name.endsWith(".zip")) {
        Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)
    } else {
        Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime(name)).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }
}
