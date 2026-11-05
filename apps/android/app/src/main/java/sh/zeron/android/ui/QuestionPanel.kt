package sh.zeron.android.ui

import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.rememberScrollState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.foundation.text.KeyboardActions
import androidx.compose.foundation.text.KeyboardOptions
import androidx.compose.foundation.verticalScroll
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.OutlinedTextField
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.material3.TextButton
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.mutableStateMapOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.text.input.ImeAction
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import uniffi.zeron_core.InputRequest
import uniffi.zeron_core.UserInputAnswer
import uniffi.zeron_core.UserInputQuestion

/**
 * Replaces the composer while the agent asks questions (iOS QuestionPanel):
 * one page per question, large option rows, "Other…" free text (or a
 * multiline editor seeded with the agent's prefill), and single-select
 * auto-advance so a three-question form is three taps.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun QuestionPanel(input: InputRequest, onSubmit: (List<UserInputAnswer>) -> Unit) {
    val items = input.questions
    var page by remember(input.requestId) { mutableIntStateOf(0) }
    val picks = remember(input.requestId) { mutableStateMapOf<String, Set<String>>() }
    // Question ids arrive from agents and synced docs: a duplicate keeps the first prefill.
    val custom = remember(input.requestId) {
        mutableStateMapOf<String, String>().apply { items.forEach { q -> if (q.id !in this) put(q.id, q.prefill ?: "") } }
    }
    // Answers go once per question set (a quick double tap on the last page would submit twice).
    var submitted by remember(input.requestId) { mutableStateOf(false) }
    var autoAdvance by remember(input.requestId) { mutableIntStateOf(0) }
    val haptics = LocalHapticFeedback.current
    val current = items.getOrNull(page) ?: return

    fun answered(q: UserInputQuestion) =
        q.multiline || picks[q.id].orEmpty().isNotEmpty() || custom[q.id].orEmpty().isNotBlank()

    fun advance() {
        val q = items.getOrNull(page) ?: return
        if (submitted || !answered(q)) return
        if (page < items.size - 1) {
            page++
            return
        }
        submitted = true
        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
        onSubmit(
            items.map { q ->
                val labels = q.options.filter { it in picks[q.id].orEmpty() }.toMutableList()
                val text = custom[q.id].orEmpty()
                if (q.multiline) labels.add(text) else if (text.isNotBlank()) labels.add(text.trim())
                UserInputAnswer(q.id, labels)
            },
        )
    }

    // No answers at all is the decline: the run reads its harness's own
    // "dismissed", and the chip records the questions closed.
    fun dismiss() {
        if (submitted) return
        submitted = true
        onSubmit(emptyList())
    }

    fun pick(q: UserInputQuestion, option: String) {
        haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
        val set = picks[q.id].orEmpty()
        picks[q.id] = if (q.multiSelect) (if (option in set) set - option else set + option) else setOf(option)
        if (!q.multiSelect) autoAdvance++
    }

    LaunchedEffect(autoAdvance) {
        if (autoAdvance == 0) return@LaunchedEffect
        delay(220)
        advance()
    }

    Surface(
        shape = RoundedCornerShape(28.dp),
        color = composerContainer(),
        border = androidx.compose.foundation.BorderStroke(1.dp, MaterialTheme.colorScheme.outlineVariant),
        modifier = Modifier.fillMaxWidth().padding(horizontal = 12.dp).padding(bottom = 8.dp).testTag("question-panel"),
    ) {
        Box {
            AnimatedContent(page, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "question") { index ->
                val q = items.getOrNull(index) ?: return@AnimatedContent
                Column(Modifier.padding(start = 16.dp, end = 16.dp, top = 16.dp, bottom = 10.dp)) {
                    val header = q.header.ifEmpty { "Question" }
                    // Header and question wrap clear of the corner close (8dp in, 36dp wide).
                    Text(
                        if (items.size > 1) "$header · ${index + 1} of ${items.size}" else header,
                        style = MaterialTheme.typography.labelLarge,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        modifier = Modifier.padding(end = 32.dp),
                    )
                    Spacer(Modifier.height(6.dp))
                    Column(Modifier.heightIn(max = 360.dp).verticalScroll(rememberScrollState())) {
                        Text(q.question, style = MaterialTheme.typography.titleMediumEmphasized, modifier = Modifier.padding(end = 32.dp))
                        Spacer(Modifier.height(14.dp))
                        Column(verticalArrangement = Arrangement.spacedBy(6.dp)) {
                            val chosen = picks[q.id].orEmpty()
                            q.options.forEachIndexed { i, option ->
                                OptionRow(i, option, option in chosen, q.multiSelect) { pick(q, option) }
                            }
                        }
                        if (q.options.isNotEmpty()) Spacer(Modifier.height(10.dp))
                        OutlinedTextField(
                            value = custom[q.id].orEmpty(),
                            onValueChange = { custom[q.id] = it },
                            placeholder = { Text(if (q.multiline) "Write your answer" else "Other…") },
                            singleLine = !q.multiline,
                            minLines = if (q.multiline) 5 else 1,
                            maxLines = if (q.multiline) 10 else 1,
                            shape = RoundedCornerShape(16.dp),
                            keyboardOptions = KeyboardOptions(imeAction = if (q.multiline) ImeAction.Default else ImeAction.Done),
                            keyboardActions = KeyboardActions(onDone = { advance() }),
                            modifier = Modifier.fillMaxWidth().testTag(if (q.multiline) "question-editor" else "question-other"),
                        )
                    }
                    Spacer(Modifier.height(10.dp))
                    Row(verticalAlignment = Alignment.CenterVertically) {
                        if (index > 0) TextButton(onClick = { page-- }) { Text("Back") }
                        Spacer(Modifier.weight(1f))
                        Button(
                            onClick = ::advance,
                            enabled = answered(current) && !submitted,
                            shapes = ButtonDefaults.shapes(),
                            modifier = Modifier.testTag("question-next"),
                        ) {
                            Text(if (index == items.size - 1) "Submit" else "Next")
                            if (index == items.size - 1) {
                                Spacer(Modifier.width(8.dp))
                                ZIcon(ZIcons.Send, null, Modifier.size(18.dp))
                            }
                        }
                    }
                }
            }
            // The model picker's close, pinned 8dp in so it is concentric with the
            // card's corner: answers nothing, and the agent reads that the questions
            // went unanswered.
            TonalCircleButton(
                ZIcons.Close,
                "Close without answering",
                onClick = ::dismiss,
                modifier = Modifier.align(Alignment.TopEnd).padding(8.dp).testTag("question-close"),
                size = 36.dp,
                container = MaterialTheme.colorScheme.surfaceContainerHighest,
            )
        }
    }
}

@Composable
private fun OptionRow(index: Int, label: String, selected: Boolean, multi: Boolean, onClick: () -> Unit) {
    val accent = MaterialTheme.colorScheme.primary
    Surface(
        onClick = onClick,
        shape = RoundedCornerShape(16.dp),
        color = if (selected) accent.copy(alpha = 0.12f) else chipContainer(),
        contentColor = if (selected) accent else MaterialTheme.colorScheme.onSurface,
        modifier = Modifier.fillMaxWidth().testTag("question-option-$index"),
    ) {
        Row(Modifier.padding(horizontal = 14.dp, vertical = 12.dp), verticalAlignment = Alignment.CenterVertically) {
            Surface(
                shape = if (multi) RoundedCornerShape(6.dp) else RoundedCornerShape(50),
                color = if (selected) accent else MaterialTheme.colorScheme.surfaceContainerHighest,
                contentColor = if (selected) MaterialTheme.colorScheme.onPrimary else MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.size(24.dp),
            ) {
                androidx.compose.foundation.layout.Box(contentAlignment = Alignment.Center) {
                    if (selected) ZIcon(ZIcons.Check, null, Modifier.size(15.dp))
                    else Text("${index + 1}", style = MaterialTheme.typography.labelMedium)
                }
            }
            Spacer(Modifier.width(12.dp))
            Text(label, style = MaterialTheme.typography.bodyLarge)
        }
    }
}
