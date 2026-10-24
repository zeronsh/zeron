package sh.zeron.android.transcript

import android.os.Handler
import android.os.Looper
import androidx.compose.runtime.Stable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.setValue
import sh.zeron.android.core.TextEngine
import uniffi.zeron_core.LayoutFrame
import uniffi.zeron_core.LayoutListener
import uniffi.zeron_core.RowKind
import uniffi.zeron_core.TranscriptView
import java.util.concurrent.atomic.AtomicBoolean
import kotlin.math.max

/**
 * One transcript's layout engine plus the viewport over it. Rust owns
 * geometry: each [LayoutFrame] gives exact row offsets, so this only tracks
 * the scroll offset (dp) and keeps the user's place across frames — anchored
 * to a row, or following the tail.
 */
@Stable
class TranscriptState {
    private val main = Handler(Looper.getMainLooper())
    private val pending = AtomicBoolean(false)

    val engine: TranscriptView = TranscriptView(TextEngine.shared, object : LayoutListener {
        // Layout thread: coalesce to one pull per main-loop turn.
        override fun frameReady(revision: ULong) {
            if (pending.compareAndSet(false, true)) main.post {
                pending.set(false)
                if (!closed) apply(engine.frame())
            }
        }
    })

    var frame by mutableStateOf<LayoutFrame?>(null)
        private set
    /** Scroll offset in dp (0 = top of the content). */
    var offset by mutableFloatStateOf(0f)
    var viewport by mutableFloatStateOf(0f)
    /** Breathing room below the last row, on top of the frame's own (dp). */
    var bottomInset by mutableFloatStateOf(12f)
    var following by mutableStateOf(true)
    var dragging by mutableStateOf(false)
    val fonts = StyleFonts()
    /** Rows present before a frame arrived don't fade in. */
    internal val knownKeys = HashSet<ULong>()
    internal var settled = false

    private val cache = object : LinkedHashMap<Triple<ULong, ULong, Float>, RowModel>(64, 0.75f, true) {
        override fun removeEldestEntry(eldest: MutableMap.MutableEntry<Triple<ULong, ULong, Float>, RowModel>?) = size > 400
    }

    private var closed = false
    private var width = 0f
    private var scale = 0f

    /** Attachment upload progress (rings on `pending://` thumbnails). */
    var uploadProgress by mutableStateOf<Double?>(null)

    // ── Runway (desktop transcript.rs `OwnTurnAnchor`) ─────────────────────
    //
    // On an immediate send the prompt glides to the top of the viewport and
    // the content is held at least one viewport tall below it, so the reply
    // streams into reserved space without the view moving on every token.
    // Once the reply fills it, the runway retires and the ordinary follow
    // takes over; a drag releases the hold (the space stays as scroll room).
    private class OwnTurn(val key: ULong?, val before: Set<ULong>)
    private var ownTurn by mutableStateOf<OwnTurn?>(null)
    private var queuedTurn: Set<ULong>? = null

    /** Reserve the reply's space below the prompt about to be sent. */
    fun beginOwnTurn() {
        queuedTurn = null
        ownTurn = OwnTurn(null, recentUserKeys())
        following = true
    }

    /** A message queued behind the live turn takes the runway once its bubble lands. */
    fun expectQueuedTurn() {
        if (queuedTurn == null) queuedTurn = recentUserKeys()
        following = true
    }

    /** The newest user row not in `before`, near the tail. */
    private fun newUserRow(f: LayoutFrame, before: Set<ULong>): ULong? {
        val n = f.rowCount().toInt()
        for (i in (n - 1) downTo max(0, n - 12)) {
            val p = f.placement(i.toUInt()) ?: continue
            if (p.kind == RowKind.USER && p.key !in before) return p.key
        }
        return null
    }

    /** Content height the runway holds, or null when it isn't holding. */
    private val runwayHeight: Float?
        get() {
            val turn = ownTurn ?: return null
            val f = frame ?: return null
            val key = turn.key ?: return null
            val p = f.indexOf(key)?.let { f.placement(it) } ?: return null
            val natural = f.totalHeight() + bottomInset
            val minimum = p.y + viewport
            return if (natural >= minimum - 0.5f) null else minimum
        }

    /** The runway is gliding the prompt up (ease-out instead of the settle spring). */
    val runwayActive: Boolean get() = runwayHeight != null

    val contentHeight: Float get() = max((frame?.totalHeight() ?: 0f) + bottomInset, runwayHeight ?: 0f)
    val maxOffset: Float get() = max(0f, contentHeight - viewport)
    val distanceFromBottom: Float get() = maxOffset - offset

    fun setViewport(widthDp: Float, heightDp: Float, textScale: Float) {
        viewport = heightDp
        if (widthDp != width || textScale != scale) {
            width = widthDp
            scale = textScale
            engine.setViewport(widthDp, textScale)
        }
    }

    fun apply(new: LayoutFrame) {
        val old = frame
        val first = old == null || old.rowCount() == 0u
        var anchor: Pair<ULong, Float>? = null
        if (!following && old != null) {
            old.indexAt(max(0f, offset))?.let { i ->
                old.placement(i)?.let { p -> anchor = p.key to (offset - p.y) }
            }
        }
        queuedTurn?.let { before ->
            newUserRow(new, before)?.let { key ->
                queuedTurn = null
                ownTurn = OwnTurn(key, before)
                following = true
            }
        }
        ownTurn?.let { turn -> if (turn.key == null) newUserRow(new, turn.before)?.let { ownTurn = OwnTurn(it, turn.before) } }
        motion = motionFor(old, new)
        frame = new
        if (first && new.rowCount() > 0u) {
            for (i in 0u until new.rowCount()) new.placement(i)?.let { knownKeys.add(it.key) }
            offset = maxOffset
            settled = true
        } else if (!following) {
            anchor?.let { (key, delta) ->
                new.indexOf(key)?.let { i -> new.placement(i)?.let { p -> offset = (p.y + delta).coerceIn(0f, maxOffset) } }
            }
        }
    }

    // ── Row motion ────────────────────────────────────────────────────────
    //
    // Fold toggles tween 140 ms ease-out; a visible tool group that grew (a
    // new call arrived) reveals over 360 ms expo — the desktop timings. Rows
    // interpolate from where the previous frame placed them.

    class Motion(val id: Int, val from: Map<ULong, Pair<Float, Float>>, val durationMs: Int, val expo: Boolean)

    var motion by mutableStateOf<Motion?>(null)
        private set
    private var motionId = 0
    private var pendingFold: ULong? = null

    private fun motionFor(old: LayoutFrame?, new: LayoutFrame): Motion? {
        if (old == null || old.rowCount() == 0u || old.width() != new.width()) return null
        fun height(f: LayoutFrame, key: ULong) = f.indexOf(key)?.let { f.placement(it)?.height }
        val band = old.rowsIn(offset - viewport, offset + viewport * 2)
        val fold = pendingFold
        val kind = when {
            fold != null && height(old, fold) != null && height(old, fold) != height(new, fold) -> {
                pendingFold = null
                140 to false
            }
            band.any { it.kind == RowKind.TOOLS && (height(new, it.key) ?: 0f) > it.height } -> 360 to true
            else -> return null
        }
        return Motion(++motionId, band.associate { it.key to (it.y to it.height) }, kind.first, kind.second)
    }

    /** The display model for a row at the frame's width (cached per version). */
    fun model(frame: LayoutFrame, index: UInt, key: ULong, version: ULong): RowModel? {
        val k = Triple(key, version, frame.width())
        cache[k]?.let { return it }
        val display = frame.display(index) ?: return null
        return RowModel(display).also { cache[k] = it }
    }

    /** User scroll by `delta` dp (positive = toward the tail). Returns the consumed amount. */
    fun scrollBy(delta: Float): Float {
        val before = offset
        offset = (offset + delta).coerceIn(0f, maxOffset)
        // Momentum carrying the list back into the last 70dp re-latches follow.
        if (!following && !dragging && delta > 0 && distanceFromBottom < 70f) following = true
        return offset - before
    }

    fun scrollToBottom() {
        following = true
    }

    /** A drag hands control to the user (the runway's space stays as scroll room). */
    fun released() {
        following = false
    }

    fun toggle(key: ULong) {
        pendingFold = key
        engine.toggle(key)
    }

    fun toggleDetail(row: ULong, detail: ULong, open: Boolean) {
        pendingFold = row
        engine.toggleDetail(row, detail, open)
    }

    /** Newest user row keys (the runway looks for a new one after a send). */
    fun recentUserKeys(): Set<ULong> {
        val f = frame ?: return emptySet()
        val n = f.rowCount().toInt()
        val out = HashSet<ULong>()
        for (i in (n - 1) downTo max(0, n - 24)) f.placement(i.toUInt())?.let { if (it.kind == RowKind.USER) out.add(it.key) }
        return out
    }

    fun close() {
        closed = true
        engine.shutdown()
        engine.close()
    }
}
