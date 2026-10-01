package sh.zeron.android.core

import org.json.JSONArray
import org.json.JSONObject
import sh.zeron.android.feedback.TransferEvent
import java.io.File
import java.util.Locale

/**
 * Device-to-device file transfer (docs/file-transfer.md) as the app speaks
 * it: the engine's RPC names, its camelCase `FileTransfer` rows parsed
 * leniently, and the pure pieces around them — guest ↔ host paths, MIME
 * types, where received files go in Downloads — kept free of Android so the
 * JVM tests cover them.
 */
object Transfers {
    const val SEND = "SendFiles"
    const val LIST = "ListFileTransfers"
    const val CANCEL = "CancelFileTransfer"
    const val ACCEPT = "AcceptFileTransfer"
    const val DECLINE = "DeclineFileTransfer"
    const val CLEAR = "ClearFileTransfers"
    const val GET_SETTINGS = "GetFileTransferSettings"
    const val SET_SETTINGS = "SetFileTransferSettings"

    /** Devices whose engine can receive advertise this on their row. */
    const val CAPABILITY = "file-transfer-v1"

    enum class Direction { Incoming, Outgoing }

    enum class State(val wire: String) {
        Preparing("preparing"),
        Connecting("connecting"),
        AwaitingAcceptance("awaitingAcceptance"),
        Transferring("transferring"),
        Verifying("verifying"),
        Reconnecting("reconnecting"),
        Completed("completed"),
        Failed("failed"),
        Cancelled("cancelled"),
        Declined("declined"),
        ;

        val terminal: Boolean get() = this == Completed || this == Failed || this == Cancelled || this == Declined
        val live: Boolean get() = !terminal

        companion object {
            fun of(wire: String): State = entries.firstOrNull { it.wire == wire } ?: Failed
        }
    }

    /**
     * The feedback event a transfer that just reached [state] deserves, or null.
     * [mine]: the user cancelled or declined it here, so its end is no news.
     */
    fun finishedEvent(incoming: Boolean, state: State, mine: Boolean): TransferEvent? = when {
        state == State.Completed -> if (incoming) TransferEvent.Received else TransferEvent.Sent
        mine -> null
        state == State.Failed || state == State.Cancelled || state == State.Declined -> TransferEvent.Failed
        else -> null
    }

    enum class Kind { File, Folder, Symlink }

    data class Item(val name: String, val kind: Kind, val size: Long, val fileCount: Long, val path: String?)

    data class Transfer(
        val id: String,
        val direction: Direction,
        val peerDeviceId: String,
        val peerDeviceName: String,
        val state: State,
        /** `p2p` or `relay`, once known. */
        val transport: String?,
        val items: List<Item>,
        val fileCount: Long,
        val totalBytes: Long,
        val doneBytes: Long,
        val bytesPerSec: Long,
        val destination: String?,
        val skipped: Long,
        val error: String?,
        val createdAt: Long,
        val updatedAt: Long,
        val finishedAt: Long?,
    ) {
        val incoming: Boolean get() = direction == Direction.Incoming

        val fraction: Float
            get() = when {
                totalBytes <= 0 -> if (state == State.Completed) 1f else 0f
                else -> (doneBytes.toDouble() / totalBytes).coerceIn(0.0, 1.0).toFloat()
            }

        /** "report.pdf", "photos", or "report.pdf and 2 more". */
        val title: String
            get() = when (items.size) {
                0 -> "Nothing"
                1 -> items[0].name
                else -> "${items[0].name} and ${items.size - 1} more"
            }
    }

    data class Settings(val requireConfirmation: Boolean, val inboxDir: String?)

    fun list(json: Any?): List<Transfer> {
        val arr = json as? JSONArray ?: return emptyList()
        return (0 until arr.length()).mapNotNull { i -> arr.optJSONObject(i)?.let(::transfer) }
    }

    fun transfer(o: JSONObject): Transfer? {
        val id = o.optString("id").ifEmpty { return null }
        val items = o.optJSONArray("items") ?: JSONArray()
        return Transfer(
            id = id,
            direction = if (o.optString("direction") == "incoming") Direction.Incoming else Direction.Outgoing,
            peerDeviceId = o.optString("peerDeviceId"),
            peerDeviceName = o.optString("peerDeviceName").ifEmpty { "Another device" },
            state = State.of(o.optString("state")),
            transport = o.str("transport"),
            items = (0 until items.length()).mapNotNull { j ->
                val it = items.optJSONObject(j) ?: return@mapNotNull null
                Item(
                    name = it.optString("name").ifEmpty { return@mapNotNull null },
                    kind = when (it.optString("kind")) {
                        "folder" -> Kind.Folder
                        "symlink" -> Kind.Symlink
                        else -> Kind.File
                    },
                    size = it.optLong("size"),
                    fileCount = it.optLong("fileCount", 1),
                    path = it.str("path"),
                )
            },
            fileCount = o.optLong("fileCount"),
            totalBytes = o.optLong("totalBytes"),
            doneBytes = o.optLong("doneBytes"),
            bytesPerSec = o.optLong("bytesPerSec"),
            destination = o.str("destination"),
            skipped = o.optLong("skipped"),
            error = o.str("error"),
            createdAt = o.optLong("createdAt"),
            updatedAt = o.optLong("updatedAt"),
            finishedAt = if (o.has("finishedAt") && !o.isNull("finishedAt")) o.optLong("finishedAt") else null,
        )
    }

    fun settings(json: Any?): Settings {
        val o = json as? JSONObject ?: return Settings(false, null)
        return Settings(o.optBoolean("requireConfirmation", false), o.str("inboxDir"))
    }

    // ── wording ──────────────────────────────────────────────────────────

    fun bytes(n: Long): String {
        if (n < 1000) return "$n B"
        val units = listOf("KB", "MB", "GB", "TB")
        var v = n / 1000.0
        var u = 0
        while (v >= 1000 && u < units.lastIndex) {
            v /= 1000
            u++
        }
        return if (v >= 100) String.format(Locale.US, "%.0f %s", v, units[u]) else String.format(Locale.US, "%.1f %s", v, units[u])
    }

    fun rate(bytesPerSec: Long): String = "${bytes(bytesPerSec)}/s"

    fun files(n: Long): String = if (n == 1L) "1 file" else "$n files"

    fun stateLabel(t: Transfer): String = when (t.state) {
        State.Preparing -> "Preparing"
        State.Connecting -> "Connecting"
        State.AwaitingAcceptance -> if (t.incoming) "Waiting for you to accept" else "Waiting for ${t.peerDeviceName} to accept"
        State.Transferring -> if (t.incoming) "Receiving" else "Sending"
        State.Verifying -> "Verifying"
        State.Reconnecting -> "Reconnecting"
        State.Completed -> if (t.incoming) "Received" else "Sent"
        State.Failed -> "Failed"
        State.Cancelled -> "Cancelled"
        State.Declined -> "Declined"
    }

    /** The row's second line: progress while live, what happened after. */
    fun detail(t: Transfer): String = when {
        t.state == State.Transferring || t.state == State.Verifying || t.state == State.Reconnecting ->
            listOfNotNull(
                "${bytes(t.doneBytes)} of ${bytes(t.totalBytes)}",
                t.bytesPerSec.takeIf { it > 0 && t.state == State.Transferring }?.let(::rate),
                transportLabel(t.transport),
            ).joinToString(" · ")
        t.state == State.Failed && t.error != null -> t.error
        else -> listOfNotNull(files(t.fileCount), bytes(t.totalBytes), transportLabel(t.transport)).joinToString(" · ")
    }

    fun transportLabel(transport: String?): String? = when (transport) {
        "p2p" -> "Direct"
        "relay" -> "Relayed"
        else -> null
    }

    // ── guest paths ──────────────────────────────────────────────────────

    /**
     * The on-device engine's filesystem as the app sees it: the guest's `/`
     * is [root] (the proot rootfs) except `/tmp`, bound from [tmp]. Paths the
     * engine reports (received items, the inbox) map through here; anything
     * relative or climbing out with `..` maps to nothing.
     */
    class GuestPaths(private val root: File, private val tmp: File) {
        fun host(guestPath: String): File? {
            if (!guestPath.startsWith("/")) return null
            val parts = guestPath.split('/').filter { it.isNotEmpty() && it != "." }
            if (parts.any { it == ".." }) return null
            if (parts.firstOrNull() == "tmp") return parts.drop(1).fold(tmp) { dir, p -> File(dir, p) }
            return parts.fold(root) { dir, p -> File(dir, p) }
        }

        /** The guest path of a host file under the rootfs (or the tmp bind). */
        fun guest(host: File): String? {
            val path = host.absoluteFile.normalize().path
            for ((base, prefix) in listOf(tmp to "/tmp", root to "")) {
                val b = base.absoluteFile.normalize().path
                if (path == b) return prefix.ifEmpty { "/" }
                if (path.startsWith("$b/")) return prefix + path.removePrefix(b)
            }
            return null
        }
    }

    /** Where Share to Zeron stages files before sending (guest paths). */
    const val OUTBOX = "/home/zeron/.zeron/outbox"

    /** The outbox batch folder a staged path belongs to (`<OUTBOX>/<batch>`), or null. */
    fun outboxBatch(guestPath: String): String? {
        if (!guestPath.startsWith("$OUTBOX/")) return null
        val batch = guestPath.removePrefix("$OUTBOX/").substringBefore('/')
        return if (batch.isEmpty() || batch == "." || batch == "..") null else "$OUTBOX/$batch"
    }

    // ── Downloads ────────────────────────────────────────────────────────

    /** MediaStore RELATIVE_PATH root for received files. */
    const val DOWNLOADS_ROOT = "Download/Zeron"

    /**
     * RELATIVE_PATH for a file at [dirs] (folder names from the top-level
     * item down) — always ending in `/`, as MediaStore wants.
     */
    fun relativePath(dirs: List<String>): String =
        (listOf(DOWNLOADS_ROOT) + dirs.map(::safeSegment)).joinToString("/") + "/"

    /** A path segment MediaStore and FAT-style volumes accept. */
    fun safeSegment(name: String): String {
        val cleaned = name.map { c -> if (c.code < 0x20 || c in "\"*/:<>?\\|") '_' else c }.joinToString("")
            .trim().trimEnd('.')
        return when (cleaned) {
            "", ".", ".." -> "_"
            else -> cleaned
        }
    }

    /** One file to copy into Downloads. */
    data class Export(val source: File, val dirs: List<String>, val name: String)

    /**
     * The files under a received item: a file is itself; a folder is walked
     * (symlinks are skipped — they may point anywhere in the guest), keeping
     * its structure under `Download/Zeron/<folder>/…`.
     */
    fun exports(item: File, isSymlink: (File) -> Boolean): List<Export> {
        if (isSymlink(item)) return emptyList()
        if (item.isFile) return listOf(Export(item, emptyList(), safeSegment(item.name)))
        if (!item.isDirectory) return emptyList()
        val out = ArrayList<Export>()
        fun walk(dir: File, dirs: List<String>) {
            for (child in dir.listFiles().orEmpty().sortedBy { it.name }) {
                if (isSymlink(child)) continue
                // Unfinished parts of another transfer never leave the inbox.
                if (child.name.startsWith(".") && child.name.contains(".zeron-") && child.name.endsWith(".part")) continue
                when {
                    child.isDirectory -> walk(child, dirs + child.name)
                    child.isFile -> out += Export(child, dirs, safeSegment(child.name))
                }
            }
        }
        walk(item, listOf(item.name))
        return out
    }

    // ── MIME ─────────────────────────────────────────────────────────────

    const val APK_MIME = "application/vnd.android.package-archive"
    const val UNKNOWN_MIME = "application/octet-stream"

    fun extension(name: String): String? {
        val dot = name.lastIndexOf('.')
        if (dot <= 0 || dot == name.length - 1) return null
        return name.substring(dot + 1).lowercase(Locale.ROOT)
    }

    /**
     * The MIME type to store and open [name] with. [platform] is Android's
     * MimeTypeMap lookup — MediaStore renames a file whose extension doesn't
     * map back to the MIME type it was given, so the same table must decide.
     * Unknown types are octet-stream (kept verbatim); APKs always open the
     * package installer.
     */
    fun mime(name: String, platform: (String) -> String?): String {
        val ext = extension(name) ?: return UNKNOWN_MIME
        if (ext == "apk") return APK_MIME
        return platform(ext) ?: UNKNOWN_MIME
    }

    private fun JSONObject.str(key: String): String? = if (isNull(key)) null else optString(key).ifEmpty { null }
}
