package sh.zeron.android.ui

import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.material3.IconButton
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.MutableState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalFocusManager
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons

/** How a long picker orders its rows; each picker offers its own subset. */
enum class PickerSort(val label: String) {
    /** The source's own order (catalog, starring, menu order). */
    Default("Suggested"),
    Name("A–Z"),
    /** Most recent activity first. */
    Recent("Recent"),
    /** Most recently created first. */
    Newest("Newest"),
}

/**
 * The search-and-sort rules every long picker shares (JVM-tested): which
 * rows match a query and in what order.
 */
object PickerSearch {
    /** Menus with at least this many choices open as a searchable sheet. */
    const val SEARCH_AT = 8

    /**
     * How well [query] matches a row, lower is better, or null for no match.
     * Every word of the query must match: in [primary] (whole, prefix, word
     * start, anywhere, ignoring punctuation, then as a subsequence) or in any
     * of [secondary] (anywhere).
     */
    fun score(query: String, primary: String, secondary: List<String?> = emptyList()): Int? {
        val words = query.lowercase().split(Regex("\\s+")).filter { it.isNotEmpty() }
        if (words.isEmpty()) return 0
        val name = primary.lowercase()
        val bare = name.alnum()
        val others = secondary.mapNotNull { it?.lowercase() }
        var total = 0
        for (word in words) {
            val w = word.alnum()
            total += when {
                name == word -> 0
                name.startsWith(word) -> 1
                wordStarts(name, word) -> 2
                name.contains(word) -> 3
                w.isNotEmpty() && bare.contains(w) -> 3
                others.any { it.contains(word) } -> 4
                w.length >= 3 && isSubsequence(w, bare) -> 6
                else -> return null
            }
        }
        return total
    }

    /** The rows matching [query], best first; ties keep their order in [rows]. */
    fun <T> filter(rows: List<T>, query: String, primary: (T) -> String, secondary: (T) -> List<String?> = { emptyList() }): List<T> {
        if (query.isBlank()) return rows
        return rows.mapIndexedNotNull { i, row -> score(query, primary(row), secondary(row))?.let { Triple(it, i, row) } }
            .sortedWith(compareBy({ it.first }, { it.second }))
            .map { it.third }
    }

    /**
     * [rows] in [sort] order; Default keeps them as given. [recent] and
     * [created] are millisecond timestamps (absent ones sort last).
     */
    fun <T> sorted(
        rows: List<T>,
        sort: PickerSort,
        name: (T) -> String,
        recent: ((T) -> Long?)? = null,
        created: ((T) -> Long?)? = null,
    ): List<T> = when (sort) {
        PickerSort.Default -> rows
        PickerSort.Name -> rows.sortedWith(compareBy(String.CASE_INSENSITIVE_ORDER, name))
        PickerSort.Recent -> recent?.let { at -> rows.sortedByDescending { at(it) ?: Long.MIN_VALUE } } ?: rows
        PickerSort.Newest -> created?.let { at -> rows.sortedByDescending { at(it) ?: Long.MIN_VALUE } } ?: rows
    }

    /**
     * A long menu section's rows: fixed choices hold their place at either
     * end, the rest take [sort]; [query] filters (and ranks) each part on its
     * own, so a matching fixed row stays at its end.
     */
    fun arrange(choices: List<MenuChoice>, query: String, sort: PickerSort): List<MenuChoice> {
        val head = choices.takeWhile { it.fixed }
        val tail = choices.drop(head.size).takeLastWhile { it.fixed }
        val middle = choices.subList(head.size, choices.size - tail.size)
        fun match(rows: List<MenuChoice>) = filter(rows, query, { it.label }, { listOf(it.supporting) })
        return match(head) + match(sorted(middle, sort, { it.label })) + match(tail)
    }

    /** The sort after [current] in [options], wrapping around. */
    fun next(current: PickerSort, options: List<PickerSort>): PickerSort =
        options.getOrNull((options.indexOf(current) + 1) % options.size.coerceAtLeast(1)) ?: current

    private fun String.alnum() = filter { it.isLetterOrDigit() }

    private fun wordStarts(text: String, word: String): Boolean {
        var at = text.indexOf(word, 1)
        while (at > 0) {
            if (!text[at - 1].isLetterOrDigit()) return true
            at = text.indexOf(word, at + 1)
        }
        return false
    }

    private fun isSubsequence(needle: String, hay: String): Boolean {
        var i = 0
        for (c in hay) if (i < needle.length && needle[i] == c) i++
        return i == needle.length
    }
}

/**
 * A picker's sort, remembered device-locally per picker ([key]) so a list
 * opens the way it was last left.
 */
@Composable
fun rememberPickerSort(key: String, options: List<PickerSort>): MutableState<PickerSort> {
    val prefs = LocalContext.current.getSharedPreferences("pickers", 0)
    val state = remember(key) {
        val saved = prefs.getString("sort:$key", null)?.let { name -> PickerSort.entries.firstOrNull { it.name == name } }
        mutableStateOf(saved?.takeIf { it in options } ?: options.first())
    }
    return remember(state) {
        object : MutableState<PickerSort> by state {
            override var value: PickerSort
                get() = state.value
                set(v) {
                    state.value = v
                    prefs.edit().putString("sort:$key", v.name).apply()
                }
        }
    }
}

/**
 * The search pill at a picker's foot — where a thumb already is, and right
 * above the keyboard once it opens — with the sort toggle beside it.
 */
@Composable
fun PickerSearchBar(
    query: String,
    onQuery: (String) -> Unit,
    placeholder: String,
    modifier: Modifier = Modifier,
    sort: PickerSort? = null,
    sorts: List<PickerSort> = emptyList(),
    onSort: (PickerSort) -> Unit = {},
) {
    val focus = LocalFocusManager.current
    Row(modifier.fillMaxWidth(), verticalAlignment = Alignment.CenterVertically) {
        Surface(shape = RoundedCornerShape(50), color = MaterialTheme.colorScheme.surfaceContainerHighest, modifier = Modifier.weight(1f)) {
            Row(Modifier.heightIn(min = 48.dp).padding(start = 16.dp, end = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                ZIcon(ZIcons.Search, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                Spacer(Modifier.width(10.dp))
                Box(Modifier.weight(1f).padding(vertical = 12.dp)) {
                    if (query.isEmpty()) {
                        Text(placeholder, style = MaterialTheme.typography.bodyLarge, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1)
                    }
                    BasicTextField(
                        query,
                        onQuery,
                        singleLine = true,
                        textStyle = MaterialTheme.typography.bodyLarge.copy(color = MaterialTheme.colorScheme.onSurface),
                        cursorBrush = SolidColor(MaterialTheme.colorScheme.primary),
                        keyboardOptions = KeyboardOptions(imeAction = ImeAction.Search),
                        keyboardActions = KeyboardActions(onSearch = { focus.clearFocus() }),
                        modifier = Modifier.fillMaxWidth().semantics { contentDescription = placeholder },
                    )
                }
                if (query.isNotEmpty()) {
                    IconButton(onClick = { onQuery("") }, modifier = Modifier.size(40.dp)) {
                        ZIcon(ZIcons.Close, "Clear search", Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                } else {
                    Spacer(Modifier.width(12.dp))
                }
            }
        }
        if (sort != null && sorts.size > 1) {
            Spacer(Modifier.width(8.dp))
            Surface(
                onClick = { onSort(PickerSearch.next(sort, sorts)) },
                shape = RoundedCornerShape(50),
                color = MaterialTheme.colorScheme.surfaceContainerHighest,
                modifier = Modifier.semantics { contentDescription = "Sort: ${sort.label}" },
            ) {
                Row(Modifier.heightIn(min = 48.dp).padding(horizontal = 14.dp), verticalAlignment = Alignment.CenterVertically) {
                    ZIcon(ZIcons.Sort, null, Modifier.size(18.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant)
                    Spacer(Modifier.width(6.dp))
                    Text(sort.label, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurface)
                }
            }
        }
    }
}
