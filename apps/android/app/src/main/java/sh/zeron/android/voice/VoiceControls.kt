package sh.zeron.android.voice

import android.os.SystemClock
import androidx.compose.foundation.ExperimentalFoundationApi
import androidx.compose.foundation.background
import androidx.compose.foundation.combinedClickable
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.heightIn
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.ListItemDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.ModalBottomSheet
import androidx.compose.material3.SegmentedListItem
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.Role
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import sh.zeron.android.R
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.ZIcon
import sh.zeron.android.ui.ChoiceMenu
import sh.zeron.android.ui.IconTile
import sh.zeron.android.ui.MenuChoice
import sh.zeron.android.ui.MenuSection
import sh.zeron.android.ui.cardColor
import sh.zeron.android.ui.segmentedShapes
import uniffi.zeron_core.DeviceView
import uniffi.zeron_core.OrbPreset

/**
 * Voice beside "New session" (iOS `VoiceAccessoryView`). Idle: a round
 * microphone button (tap to call, hold for host and style). Live: the call
 * strip — orb, call clock, mute and hang-up — that opens the stage. Renders
 * nothing while no host can take a call.
 *
 * [container]/[content] tint the idle button for the bar it sits in.
 */
@Composable
fun VoiceNewSessionAccessory(
    controller: VoiceController,
    onOpenStage: () -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
    container: Color = MaterialTheme.colorScheme.primary,
    content: Color = MaterialTheme.colorScheme.onPrimary,
) {
    val s by controller.snapshot.collectAsState()
    val devices by controller.devices.collectAsState()
    when {
        s.live -> VoiceCallStrip(controller, onOpenStage, modifier)
        controller.availableAmong(devices) -> VoiceStartButton(controller, onOpenStage, onOpenSettings, modifier, container = container, content = content)
    }
}

/** Whether voice controls should show at all (a host can run voice, or a call is up). */
@Composable
fun rememberVoiceVisible(controller: VoiceController): Boolean {
    val s by controller.snapshot.collectAsState()
    val devices by controller.devices.collectAsState()
    return s.live || controller.availableAmong(devices)
}

/**
 * The idle call button: tap calls the remembered (or only) host and opens
 * the stage, or offers the host picker; hold for host, style and settings.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun VoiceStartButton(
    controller: VoiceController,
    onOpenStage: () -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
    size: Dp = 48.dp,
    container: Color = MaterialTheme.colorScheme.primary,
    content: Color = MaterialTheme.colorScheme.onPrimary,
) {
    val haptics = LocalHapticFeedback.current
    var menu by remember { mutableStateOf(false) }
    var picker by remember { mutableStateOf(false) }
    Box(modifier) {
        Box(
            Modifier
                .size(size)
                .clip(CircleShape)
                .background(container)
                .combinedClickable(
                    role = Role.Button,
                    onClickLabel = "Talk to Codex",
                    onLongClickLabel = "Choose the device and voice",
                    onLongClick = {
                        haptics.performHapticFeedback(HapticFeedbackType.LongPress)
                        menu = true
                    },
                    onClick = {
                        haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                        if (controller.open()) onOpenStage() else picker = true
                    },
                )
                .semantics { contentDescription = "Talk to Codex" },
            contentAlignment = Alignment.Center,
        ) { ZIcon(R.drawable.zi_microphone, null, Modifier.size(size * 0.46f), tint = content) }
        VoiceMenu(controller, menu, { menu = false }, onOpenSettings)
    }
    if (picker) {
        VoiceHostPicker(controller, onDismiss = { picker = false }, onStarted = onOpenStage, onOpenSettings = onOpenSettings)
    }
}

/** The live call as a strip: orb, "Codex" and the clock, mute and hang-up. Tap for the stage. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun VoiceCallStrip(controller: VoiceController, onOpenStage: () -> Unit, modifier: Modifier = Modifier, showTitle: Boolean = true) {
    val s by controller.snapshot.collectAsState()
    val haptics = LocalHapticFeedback.current
    val now by rememberSecondTicker()
    val elapsed = s.elapsedText(now)
    val detail = if (s.active) listOfNotNull(elapsed, if (s.muted) "Muted" else null).joinToString(" · ") else "Connecting…"
    Row(
        modifier
            .clip(RoundedCornerShape(28.dp))
            .combinedClickable(role = Role.Button, onClickLabel = "Open the call", onClick = onOpenStage)
            .semantics { contentDescription = "Voice call, ${s.statusText}" + (elapsed?.let { ", $it" } ?: "") }
            .heightIn(min = 48.dp)
            .padding(end = 4.dp),
        verticalAlignment = Alignment.CenterVertically,
    ) {
        VoiceOrbView(s.orb, OrbPreset.AVATAR, Modifier.size(40.dp), microphone = s.state?.microphone ?: 0f, speaker = s.state?.speaker ?: 0f)
        Spacer(Modifier.width(8.dp))
        Column(Modifier.weight(1f)) {
            if (showTitle) {
                Text(
                    if (s.awaitingInput) "Codex needs you" else "Codex",
                    style = MaterialTheme.typography.titleSmallEmphasized,
                    color = if (s.awaitingInput) warningColor() else MaterialTheme.colorScheme.onSurface,
                    maxLines = 1,
                )
            }
            Text(detail, fontFamily = GeistMono, fontWeight = FontWeight.Medium, fontSize = 12.sp, color = MaterialTheme.colorScheme.onSurfaceVariant, maxLines = 1, overflow = TextOverflow.Ellipsis)
        }
        Spacer(Modifier.width(8.dp))
        FilledIconButton(
            onClick = {
                haptics.performHapticFeedback(if (s.muted) HapticFeedbackType.ToggleOff else HapticFeedbackType.ToggleOn)
                controller.toggleMute()
            },
            shapes = IconButtonDefaults.shapes(),
            colors = IconButtonDefaults.filledIconButtonColors(
                containerColor = if (s.muted) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.surfaceContainerHighest,
                contentColor = if (s.muted) MaterialTheme.colorScheme.surface else MaterialTheme.colorScheme.onSurface,
            ),
            modifier = Modifier.size(40.dp),
        ) { ZIcon(if (s.muted) R.drawable.zi_microphone_off else R.drawable.zi_microphone, if (s.muted) "Unmute microphone" else "Mute microphone", Modifier.size(20.dp)) }
        Spacer(Modifier.width(6.dp))
        FilledIconButton(
            onClick = {
                haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                controller.stop()
            },
            shapes = IconButtonDefaults.shapes(),
            colors = IconButtonDefaults.filledIconButtonColors(containerColor = endRed(), contentColor = Color.White),
            modifier = Modifier.size(40.dp),
        ) { ZIcon(R.drawable.zi_phone_hang_up, "End call", Modifier.size(20.dp)) }
    }
}

/**
 * Voice in a screen header (iOS `VoiceBarItem`), beside an open session: a
 * microphone to start (hold for host and style), the live orb while a call
 * runs (tap for the stage). Renders nothing while no host can take a call.
 */
@OptIn(ExperimentalFoundationApi::class)
@Composable
fun VoiceHeaderButton(
    controller: VoiceController,
    onOpenStage: () -> Unit,
    onOpenSettings: () -> Unit,
    modifier: Modifier = Modifier,
    size: Dp = 48.dp,
    container: Color = MaterialTheme.colorScheme.surfaceContainerHighest,
) {
    val s by controller.snapshot.collectAsState()
    if (!rememberVoiceVisible(controller)) return
    if (!s.live) {
        VoiceStartButton(controller, onOpenStage, onOpenSettings, modifier, size = size, container = container, content = MaterialTheme.colorScheme.onSurface)
        return
    }
    Box(
        modifier
            .size(size)
            .clip(CircleShape)
            .background(container)
            .combinedClickable(role = Role.Button, onClickLabel = "Open the call", onClick = onOpenStage)
            .semantics { contentDescription = "Voice call, ${s.statusText}" },
        contentAlignment = Alignment.Center,
    ) {
        VoiceOrbView(s.orb, OrbPreset.AVATAR, Modifier.size(size - 8.dp), microphone = s.state?.microphone ?: 0f, speaker = s.state?.speaker ?: 0f)
    }
}

/** Hold menu of a voice control: host and style for the next call, and settings. */
@Composable
fun VoiceMenu(controller: VoiceController, expanded: Boolean, onDismiss: () -> Unit, onOpenSettings: () -> Unit) {
    if (!expanded) return
    val selectedHost by controller.selectedHost.collectAsState()
    val selectedStyle by controller.selectedStyle.collectAsState()
    val s by controller.snapshot.collectAsState()
    val hosts = remember(expanded) { controller.hosts() }
    val chosen = remember(expanded, selectedHost) { controller.startHost()?.id }
    val sections = buildList {
        if (hosts.isNotEmpty() && !s.live) {
            add(MenuSection("Codex runs on", hosts.map { device -> MenuChoice(device.name, device.id == chosen, leading = { ZIcon(platformIcon(device), null) }) { controller.selectHost(device.id) } }))
        }
        add(
            MenuSection(
                "Voice",
                (listOf<String?>(null) + s.styles).map { style ->
                    MenuChoice(VoiceController.styleLabel(style), style == selectedStyle) { controller.selectStyle(style) }
                },
            ),
        )
        add(MenuSection(null, listOf(MenuChoice("Voice settings", leading = { ZIcon(R.drawable.zi_settings_minimalistic, null) }, onClick = onOpenSettings))))
    }
    ChoiceMenu(expanded, onDismiss, sections)
}

/**
 * "Talk to Codex on…": no host was remembered and more than one (or none) is
 * online. Picking one calls it and remembers it.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun VoiceHostPicker(controller: VoiceController, onDismiss: () -> Unit, onStarted: () -> Unit, onOpenSettings: () -> Unit) {
    val hosts = remember { controller.hosts() }
    ModalBottomSheet(onDismissRequest = onDismiss) {
        Column(Modifier.fillMaxWidth().navigationBarsPadding().padding(bottom = 16.dp)) {
            Text("Talk to Codex on…", style = MaterialTheme.typography.titleLargeEmphasized, modifier = Modifier.padding(horizontal = 24.dp))
            Spacer(Modifier.size(4.dp))
            Text(
                if (hosts.isEmpty()) {
                    "No online device can host Codex voice. Start Zeron with remote voice enabled on your Mac or server."
                } else {
                    "Audio stays on this phone. Codex and its tools run on the device you pick."
                },
                style = MaterialTheme.typography.bodyMedium,
                color = MaterialTheme.colorScheme.onSurfaceVariant,
                modifier = Modifier.padding(horizontal = 24.dp),
            )
            Spacer(Modifier.size(16.dp))
            val rows = hosts.size + 1
            Column(Modifier.padding(horizontal = 16.dp), verticalArrangement = Arrangement.spacedBy(ListItemDefaults.SegmentedGap)) {
                hosts.forEachIndexed { i, device ->
                    SegmentedListItem(
                        onClick = {
                            onDismiss()
                            if (controller.start(device)) onStarted()
                        },
                        shapes = segmentedShapes(i, rows),
                        colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                        leadingContent = { IconTile(platformIcon(device)) },
                        supportingContent = { Text("Ready") },
                    ) { Text(device.name) }
                }
                SegmentedListItem(
                    onClick = {
                        onDismiss()
                        onOpenSettings()
                    },
                    shapes = segmentedShapes(hosts.size, rows),
                    colors = ListItemDefaults.segmentedColors(containerColor = cardColor()),
                    leadingContent = { IconTile(R.drawable.zi_settings_minimalistic) },
                ) { Text("Voice settings") }
            }
        }
    }
}

/** Laptop for macOS and Windows machines, a server otherwise (iOS `symbol(for:)`). */
internal fun platformIcon(device: DeviceView): Int {
    val p = device.platform.lowercase()
    return when {
        !device.isExecutionHost -> R.drawable.zi_smartphone
        "mac" in p || "darwin" in p -> R.drawable.zi_laptop
        "win" in p -> R.drawable.zi_monitor
        else -> R.drawable.zi_remote_server
    }
}

/** `SystemClock.elapsedRealtime()`, ticking on the second (call clocks). */
@Composable
internal fun rememberSecondTicker() = produceState(SystemClock.elapsedRealtime()) {
    while (true) {
        value = SystemClock.elapsedRealtime()
        delay(1000 - value % 1000)
    }
}
