package sh.zeron.android.core

import java.time.Duration
import java.time.Instant
import java.time.ZoneId
import java.time.format.DateTimeFormatter
import java.util.Locale
import kotlin.math.roundToInt

/** The desktop account card's binding limit and reset wording. */
object AccountUsage {
    fun fraction(accounts: Agents.Accounts?, harness: String): Float? = accounts
        ?.forHarness(harness)?.firstOrNull { it.active }
        ?.usageWindows?.maxOfOrNull { it.usedFraction.coerceIn(0f, 1f) }

    fun percent(fraction: Float): String = "${(fraction * 100).roundToInt()}% used"

    fun reset(
        value: String?,
        now: Instant = Instant.now(),
        zone: ZoneId = ZoneId.systemDefault(),
        locale: Locale = Locale.getDefault(),
    ): String? {
        val at = value?.let { runCatching { Instant.parse(it) }.getOrNull() } ?: return null
        val hours = Duration.between(now, at).toHours()
        val pattern = when {
            hours < 22 -> "h:mm a"
            hours < 24 * 7 -> "EEE"
            else -> "MMM d"
        }
        return "resets ${DateTimeFormatter.ofPattern(pattern, locale).format(at.atZone(zone))}"
    }
}
