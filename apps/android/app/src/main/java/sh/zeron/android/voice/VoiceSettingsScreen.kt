package sh.zeron.android.voice

import androidx.compose.foundation.background
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.WindowInsets
import androidx.compose.foundation.layout.asPaddingValues
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBars
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBars
import androidx.compose.foundation.layout.widthIn
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.LazyListScope
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.role
import androidx.compose.ui.semantics.selected
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.unit.dp
import sh.zeron.android.R
import sh.zeron.android.design.ZIcon
import sh.zeron.android.ui.ChoiceMenu
import sh.zeron.android.ui.IconTile
import sh.zeron.android.ui.MenuChoice
import sh.zeron.android.ui.MenuSection
import sh.zeron.android.ui.StatusBarScrim
import sh.zeron.android.ui.TonalCircleButton
import sh.zeron.android.ui.cardColor
import sh.zeron.android.ui.segmentedShapes
import sh.zeron.android.ui.successColor
import uniffi.zeron_core.OrbPreset
import uniffi.zeron_core.VoiceOrb

/**
 * Voice settings (iOS `VoiceViewController`), in the Settings list style:
 * which registered device runs Codex for a call and which voice it speaks
 * with. Calls start from the bar.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun VoiceSettingsScreen(controller: VoiceController, onBack: () -> Unit) {
    val s by controller.snapshot.collectAsState()
    val all by controller.devices.collectAsState()
    val selectedHost by controller.selectedHost.collectAsState()
    val selectedStyle by controller.selectedStyle.collectAsState()
    // Phones can't host Codex; only machines that can are listed.
    val devices = all.filter { it.isExecutionHost }
    val chosen = remember(all, selectedHost) { controller.startHost()?.id }
    val haptics = LocalHapticFeedback.current
    var styles by remember { mutableStateOf(false) }
    val list = rememberLazyListState()

    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.background)) {
        LazyColumn(
            Modifier.fillMaxSize(),
            state = list,
            contentPadding = PaddingValues(
                top = WindowInsets.statusBars.asPaddingValues().calculateTopPadding() + 8.dp,
                bottom = WindowInsets.navigationBars.asPaddingValues().calculateBottomPadding() + 32.dp,
            ),
        ) {
            item {
                Row(Modifier.fillMaxWidth().padding(horizontal = 12.dp, vertical = 4.dp), verticalAlignment = Alignment.CenterVertically) {
                    TonalCircleButton(R.drawable.zi_arrow_left, "Back", onClick = onBack, container = MaterialTheme.colorScheme.surfaceContainerHighest)
                    Spacer(Modifier.size(12.dp))
                    Text("Voice", style = MaterialTheme.typography.titleLargeEmphasized)
                }
            }
            // The hero: the desktop orb at rest over a one-line explanation.
            item {
                Column(
                    Modifier.fillMaxWidth().padding(start = 24.dp, end = 24.dp, top = 8.dp, bottom = 8.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                ) {
                    VoiceOrbView(VoiceOrb.IDLE, OrbPreset.LARGE, Modifier.size(112.dp))
                    Spacer(Modifier.height(10.dp))
                    Text("Talk with Codex", style = MaterialTheme.typography.titleLargeEmphasized, textAlign = TextAlign.Center)
                    Spacer(Modifier.height(6.dp))
                    Text(
                        "A voice orchestrator that starts sessions, checks on them and reports back.",
                        style = MaterialTheme.typography.bodyMedium,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.widthIn(max = 320.dp),
                    )
                }
            }

            section("Codex runs on")
            item {
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    if (devices.isEmpty()) {
                        SegmentedListItem(
                            onClick = {},
                            enabled = false,
                            shapes = segmentedShapes(0, 1),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = { IconTile(R.drawable.zi_laptop) },
                            supportingContent = { Text("Open Zeron on your Mac or server with remote voice enabled.") },
                        ) { Text("No Mac or server yet") }
                    }
                    devices.forEachIndexed { i, device ->
                        val compatible = controller.callable(device)
                        val ready = device.online && compatible
                        val selected = device.id == chosen
                        SegmentedListItem(
                            onClick = {
                                haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
                                controller.selectHost(device.id)
                            },
                            enabled = ready && !s.live,
                            shapes = segmentedShapes(i, devices.size),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = { IconTile(platformIcon(device)) },
                            supportingContent = {
                                Text(
                                    when {
                                        !device.online -> "Offline"
                                        !compatible -> "Update Zeron and enable remote voice"
                                        else -> "Ready"
                                    },
                                )
                            },
                            trailingContent = {
                                Row(verticalAlignment = Alignment.CenterVertically) {
                                    if (selected) {
                                        ZIcon(R.drawable.zi_check, null, Modifier.size(22.dp), tint = MaterialTheme.colorScheme.primary)
                                        Spacer(Modifier.size(10.dp))
                                    }
                                    Box(
                                        Modifier.size(8.dp).clip(CircleShape)
                                            .background(if (ready) successColor() else MaterialTheme.colorScheme.outlineVariant),
                                    )
                                }
                            },
                            modifier = Modifier.semantics {
                                role = Role.RadioButton
                                this.selected = selected
                            },
                        ) { Text(device.name) }
                    }
                }
            }
            footer(
                if (s.live) {
                    "End the call to change devices."
                } else {
                    "Audio stays on this phone. Codex, its sign-in and its tools run on the device you choose."
                },
            )

            section("Voice")
            item {
                Box(Modifier.padding(horizontal = 16.dp)) {
                    SegmentedListItem(
                        onClick = { styles = true },
                        shapes = segmentedShapes(0, 1),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(R.drawable.zi_volume_loud) },
                        supportingContent = { Text(VoiceController.styleLabel(selectedStyle)) },
                        trailingContent = { ZIcon(R.drawable.zi_alt_arrow_down, null, Modifier.size(20.dp), tint = MaterialTheme.colorScheme.onSurfaceVariant) },
                    ) { Text("Voice") }
                    // Every voice the host offers, the current one checked.
                    ChoiceMenu(
                        styles,
                        { styles = false },
                        listOf(
                            MenuSection(
                                null,
                                (listOf<String?>(null) + s.styles).map { style ->
                                    MenuChoice(VoiceController.styleLabel(style), style == selectedStyle) {
                                        haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
                                        controller.selectStyle(style)
                                    }
                                },
                            ),
                        ),
                    )
                }
            }
            footer("Applies to your next call.")

            section("During a call")
            item {
                val tips = listOf(
                    Triple(R.drawable.zi_smartphone, "Hold it to your ear", "The screen turns off and Codex plays through the earpiece."),
                    Triple(R.drawable.zi_phone_hang_up, "Lock your phone", "The call keeps going. Its notification has Mute and End; an incoming call ends it."),
                    Triple(R.drawable.zi_microphone, "Start from the bottom bar", "Tap the microphone; hold it to switch device or voice."),
                )
                Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                    tips.forEachIndexed { i, (icon, title, detail) ->
                        SegmentedListItem(
                            onClick = {},
                            shapes = segmentedShapes(i, tips.size),
                            colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                            leadingContent = { IconTile(icon) },
                            supportingContent = { Text(detail) },
                        ) { Text(title) }
                    }
                }
            }
        }
        StatusBarScrim(scrolled = list.firstVisibleItemIndex > 0 || list.firstVisibleItemScrollOffset > 0)
    }
}

private fun LazyListScope.section(title: String) {
    item {
        Text(
            title,
            style = MaterialTheme.typography.titleSmallEmphasized,
            color = MaterialTheme.colorScheme.primary,
            modifier = Modifier.padding(start = 28.dp, top = 24.dp, bottom = 8.dp),
        )
    }
}

private fun LazyListScope.footer(text: String) {
    item {
        Text(
            text,
            style = MaterialTheme.typography.bodySmall,
            color = MaterialTheme.colorScheme.onSurfaceVariant,
            modifier = Modifier.padding(start = 28.dp, end = 28.dp, top = 8.dp),
        )
    }
}
