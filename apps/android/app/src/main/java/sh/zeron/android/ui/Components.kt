package sh.zeron.android.ui

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.sync.withLock
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.outlined.Home
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialShapes
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.toShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Shape
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.core.SessionActivity
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.ProjectColors
import uniffi.zeron_core.ChatIndicator
import uniffi.zeron_core.SessionRow
import uniffi.zeron_core.projectColorIndex

/**
 * Grouped-list corners (Android 16 settings style): the group's outer
 * corners are large, the seams between items small.
 */
fun segmentShape(index: Int, count: Int, outer: Dp = 24.dp, inner: Dp = 6.dp): Shape {
    val top = if (index == 0) outer else inner
    val bottom = if (index == count - 1) outer else inner
    return RoundedCornerShape(topStart = top, topEnd = top, bottomStart = bottom, bottomEnd = bottom)
}

/** A section title above a group. */
@Composable
fun SectionHeader(text: String, modifier: Modifier = Modifier, trailing: @Composable (() -> Unit)? = null) {
    Box(modifier.fillMaxWidth().padding(start = 20.dp, end = 12.dp, top = 20.dp, bottom = 8.dp)) {
        Text(
            text,
            style = MaterialTheme.typography.labelLarge,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.align(Alignment.CenterStart),
        )
        trailing?.let { Box(Modifier.align(Alignment.CenterEnd)) { it() } }
    }
}

/**
 * A group of items with segmented corners and 2dp seams.
 */
@Composable
fun SegmentedGroup(
    count: Int,
    modifier: Modifier = Modifier,
    item: @Composable (index: Int, shape: Shape) -> Unit,
) {
    Column(modifier.padding(horizontal = 12.dp)) {
        for (i in 0 until count) {
            item(i, segmentShape(i, count))
            if (i < count - 1) Spacer(Modifier.height(2.dp))
        }
    }
}

/** Project monogram tile, toned by the core's stable project color. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun ProjectTile(name: String?, colorIndex: Int, size: Dp = 40.dp, modifier: Modifier = Modifier) {
    val tone = ProjectColors.color(colorIndex, LocalDarkTheme.current)
    // Expressive shapes: projects are "cookies", projectless sessions a circle.
    val shape = when {
        name == null -> CircleShape
        size < 28.dp -> RoundedCornerShape(size * 0.28f)
        else -> MaterialShapes.Cookie9Sided.toShape()
    }
    Box(
        modifier.size(size).clip(shape).background(tone.copy(alpha = 0.16f)),
        contentAlignment = Alignment.Center,
    ) {
        if (name != null) {
            val fontSize = (size.value * if (size < 28.dp) 0.62f else 0.42f).sp
            Text(
                name.trim().take(1).uppercase(),
                color = tone,
                fontWeight = FontWeight.SemiBold,
                fontSize = fontSize,
                style = androidx.compose.ui.text.TextStyle(
                    lineHeight = fontSize,
                    lineHeightStyle = androidx.compose.ui.text.style.LineHeightStyle(
                        androidx.compose.ui.text.style.LineHeightStyle.Alignment.Center,
                        androidx.compose.ui.text.style.LineHeightStyle.Trim.Both,
                    ),
                ),
            )
        } else {
            sh.zeron.android.design.ZIcon(sh.zeron.android.design.ZIcons.Home, null, Modifier.size(size * 0.5f), tint = tone)
        }
    }
}

fun SessionRow.colorIndex(): Int = (project?.colorIndex ?: projectColorIndex("home")).toInt()

@Composable
fun successColor(): Color = if (LocalDarkTheme.current) Color(0xFF34D399) else Color(0xFF15803D)

/** Hands out frames one at a time: each spinner waiting its turn composes in a frame of its own. */
private object SpinnerQueue {
    private val turn = kotlinx.coroutines.sync.Mutex()
    suspend fun next() = turn.withLock { androidx.compose.runtime.withFrameNanos { } }
}

/**
 * The working indicator of a row. Its shape-morph tables are the costliest thing in a row (a third of a cold
 * list's composition), so the row paints with a still dot, and the spinner takes over a frame later, one spinner per
 * frame. A page that is hidden (see [LocalMotionActive]) drops it again: nothing should animate unseen.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun WorkingSpinner(shape: SessionActivity.Shape) {
    val tone = sessionActivityColor(shape)
    val motion = LocalMotionActive.current
    var spinning by remember { mutableStateOf(false) }
    LaunchedEffect(motion) {
        if (motion) {
            SpinnerQueue.next()
            spinning = true
        } else {
            spinning = false
        }
    }
    Box(Modifier.size(26.dp).semantics(mergeDescendants = true) { contentDescription = shape.description }, contentAlignment = Alignment.Center) {
        if (spinning) LoadingIndicator(Modifier.fillMaxSize(), color = tone)
        else Box(Modifier.size(8.dp).clip(CircleShape).background(tone))
    }
}

/** Live status at the trailing edge of a session row (desktop wording). */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun StatusLabel(row: SessionRow) {
    val activity = SessionActivity.shape(row.indicator, row.runningSubagents, row.pendingCallbacks)
    if (activity != null) {
        WorkingSpinner(activity)
        return
    }
    @Composable
    fun label(text: String, color: Color, dot: Boolean) {
        Row(verticalAlignment = Alignment.CenterVertically) {
            if (dot) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(color))
            } else {
                sh.zeron.android.design.ZIcon(sh.zeron.android.design.ZIcons.Check, null, Modifier.size(15.dp), tint = color)
            }
            Spacer(Modifier.width(5.dp))
            Text(text, style = MaterialTheme.typography.labelLarge, color = color)
        }
    }
    when (row.indicator) {
        ChatIndicator.WORKING -> WorkingSpinner(SessionActivity.Shape.MainRunning)
        ChatIndicator.AWAITING_INPUT -> label("Input", MaterialTheme.colorScheme.primary, dot = true)
        ChatIndicator.ERRORED -> label("Failed", MaterialTheme.colorScheme.error, dot = true)
        ChatIndicator.COMPLETED -> label("Done", successColor(), dot = false)
        ChatIndicator.IDLE -> Row(verticalAlignment = Alignment.CenterVertically) {
            if (row.unseen) {
                Box(Modifier.size(7.dp).clip(CircleShape).background(MaterialTheme.colorScheme.primary))
                Spacer(Modifier.width(5.dp))
            }
            Text(row.timeLabel, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.outline)
        }
    }
}

/** The composer's surface: bright in light (like a sheet of paper), raised in dark. */
@Composable
fun composerContainer(): Color =
    if (LocalDarkTheme.current) MaterialTheme.colorScheme.surfaceContainerHigh else MaterialTheme.colorScheme.surfaceContainerLowest

@Composable
fun chipContainer(): Color =
    if (LocalDarkTheme.current) MaterialTheme.colorScheme.surfaceContainerHighest else MaterialTheme.colorScheme.surfaceContainer

/** Segmented list shapes, with a lone item rounded like a whole group. */
@Composable
fun segmentedShapes(index: Int, count: Int): androidx.compose.material3.ListItemShapes =
    if (count == 1) {
        androidx.compose.material3.ListItemDefaults.shapes(shape = RoundedCornerShape(24.dp))
    } else {
        androidx.compose.material3.ListItemDefaults.segmentedShapes(index, count)
    }
