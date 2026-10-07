package sh.zeron.android.ui

import androidx.compose.animation.AnimatedVisibility
import androidx.compose.animation.core.tween
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.scaleIn
import androidx.compose.animation.scaleOut
import androidx.compose.foundation.background
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameMillis
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.R
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import sh.zeron.android.design.glassSurface

/** One of the user's own messages in the transcript: its row key, top (dp), and text. */
data class UserMark(val key: ULong, val y: Float, val text: String)

/**
 * The desktop MessageRail's pure rules (crates/ui/src/rail.rs), for the
 * phone's message navigator: hidden under two marks, at most [MAX_TICKS]
 * ticks (past that, even buckets over the conversation), the active tick is
 * the last message at or above the reading line, previews are one line.
 */
object MessageNav {
    /** rail.rs `MAX_RAIL_TICKS`: the outline keeps a fixed, compact footprint. */
    const val MAX_TICKS = 12
    /** rail.rs: a minimap of one exchange is noise, not navigation. */
    const val MIN_MARKS = 2
    const val PREVIEW_CHARS = 80

    fun visible(marks: Int): Boolean = marks >= MIN_MARKS

    /** rail.rs `tick_buckets`: `[start, end)` ranges; the identity when `n <= capacity`. */
    fun buckets(n: Int, capacity: Int = MAX_TICKS): List<IntRange> {
        if (n <= 0) return emptyList()
        val cap = capacity.coerceIn(1, n)
        return (0 until cap).map { k -> (k * n / cap) until ((k + 1) * n / cap) }
    }

    /** rail.rs `active_tick`: the last mark whose top is at or above [top]; the first before any. */
    fun activeIndex(ys: List<Float>, top: Float): Int {
        if (ys.isEmpty()) return -1
        val i = ys.indexOfLast { it <= top }
        return if (i < 0) 0 else i
    }

    /** rail.rs `truncate_preview`: whitespace runs collapse, then a char cap with an ellipsis. */
    fun preview(text: String, max: Int = PREVIEW_CHARS): String {
        val flat = text.trim().replace(Regex("\\s+"), " ")
        if (flat.length <= max) return flat
        return flat.take(max - 1).trimEnd() + "…"
    }
}

/**
 * The session's message navigator (desktop MessageRail, moved to the right
 * edge for thumbs): a column of short ticks, one per message you sent (or per
 * bucket of them), on a faint glass strip at the right-middle of the
 * transcript. The message being read brightens. Tapping the strip opens a
 * card beside it listing one-line previews; tapping one glides the
 * transcript to that message. Hidden under two messages.
 */
@Composable
fun MessageNavigator(
    marks: List<UserMark>,
    active: Int,
    colors: ZeronColors,
    onPick: (UserMark) -> Unit,
    modifier: Modifier = Modifier,
    initiallyOpen: Boolean = false,
) {
    if (!MessageNav.visible(marks.size)) return
    var open by remember { mutableStateOf(initiallyOpen) }
    val buckets = remember(marks.size) { MessageNav.buckets(marks.size) }
    val activeBucket = buckets.indexOfFirst { active in it }
    val openLabel = stringResource(R.string.msg_nav_open)
    val cardScroll = rememberScrollState()
    // Opening the card parks it on the newest entry: the list is
    // oldest-first and the last row is what you usually came for.
    LaunchedEffect(open, buckets.size) {
        if (!open) return@LaunchedEffect
        withFrameMillis { }
        cardScroll.scrollTo(cardScroll.maxValue)
    }
    Box(modifier.fillMaxSize()) {
        if (open) {
            // Tap anywhere else to put it away.
            Box(
                Modifier.fillMaxSize().clickable(interactionSource = remember { MutableInteractionSource() }, indication = null) { open = false },
            )
        }
        Row(Modifier.align(Alignment.CenterEnd), verticalAlignment = Alignment.CenterVertically) {
            AnimatedVisibility(
                visible = open,
                enter = fadeIn(tween(160)) + scaleIn(tween(200), initialScale = 0.92f, transformOrigin = androidx.compose.ui.graphics.TransformOrigin(1f, 0.5f)),
                exit = fadeOut(tween(120)) + scaleOut(tween(140), targetScale = 0.92f, transformOrigin = androidx.compose.ui.graphics.TransformOrigin(1f, 0.5f)),
            ) {
                Column(
                    Modifier
                        .testTag("msg-nav-card")
                        .width(252.dp)
                        .heightIn(max = 420.dp)
                        .glassSurface(colors, 14.dp)
                        .verticalScroll(cardScroll)
                        .padding(vertical = 6.dp),
                ) {
                    Text(
                        stringResource(R.string.msg_nav_title),
                        color = colors.secondary,
                        fontFamily = ZeronType.Sans,
                        fontWeight = FontWeight.Medium,
                        fontSize = 12.sp,
                        modifier = Modifier.padding(horizontal = 14.dp, vertical = 4.dp),
                    )
                    // One row per message: the strip buckets marks for
                    // space, but the card scrolls — folding them under
                    // "+N more" made most messages unreachable here.
                    marks.forEachIndexed { i, mark ->
                        val isActive = i == active
                        Row(
                            Modifier
                                .fillMaxWidth()
                                .testTag("msg-nav-item")
                                .clickable { open = false; onPick(mark) }
                                .background(if (isActive) colors.text.copy(alpha = 0.06f) else Color.Transparent)
                                .padding(horizontal = 14.dp, vertical = 9.dp),
                            verticalAlignment = Alignment.CenterVertically,
                        ) {
                            Box(
                                Modifier.width(3.dp).height(14.dp).clip(RoundedCornerShape(2.dp))
                                    .background(if (isActive) colors.accent else colors.text.copy(alpha = 0.16f)),
                            )
                            Spacer(Modifier.width(10.dp))
                            Text(
                                MessageNav.preview(mark.text).ifEmpty { stringResource(R.string.msg_nav_attachment) },
                                color = if (isActive) colors.text else colors.text.copy(alpha = 0.8f),
                                fontFamily = ZeronType.Sans,
                                fontWeight = if (isActive) FontWeight.Medium else FontWeight.Normal,
                                fontSize = 14.sp,
                                maxLines = 1,
                                overflow = TextOverflow.Ellipsis,
                                modifier = Modifier.weight(1f),
                            )
                        }
                    }
                }
            }
            Spacer(Modifier.width(6.dp))
            // The strip: 2dp ticks, 12dp wide (the active one 16dp and bright),
            // on a 10dp slot with 3dp gaps (rail.rs TICK_SLOT / TICK_GAP).
            Column(
                Modifier
                    .testTag("msg-nav")
                    .semantics { contentDescription = openLabel }
                    .padding(end = 4.dp)
                    .clip(RoundedCornerShape(10.dp))
                    .background(if (open) colors.text.copy(alpha = 0.08f) else colors.background.copy(alpha = 0.45f))
                    .clickable { open = !open }
                    .padding(horizontal = 6.dp, vertical = 8.dp)
                    .width(16.dp),
                horizontalAlignment = Alignment.End,
                verticalArrangement = Arrangement.spacedBy(3.dp),
            ) {
                buckets.forEachIndexed { b, _ ->
                    Box(Modifier.height(10.dp).fillMaxWidth(), contentAlignment = Alignment.CenterEnd) {
                        val isActive = b == activeBucket
                        Box(
                            Modifier.height(2.dp).width(if (isActive) 16.dp else 12.dp).clip(RoundedCornerShape(1.dp))
                                .background(if (isActive) colors.text.copy(alpha = 0.8f) else colors.text.copy(alpha = 0.22f)),
                        )
                    }
                }
            }
        }
    }
}
