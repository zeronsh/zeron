package sh.zeron.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.material3.LinearWavyProgressIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.geometry.Size
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.unit.dp
import uniffi.zeron_core.ContextUsage

/** The context window's wording, as the desktop's context ring card (crates/ui/src/context_usage.rs). */
object ContextUsageText {
    /** Only a harness that reports a window gets an indicator (never a permanently empty ring). */
    fun hasWindow(usage: ContextUsage?): Boolean = (usage?.window ?: 0uL) > 0uL

    fun fraction(usage: ContextUsage?): Double? {
        val tokens = usage?.tokens ?: return null
        val window = usage.window?.takeIf { it > 0uL } ?: return null
        return tokens.toDouble() / window.toDouble()
    }

    /** "42%", or a dash while the harness hasn't reported usage. */
    fun percent(usage: ContextUsage?): String = fraction(usage)?.let { "${Math.round(it * 100)}%" } ?: "—"

    fun grouped(n: ULong): String {
        val digits = n.toString()
        return buildString {
            digits.forEachIndexed { i, c ->
                if (i > 0 && (digits.length - i) % 3 == 0) append(',')
                append(c)
            }
        }
    }

    /** Two lines: usage against the window, then what's left (or why it's unknown). */
    fun details(usage: ContextUsage?): List<String> {
        val tokens = usage?.tokens
        val window = usage?.window?.takeIf { it > 0uL }
        return when {
            tokens != null && window != null -> listOf(
                "${grouped(tokens)} / ${grouped(window)} tokens",
                "${grouped(if (tokens >= window) 0uL else window - tokens)} tokens remaining",
            )
            tokens != null -> listOf("${grouped(tokens)} tokens used", "Context limit not reported")
            window != null -> listOf("${grouped(window)} token capacity", "Waiting for context usage")
            else -> listOf("Context usage not reported by this harness yet")
        }
    }

    enum class Level { Normal, Warning, Danger }

    /** The desktop's thresholds: amber from 75%, red from 90%. */
    fun level(usage: ContextUsage?): Level {
        val f = fraction(usage) ?: return Level.Normal
        return when {
            f >= 0.9 -> Level.Danger
            f >= 0.75 -> Level.Warning
            else -> Level.Normal
        }
    }
}

@Composable
private fun levelColor(level: ContextUsageText.Level): Color = when (level) {
    ContextUsageText.Level.Danger -> MaterialTheme.colorScheme.error
    ContextUsageText.Level.Warning -> warningColor()
    ContextUsageText.Level.Normal -> MaterialTheme.colorScheme.onSurfaceVariant
}

/** The composer's context chip: a ring and the percentage; tap for the window's numbers. */
@Composable
fun ContextUsageChip(usage: ContextUsage?) {
    if (!ContextUsageText.hasWindow(usage)) return
    var open by remember { mutableStateOf(false) }
    val level = ContextUsageText.level(usage)
    val color = levelColor(level)
    val fraction = (ContextUsageText.fraction(usage) ?: 0.0).toFloat()
    ContextChip(
        ContextUsageText.percent(usage),
        leading = { Ring(fraction, if (level == ContextUsageText.Level.Normal) MaterialTheme.colorScheme.primary else color) },
        onClick = { open = true },
        tint = color,
    ) {
        AnchoredPopover(open, { open = false }, wide = false) {
            Column(Modifier.width(300.dp).padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                Row(verticalAlignment = Alignment.CenterVertically) {
                    Text("Context window", style = MaterialTheme.typography.titleSmallEmphasized, modifier = Modifier.weight(1f))
                    Text(ContextUsageText.percent(usage), style = MaterialTheme.typography.titleSmallEmphasized, color = if (level == ContextUsageText.Level.Normal) MaterialTheme.colorScheme.onSurface else color)
                }
                LinearWavyProgressIndicator(
                    progress = { fraction.coerceIn(0f, 1f) },
                    color = if (level == ContextUsageText.Level.Normal) MaterialTheme.colorScheme.primary else color,
                    modifier = Modifier.fillMaxWidth(),
                )
                ContextUsageText.details(usage).forEachIndexed { i, line ->
                    Text(
                        line,
                        style = if (i == 0) MaterialTheme.typography.bodyLarge else MaterialTheme.typography.bodyMedium,
                        color = if (i == 0) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
                if (level != ContextUsageText.Level.Normal) {
                    Spacer(Modifier.size(2.dp))
                    Text(
                        "Nearly full — the agent compacts or forgets older turns soon. Start a new session for a fresh window.",
                        style = MaterialTheme.typography.bodySmall,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                }
            }
        }
    }
}

/** A 14dp progress ring from twelve o'clock over a faint track (the desktop footer's ring). */
@Composable
private fun Ring(fraction: Float, color: Color) {
    val track = MaterialTheme.colorScheme.outlineVariant
    Canvas(Modifier.size(14.dp)) {
        val stroke = 2.dp.toPx()
        val inset = stroke / 2
        val arc = Size(size.width - stroke, size.height - stroke)
        drawArc(track, 0f, 360f, false, Offset(inset, inset), arc, style = Stroke(stroke))
        if (fraction > 0f) drawArc(color, -90f, 360f * fraction.coerceIn(0f, 1f), false, Offset(inset, inset), arc, style = Stroke(stroke, cap = StrokeCap.Round))
    }
}
