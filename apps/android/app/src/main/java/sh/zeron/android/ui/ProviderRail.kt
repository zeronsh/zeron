package sh.zeron.android.ui

import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxScope
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.MaterialTheme
import androidx.compose.runtime.Composable
import androidx.compose.runtime.Stable
import androidx.compose.runtime.State
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.shadow
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.LayoutCoordinates
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.onClick
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import kotlin.math.roundToInt

/** The rail's width; half of it overhangs the card's start edge. */
internal val RailWidth = 36.dp
internal val RailOverhang = RailWidth / 2

/**
 * The model list's sections and what the provider rail needs to know about
 * them. Pure and index based so it can be tested: Favorites (starred models,
 * which the list already puts first), then one section per provider in catalog
 * order, with providers the app has no mark for gathered under "Other".
 */
object RailRules {
    const val FAVORITES = "favorites"
    const val OTHER = "other"

    /** A section of the list. [harness] picks the rail mark and the provider's sound. */
    data class Section(val id: String, val harness: String, val label: String, val item: Int)

    sealed interface Item {
        val key: String

        data class Header(val section: Section) : Item {
            override val key get() = "section/${section.id}"
        }

        data class Row(val entry: ModelPickerRules.Entry) : Item {
            override val key get() = entry.key
        }
    }

    /**
     * The list's items and sections. With [headers] the rows are grouped by
     * section (stable, in order of first appearance) and each group opens with
     * a [Item.Header]; without them the rows keep their order.
     */
    class Layout(val items: List<Item>, val sections: List<Section>, private val sectionOfItem: IntArray) {
        /** The section index of the list item at [item] (clamped). */
        fun sectionAt(item: Int): Int = if (sections.isEmpty()) -1 else sectionOfItem[item.coerceIn(0, sectionOfItem.size - 1)]

        fun itemOfKey(key: String): Int = items.indexOfFirst { it.key == key }
    }

    /** The canonical provider id for a harness, or [OTHER] when the app has no mark for it. */
    fun providerOf(harness: String): String = when {
        harness.startsWith("claude") -> "claude-code"
        harness.startsWith("codex") -> "codex"
        harness.startsWith("cursor") -> "cursor"
        harness.startsWith("devin") -> "devin"
        harness.startsWith("grok") -> "grok"
        harness.startsWith("hermes") -> "hermes"
        harness == "pi" || harness.startsWith("pi-") -> "pi"
        harness.startsWith("opencode") -> "opencode"
        harness.startsWith("antigravity") -> "antigravity"
        else -> OTHER
    }

    fun sectionId(entry: ModelPickerRules.Entry): String =
        if (entry.starred && !entry.selectedOnly) FAVORITES else providerOf(entry.choice.harness)

    /** Whether the rail has something to offer: no search running and at least two sections. */
    fun visible(entries: List<ModelPickerRules.Entry>, query: String): Boolean =
        query.isBlank() && entries.mapTo(HashSet()) { sectionId(it) }.size >= 2

    fun layout(entries: List<ModelPickerRules.Entry>, headers: Boolean): Layout {
        val order = ArrayList<String>()
        val first = HashMap<String, ModelPickerRules.Entry>()
        for (e in entries) {
            val id = sectionId(e)
            if (id !in first) {
                order += id
                first[id] = e
            }
        }
        val grouped = if (headers) order.flatMap { id -> entries.filter { sectionId(it) == id } } else entries
        val items = ArrayList<Item>(grouped.size + order.size)
        val sections = ArrayList<Section>(order.size)
        val sectionOfItem = ArrayList<Int>(grouped.size + order.size)
        for ((ix, id) in order.withIndex()) {
            val lead = first.getValue(id)
            val harness = when (id) {
                FAVORITES -> FAVORITES
                OTHER -> OTHER
                else -> lead.choice.harness
            }
            val label = when (id) {
                FAVORITES -> "Favorites"
                OTHER -> "Other"
                else -> lead.choice.harnessLabel
            }
            val section = Section(id, harness, label, 0)
            val at = items.size
            val real = section.copy(item = if (headers) at else grouped.indexOfFirst { sectionId(it) == id })
            sections += real
            if (headers) {
                items += Item.Header(real)
                sectionOfItem += ix
                for (e in grouped) if (sectionId(e) == id) {
                    items += Item.Row(e)
                    sectionOfItem += ix
                }
            }
        }
        if (!headers) {
            val index = order.withIndex().associate { (i, id) -> id to i }
            for (e in entries) {
                items += Item.Row(e)
                sectionOfItem += index.getValue(sectionId(e))
            }
        }
        return Layout(items, sections, sectionOfItem.toIntArray())
    }

    /**
     * The section to highlight: the one at the top of the list, or the last
     * when the list can scroll no further (a short last section could never
     * reach the top otherwise).
     */
    fun current(layout: Layout, firstVisibleItem: Int, canScrollForward: Boolean): Int {
        if (layout.sections.isEmpty()) return -1
        if (!canScrollForward && layout.items.size > 1 && firstVisibleItem > 0) return layout.sections.lastIndex
        return layout.sectionAt(firstVisibleItem)
    }

    /** The rail slot under a finger [y] px from the rail's top (clamped), for [count] slots of [slotPx] below [padPx] of padding. */
    fun slotAt(y: Float, slotPx: Float, padPx: Float, count: Int): Int {
        if (count <= 0 || slotPx <= 0f) return 0
        return ((y - padPx) / slotPx).toInt().coerceIn(0, count - 1)
    }

    /** The slot height that fits [count] marks in [available] px, between [minPx] and [maxPx]. */
    fun slotHeight(available: Float, count: Int, minPx: Float, maxPx: Float): Float =
        if (count <= 0) maxPx else (available / count).coerceIn(minPx, maxPx)
}

/**
 * Where the rail is drawn: the picker's card clips its content, so the rail,
 * which straddles the card's start edge, lives in the popover's overlay layer.
 * The model list publishes the rail here and where its rows are.
 */
@Stable
class RailHost {
    var content by mutableStateOf<(@Composable () -> Unit)?>(null)
    var top by mutableFloatStateOf(0f)
    var height by mutableFloatStateOf(0f)
    private var overlay: LayoutCoordinates? = null
    private var list: LayoutCoordinates? = null

    fun overlayPlaced(c: LayoutCoordinates) {
        overlay = c
        measure()
    }

    fun listPlaced(c: LayoutCoordinates) {
        list = c
        measure()
    }

    private fun measure() {
        val o = overlay ?: return
        val l = list ?: return
        if (!o.isAttached || !l.isAttached) return
        top = o.localPositionOf(l, Offset.Zero).y
        height = l.size.height.toFloat()
    }
}

/** The popover's overlay layer: draws the published rail straddling the card's start edge, level with the list. */
@Composable
fun BoxScope.RailOverlay(host: RailHost) {
    Box(Modifier.fillMaxSize().onGloballyPositioned { host.overlayPlaced(it) }) {
        val content = host.content ?: return@Box
        Box(
            Modifier
                .align(Alignment.TopStart)
                .offset { IntOffset(0, (host.top + 8.dp.toPx()).roundToInt()) }
                .width(RailWidth),
        ) { content() }
    }
}

/** The rail's available height inside the list, less its margins. */
internal fun railMaxHeight(listHeightPx: Float, density: androidx.compose.ui.unit.Density): Dp =
    with(density) { (listHeightPx - 16.dp.toPx()).coerceAtLeast(0f).toDp() }

/**
 * The provider rail: a slim, translucent, glassy vertical capsule that overhangs
 * the model list's start edge. One mark per section; the section at the top of
 * the list ([selected]) is lit and tapping or scrubbing a finger down the rail
 * jumps to a section. [onSelect] gets the slot and whether it was the initial
 * touch (a tap) or the finger moving onto a new slot.
 */
@Composable
fun ProviderRail(
    sections: List<RailRules.Section>,
    selected: State<Int>,
    onSelect: (index: Int, tap: Boolean) -> Unit,
    maxHeight: Dp,
    modifier: Modifier = Modifier,
) {
    if (sections.isEmpty()) return
    val density = LocalDensity.current
    val dark = LocalDarkTheme.current
    val scheme = MaterialTheme.colorScheme
    val padding = 5.dp
    val slot = with(density) { RailRules.slotHeight((maxHeight - padding * 2).toPx(), sections.size, 26.dp.toPx(), 36.dp.toPx()) }
    val slotDp = with(density) { slot.toDp() }
    val padPx = with(density) { padding.toPx() }
    val currentSelect by rememberUpdatedState(onSelect)
    val index by selected
    val indicator by animateFloatAsState(index.coerceAtLeast(0) * slot, MaterialTheme.motionScheme.fastSpatialSpec(), label = "rail indicator")
    val shape = CircleShape
    val glass = if (dark) Color(0xFF2B2B31) else Color.White
    Column(
        modifier
            .shadow(10.dp, shape, ambientColor = Color.Black.copy(alpha = 0.25f), spotColor = Color.Black.copy(alpha = 0.3f))
            .clip(shape)
            .background(Brush.verticalGradient(listOf(glass.copy(alpha = if (dark) 0.78f else 0.84f), glass.copy(alpha = if (dark) 0.62f else 0.66f))))
            .border(
                0.8.dp,
                Brush.verticalGradient(listOf(Color.White.copy(alpha = if (dark) 0.30f else 0.9f), Color.White.copy(alpha = if (dark) 0.06f else 0.35f))),
                shape,
            )
            .padding(vertical = padding)
            .pointerInput(sections.size, slot) {
                awaitEachGesture {
                    val down = awaitFirstDown(requireUnconsumed = false)
                    down.consume()
                    var at = RailRules.slotAt(down.position.y, slot, 0f, sections.size)
                    currentSelect(at, true)
                    while (true) {
                        val event = awaitPointerEvent()
                        val change = event.changes.firstOrNull { it.id == down.id } ?: break
                        if (!change.pressed) break
                        val next = RailRules.slotAt(change.position.y, slot, 0f, sections.size)
                        if (next != at) {
                            at = next
                            currentSelect(at, false)
                        }
                        change.consume()
                    }
                }
            },
        horizontalAlignment = Alignment.CenterHorizontally,
    ) {
        Box {
            // The lit slot: a soft pill that glides between marks.
            Box(
                Modifier
                    .offset { IntOffset(0, indicator.roundToInt()) }
                    .size(RailWidth - 6.dp, slotDp)
                    .align(Alignment.TopCenter)
                    .clip(shape)
                    .background(scheme.primary.copy(alpha = if (dark) 0.28f else 0.16f)),
            )
            Column(horizontalAlignment = Alignment.CenterHorizontally) {
                sections.forEachIndexed { i, section ->
                    val on = i == index
                    val tint = if (on) scheme.onSurface else scheme.onSurfaceVariant
                    Box(
                        Modifier
                            .size(RailWidth, slotDp)
                            .semantics {
                                contentDescription = section.label
                                this.selected = on
                                role = Role.Tab
                                onClick(label = "Jump to ${section.label}") {
                                    currentSelect(i, true)
                                    true
                                }
                            },
                        contentAlignment = Alignment.Center,
                    ) {
                        when (section.id) {
                            RailRules.FAVORITES -> ZIcon(if (on) ZIcons.StarFilled else ZIcons.Star, null, Modifier.size(18.dp), tint = warningColor())
                            RailRules.OTHER -> ZIcon(ZIcons.More, null, Modifier.size(18.dp), tint = tint)
                            else -> HarnessMark(section.harness, 17.dp, tint = tint)
                        }
                    }
                }
            }
        }
    }
}
