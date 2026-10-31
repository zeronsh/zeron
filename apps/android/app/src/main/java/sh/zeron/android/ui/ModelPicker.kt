package sh.zeron.android.ui

import android.view.HapticFeedbackConstants
import androidx.annotation.DrawableRes
import androidx.compose.animation.animateColorAsState
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateDpAsState
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.interaction.collectIsPressedAsState
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxHeight
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.pager.HorizontalPager
import androidx.compose.foundation.pager.PagerState
import androidx.compose.foundation.pager.rememberPagerState
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.OutlinedTextFieldDefaults
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.rememberModalBottomSheetState
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.runtime.snapshotFlow
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.scale
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.layout.boundsInParent
import androidx.compose.ui.layout.onGloballyPositioned
import androidx.compose.ui.platform.LocalConfiguration
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.IntOffset
import androidx.compose.ui.unit.dp
import androidx.compose.ui.util.lerp
import kotlinx.coroutines.async
import kotlinx.coroutines.delay
import kotlinx.coroutines.launch
import sh.zeron.android.design.HarnessMark
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import kotlin.math.abs
import kotlin.math.roundToInt

/** One model in the picker. */
data class ModelPickerRow(val id: String, val label: String, val detail: String?)

/** One provider in the picker: its tab and its models, loaded when the picker opens. */
class ModelPickerTab(
    val harness: String,
    val label: String,
    /** Over the list: what picking here does to the chat. */
    val footer: String? = null,
    val load: suspend () -> List<ModelPickerRow>,
)

/**
 * `block`'s answer, or null after `ms` — even when it can't be cancelled (an
 * FFI call waiting on a host that doesn't answer keeps running, unwatched).
 */
suspend fun <T> within(ms: Long, block: suspend () -> T): T? {
    val answer = kotlinx.coroutines.CoroutineScope(kotlinx.coroutines.Dispatchers.Default).async { runCatching { block() }.getOrNull() }
    return kotlinx.coroutines.withTimeoutOrNull(ms) { answer.await() }
}

/** A list this long gets a search field (OpenCode can offer hundreds). */
private const val SEARCH_THRESHOLD = 12

/**
 * The model picker (iOS `ModelPickerViewController`): a sheet sized to its
 * list. A left-aligned title says where the chat runs; with more than one
 * provider a switcher sits over swipeable pages (the current provider
 * first). Models sit right on the sheet — no cards, no rules — and the
 * current one is held in a soft accent tint with a check.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ModelPickerSheet(
    tabs: List<ModelPickerTab>,
    harness: String,
    model: String?,
    host: String,
    @DrawableRes hostIcon: Int,
    note: String? = null,
    onPick: (harness: String, row: ModelPickerRow) -> Unit,
    onDismiss: () -> Unit,
) {
    val sheet = rememberModalBottomSheetState()
    val scope = rememberCoroutineScope()
    val view = LocalView.current
    val start = tabs.indexOfFirst { it.harness == harness }.coerceAtLeast(0)
    val pager = rememberPagerState(initialPage = start) { tabs.size }
    val rows = remember { mutableStateMapOf<String, List<ModelPickerRow>>() }
    var current by remember { mutableStateOf(harness to model) }
    var picked by remember { mutableStateOf(false) }

    // The current provider first, then the rest, so a swipe lands on a loaded
    // list; a provider that joins later (the host's list arrived) loads then.
    val started = remember { HashSet<String>() }
    LaunchedEffect(tabs.map { it.harness }) {
        for (tab in tabs.sortedBy { it.harness != harness }) {
            if (!started.add(tab.harness)) continue
            launch {
                val seen = HashSet<String>()
                rows[tab.harness] = (within(15_000) { tab.load() } ?: emptyList()).filter { seen.add(it.id) }
            }
        }
    }
    // Changing provider ticks, as the thumb crosses.
    LaunchedEffect(pager) {
        var last = pager.currentPage
        snapshotFlow { pager.currentPage }.collect {
            if (it != last) view.performHapticFeedback(HapticFeedbackConstants.CLOCK_TICK)
            last = it
        }
    }

    fun pick(tab: ModelPickerTab, row: ModelPickerRow) {
        if (picked) return
        picked = true
        current = tab.harness to row.id
        view.performHapticFeedback(HapticFeedbackConstants.CONFIRM)
        onPick(tab.harness, row)
        // A beat to see the selection land.
        scope.launch {
            delay(220)
            sheet.hide()
            onDismiss()
        }
    }

    // Tall enough for the longest provider's list, up to most of the screen.
    val screen = LocalConfiguration.current.screenHeightDp.dp
    val longest = tabs.indices.maxOfOrNull { ix ->
        val list = rows[tabs[ix].harness]
        val notice = if ((tabs[ix].footer ?: if (ix == start) note else null) != null) 72 else 0
        val search = if ((list?.size ?: 0) > SEARCH_THRESHOLD) 64 else 0
        (list?.size ?: 4).coerceAtMost(14) * 66 + notice + search + 24
    } ?: 300
    val pagerHeight by animateDpAsState(longest.dp.coerceIn(260.dp, screen * 0.66f), MaterialTheme.motionScheme.defaultSpatialSpec(), label = "fit")

    ModalBottomSheet(
        onDismissRequest = onDismiss,
        sheetState = sheet,
        containerColor = MaterialTheme.colorScheme.surfaceContainerLow,
    ) {
        // "Model" over where it runs (with one provider, that provider first), and a close button.
        Row(Modifier.fillMaxWidth().padding(start = 24.dp, end = 16.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f).semantics { contentDescription = "Model, $host" }) {
                Text("Model", style = MaterialTheme.typography.headlineSmallEmphasized)
                Spacer(Modifier.height(2.dp))
                Row(verticalAlignment = Alignment.CenterVertically) {
                    val only = tabs.singleOrNull()
                    if (only != null) {
                        HarnessMark(only.harness, 14.dp, tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    } else {
                        ZIcon(hostIcon, null, Modifier.size(14.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Spacer(Modifier.width(6.dp))
                    Text(
                        if (only != null) "${only.label} · $host" else host,
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        maxLines = 1,
                        overflow = TextOverflow.Ellipsis,
                    )
                }
            }
            TonalCircleButton(ZIcons.Close, "Close", onClick = {
                scope.launch {
                    sheet.hide()
                    onDismiss()
                }
            }, size = 40.dp, container = MaterialTheme.colorScheme.surfaceContainerHighest)
        }
        if (tabs.size > 1) {
            ProviderSwitcher(
                tabs,
                pager,
                onSelect = { ix -> scope.launch { pager.animateScrollToPage(ix, animationSpec = androidx.compose.animation.core.spring(dampingRatio = 0.86f, stiffness = 380f)) } },
                modifier = Modifier.padding(start = 20.dp, end = 20.dp, top = 18.dp),
            )
        }
        Spacer(Modifier.height(10.dp))
        HorizontalPager(pager, modifier = Modifier.fillMaxWidth().height(pagerHeight), beyondViewportPageCount = 1, verticalAlignment = Alignment.Top) { ix ->
            val tab = tabs[ix]
            ProviderPage(
                tab,
                rows[tab.harness],
                notice = tab.footer ?: if (ix == start) note else null,
                current = current,
                onPick = { pick(tab, it) },
            )
        }
    }
}

/** One provider's models: a notice over them when there's something to know, and a search field once the list is long. */
@Composable
private fun ProviderPage(
    tab: ModelPickerTab,
    rows: List<ModelPickerRow>?,
    notice: String?,
    current: Pair<String, String?>,
    onPick: (ModelPickerRow) -> Unit,
) {
    var query by remember { mutableStateOf("") }
    val visible = remember(rows, query) {
        val q = query.trim().lowercase()
        if (q.isEmpty()) rows.orEmpty() else rows.orEmpty().filter { it.label.lowercase().contains(q) || it.id.lowercase().contains(q) }
    }
    Column(Modifier.fillMaxSize()) {
        if ((rows?.size ?: 0) > SEARCH_THRESHOLD) {
            OutlinedTextField(
                query,
                { query = it },
                placeholder = { Text("Search ${tab.label} models") },
                leadingIcon = { ZIcon(ZIcons.Search, null, Modifier.size(20.dp)) },
                singleLine = true,
                shape = CircleShape,
                colors = OutlinedTextFieldDefaults.colors(
                    unfocusedContainerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                    focusedContainerColor = MaterialTheme.colorScheme.surfaceContainerHighest,
                    unfocusedBorderColor = Color.Transparent,
                ),
                modifier = Modifier.fillMaxWidth().padding(horizontal = 20.dp, vertical = 4.dp),
            )
        }
        // The current model in view once the list arrives (it may sit below the fold).
        val list = androidx.compose.foundation.lazy.rememberLazyListState()
        LaunchedEffect(rows != null) {
            val at = visible.indexOfFirst { tab.harness == current.first && it.id == current.second }
            if (at > 2) list.scrollToItem(at - 1 + (if (notice != null) 1 else 0))
        }
        LazyColumn(
            state = list,
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 20.dp),
            verticalArrangement = Arrangement.spacedBy(2.dp),
            modifier = Modifier.fillMaxSize(),
        ) {
            notice?.let { text -> item("notice") { Notice(text) } }
            when {
                rows == null -> items(4) { Skeleton(it) }
                visible.isEmpty() -> item("empty") {
                    Text(
                        if (query.isEmpty()) "No models available" else "No matches",
                        style = MaterialTheme.typography.bodyLarge,
                        color = MaterialTheme.colorScheme.outline,
                        modifier = Modifier.fillMaxWidth().padding(top = 40.dp).padding(horizontal = 24.dp),
                    )
                }
                else -> items(visible, key = { it.id }) { row ->
                    ModelRow(row, tab.harness == current.first && row.id == current.second) { onPick(row) }
                }
            }
        }
    }
}

/** A model: its name over a line or two of what it's for. The current one sits in a soft accent tint with a check. */
@Composable
private fun ModelRow(row: ModelPickerRow, current: Boolean, onClick: () -> Unit) {
    val interaction = remember { MutableInteractionSource() }
    val pressed by interaction.collectIsPressedAsState()
    val scheme = MaterialTheme.colorScheme
    val plate by animateColorAsState(
        when {
            current -> scheme.primary.copy(alpha = 0.14f)
            pressed -> scheme.onSurface.copy(alpha = 0.08f)
            else -> Color.Transparent
        },
        MaterialTheme.motionScheme.fastEffectsSpec(),
        label = "plate",
    )
    val scale by animateFloatAsState(if (pressed) 0.985f else 1f, MaterialTheme.motionScheme.fastSpatialSpec(), label = "press")
    val check by animateFloatAsState(if (current) 1f else 0f, MaterialTheme.motionScheme.fastEffectsSpec(), label = "check")
    Surface(
        onClick = onClick,
        interactionSource = interaction,
        shape = RoundedCornerShape(18.dp),
        color = plate,
        modifier = Modifier
            .fillMaxWidth()
            .scale(scale)
            .semantics {
                role = Role.RadioButton
                selected = current
            },
    ) {
        Row(Modifier.padding(start = 14.dp, end = 16.dp, top = 12.dp, bottom = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Column(Modifier.weight(1f)) {
                Text(row.label, style = MaterialTheme.typography.titleMedium, color = scheme.onSurface)
                row.detail?.takeIf { it.isNotEmpty() }?.let {
                    Spacer(Modifier.height(3.dp))
                    Text(it, style = MaterialTheme.typography.bodyMedium, color = scheme.onSurfaceVariant, maxLines = 2, overflow = TextOverflow.Ellipsis)
                }
            }
            Spacer(Modifier.width(12.dp))
            ZIcon(ZIcons.Check, null, Modifier.size(20.dp).alpha(check), tint = scheme.primary)
        }
    }
}

/** What to know before picking ("Your next message continues this chat with Codex"): a quiet tinted line over the list. */
@Composable
private fun Notice(text: String) {
    Surface(shape = RoundedCornerShape(14.dp), color = MaterialTheme.colorScheme.surfaceContainerHighest, modifier = Modifier.fillMaxWidth().padding(bottom = 8.dp)) {
        Row(Modifier.padding(horizontal = 14.dp, vertical = 11.dp)) {
            ZIcon(ZIcons.Info, null, Modifier.size(16.dp).offset(y = 1.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.width(8.dp))
            Text(text, style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        }
    }
}

/** A loading row: two soft bars that breathe until the models arrive. */
@Composable
private fun Skeleton(seed: Int) {
    val pulse = rememberInfiniteTransition(label = "skeleton")
    val alpha by pulse.animateFloat(1f, 0.4f, infiniteRepeatable(tween(900, delayMillis = seed * 120), RepeatMode.Reverse), label = "pulse")
    val bar = MaterialTheme.colorScheme.onSurface.copy(alpha = 0.08f)
    Column(Modifier.fillMaxWidth().padding(horizontal = 14.dp, vertical = 16.dp).alpha(alpha)) {
        Box(Modifier.fillMaxWidth(listOf(0.3f, 0.38f, 0.26f, 0.34f)[seed % 4]).height(13.dp).clip(RoundedCornerShape(5.dp)).background(bar))
        Spacer(Modifier.height(9.dp))
        Box(Modifier.fillMaxWidth(listOf(0.6f, 0.48f, 0.55f, 0.42f)[seed % 4]).height(10.dp).clip(RoundedCornerShape(5.dp)).background(bar))
    }
}

/**
 * A capsule track of provider segments (mark + name) with a thumb that
 * follows the pages under it, finger for finger. When the names don't all
 * fit, the track scrolls and keeps the chosen one in view.
 */
@Composable
private fun ProviderSwitcher(tabs: List<ModelPickerTab>, pager: PagerState, onSelect: (Int) -> Unit, modifier: Modifier = Modifier) {
    val bounds = remember { mutableStateMapOf<Int, Rect>() }
    val scroll = rememberScrollState()
    val density = LocalDensity.current
    // Few enough to share the width equally (iOS fits names when they do).
    val equal = tabs.size <= 3
    val position = pager.currentPage + pager.currentPageOffsetFraction
    LaunchedEffect(pager.currentPage, bounds.size) {
        val r = bounds[pager.currentPage] ?: return@LaunchedEffect
        if (!equal) scroll.animateScrollTo((r.center.x - scroll.viewportSize / 2f).roundToInt().coerceAtLeast(0))
    }
    Box(
        modifier
            .fillMaxWidth()
            .height(44.dp)
            .clip(CircleShape)
            .background(MaterialTheme.colorScheme.surfaceContainerHighest),
    ) {
        Box(Modifier.fillMaxHeight().then(if (equal) Modifier.fillMaxWidth() else Modifier.horizontalScroll(scroll))) {
            // The thumb, between the two segments the pager is between.
            val lower = position.toInt().coerceIn(0, tabs.lastIndex)
            val upper = (lower + 1).coerceAtMost(tabs.lastIndex)
            val t = (position - lower).coerceIn(0f, 1f)
            val a = bounds[lower]
            val b = bounds[upper] ?: a
            if (a != null && b != null) {
                val left = lerp(a.left, b.left, t)
                val width = lerp(a.width, b.width, t)
                Box(
                    Modifier
                        .offset { IntOffset(left.roundToInt(), 0) }
                        .padding(vertical = 4.dp)
                        .width(with(density) { width.toDp() })
                        .fillMaxHeight()
                        .clip(CircleShape)
                        .background(MaterialTheme.colorScheme.surfaceBright),
                )
            }
            Row(Modifier.fillMaxHeight().then(if (equal) Modifier.fillMaxWidth() else Modifier), verticalAlignment = Alignment.CenterVertically) {
                tabs.forEachIndexed { ix, tab ->
                    val emphasis = (1f - abs(position - ix)).coerceIn(0f, 1f)
                    val color = lerp(MaterialTheme.colorScheme.onSurfaceVariant, MaterialTheme.colorScheme.onSurface, emphasis)
                    Row(
                        Modifier
                            .then(if (equal) Modifier.weight(1f) else Modifier)
                            .padding(horizontal = 4.dp)
                            .onGloballyPositioned { bounds[ix] = it.boundsInParent() }
                            .clip(CircleShape)
                            .semantics {
                                role = Role.Tab
                                selected = ix == pager.currentPage
                            }
                            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) { onSelect(ix) }
                            .padding(horizontal = if (equal) 8.dp else 14.dp)
                            .fillMaxHeight(),
                        horizontalArrangement = Arrangement.Center,
                        verticalAlignment = Alignment.CenterVertically,
                    ) {
                        HarnessMark(tab.harness, 16.dp, tint = color)
                        Spacer(Modifier.width(7.dp))
                        // The tab is short on room: "Claude", not "Claude Code".
                        Text(
                            if (tab.label == "Claude Code") "Claude" else tab.label,
                            style = MaterialTheme.typography.labelLarge,
                            color = color,
                            maxLines = 1,
                        )
                    }
                }
            }
        }
    }
}

private fun lerp(a: Color, b: Color, t: Float) = androidx.compose.ui.graphics.lerp(a, b, t)
