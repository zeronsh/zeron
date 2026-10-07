package sh.zeron.android.l10n

import androidx.test.core.app.ApplicationProvider
import android.content.Context
import org.junit.Assert.assertEquals
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import sh.zeron.android.core.AppLanguage
import sh.zeron.android.ui.ElapsedFormat
import sh.zeron.android.ui.RelativeTime

/** Compact times stay compact in Chinese; AppLanguage maps locale tags to its three choices. */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "zh-rCN")
class LocalizedFormatTest {
    private val res get() = ApplicationProvider.getApplicationContext<Context>().resources

    @Test
    fun relativeTimeInChinese() {
        val now = 10_000_000_000L
        assertEquals("刚刚", RelativeTime.label(now - 5_000, now, res))
        assertEquals("34分钟", RelativeTime.label(now - 34 * 60_000, now, res))
        assertEquals("4小时", RelativeTime.label(now - 4 * 3_600_000, now, res))
        assertEquals("2天", RelativeTime.label(now - 2 * 86_400_000, now, res))
    }

    @Test
    fun elapsedInChinese() {
        assertEquals("5秒", ElapsedFormat.format(5, res))
        assertEquals("1分5秒", ElapsedFormat.format(65, res))
        assertEquals("1小时2分", ElapsedFormat.format(3725, res))
        assertEquals("1天1小时", ElapsedFormat.format(90061, res))
    }

    @Test
    @Config(qualifiers = "en")
    fun englishResourcesMatchThePureKotlinForm() {
        val now = 10_000_000_000L
        for (ago in listOf(5_000L, 34 * 60_000L, 4 * 3_600_000L, 2 * 86_400_000L)) {
            assertEquals(RelativeTime.label(now - ago, now), RelativeTime.label(now - ago, now, res))
        }
        for (s in listOf(5L, 65L, 3725L, 90061L)) assertEquals(ElapsedFormat.format(s), ElapsedFormat.format(s, res))
    }

    @Test
    fun languageTagsNormalize() {
        assertEquals(AppLanguage.SYSTEM, AppLanguage.normalize(""))
        assertEquals(AppLanguage.CHINESE, AppLanguage.normalize("zh-Hans-CN"))
        assertEquals(AppLanguage.CHINESE, AppLanguage.normalize("zh-TW,en"))
        assertEquals(AppLanguage.ENGLISH, AppLanguage.normalize("en-GB"))
        assertEquals(AppLanguage.SYSTEM, AppLanguage.normalize("fr"))
    }
}
