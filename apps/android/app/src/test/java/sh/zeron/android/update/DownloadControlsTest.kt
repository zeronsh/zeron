package sh.zeron.android.update

import android.app.Application
import android.os.Looper
import androidx.compose.ui.test.junit4.createEmptyComposeRule
import androidx.compose.ui.test.onAllNodesWithText
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performClick
import androidx.lifecycle.ViewModelProvider
import androidx.test.core.app.ActivityScenario
import androidx.test.core.app.ApplicationProvider
import org.junit.Assert.assertEquals
import org.junit.Assert.assertFalse
import org.junit.Assert.assertTrue
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import sh.zeron.android.BuildConfig
import sh.zeron.android.MainActivity
import sh.zeron.android.R
import sh.zeron.android.core.Updater
import sh.zeron.android.core.ZeronModel
import sh.zeron.android.screenshots.FakeAndroidKeyStore
import sh.zeron.android.screenshots.Screenshots
import java.io.File

/**
 * Cancel / switch mirror while an update downloads: the badge opens the
 * download sheet, 换个镜像 (Switch mirror) lists every source with what it did last, and
 * 取消下载 (Cancel download) puts the arrow back, keeping the partial file the first time and
 * deleting it the second.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class DownloadControlsTest {
    @get:Rule
    val compose = createEmptyComposeRule()

    @Before
    fun gate() = Screenshots.assumeHostCore()

    @Test
    fun badgeSheetPickerAndCancel() {
        val app = ApplicationProvider.getApplicationContext<Application>()
        FakeAndroidKeyStore.install()
        app.getSharedPreferences("zeron-update", 0).edit().putLong("lastCheck", System.currentTimeMillis()).putBoolean("autoUpdate", false).commit()
        val scenario = ActivityScenario.launch(MainActivity::class.java)
        lateinit var model: ZeronModel
        scenario.onActivity { model = ViewModelProvider(it)[ZeronModel::class.java] }
        settleUntil("demo workspace") { model.phase == ZeronModel.Phase.Ready && model.workspace != null }
        // The start-up check sweeps the updates cache; let it finish before
        // the test writes a partial download there.
        settleUntil("start-up update check") { !model.updateCheckRunning }

        val release = Updater.Release(
            "round9-9", "round9-9", "", BuildConfig.VERSION_CODE.toLong() + 1,
            "zeron-android-round9-9.apk", "", "https://example.invalid/a.apk", 1_000_000, "", null,
        )
        // The model's own Application: under Robolectric the shared
        // AndroidViewModelFactory can hold an earlier test's, with its own
        // cache dir.
        val part = File(model.getApplication<Application>().cacheDir, "updates/round9-9.apk.part")
        fun downloading() {
            model.updateRelease = release
            model.updateProgress = 0.4f
            model.updateStatus = ZeronModel.UpdateStatus("ghfast.top", 400_000, 1_000_000, 250_000, "https://ghfast.top/")
        }
        downloading()
        model.sourceStats = mapOf(
            "https://ghfast.top/" to ZeronModel.SourceStat(bytesPerSec = 250_000),
            "https://gh-proxy.com/" to ZeronModel.SourceStat(error = app.getString(R.string.update_reason_timeout)),
        )
        settle()
        assertEquals(ZeronModel.UpdateBadge.DOWNLOADING, model.updateBadge)

        // Badge tap -> the download sheet (not the full update screen).
        compose.onNodeWithTag("update-badge").performClick()
        settle()
        assertTrue(model.showDownloadSheet)
        assertFalse(model.showUpdate)
        compose.onNodeWithTag("download-sheet").assertExists()

        // 换个镜像 (Switch mirror) -> every source, the current one ticked, the rest with their history.
        compose.onNodeWithTag("download-switch").performClick()
        settle()
        compose.onNodeWithTag("source-picker").assertExists()
        val choices = model.sourceChoices()
        assertEquals(listOf("GitHub", "ghfast.top", "gh-proxy.com", "gh.llkk.cc"), choices.map { it.source.label })
        assertEquals(listOf(false, true, false, false), choices.map { it.current })
        assertTrue(text(app.getString(R.string.source_current), substring = true) >= 1)
        assertTrue(text(app.getString(R.string.source_failed_last, app.getString(R.string.update_reason_timeout)), substring = true) >= 1)
        assertTrue(text(app.getString(R.string.source_untried), substring = true) >= 2) // GitHub, gh.llkk.cc
        model.showSourcePicker = false

        // First cancel: arrow back, bytes kept for next time. (The part is
        // written only now: the start-up update check may clear the cache dir.)
        part.parentFile!!.mkdirs()
        part.writeBytes(ByteArray(400_000))
        model.showDownloadSheet = true
        settle()
        compose.onNodeWithTag("download-cancel").performClick()
        settle()
        assertEquals(ZeronModel.UpdateBadge.AVAILABLE, model.updateBadge)
        assertFalse(model.showDownloadSheet)
        settleUntil("keep toast") { model.toast != null }
        assertTrue(model.toast!!, model.toast!!.startsWith("Cancelled. The "))
        assertTrue(part.exists())
        assertEquals(400_000L, model.updater.partialBytes(release))

        // Second cancel of the same release: the partial file goes.
        downloading()
        settle()
        model.cancelDownload()
        settleUntil("partial deleted") { !part.exists() }
        assertEquals(app.getString(R.string.download_cancelled_clean), model.toast)
        assertEquals(ZeronModel.UpdateBadge.AVAILABLE, model.updateBadge)
        scenario.close()
    }

    private fun text(s: String, substring: Boolean = false) = compose.onAllNodesWithText(s, substring = substring).fetchSemanticsNodes().size

    private fun settle(ms: Long = 600) {
        val end = System.currentTimeMillis() + ms
        while (System.currentTimeMillis() < end) {
            shadowOf(Looper.getMainLooper()).idleFor(java.time.Duration.ofMillis(50))
            compose.mainClock.advanceTimeBy(50)
            Thread.sleep(20)
        }
        compose.waitForIdle()
    }

    private fun settleUntil(what: String, timeoutMs: Long = 30_000, done: () -> Boolean) {
        val end = System.currentTimeMillis() + timeoutMs
        while (!done()) {
            check(System.currentTimeMillis() < end) { "timed out waiting for $what" }
            settle(200)
        }
    }
}
