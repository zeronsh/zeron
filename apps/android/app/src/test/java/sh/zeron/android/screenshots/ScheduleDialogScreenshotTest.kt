package sh.zeron.android.screenshots

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createComposeRule
import com.github.takahirom.roborazzi.ExperimentalRoborazziApi
import com.github.takahirom.roborazzi.captureScreenRoboImage
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
import sh.zeron.android.design.ZeronLight
import sh.zeron.android.ui.ScheduleMode
import sh.zeron.android.ui.ScheduleSendDialog

/** The Schedule send time picker (long-press Send). */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-night-xxhdpi")
open class ScheduleDialogScreenshotTest {
    protected open val subdir: String = ""

    @get:Rule
    val compose = createComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        GlassFrameLayout.backdrop = null
        // The usual case (USE_EXACT_ALARM on 13+): no "exact alarms are off" hint.
        org.robolectric.shadows.ShadowAlarmManager.setCanScheduleExactAlarms(true)
    }

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun schedulePicker() = render(ZeronDark, ScheduleMode.AT, "10-schedule-picker.png")

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun schedulePickerAfter() = render(ZeronDark, ScheduleMode.AFTER, "10b-schedule-picker-after.png")

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun schedulePickerLight() = render(ZeronLight, ScheduleMode.AT, "10c-schedule-picker-light.png")

    @OptIn(ExperimentalRoborazziApi::class)
    @Test
    fun schedulePickerAfterLight() = render(ZeronLight, ScheduleMode.AFTER, "10d-schedule-picker-after-light.png")

    @OptIn(ExperimentalRoborazziApi::class)
    private fun render(colors: sh.zeron.android.design.ZeronColors, mode: ScheduleMode, name: String) {
        compose.setContent {
            CompositionLocalProvider(LocalZeronColors provides colors) {
                ZeronMaterialTheme(colors) {
                    Box(Modifier.fillMaxSize().background(colors.background))
                    ScheduleSendDialog(colors, initialMode = mode, onDismiss = {}) { }
                }
            }
        }
        compose.waitForIdle()
        captureScreenRoboImage(Screenshots.path(subdir + name))
    }
}
