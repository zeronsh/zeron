package sh.zeron.android.ui

import org.junit.Assert.assertEquals
import org.junit.Test

class ProjectOrderTest {
    @Test
    fun nameIsCaseInsensitive() {
        assertEquals(listOf("apple", "Banana", "cherry", "Zeron_Villa"), ProjectOrder.byName(listOf("Zeron_Villa", "cherry", "Banana", "apple")) { it })
    }

    @Test
    fun chineseNamesSortByPinyin() {
        // a-li, bei-jing, zhong-wen
        assertEquals(listOf("阿里", "北京", "中文"), ProjectOrder.byName(listOf("中文", "阿里", "北京")) { it })
    }

    @Test
    fun recentFirstAndTiesKeepCreationOrder() {
        val items = listOf("old" to 10L, "a" to 50L, "b" to 50L, "new" to 90L)
        assertEquals(listOf("new", "a", "b", "old"), ProjectOrder.byRecent(items) { it.second }.map { it.first })
    }
}
