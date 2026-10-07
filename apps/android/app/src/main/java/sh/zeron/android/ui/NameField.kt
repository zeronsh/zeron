package sh.zeron.android.ui

import android.view.WindowManager
import androidx.compose.foundation.text.BasicTextField
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Modifier
import androidx.compose.ui.focus.FocusRequester
import androidx.compose.ui.focus.focusRequester
import androidx.compose.ui.graphics.SolidColor
import androidx.compose.ui.platform.LocalSoftwareKeyboardController
import androidx.compose.ui.platform.LocalView
import androidx.compose.ui.window.DialogWindowProvider
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.TextRange
import androidx.compose.ui.text.TextStyle
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.text.input.KeyboardCapitalization
import androidx.compose.ui.text.input.TextFieldValue
import androidx.compose.ui.unit.sp
import sh.zeron.android.design.ZeronColors
import sh.zeron.android.design.ZeronType

/**
 * The text field of a rename / name dialog: focused as soon as it shows,
 * keyboard up, and the current name selected so typing replaces it (a tap
 * puts the caret back for an edit). Done on the keyboard confirms.
 */
@Composable
internal fun AutoFocusNameField(
    initial: String,
    colors: ZeronColors,
    onChange: (String) -> Unit,
    modifier: Modifier = Modifier,
    onDone: (() -> Unit)? = null,
) {
    var value by remember { mutableStateOf(TextFieldValue(initial, selection = TextRange(0, initial.length))) }
    val focus = remember { FocusRequester() }
    val keyboard = LocalSoftwareKeyboardController.current
    val view = LocalView.current
    BasicTextField(
        value = value,
        onValueChange = {
            value = it
            onChange(it.text)
        },
        textStyle = TextStyle(color = colors.text, fontFamily = ZeronType.Sans, fontSize = 16.sp),
        cursorBrush = SolidColor(colors.accent),
        singleLine = true,
        keyboardOptions = KeyboardOptions(capitalization = KeyboardCapitalization.Sentences, imeAction = ImeAction.Done),
        keyboardActions = KeyboardActions(onDone = { onDone?.invoke() }),
        modifier = modifier.focusRequester(focus).testTag("name-field"),
    )
    LaunchedEffect(Unit) {
        // After the first frame: a dialog's window must be attached before
        // it can take focus and raise the keyboard.
        withFrameNanos { }
        // In a dialog, also ask its window to keep the keyboard up (some
        // IMEs ignore a show request made while the window is appearing).
        (view.parent as? DialogWindowProvider)?.window?.let { w ->
            val adjust = w.attributes.softInputMode and WindowManager.LayoutParams.SOFT_INPUT_MASK_ADJUST
            w.setSoftInputMode(adjust or WindowManager.LayoutParams.SOFT_INPUT_STATE_ALWAYS_VISIBLE)
        }
        focus.requestFocus()
        keyboard?.show()
    }
}
