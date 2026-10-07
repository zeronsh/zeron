package sh.zeron.android.ui

import android.os.Looper
import androidx.compose.material3.AlertDialog
import androidx.compose.material3.Text
import androidx.compose.ui.semantics.SemanticsProperties
import androidx.compose.ui.test.assertIsFocused
import androidx.compose.ui.test.junit4.createComposeRule
import androidx.compose.ui.test.onNodeWithTag
import androidx.compose.ui.test.performTextInput
import androidx.compose.ui.text.TextRange
import org.junit.Assert.assertEquals
import org.junit.Rule
import org.junit.Test
import org.junit.runner.RunWith
import org.robolectric.RobolectricTestRunner
import org.robolectric.Shadows.shadowOf
import org.robolectric.annotation.Config
import sh.zeron.android.design.ZeronLight

/**
 * Rename dialogs open ready to type: the field has focus and the whole
 * current name is selected, so typing replaces it.
 */
@RunWith(RobolectricTestRunner::class)
@Config(sdk = [34])
class NameFieldTest {
    @get:Rule
    val compose = createComposeRule()

    @Test
    fun renameDialogFocusesAndSelectsTheName() {
        var latest = ""
        compose.setContent {
            AlertDialog(
                onDismissRequest = {},
                title = { Text("Rename") },
                text = { AutoFocusNameField("Fix flaky test", ZeronLight, onChange = { latest = it }) },
                confirmButton = {},
            )
        }
        compose.waitForIdle()
        shadowOf(Looper.getMainLooper()).idle()
        compose.waitForIdle()
        val field = compose.onNodeWithTag("name-field")
        field.assertIsFocused()
        val selection = field.fetchSemanticsNode().config[SemanticsProperties.TextSelectionRange]
        assertEquals(TextRange(0, "Fix flaky test".length), selection)
        // (Keyboard: Compose raises it through WindowInsetsController, which
        // Robolectric doesn't record; checked on device.)
        // Typing replaces the selected name.
        field.performTextInput("Deflake")
        assertEquals("Deflake", latest)
    }
}
