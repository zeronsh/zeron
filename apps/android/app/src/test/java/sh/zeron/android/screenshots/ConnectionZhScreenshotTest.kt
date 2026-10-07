package sh.zeron.android.screenshots

import org.robolectric.annotation.Config
import org.robolectric.annotation.GraphicsMode

/** [ConnectionScreenshotTest] in Simplified Chinese; writes to zh/. */
@GraphicsMode(GraphicsMode.Mode.NATIVE)
@Config(sdk = [34], qualifiers = "zh-rCN-w411dp-h891dp-night-xxhdpi")
class ConnectionZhScreenshotTest : ConnectionScreenshotTest() {
    override val subdir = "zh/"
}
