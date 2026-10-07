package sh.zeron.android.home

import androidx.compose.foundation.layout.Column
import androidx.compose.material3.Text
import androidx.compose.runtime.CompositionLocalProvider
import androidx.compose.ui.Modifier
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onAllNodesWithTag
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.unit.sp
import org.junit.Assert.assertTrue
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode
import sh.zeron.android.design.LocalZeronColors
import sh.zeron.android.design.ZeronDark
import sh.zeron.android.design.ZeronType
import sh.zeron.android.ui.HomeStatsCapsule

/**
 * The capsule replaces the 28sp bold "会话" / "Sessions" title and must not
 * leave the computer chip less room than that title did, in Chinese (the
 * short title: the tight case) for everyday counts, and in English.
 */
@RunWith(RobolectricTestRunner::class)
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-xxhdpi")
open class HomeStatsCapsuleWidthTest {
    @get:Rule val compose = createComposeRule()

    protected open val oldTitle = "会话"

    @Test fun noWiderThanTheOldTitle() {
        val cases = listOf(0 to 0, 1 to 0, 0 to 1, 1 to 1, 3 to 2, 9 to 9)
        compose.setContent {
            CompositionLocalProvider(LocalZeronColors provides ZeronDark) {
                Column {
                    Text(oldTitle, fontFamily = ZeronType.Sans, fontWeight = FontWeight.Bold, fontSize = 28.sp, maxLines = 1, modifier = Modifier.testTag("old"))
                    for ((r, f) in cases) HomeStatsCapsule(r, f, ZeronDark) {}
                }
            }
        }
        val density = compose.density.density
        val old = compose.onNodeWithTag("old").fetchSemanticsNode().size.width / density
        val capsules = compose.onAllNodesWithTag("home-stats").fetchSemanticsNodes()
        cases.forEachIndexed { i, (r, f) ->
            val w = capsules[i].size.width / density
            // Old: title + 6dp gap before the chip; new: capsule + 4dp gap.
            assertTrue("running $r failed $f: capsule ${w}dp + 4 vs old title ${old}dp + 6", w + 4f <= old + 6f + 0.5f)
        }
    }
}

/** English: "Sessions" was far wider. */
@Config(sdk = [34], qualifiers = "w411dp-h891dp-xxhdpi")
class HomeStatsCapsuleWidthEnTest : HomeStatsCapsuleWidthTest() {
    override val oldTitle = "Sessions"
}
