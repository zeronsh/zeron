package sh.zeron.android.ui

import android.content.res.Resources
import androidx.compose.runtime.Composable
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.compositionLocalOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import kotlinx.coroutines.delay
import sh.zeron.android.R

/**
 * Compact age label for session rows: `now`, `34m`, `4h`, `2d` — a Kotlin copy
 * of the core's `relative_time_label` (crates/client/src/workspace/view.rs),
 * so the row can re-derive it instead of keeping the snapshot's `timeLabel`
 * (which goes stale until the next refresh). Future timestamps read as `now`.
 * With [Resources] the units are localized but stay compact (`34分钟` in Chinese).
 */
object RelativeTime {
    fun label(atMs: Long, nowMs: Long, res: Resources? = null): String {
        val secs = (nowMs - atMs).coerceAtLeast(0) / 1000
        if (res != null) {
            return when {
                secs < 60 -> res.getString(R.string.time_now)
                secs < 3_600 -> res.getString(R.string.time_minutes, (secs / 60).toInt())
                secs < 86_400 -> res.getString(R.string.time_hours, (secs / 3_600).toInt())
                else -> res.getString(R.string.time_days, (secs / 86_400).toInt())
            }
        }
        return when {
            secs < 60 -> "now"
            secs < 3_600 -> "${secs / 60}m"
            secs < 86_400 -> "${secs / 3_600}h"
            else -> "${secs / 86_400}d"
        }
    }
}

/** Wall-clock ms that [RelativeTime] labels are computed against. */
val LocalNow = compositionLocalOf { System.currentTimeMillis() }

/** Provides [LocalNow], refreshed every 30 s while the content is composed. */
@Composable
fun NowProvider(content: @Composable () -> Unit) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(30_000)
            now = System.currentTimeMillis()
        }
    }
    CompositionLocalProvider(LocalNow provides now, content = content)
}
