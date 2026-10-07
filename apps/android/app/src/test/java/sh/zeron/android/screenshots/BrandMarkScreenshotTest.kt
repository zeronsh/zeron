package sh.zeron.android.screenshots

import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import com.github.takahirom.roborazzi.captureRoboImage
import org.junit.Before
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import sh.zeron.android.design.BrandMark
import sh.zeron.android.design.GlassFrameLayout
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronLight
import sh.zeron.android.design.ZeronMaterialTheme
import sh.zeron.android.design.ZeronType

/** Every agent mark at row size (20 dp) in a faint square: none may look stretched. */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h400dp-xxhdpi")
class BrandMarkScreenshotTest {
    @get:Rule
    val compose = createComposeRule()

    @Before
    fun gate() {
        Screenshots.assumeEnabled()
        GlassFrameLayout.backdrop = null
    }

    @Test
    fun brandMarks() {
        val colors = ZeronLight
        val harnesses = listOf("claude-code", "codex", "cursor", "devin", "grok", "hermes", "pi", "opencode", "antigravity")
        compose.setContent {
            CompositionLocalProvider(LocalZeronColors provides colors) {
                ZeronMaterialTheme(colors) {
                    Column(Modifier.fillMaxSize().background(colors.background).padding(20.dp), verticalArrangement = Arrangement.spacedBy(10.dp)) {
                        harnesses.forEach { h ->
                            Row(verticalAlignment = Alignment.CenterVertically) {
                                BrandMark(h, colors, 20.dp, Modifier.border(0.5.dp, colors.tertiary.copy(alpha = 0.4f)))
                                Text("  $h", color = colors.secondary, fontFamily = ZeronType.Sans, fontSize = 13.sp)
                            }
                        }
                    }
                }
            }
        }
        compose.waitForIdle()
        compose.onRoot().captureRoboImage(Screenshots.path("11-brand-marks.png"))
    }
}
