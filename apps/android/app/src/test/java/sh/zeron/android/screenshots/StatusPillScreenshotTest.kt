package sh.zeron.android.screenshots

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.dp
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import sh.zeron.android.design.GlassFrameLayout
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronDark
import sh.zeron.android.design.ZeronMaterialTheme
import sh.zeron.android.ui.ElapsedFormat
import sh.zeron.android.ui.StatusPillView

/** The chat's working timer on the status pill, at a few elapsed times. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h480dp-night-xxhdpi")
class StatusPillScreenshotTest {
    @get:Rule
    val compose = createComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        // No window backdrop here: drop any blur sample another test left behind.
        GlassFrameLayout.backdrop = null
    }

    @Test
    fun workingTimer() {
        val colors = ZeronDark
        val texts = listOf(5L, 65L, 3720L, 90_000L).map { "Working · ${ElapsedFormat.format(it)}" } + "Writing…"
        compose.setContent {
            CompositionLocalProvider(LocalZeronColors provides colors) {
                ZeronMaterialTheme(colors) {
                    Column(Modifier.fillMaxSize().background(colors.background).padding(24.dp)) {
                        texts.forEach { StatusPillView(colors.working, it, colors) {} }
                    }
                }
            }
        }
        compose.waitForIdle()
        compose.onRoot().captureRoboImage(Screenshots.path("08-working-timer.png"))
    }
}
