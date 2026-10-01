package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import sh.zeron.android.feedback.feedbackClickable
import sh.zeron.android.feedback.OpenCloseFeedback
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.Cue
import androidx.compose.animation.animateContentSize
import androidx.compose.animation.core.FastOutSlowInEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.animateFloatAsState
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.items
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Badge
import androidx.compose.material3.BadgedBox
import androidx.compose.material3.ExperimentalMaterial3Api
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.State
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.lerp
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.SubagentGroups
import uniffi.zeron_core.SubagentState
import uniffi.zeron_core.SubagentView

/** The activity colour: what a running turn's indicator is drawn in. */
@Composable
fun activityColor(): Color = MaterialTheme.colorScheme.primary

/**
 * 0.45 → 1 → 0.45 while [active] (and the page showing it is: see [LocalMotionActive]); a steady 1 otherwise.
 * A State, so a caller reads it in a draw block and the animation never recomposes anything.
 */
@Composable
private fun breath(active: Boolean): State<Float> {
    if (!active || !LocalMotionActive.current) return remember { mutableFloatStateOf(1f) }
    val transition = rememberInfiniteTransition(label = "breath")
    return transition.animateFloat(
        initialValue = 0.45f,
        targetValue = 1f,
        animationSpec = infiniteRepeatable(tween(1100, easing = FastOutSlowInEasing), RepeatMode.Reverse),
        label = "breath",
    )
}

/** Wall clock, ticking while [live] (running ages). */
@Composable
fun rememberNow(live: Boolean, periodMs: Long = 1_000): Long {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(live) {
        now = System.currentTimeMillis()
        while (live) {
            delay(periodMs)
            now = System.currentTimeMillis()
        }
    }
    return now
}

/**
 * "● N" — subagents running. Drawn beside a row's own status (never in its
 * place), also when the parent's turn is done. Digits in Geist Mono; the
 * dot breathes in the activity colour.
 */
@Composable
fun RunningPill(count: Int, modifier: Modifier = Modifier) {
    if (count <= 0) return
    val tone = activityColor()
    val pulse = breath(true)
    Surface(
        shape = RoundedCornerShape(50),
        color = tone.copy(alpha = 0.12f),
        contentColor = tone,
        modifier = modifier.semantics { contentDescription = "$count subagents running" },
    ) {
        Row(
            Modifier.heightIn(min = 20.dp).padding(start = 7.dp, end = 8.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Box(Modifier.size(6.dp).graphicsLayer { alpha = pulse.value }.clip(CircleShape).background(tone))
            Spacer(Modifier.width(4.dp))
            Text(
                Subagents.countLabel(count),
                fontFamily = GeistMono,
                fontWeight = FontWeight.Medium,
                fontSize = 12.sp,
                lineHeight = 14.sp,
            )
        }
    }
}

/**
 * The session header's subagents button: a count badge while any run, and
 * the button's face breathing in the activity colour so they can be found
 * with the panel closed.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SubagentsButton(running: Int, onClick: () -> Unit) {
    val scheme = MaterialTheme.colorScheme
    val pulse by breath(running > 0)
    val face = if (running > 0) lerp(scheme.surfaceContainerHighest, scheme.primaryContainer, pulse) else scheme.surfaceContainerHighest
    BadgedBox(
        badge = {
            if (running > 0) {
                Badge(containerColor = activityColor(), contentColor = scheme.onPrimary) {
                    Text(Subagents.countLabel(running), fontFamily = GeistMono, fontWeight = FontWeight.Medium, fontSize = 10.sp)
                }
            }
        },
    ) {
        TonalCircleButton(
            ZIcons.Bot,
            if (running > 0) "Subagents, $running running" else "Subagents",
            onClick = onClick,
            container = face,
            content = if (running > 0) scheme.onPrimaryContainer else scheme.onSurface,
        )
    }
}

/** A subagent's status at the head of its row. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SubagentGlyph(state: SubagentState, modifier: Modifier = Modifier) {
    Box(modifier.size(28.dp), contentAlignment = Alignment.Center) {
        when (state) {
            SubagentState.RUNNING -> LoadingIndicator(Modifier.size(28.dp))
            SubagentState.PENDING -> Box(Modifier.size(8.dp).clip(CircleShape).background(MaterialTheme.colorScheme.outline))
            SubagentState.COMPLETED -> ZIcon(ZIcons.Check, "Completed", Modifier.size(18.dp), tint = successColor())
            SubagentState.FAILED -> ZIcon(ZIcons.Warning, "Failed", Modifier.size(18.dp), tint = MaterialTheme.colorScheme.error)
        }
    }
}

/**
 * The Subagents panel for one chat: running rows on top, then Finished
 * (closed by default) with Completed and Failed inside, paging at ten.
 * Nothing is hidden: rows are the transcript's spawn chips.
 */
@OptIn(ExperimentalMaterial3Api::class, ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SubagentsSheet(groups: SubagentGroups, onOpen: (SubagentView) -> Unit, onDismiss: () -> Unit, initial: SubagentPanelState = SubagentPanelState()) {
    var state by remember { mutableStateOf(initial) }
    val running = groups.running.toInt()
    val now = rememberNow(running > 0)
    val slots = remember(groups, state) { Subagents.slots(groups, state) }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        OpenCloseFeedback()
        Row(
            Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, bottom = 4.dp),
            verticalAlignment = Alignment.CenterVertically,
        ) {
            Text("Subagents", style = MaterialTheme.typography.titleLargeEmphasized)
            Spacer(Modifier.width(10.dp))
            if (running > 0) {
                RunningPill(running)
            } else {
                Text(
                    "${Subagents.total(groups)}",
                    fontFamily = GeistMono,
                    style = MaterialTheme.typography.titleMedium,
                    color = MaterialTheme.colorScheme.onSurfaceVariant,
                )
            }
        }
        val runsIn = listOfNotNull(groups.harnessLabel, groups.modelLabel).joinToString(" · ")
        if (runsIn.isNotEmpty()) {
            Text(
                runsIn,
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
        }
        Spacer(Modifier.height(8.dp))
        if (slots.isEmpty()) {
            Text(
                "No subagents in this chat yet.",
                style = MaterialTheme.typography.bodyLarge,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(24.dp),
            )
        }
        LazyColumn(
            contentPadding = PaddingValues(start = 12.dp, end = 12.dp, bottom = 24.dp),
            modifier = Modifier.navigationBarsPadding().animateContentSize(),
        ) {
            items(slots, key = { it.key }) { slot ->
                when (slot) {
                    is SubagentSlot.Item -> SubagentRow(slot.view, slot.nested, now) { onOpen(slot.view) }
                    is SubagentSlot.Header -> GroupHeader(slot) { state = state.toggled(slot.group) }
                    is SubagentSlot.ShowMore -> TextButton(
                        onClick = tapAction { state = state.pagedUp(slot.group) },
                        modifier = Modifier.padding(start = 48.dp).animateItem(),
                    ) { Text(slot.label) }
                }
            }
        }
    }
}

@Composable
private fun GroupHeader(slot: SubagentSlot.Header, onToggle: () -> Unit) {
    val turn by animateFloatAsState(if (slot.open) 90f else 0f, MaterialTheme.motionScheme.defaultSpatialSpec(), label = "chevron")
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .feedbackClickable(Haptic.Tick, if (slot.open) Cue.Close else Cue.Open, onClickLabel = if (slot.open) "Collapse" else "Expand", onClick = onToggle)
            .padding(start = if (slot.nested) 28.dp else 12.dp, end = 12.dp)
            .heightIn(min = 44.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        ZIcon(ZIcons.ChevronRight, null, Modifier.size(16.dp).rotate(turn), tint = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.width(10.dp))
        Text(
            slot.group.label,
            style = if (slot.nested) MaterialTheme.typography.titleSmall else MaterialTheme.typography.titleSmallEmphasized,
            color = if (slot.nested) MaterialTheme.colorScheme.onSurfaceVariant else MaterialTheme.colorScheme.primary,
        )
        Spacer(Modifier.width(6.dp))
        Text(
            "(${slot.count})",
            fontFamily = GeistMono,
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
        )
    }
}

@Composable
private fun SubagentRow(view: SubagentView, nested: Boolean, now: Long, onClick: () -> Unit) {
    Row(
        Modifier
            .fillMaxWidth()
            .clip(RoundedCornerShape(16.dp))
            .clickable(onClickLabel = "Open subagent", onClick = onClick)
            .padding(start = if (nested) 28.dp else 8.dp, end = 8.dp, top = 8.dp, bottom = 8.dp)
            .semantics(mergeDescendants = true) {},
        verticalAlignment = Alignment.CenterVertically,
    ) {
        SubagentGlyph(view.state)
        Spacer(Modifier.width(12.dp))
        Column(Modifier.weight(1f)) {
            Text(
                view.title,
                style = MaterialTheme.typography.titleMedium,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
            Text(
                Subagents.subtitle(view, now),
                style = MaterialTheme.typography.bodyMedium,
                color = if (view.state == SubagentState.FAILED) MaterialTheme.colorScheme.error else MaterialTheme.colorScheme.onSurfaceVariant,
                maxLines = 1,
                overflow = TextOverflow.Ellipsis,
            )
        }
        ZIcon(ZIcons.ChevronRight, null, Modifier.size(16.dp), tint = MaterialTheme.colorScheme.outline)
    }
}

/** The subagents of [chatId] by their spawn chips, as Rust groups them. */
fun groupsOf(core: uniffi.zeron_core.CoreClient, chatId: String): SubagentGroups =
    runCatching { core.subagents(chatId) }.getOrElse {
        SubagentGroups(emptyList(), emptyList(), emptyList(), 0u, null, null, null)
    }
