package sh.zeron.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.clickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType
import uniffi.zeron_core.AgentUsage
import uniffi.zeron_core.ContextUsage

/*
 * The desktop composer footer's ring cluster (crates/ui account_usage.rs +
 * context_usage.rs): a plan-usage ring for the session harness's live
 * account, then the context ring. Each is a 16dp progress ring plus a
 * percent. Here both open the Usage sheet.
 */

/** Harnesses with an Accounts section that report plan usage (desktop `signs_in && reports_usage`). */
private val PlanHarnesses = setOf("claude-code", "codex", "cursor", "grok", "devin", "opencode", "pi", "hermes")

/** Desktop `USAGE_WARN_FRACTION` / `USAGE_CRITICAL_FRACTION`. */
internal const val UsageWarn = 0.80f
internal const val UsageCritical = 0.95f

/**
 * The plan ring's reading: the harness's live account's most-used window
 * (desktop `used_fraction(active_account(..))`), or null when there is no
 * live account with usage to show.
 */
internal fun reportsPlanUsage(harness: String?): Boolean = harness != null && harness in PlanHarnesses

internal fun planFraction(accounts: List<AgentUsage>?, harness: String?): Float? {
    if (!reportsPlanUsage(harness)) return null
    val live = accounts?.firstOrNull { it.harness == harness && it.active } ?: return null
    return live.windows.maxOfOrNull { it.usedFraction.coerceIn(0f, 1f) }
}

/** Desktop `usage_color(usage_level(f))`: accent, amber at 80%, red at 95%. */
internal fun planTone(colors: ZeronColors, fraction: Float): Color = when {
    fraction >= UsageCritical -> colors.danger
    fraction >= UsageWarn -> colors.warning
    else -> colors.accent
}

/** The context ring's fraction, only when the harness reports a window (desktop `has_window`). */
internal fun contextFraction(usage: ContextUsage?): Float? {
    val window = usage?.window?.toLong()?.takeIf { it > 0 } ?: return null
    val tokens = usage.tokens?.toLong() ?: return -1f
    return (tokens.toDouble() / window).coerceIn(0.0, 1.0).toFloat()
}

@Composable
internal fun UsageRings(
    colors: ZeronColors,
    plan: Float?,
    context: ContextUsage?,
    onTap: () -> Unit,
    modifier: Modifier = Modifier,
) {
    val ctx = contextFraction(context)
    if (plan == null && ctx == null) return
    Row(modifier, horizontalArrangement = Arrangement.spacedBy(2.dp), verticalAlignment = Alignment.CenterVertically) {
        if (plan != null) {
            val arc = planTone(colors, plan)
            RingChip(colors, plan, arc, if (plan >= UsageWarn) arc else colors.secondary, "${Math.round(plan * 100)}%", onTap)
        }
        if (ctx != null) {
            val f = ctx.takeIf { it >= 0f }
            val tone = when {
                f == null -> colors.tertiary
                f >= 0.9f -> colors.danger
                f >= 0.75f -> colors.warning
                else -> colors.secondary
            }
            RingChip(colors, f ?: 0f, tone, tone, f?.let { "${Math.round(it * 100)}%" } ?: "—", onTap)
        }
    }
}

/** Desktop `ring_chip`: ring + percent, identical geometry for every ring. */
@Composable
private fun RingChip(colors: ZeronColors, fraction: Float, arc: Color, text: Color, label: String, onTap: () -> Unit) {
    Row(
        Modifier.height(28.dp).clip(RoundedCornerShape(8.dp)).clickable(onClick = onTap).padding(horizontal = 5.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        UsageRing(fraction, arc, colors.tertiary.copy(alpha = 0.25f), Modifier.size(16.dp))
        Spacer(Modifier.width(5.dp))
        Text(label, color = text, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 12.sp, maxLines = 1)
    }
}

/** Desktop `ring`: a faint full track under a [fraction] arc from twelve o'clock. */
@Composable
private fun UsageRing(fraction: Float, color: Color, track: Color, modifier: Modifier) {
    Canvas(modifier) {
        val stroke = 1.8.dp.toPx()
        val r = 6.dp.toPx()
        val topLeft = Offset(center.x - r, center.y - r)
        val box = Size(r * 2, r * 2)
        drawArc(track, 0f, 360f, useCenter = false, topLeft = topLeft, size = box, style = Stroke(stroke))
        val f = fraction.coerceIn(0f, 1f)
        if (f > 0f) drawArc(color, -90f, 360f * f, useCenter = false, topLeft = topLeft, size = box, style = Stroke(stroke, cap = StrokeCap.Butt))
    }
}
