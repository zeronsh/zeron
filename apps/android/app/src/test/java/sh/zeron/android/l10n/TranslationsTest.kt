package sh.zeron.android.l10n

import java.io.File
import javax.xml.parsers.DocumentBuilderFactory
import org.junit.Assert.assertEquals
import org.junit.Assert.assertTrue
import org.junit.Test
import org.w3c.dom.Element

/**
 * Every English string has a Simplified Chinese translation (values-zh, which
 * serves all zh locales) with the same format arguments, and no other
 * half-translated locale folder sneaks back in. Plain JVM: parses res/.
 */
class TranslationsTest {
    private val res = listOf(File("src/main/res"), File("app/src/main/res")).first { it.isDirectory }

    private data class Entry(val texts: List<String>)

    private fun load(dir: String): Map<String, Entry> {
        val file = File(res, "$dir/strings.xml")
        val doc = DocumentBuilderFactory.newInstance().newDocumentBuilder().parse(file)
        val out = LinkedHashMap<String, Entry>()
        val root = doc.documentElement
        val nodes = root.childNodes
        for (i in 0 until nodes.length) {
            val el = nodes.item(i) as? Element ?: continue
            if (el.getAttribute("translatable") == "false") continue
            val name = el.getAttribute("name")
            out[name] = when (el.tagName) {
                "string" -> Entry(listOf(el.textContent))
                "plurals" -> {
                    val items = el.getElementsByTagName("item")
                    Entry((0 until items.length).map { items.item(it).textContent })
                }
                else -> continue
            }
        }
        return out
    }

    private fun args(text: String): Set<String> =
        Regex("%(\\d+\\$)?[sdf]").findAll(text).map { it.value }.toSet()

    @Test
    fun everyKeyHasAChineseTranslation() {
        val en = load("values")
        val zh = load("values-zh")
        val missing = en.keys - zh.keys
        val extra = zh.keys - en.keys
        assertTrue("missing in values-zh: $missing", missing.isEmpty())
        assertTrue("only in values-zh (stale?): $extra", extra.isEmpty())
    }

    @Test
    fun formatArgumentsMatch() {
        val en = load("values")
        val zh = load("values-zh")
        for ((name, entry) in en) {
            val want = entry.texts.flatMap { args(it) }.toSet()
            val got = zh[name]?.texts?.flatMap { args(it) }?.toSet() ?: continue
            assertEquals("format args of $name", want, got)
        }
    }

    @Test
    fun noHalfTranslatedLocales() {
        val locales = res.listFiles().orEmpty()
            .filter { it.isDirectory && it.name.startsWith("values-") && File(it, "strings.xml").exists() }
            .map { it.name }
            .sorted()
        assertEquals(listOf("values-zh"), locales)
    }

    @Test
    fun localeConfigListsTheShippedLanguages() {
        val xml = File(res, "xml/locales_config.xml").readText()
        assertTrue(xml.contains("android:name=\"en\""))
        assertTrue(xml.contains("android:name=\"zh-CN\""))
    }
}
