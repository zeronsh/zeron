package sh.zeron.android.ui

import java.text.Collator
import java.util.Locale

/**
 * Order of the New Session project picker within each computer. The engine
 * lists projects in creation order (oldest first); the picker offers:
 *
 * - [byRecent]: most recently used first — the newest `lastActivityMs` among
 *   the project's active sessions, or the project's `createdAtMs` when it has
 *   none. Ties keep the engine's creation order.
 * - [byName]: A–Z, case-insensitive, with Chinese names in pinyin order
 *   (zh collation, whatever the UI language).
 *
 * Generic so the JVM tests don't need the core's records.
 */
object ProjectOrder {
    private val collator: Collator = Collator.getInstance(Locale.SIMPLIFIED_CHINESE).apply { strength = Collator.SECONDARY }

    fun <T> byName(items: List<T>, name: (T) -> String): List<T> =
        items.sortedWith { a, b -> collator.compare(name(a).trim(), name(b).trim()) }

    fun <T> byRecent(items: List<T>, lastUsedMs: (T) -> Long): List<T> =
        items.sortedByDescending(lastUsedMs)
}
