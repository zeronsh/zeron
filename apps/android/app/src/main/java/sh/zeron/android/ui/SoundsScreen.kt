package sh.zeron.android.ui

import sh.zeron.android.feedback.tapAction
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.material3.ButtonGroupDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Slider
import androidx.compose.material3.Surface
import androidx.compose.material3.Switch
import androidx.compose.material3.Text
import androidx.compose.material3.ToggleButton
import androidx.compose.material3.ToggleButtonDefaults
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.setValue
import androidx.compose.runtime.mutableIntStateOf
import androidx.compose.runtime.remember
import androidx.compose.ui.Modifier
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.unit.dp
import kotlinx.coroutines.delay
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import sh.zeron.android.feedback.AndroidFeedback
import sh.zeron.android.feedback.Cue
import sh.zeron.android.feedback.FeedbackSettings
import sh.zeron.android.feedback.Haptic
import sh.zeron.android.feedback.HapticStrength
import sh.zeron.android.feedback.LocalFeedback
import sh.zeron.android.feedback.toggleAction

/**
 * Settings > Sounds & haptics. Mirrors the desktop's Notifications settings
 * (one master, independent completion / input / error chimes) and adds the
 * phone's own layers: interface sounds, volume, and haptics with a strength.
 * Everything here previews itself: tap a row in "Try them" to hear and feel
 * the real thing.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun SoundsScreen(model: AppModel, onBack: () -> Unit) {
    val engine = model.feedback
    val s by engine.store.settings.collectAsState()
    val update = engine.store::update
    val fb = LocalFeedback.current
    SubPage("Sounds & haptics", "Chimes, taps and vibration", onBack) {
        item("master") {
            Group {
                SwitchRow(
                    0, 1, ZIcons.Volume, "Sounds & haptics",
                    "Off silences every sound and vibration. On lets the switches below decide.",
                    s.master,
                ) { on -> update { copy(master = on) } }
            }
        }

        sectionTitle("Sounds")
        item("sounds") {
            Group {
                SwitchRow(0, 2, ZIcons.Volume, "Sounds", "Every sound in Zeron. Off keeps the app silent.", s.sounds, enabled = s.master) { on -> update { copy(sounds = on) } }
                SwitchRow(1, 2, ZIcons.Context, "Interface sounds", "Soft taps for toggles, menus, sheets and actions", s.interfaceSounds, enabled = s.soundsOn) { on -> update { copy(interfaceSounds = on) } }
            }
        }
        item("volume") {
            Spacer(Modifier.height(GroupGap)) // the switch group above and the slider card are separate rows
            VolumeRow(engine, s)
        }

        sectionTitle("Session sounds")
        item("session") {
            Group {
                SwitchRow(0, 4, ZIcons.Bell, "Session sounds", "Chimes when a session needs you or finishes", s.sessionSounds, enabled = s.soundsOn) { on -> update { copy(sessionSounds = on) } }
                val on = s.soundsOn && s.sessionSounds
                SwitchRow(1, 4, ZIcons.Check, "Completion", "A session finished its turn", s.completionSound, enabled = on) { v -> update { copy(completionSound = v) } }
                SwitchRow(2, 4, ZIcons.Chat, "Input required", "A session is asking you something", s.inputSound, enabled = on) { v -> update { copy(inputSound = v) } }
                SwitchRow(3, 4, ZIcons.Warning, "Errors", "A session failed, or the connection dropped mid-turn", s.errorSound, enabled = on) { v -> update { copy(errorSound = v) } }
            }
        }
        item("notifications") {
            Spacer(Modifier.height(GroupGap)) // the Errors row above ends its group; this is the next one
            val access = rememberNotificationAccess(model)
            val granted = access.granted
            Group {
                SegmentedListItem(
                    onClick = tapAction(action = if (granted) access.settings else access.ask),
                    shapes = segmentedShapes(0, 1),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(ZIcons.Bell) },
                    supportingContent = { Text(if (granted) "On: sessions alert you while Zeron is closed. Tap for system settings" else "Needed to alert you while Zeron is in the background") },
                ) { Text(if (granted) "Background alerts" else "Allow notifications") }
            }
        }
        item("session-note") {
            Note("Only while Zeron is open. In the background the same chimes arrive with the notification, and follow your phone's notification settings.")
        }

        sectionTitle("Haptics")
        item("haptics") {
            Group {
                SwitchRow(0, 2, ZIcons.Phone, "Haptics", engine.hapticTier(), s.haptics, enabled = s.master) { on -> update { copy(haptics = on) } }
                StrengthRow(1, 2, s, engine)
            }
        }

        sectionTitle("Try them")
        item("preview") { PreviewRows(engine, s, fb) }
    }
}

@Composable
private fun Group(content: @Composable () -> Unit) {
    Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) { content() }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun SwitchRow(
    index: Int,
    count: Int,
    icon: Int,
    title: String,
    supporting: String,
    checked: Boolean,
    enabled: Boolean = true,
    onChange: (Boolean) -> Unit,
) {
    val change = toggleAction(onChange)
    SegmentedListItem(
        onClick = tapAction { change(!checked) },
        enabled = enabled,
        shapes = segmentedShapes(index, count),
        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
        leadingContent = { IconTile(icon) },
        supportingContent = { Text(supporting) },
        trailingContent = { Switch(checked, change, enabled = enabled) },
    ) { Text(title) }
}

private val VolumeSteps = 10

/** Space between two groups of rows that sit directly one under the other (a section title has its own, larger one). */
private val GroupGap = 10.dp

/** The master level: a ten-step slider whose detents climb the family's scale as you drag. */
@Composable
private fun VolumeRow(engine: AndroidFeedback, s: FeedbackSettings) {
    var lastStep by remember { mutableIntStateOf((s.volume * VolumeSteps).toInt()) }
    Group {
        Surface(shape = segmentedShapes(0, 1).shape, color = cardColor()) {
            Column(Modifier.padding(horizontal = 16.dp, vertical = 12.dp)) {
                Row(verticalAlignment = androidx.compose.ui.Alignment.CenterVertically) {
                    IconTile(ZIcons.Volume)
                    Spacer(Modifier.size(16.dp))
                    Column(Modifier.weight(1f)) {
                        Text("Volume", style = MaterialTheme.typography.titleMedium)
                        Text("Relative to your phone's volume. 100% is twice as loud as 50%", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
                    }
                    Spacer(Modifier.size(12.dp))
                    Text("${(s.volume * 100).toInt()}%", style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.onSurfaceVariant)
                }
                Slider(
                    value = s.volume,
                    onValueChange = { v ->
                        engine.store.update { copy(volume = v) }
                        val step = Math.round(v * VolumeSteps)
                        if (step != lastStep) {
                            lastStep = step
                            engine.preview(Haptic.Tick, Cue.Detent, step)
                        }
                    },
                    onValueChangeFinished = { engine.preview(null, Cue.Select) },
                    steps = VolumeSteps - 1,
                    enabled = s.soundsOn,
                )
            }
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun StrengthRow(index: Int, count: Int, s: FeedbackSettings, engine: AndroidFeedback) {
    Surface(shape = segmentedShapes(index, count).shape, color = cardColor()) {
        Column(Modifier.padding(16.dp)) {
            Text("Strength", style = MaterialTheme.typography.titleMedium)
            Text("How firmly taps and alerts are felt", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            Spacer(Modifier.size(12.dp))
            Row(horizontalArrangement = Arrangement.spacedBy(ButtonGroupDefaults.ConnectedSpaceBetween)) {
                val levels = HapticStrength.entries
                levels.forEachIndexed { i, level ->
                    ToggleButton(
                        checked = s.strength == level,
                        onCheckedChange = {
                            engine.store.update { copy(strength = level) }
                            // Feel the new strength at once: a choice, then a confirmation.
                            engine.preview(Haptic.Confirm, Cue.Select)
                        },
                        enabled = s.hapticsOn,
                        modifier = Modifier.weight(1f).semantics { role = Role.RadioButton },
                        shapes = when (i) {
                            0 -> ButtonGroupDefaults.connectedLeadingButtonShapes()
                            levels.lastIndex -> ButtonGroupDefaults.connectedTrailingButtonShapes()
                            else -> ButtonGroupDefaults.connectedMiddleButtonShapes()
                        },
                    ) { Text(level.label) }
                }
            }
        }
    }
}

private class Sample(val title: String, val supporting: String, val haptic: Haptic?, val cue: Cue?, val steps: List<Int>? = null)

private val samples = listOf(
    Sample("Completion", "A session finished", Haptic.Success, Cue.Done),
    Sample("Input required", "A session is asking you something", Haptic.Attention, Cue.Request),
    Sample("Error", "A session failed", Haptic.Error, Cue.Attention),
    Sample("Send", "A message sent", Haptic.Confirm, Cue.Send),
    Sample("Toggle", "A switch, on then off", Haptic.ToggleOn, Cue.ToggleOn),
    Sample("Slider", "Detents climbing the scale", Haptic.Tick, Cue.Detent, listOf(0, 1, 2, 3, 4)),
    Sample("Delete", "A weighty, destructive step", Haptic.Heavy, Cue.Delete),
    Sample("Reconnected", "The connection came back", Haptic.Confirm, Cue.Reconnected),
)

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PreviewRows(engine: AndroidFeedback, s: FeedbackSettings, fb: sh.zeron.android.feedback.Feedback) {
    var running by remember { mutableIntStateOf(-1) }
    LaunchedEffect(running) {
        val sample = samples.getOrNull(running) ?: return@LaunchedEffect
        when {
            sample.steps != null -> for (step in sample.steps) {
                engine.preview(sample.haptic, sample.cue, step)
                delay(110)
            }
            sample.cue == Cue.ToggleOn -> {
                engine.preview(Haptic.ToggleOn, Cue.ToggleOn)
                delay(450)
                engine.preview(Haptic.ToggleOff, Cue.ToggleOff)
            }
            else -> engine.preview(sample.haptic, sample.cue)
        }
        running = -1
    }
    Group {
        samples.forEachIndexed { i, sample ->
            SegmentedListItem(
                onClick = { running = i }, // the preview is the answer; no extra tap on top
                enabled = running == -1 && (s.soundsOn || s.hapticsOn),
                shapes = segmentedShapes(i, samples.size),
                colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                leadingContent = { IconTile(ZIcons.Play) },
                supportingContent = { Text(sample.supporting) },
            ) { Text(sample.title) }
        }
    }
}

@Composable
private fun Note(text: String) {
    Text(
        text,
        style = MaterialTheme.typography.bodySmall,
        color = MaterialTheme.colorScheme.onSurfaceVariant,
        modifier = Modifier.padding(start = 28.dp, end = 28.dp, top = 8.dp),
    )
}
