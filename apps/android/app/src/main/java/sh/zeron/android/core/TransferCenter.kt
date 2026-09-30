package sh.zeron.android.core

import android.app.Application
import android.app.DownloadManager
import android.content.ContentValues
import android.content.Context
import android.content.Intent
import android.net.Uri
import android.provider.MediaStore
import android.provider.OpenableColumns
import android.util.Log
import android.webkit.MimeTypeMap
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.MainScope
import kotlinx.coroutines.channels.Channel
import kotlinx.coroutines.flow.MutableStateFlow
import kotlinx.coroutines.flow.StateFlow
import kotlinx.coroutines.flow.asStateFlow
import kotlinx.coroutines.flow.collectLatest
import kotlinx.coroutines.flow.combine
import kotlinx.coroutines.flow.distinctUntilChanged
import kotlinx.coroutines.flow.first
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import kotlinx.coroutines.withTimeoutOrNull
import org.json.JSONArray
import org.json.JSONObject
import sh.zeron.android.core.Transfers.Transfer
import uniffi.zeron_core.DeviceView
import java.io.File
import java.nio.file.Files
import java.util.UUID

/**
 * File transfers on this phone's engine (docs/file-transfer.md § Clients):
 * polls `ListFileTransfers` over `host_call` — every second while the
 * Transfers screen or a share is open or a transfer is live, every five
 * otherwise (RuntimeService keeps the process alive while the engine runs) — and
 * reacts to what it sees: incoming notifications, a copy of received files
 * in Download/Zeron (MediaStore), and cleaning the share outbox once an
 * outgoing transfer ends.
 */
class TransferCenter(private val app: Application, private val model: AppModel) {
    private val scope = MainScope()
    private val prefs = app.getSharedPreferences("transfers", 0)

    private val _list = MutableStateFlow<List<Transfer>>(emptyList())
    val list: StateFlow<List<Transfer>> = _list.asStateFlow()

    private val _settings = MutableStateFlow<Transfers.Settings?>(null)
    val settings: StateFlow<Transfers.Settings?> = _settings.asStateFlow()

    /** transferId → item name → exported content URI, or `dir:<relative path>` for a folder. */
    private val _exports = MutableStateFlow(loadExports())
    val exports: StateFlow<Map<String, Map<String, String>>> = _exports.asStateFlow()

    /** The last poll's failure (engine unreachable…), cleared by the next success. */
    private val _error = MutableStateFlow<String?>(null)
    val error: StateFlow<String?> = _error.asStateFlow()

    private val watchers = MutableStateFlow(0)
    private val kick = Channel<Unit>(Channel.CONFLATED)
    private val seenLive = HashSet<String>()
    private val asked = HashSet<String>()
    private val exporting = HashSet<String>()
    private val cleaned = HashSet<String>()
    private var swept = false
    private var started = false

    /**
     * Follow this phone's engine whenever the app has a client for it (not the
     * demo). A new engine device id (custom server, reset) restarts the loop.
     */
    fun start() {
        if (started) return
        started = true
        scope.launch {
            combine(model.client, model.engineDeviceId) { c, id -> id.takeIf { c != null && !c.isDemo() } }
                .distinctUntilChanged()
                .collectLatest { id ->
                    _list.value = emptyList()
                    _settings.value = null
                    if (id != null) run()
                }
        }
    }

    private suspend fun run() {
        while (true) {
            poll()
            val fast = watchers.value > 0 || _list.value.any { it.state.live }
            withTimeoutOrNull(if (fast) 1_000L else 5_000L) { kick.receive() }
        }
    }

    /** A screen (Transfers, the share sheet) is showing transfers: poll fast until [release]d. */
    fun watch(): () -> Unit {
        watchers.value++
        kick.trySend(Unit)
        var released = false
        return {
            if (!released) {
                released = true
                watchers.value--
            }
        }
    }

    fun refresh() {
        kick.trySend(Unit)
    }

    /** This phone's (engine's) device id, or null before the engine answered. */
    fun engineId(): String? = model.engineDeviceId.value

    private suspend fun call(method: String, params: JSONObject = JSONObject()): Any {
        // A notification action can arrive while the app (re)connects to its engine.
        val id = engineId()
            ?: withTimeoutOrNull(8_000L) { model.engineDeviceId.first { it != null } }
            ?: throw IllegalStateException("The on-device engine hasn't started yet.")
        return model.hostCall(id, method, params)
    }

    private suspend fun poll() {
        val next = try {
            Transfers.list(call(Transfers.LIST))
        } catch (e: Exception) {
            _error.value = e.userMessage()
            return
        }
        _error.value = null
        val prev = _list.value
        _list.value = next
        if (_settings.value == null && watchers.value > 0) loadSettings()
        onChange(prev, next)
    }

    private suspend fun loadSettings() {
        runCatching { Transfers.settings(call(Transfers.GET_SETTINGS)) }.onSuccess { _settings.value = it }
    }

    private fun onChange(prev: List<Transfer>, next: List<Transfer>) {
        val before = prev.associateBy { it.id }
        val now = System.currentTimeMillis()
        for (t in next) {
            if (t.incoming) {
                when {
                    t.state == Transfers.State.AwaitingAcceptance -> {
                        seenLive += t.id
                        if (asked.add(t.id)) model.notifier.transferAsk(t)
                    }
                    t.state.live -> {
                        seenLive += t.id
                        if (before[t.id] != t) model.notifier.transferProgress(t)
                    }
                    t.state == Transfers.State.Completed -> {
                        // Notify for what finished while we watched, or just before launch.
                        val fresh = t.id in seenLive || (t.finishedAt ?: 0) > now - 5 * 60_000
                        if (t.id !in _exports.value && exporting.add(t.id)) {
                            scope.launch { export(t, notify = fresh) }
                        } else if (seenLive.remove(t.id)) {
                            model.notifier.transferReceived(t, openIntent(t))
                        }
                    }
                    else -> if (seenLive.remove(t.id)) model.notifier.transferEnded(t)
                }
            } else if (t.state.terminal && t.id !in cleaned) {
                cleaned += t.id
                cleanOutbox(t)
            }
        }
        // Rows the engine forgot (cleared, aged out) keep no export record.
        val ids = next.mapTo(HashSet()) { it.id }
        val kept = _exports.value.filterKeys { it in ids || it in exporting }
        if (kept.size != _exports.value.size) saveExports(kept)
        if (!swept) {
            swept = true
            scope.launch(Dispatchers.IO) { sweepOutbox(next) }
        }
    }

    // ── actions ──────────────────────────────────────────────────────────

    suspend fun cancel(id: String) = act(Transfers.CANCEL, id)
    suspend fun accept(id: String) = act(Transfers.ACCEPT, id)
    suspend fun decline(id: String) = act(Transfers.DECLINE, id)

    suspend fun clear(id: String? = null) {
        call(Transfers.CLEAR, JSONObject().apply { if (id != null) put("transferId", id) })
        refresh()
    }

    private suspend fun act(method: String, id: String) {
        call(method, JSONObject().put("transferId", id))
        model.notifier.cancelTransfer(id)
        refresh()
    }

    /** Notification actions: accept or decline without opening the app. */
    fun respond(id: String, accept: Boolean, done: () -> Unit) {
        scope.launch {
            runCatching { if (accept) accept(id) else decline(id) }.onFailure { Log.w("Zeron", "transfer ${if (accept) "accept" else "decline"} failed", it) }
            done()
        }
    }

    suspend fun setRequireConfirmation(on: Boolean) {
        val current = _settings.value ?: Transfers.settings(call(Transfers.GET_SETTINGS))
        val params = JSONObject().put("requireConfirmation", on)
        current.inboxDir?.let { params.put("inboxDir", it) }
        call(Transfers.SET_SETTINGS, params)
        _settings.value = current.copy(requireConfirmation = on)
    }

    /** Send guest [paths] from this phone's engine; returns the transfer id. */
    suspend fun send(toDeviceId: String, paths: List<String>): String {
        val reply = call(Transfers.SEND, JSONObject().put("toDeviceId", toDeviceId).put("paths", JSONArray(paths))) as? JSONObject
        refresh()
        return reply?.optString("transferId")?.ifEmpty { null } ?: error("The engine didn't start the transfer.")
    }

    /** Devices that can receive from this phone: other engines advertising the capability. */
    fun recipients(): List<DeviceView> {
        val self = engineId()
        val devices = runCatching { model.client.value?.devices() }.getOrNull().orEmpty()
        return devices
            .filter { it.id != self && !it.isSelf && Transfers.CAPABILITY in it.capabilities }
            .sortedWith(compareByDescending<DeviceView> { it.online }.thenBy { it.name.lowercase() })
    }

    // ── share outbox ─────────────────────────────────────────────────────

    data class Staged(val name: String, val size: Long, val guestPath: String)

    /**
     * Copy shared content into `OUTBOX/<batch>/<name>` inside the guest, so the
     * engine can read it. Returns the batch's guest folder and its files.
     */
    suspend fun stage(uris: List<Uri>, text: String?): Pair<String, List<Staged>> = withContext(Dispatchers.IO) {
        val batch = "${Transfers.OUTBOX}/${UUID.randomUUID()}"
        val dir = model.phone.paths.host(batch) ?: error("No guest")
        dir.mkdirs()
        val used = HashSet<String>()
        fun unique(name: String): String {
            var candidate = name
            var n = 2
            while (!used.add(candidate.lowercase())) {
                val ext = Transfers.extension(name)?.let { ".$it" }.orEmpty()
                candidate = "${name.removeSuffix(ext)} ($n)$ext"
                n++
            }
            return candidate
        }
        val out = ArrayList<Staged>()
        val resolver = app.contentResolver
        try {
            for (uri in uris) {
                val display = runCatching {
                    resolver.query(uri, arrayOf(OpenableColumns.DISPLAY_NAME), null, null, null)?.use { c ->
                        if (c.moveToFirst()) c.getString(0) else null
                    }
                }.getOrNull() ?: uri.lastPathSegment?.substringAfterLast('/')
                val name = unique(Transfers.safeSegment(display ?: "Shared file"))
                val file = File(dir, name)
                val input = resolver.openInputStream(uri) ?: error("Couldn't read $name")
                input.use { src -> file.outputStream().use { src.copyTo(it, 1 shl 16) } }
                out += Staged(name, file.length(), "$batch/$name")
            }
            if (uris.isEmpty() && !text.isNullOrEmpty()) {
                val name = unique("Shared text.txt")
                val file = File(dir, name).apply { writeText(text) }
                out += Staged(name, file.length(), "$batch/$name")
            }
        } catch (e: Exception) {
            dir.deleteRecursively()
            throw e
        }
        batch to out
    }

    /** Drop a staged batch (the share was abandoned). */
    fun discard(batch: String) {
        scope.launch(Dispatchers.IO) { Transfers.outboxBatch("$batch/x")?.let { model.phone.paths.host(it)?.deleteRecursively() } }
    }

    private fun cleanOutbox(t: Transfer) {
        val batches = t.items.mapNotNull { it.path?.let(Transfers::outboxBatch) }.toSet()
        if (batches.isEmpty()) return
        scope.launch(Dispatchers.IO) { for (b in batches) model.phone.paths.host(b)?.deleteRecursively() }
    }

    /** Batches a day old that no live transfer reads (the app died mid-share). */
    private fun sweepOutbox(list: List<Transfer>) {
        val root = model.phone.paths.host(Transfers.OUTBOX) ?: return
        val live = list.filter { it.state.live }.flatMap { t -> t.items.mapNotNull { it.path?.let(Transfers::outboxBatch) } }.toSet()
        val cutoff = System.currentTimeMillis() - 24 * 3600_000L
        for (dir in root.listFiles().orEmpty()) {
            if (dir.lastModified() < cutoff && "${Transfers.OUTBOX}/${dir.name}" !in live) dir.deleteRecursively()
        }
    }

    // ── Downloads ────────────────────────────────────────────────────────

    private suspend fun export(t: Transfer, notify: Boolean) {
        val record = LinkedHashMap<String, String>()
        try {
            withContext(Dispatchers.IO) {
                for (item in t.items) {
                    val host = item.path?.let(model.phone.paths::host) ?: continue
                    val files = Transfers.exports(host) { Files.isSymbolicLink(it.toPath()) }
                    if (host.isDirectory) {
                        for (e in files) saveToDownloads(e)
                        record[item.name] = "dir:" + Transfers.relativePath(listOf(host.name))
                    } else {
                        files.firstOrNull()?.let { record[item.name] = saveToDownloads(it).toString() }
                    }
                }
            }
            saveExports(_exports.value + (t.id to record))
            seenLive.remove(t.id)
            if (notify) model.notifier.transferReceived(t, openIntent(t))
        } catch (e: Exception) {
            Log.w("Zeron", "export of transfer ${t.id} to Downloads failed", e)
            model.notifier.transferReceived(t, null, "Couldn't copy to Downloads: ${e.message}")
        } finally {
            exporting.remove(t.id)
        }
    }

    /** MediaStore insert with the IS_PENDING dance; no storage permission needed. */
    private fun saveToDownloads(e: Transfers.Export): Uri {
        val resolver = app.contentResolver
        val collection = MediaStore.Downloads.getContentUri(MediaStore.VOLUME_EXTERNAL_PRIMARY)
        val values = ContentValues().apply {
            put(MediaStore.MediaColumns.DISPLAY_NAME, e.name)
            put(MediaStore.MediaColumns.MIME_TYPE, mimeOf(e.name))
            put(MediaStore.MediaColumns.RELATIVE_PATH, Transfers.relativePath(e.dirs))
            put(MediaStore.MediaColumns.IS_PENDING, 1)
        }
        val uri = resolver.insert(collection, values) ?: error("MediaStore refused ${e.name}")
        try {
            (resolver.openOutputStream(uri) ?: error("Couldn't write ${e.name}")).use { out ->
                e.source.inputStream().use { it.copyTo(out, 1 shl 16) }
            }
            resolver.update(uri, ContentValues().apply { put(MediaStore.MediaColumns.IS_PENDING, 0) }, null, null)
        } catch (err: Exception) {
            runCatching { resolver.delete(uri, null, null) }
            throw err
        }
        return uri
    }

    /** What tapping a received item does: open the file, or Downloads for a folder. */
    fun openIntent(t: Transfer, item: Transfers.Item? = t.items.singleOrNull()): Intent? {
        val record = _exports.value[t.id] ?: return null
        if (item == null) return Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)
        val value = record[item.name] ?: return null
        if (value.startsWith("dir:")) return Intent(DownloadManager.ACTION_VIEW_DOWNLOADS)
        val uri = Uri.parse(value)
        return Intent(Intent.ACTION_VIEW)
            .setDataAndType(uri, mimeOf(item.name))
            .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    }

    /** Opens [item]; false if it isn't in Downloads (yet) or nothing can open it. */
    fun open(context: Context, t: Transfer, item: Transfers.Item? = null): Boolean {
        val intent = openIntent(t, item ?: t.items.singleOrNull()) ?: return false
        // From a screen the viewer stacks on Zeron's task (Back returns here).
        if (context !is android.app.Activity) intent.addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)
        return try {
            context.startActivity(intent)
            true
        } catch (e: Exception) {
            Log.w("Zeron", "open failed", e)
            false
        }
    }

    fun mimeOf(name: String): String = Transfers.mime(name) { MimeTypeMap.getSingleton().getMimeTypeFromExtension(it) }

    private fun loadExports(): Map<String, Map<String, String>> = runCatching {
        val o = JSONObject(prefs.getString("exported", "{}")!!)
        o.keys().asSequence().associateWith { id ->
            val items = o.getJSONObject(id)
            items.keys().asSequence().associateWith { items.getString(it) }
        }
    }.getOrDefault(emptyMap())

    private fun saveExports(value: Map<String, Map<String, String>>) {
        _exports.value = value
        val o = JSONObject()
        for ((id, items) in value) o.put(id, JSONObject(items))
        prefs.edit().putString("exported", o.toString()).apply()
    }
}
