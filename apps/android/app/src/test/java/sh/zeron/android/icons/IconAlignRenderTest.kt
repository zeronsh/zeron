package sh.zeron.android.icons

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.RowScope
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.width
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onRoot
import androidx.compose.ui.text.font.FontWeight
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
import sh.zeron.android.design.TitleLineMark
import sh.zeron.android.design.ZeronLight
import sh.zeron.android.design.ZeronType
import sh.zeron.android.screenshots.Screenshots
import java.io.File

/**
 * Agent marks beside a session title, in the home row's exact geometry
 * (20dp mark, 14dp gap, title line 10..32dp in a 62dp row), one row per
 * harness on white at xxhdpi, for pixel measurement of where each mark
 * sits against the title's caps / x-height / line box. Writes
 * icon-align/rows-<variant>.png and the title's text layout metrics (px)
 * as JSON; icon-align/measure.py in the renders dir reads the PNGs.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class IconAlignRenderTest {
    @get:Rule
    val compose = createComposeRule()

    @Before
    fun gate() = Screenshots.assumeEnabled()

    private val harnesses = listOf("claude-code", "codex", "cursor", "devin", "opencode", "hermes", "grok", "pi", "antigravity")

    @Composable
    private fun Rows(mark: @Composable RowScope.(String) -> Unit, metrics: StringBuilder?) {
        Column(Modifier.background(Color.White)) {
            harnesses.forEachIndexed { i, h ->
                Row(Modifier.fillMaxWidth().height(62.dp).padding(horizontal = 20.dp)) {
                    mark(h)
                    Spacer(Modifier.width(14.dp))
                    Column(Modifier.weight(1f).align(Alignment.Top).padding(top = 10.dp)) {
                        Row(Modifier.height(22.dp), verticalAlignment = Alignment.CenterVertically) {
                            for (sample in listOf("HHH", "xxx")) {
                                Box(Modifier.width(56.dp)) { Title(sample, if (i == 0) metrics?.takeIf { sample == "HHH" } else null) }
                            }
                            Title("Fix flaky test")
                        }
                    }
                }
            }
        }
    }

    @Composable
    private fun Title(text: String, metrics: StringBuilder? = null) {
        Text(
            text, color = Color.Black, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Medium, fontSize = 16.5.sp, maxLines = 1,
            onTextLayout = { l ->
                metrics?.apply {
                    setLength(0)
                    append("{\"height\":${l.size.height},\"lineTop\":${l.getLineTop(0)},\"lineBottom\":${l.getLineBottom(0)},\"baseline\":${l.firstBaseline}}")
                }
            },
        )
    }

    private fun render(variant: String, mark: @Composable RowScope.(String) -> Unit) {
        val metrics = StringBuilder()
        compose.setContent { Rows(mark, metrics) }
        compose.waitForIdle()
        compose.onRoot().captureRoboImage(Screenshots.path("icon-align/rows-$variant.png"))
        File(Screenshots.path("icon-align/metrics-$variant.json")).writeText(metrics.toString())
    }

    @Test
    fun before() = render("before") { h ->
        // As shipped: 20dp slot at top 11dp, i.e. centred on the 22dp line box.
        BrandMark(h, ZeronLight, 20.dp, Modifier.align(Alignment.Top).padding(top = 11.dp), fitInk = false)
    }

    @Test
    fun after() = render("after") { h ->
        // The row's code: ink-fitted 18dp, optically centred on the title.
        TitleLineMark(h, ZeronLight, 16.5.sp, FontWeight.Medium, lineTop = 10.dp, lineHeight = 22.dp, modifier = Modifier.align(Alignment.Top))
    }
}
