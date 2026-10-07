package sh.zeron.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.res.stringResource
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import sh.zeron.android.R
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.design.ZeronColors

/**
 * Title-bar update badge, right of the computer chip: theme green on a
 * light-green circle. Arrow = a newer release (tap downloads), ring = the
 * download running, checkmark = downloaded and verified (tap installs;
 * the system still asks). Long-press opens the update screen (version and
 * notes). Hidden when there's nothing newer, and while auto-update is
 * still fetching in the background.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
internal fun UpdateBadge(model: ZeronModel, colors: ZeronColors, modifier: Modifier = Modifier) {
    val badge = model.updateBadge ?: return
    val name = model.updateRelease?.name.orEmpty()
    val progress = model.updateProgress ?: 0f
    val desc = when (badge) {
        ZeronModel.UpdateBadge.AVAILABLE -> stringResource(R.string.badge_available_desc, name)
        ZeronModel.UpdateBadge.DOWNLOADING -> stringResource(R.string.badge_downloading_desc, (progress * 100).toInt())
        ZeronModel.UpdateBadge.READY -> stringResource(R.string.badge_ready_desc, name)
    }
    val details = stringResource(R.string.badge_details)
    val green = colors.success
    Box(
        modifier
            .size(30.dp)
            .clip(CircleShape)
            .background(green.copy(alpha = if (colors.dark) 0.20f else 0.14f))
            .combinedClickable(
                role = Role.Button,
                onLongClickLabel = details,
                onLongClick = { model.showUpdate = true },
                onClick = { model.tapUpdateBadge() },
            )
            .semantics { contentDescription = desc }
            .testTag("update-badge"),
        contentAlignment = Alignment.Center,
    ) {
        when (badge) {
            ZeronModel.UpdateBadge.AVAILABLE -> DownArrow(green, Modifier.size(16.dp))
            ZeronModel.UpdateBadge.DOWNLOADING -> {
                ProgressRing(green, progress, Modifier.size(30.dp))
                DownArrow(green.copy(alpha = 0.75f), Modifier.size(12.dp))
            }
            ZeronModel.UpdateBadge.READY -> Check(green, Modifier.size(16.dp))
        }
    }
}

@Composable
private fun DownArrow(color: Color, modifier: Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val p = Path().apply {
            moveTo(s * 0.5f, s * 0.18f)
            lineTo(s * 0.5f, s * 0.80f)
            moveTo(s * 0.24f, s * 0.54f)
            lineTo(s * 0.5f, s * 0.80f)
            lineTo(s * 0.76f, s * 0.54f)
        }
        drawPath(p, color, style = Stroke(width = s * 0.13f, cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

@Composable
private fun Check(color: Color, modifier: Modifier) {
    Canvas(modifier) {
        val s = size.minDimension
        val p = Path().apply {
            moveTo(s * 0.18f, s * 0.52f)
            lineTo(s * 0.41f, s * 0.75f)
            lineTo(s * 0.82f, s * 0.28f)
        }
        drawPath(p, color, style = Stroke(width = s * 0.13f, cap = StrokeCap.Round, join = StrokeJoin.Round))
    }
}

/** Faint full track plus the done part, clockwise from 12 o'clock. */
@Composable
private fun ProgressRing(color: Color, progress: Float, modifier: Modifier) {
    Canvas(modifier) {
        val stroke = 2.5.dp.toPx()
        val inset = stroke / 2 + 1.dp.toPx()
        val arcSize = androidx.compose.ui.geometry.Size(size.width - inset * 2, size.height - inset * 2)
        val topLeft = Offset(inset, inset)
        drawArc(color.copy(alpha = 0.22f), 0f, 360f, false, topLeft, arcSize, style = Stroke(stroke))
        drawArc(color, -90f, 360f * progress.coerceIn(0.02f, 1f), false, topLeft, arcSize, style = Stroke(stroke, cap = StrokeCap.Round))
    }
}
