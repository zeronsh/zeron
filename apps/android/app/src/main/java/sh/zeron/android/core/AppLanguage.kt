package sh.zeron.android.core

import android.content.Context
import android.content.res.Configuration
import androidx.annotation.StringRes
import androidx.appcompat.app.AppCompatDelegate
import androidx.compose.runtime.mutableIntStateOf
import androidx.core.os.LocaleListCompat

/**
 * The per-app language (Settings > Language): follow the system, English, or
 * Simplified Chinese. Backed by AndroidX per-app locales, so on Android 13+
 * it is the same setting as the system's per-app language page (see
 * res/xml/locales_config.xml); below 13 AppCompat stores it
 * (AppLocalesMetadataHolderService, autoStoreLocales).
 *
 * The choice is mirrored in prefs so code that runs without an activity
 * (the model's toasts, scheduled-send notifications in the background) can
 * localize through [context] even before AppCompat has restored it.
 */
object AppLanguage {
    const val SYSTEM = ""
    const val ENGLISH = "en"
    const val CHINESE = "zh-CN"

    private const val PREFS = "zeron"
    private const val KEY = "language"

    /**
     * Bumped on every [apply]; composables that show the choice read it so
     * they update in place (the activity is not recreated, see MainActivity).
     */
    val changes = mutableIntStateOf(0)

    /** The stored choice: [SYSTEM], [ENGLISH] or [CHINESE]. */
    fun current(context: Context): String {
        val tags = AppCompatDelegate.getApplicationLocales().toLanguageTags()
        if (tags.isNotEmpty()) return normalize(tags)
        return normalize(context.getSharedPreferences(PREFS, 0).getString(KEY, SYSTEM).orEmpty())
    }

    fun apply(context: Context, tag: String) {
        val value = normalize(tag)
        context.getSharedPreferences(PREFS, 0).edit().putString(KEY, value).apply()
        cached = null
        AppCompatDelegate.setApplicationLocales(
            if (value == SYSTEM) LocaleListCompat.getEmptyLocaleList() else LocaleListCompat.forLanguageTags(value),
        )
        changes.intValue++
    }

    /** [base] with the chosen language applied (itself when following the system). */
    fun context(base: Context): Context {
        val tag = current(base)
        if (tag == SYSTEM) return base
        cached?.let { (t, ctx) -> if (t == tag) return ctx }
        val config = Configuration(base.resources.configuration)
        config.setLocales(android.os.LocaleList.forLanguageTags(tag))
        return base.createConfigurationContext(config).also { cached = tag to it }
    }

    fun string(base: Context, @StringRes id: Int, vararg args: Any): String = context(base).getString(id, *args)

    @Volatile private var cached: Pair<String, Context>? = null

    internal fun normalize(tags: String): String {
        val first = tags.substringBefore(',').trim()
        return when {
            first.isEmpty() -> SYSTEM
            first.startsWith("zh", ignoreCase = true) -> CHINESE
            first.startsWith("en", ignoreCase = true) -> ENGLISH
            else -> SYSTEM
        }
    }
}
