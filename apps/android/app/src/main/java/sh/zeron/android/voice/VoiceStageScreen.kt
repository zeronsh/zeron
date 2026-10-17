package sh.zeron.android.voice

import android.content.Context
import android.content.Intent
import android.net.Uri
import android.os.SystemClock
import androidx.activity.compose.BackHandler
import androidx.annotation.DrawableRes
import androidx.compose.animation.AnimatedContent
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.animation.togetherWith
import androidx.compose.foundation.background
import androidx.compose.foundation.border
import androidx.compose.foundation.gestures.detectVerticalDragGestures
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.PaddingValues
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.statusBarsPadding
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.shape.CircleShape
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.Button
import androidx.compose.material3.ButtonDefaults
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.FilledIconButton
import androidx.compose.material3.IconButtonDefaults
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberUpdatedState
import androidx.compose.runtime.setValue
import androidx.compose.runtime.withFrameNanos
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.alpha
import androidx.compose.ui.draw.clip
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.LiveRegionMode
import androidx.compose.ui.semantics.clearAndSetSemantics
import androidx.compose.ui.semantics.liveRegion
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.font.FontWeight
import androidx.compose.ui.text.style.TextAlign
import androidx.compose.ui.text.style.TextOverflow
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import sh.zeron.android.R
import sh.zeron.android.core.WallpaperStore
import sh.zeron.android.design.GeistMono
import sh.zeron.android.design.LocalDarkTheme
import sh.zeron.android.design.WallpaperHero
import sh.zeron.android.design.ZIcon
import sh.zeron.android.ui.TonalCircleButton
import sh.zeron.android.ui.successColor
import uniffi.zeron_core.CaptionFader
import uniffi.zeron_core.CaptionFrame
import uniffi.zeron_core.OrbPreset
import uniffi.zeron_core.VoiceEndReason
import uniffi.zeron_core.VoiceOrb
import uniffi.zeron_core.VoiceSpeaker
import kotlin.math.min

/**
 * The full-screen call (desktop `voice_stage.rs`, iOS
 * `VoiceStageViewController`): the session's orb over the wallpaper, the live
 * caption, and the call controls. Swipe down or go back to return to the app;
 * the call continues until End. Held to the ear it behaves like a phone call —
 * screen off, audio in the earpiece.
 */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
fun VoiceStageScreen(
    controller: VoiceController,
    onClose: () -> Unit,
    onOpenChat: (chatId: String) -> Unit,
    modifier: Modifier = Modifier,
    wallpaper: WallpaperStore? = null,
) {
    val s by controller.snapshot.collectAsState()
    val context = LocalContext.current
    val haptics = LocalHapticFeedback.current
    var closed by remember { mutableStateOf(false) }
    val onCloseNow by rememberUpdatedState(onClose)
    val close = {
        if (!closed) {
            closed = true
            onCloseNow()
        }
    }

    DisposableEffect(controller) {
        controller.stageShown(true)
        onDispose {
            controller.stageShown(false)
            if (!controller.live) controller.dismissEndReason()
        }
    }
    // Hung up (here, in the notification or elsewhere): nothing left to show.
    LaunchedEffect(s.live, s.endReason) {
        val now = controller.snapshot.value
        if (!now.live && now.endReason == null) close()
    }
    BackHandler(onBack = close)

    val now by produceState(SystemClock.elapsedRealtime()) {
        while (true) {
            value = SystemClock.elapsedRealtime()
            delay(1000 - value % 1000)
        }
    }
    val state = s.state
    val ended = !s.live
    val reason = s.endReason

    Box(
        modifier
            .fillMaxSize()
            .background(MaterialTheme.colorScheme.background)
            .pointerInput(Unit) {
                var travel = 0f
                detectVerticalDragGestures(
                    onDragStart = { travel = 0f },
                    onDragEnd = { if (travel > 120.dp.toPx()) close() },
                ) { _, dy -> travel += dy }
            },
    ) {
        wallpaper?.let { WallpaperHero(it) }
        Column(Modifier.fillMaxSize().statusBarsPadding().navigationBarsPadding()) {
            // Minimize, the call clock and host, the transcript.
            Box(Modifier.fillMaxWidth().padding(horizontal = 16.dp, vertical = 8.dp)) {
                TonalCircleButton(
                    R.drawable.zi_alt_arrow_down,
                    "Back to Zeron",
                    onClick = close,
                    modifier = Modifier.align(Alignment.CenterStart),
                    container = MaterialTheme.colorScheme.surfaceContainerHighest,
                )
                Row(Modifier.align(Alignment.Center), verticalAlignment = Alignment.CenterVertically) {
                    val dot = when {
                        !s.active -> MaterialTheme.colorScheme.outline
                        s.awaitingInput -> warningColor()
                        s.muted -> MaterialTheme.colorScheme.outline
                        else -> successColor()
                    }
                    Box(Modifier.size(7.dp).clip(CircleShape).background(dot))
                    Spacer(Modifier.width(7.dp))
                    Text(
                        s.elapsedText(now) ?: "–:––",
                        fontFamily = GeistMono,
                        fontWeight = FontWeight.Medium,
                        fontSize = 13.sp,
                        color = MaterialTheme.colorScheme.onSurfaceVariant,
                    )
                    Spacer(Modifier.width(7.dp))
                    Text(s.hostName, style = MaterialTheme.typography.labelLarge, color = MaterialTheme.colorScheme.outline, maxLines = 1)
                }
                val chatId = state?.chatId
                if (chatId != null) {
                    Box(Modifier.align(Alignment.CenterEnd)) {
                        TonalCircleButton(
                            R.drawable.zi_chat_round_line,
                            if (s.awaitingInput) "Answer Codex in the transcript" else "Open the voice transcript",
                            onClick = { onOpenChat(chatId) },
                            container = MaterialTheme.colorScheme.surfaceContainerHighest,
                        )
                        if (s.awaitingInput) {
                            Box(
                                Modifier
                                    .align(Alignment.TopEnd)
                                    .padding(2.dp)
                                    .size(12.dp)
                                    .clip(CircleShape)
                                    .background(warningColor())
                                    .border(2.dp, MaterialTheme.colorScheme.background, CircleShape),
                            )
                        }
                    }
                }
            }

            // The orb, what the call is doing, and the caption.
            BoxWithConstraints(Modifier.weight(1f).fillMaxWidth()) {
                val side: Dp = min(min(340f, maxWidth.value - 32f), maxHeight.value - 190f).coerceAtLeast(96f).dp
                Column(
                    Modifier.fillMaxSize().padding(bottom = 24.dp),
                    horizontalAlignment = Alignment.CenterHorizontally,
                    verticalArrangement = Arrangement.Center,
                ) {
                    VoiceOrbView(
                        orb = if (s.live) s.orb else VoiceOrb.IDLE,
                        preset = OrbPreset.HERO,
                        modifier = Modifier.size(side),
                        microphone = state?.microphone ?: 0f,
                        speaker = state?.speaker ?: 0f,
                    )
                    Spacer(Modifier.height(4.dp))
                    Text(
                        if (ended && reason != null) "Call ended" else s.statusText,
                        style = MaterialTheme.typography.titleMedium.copy(fontSize = 17.sp),
                        color = MaterialTheme.colorScheme.onSurface,
                        textAlign = TextAlign.Center,
                        modifier = Modifier.padding(horizontal = 32.dp).semantics { liveRegion = LiveRegionMode.Polite },
                    )
                    Spacer(Modifier.height(10.dp))
                    VoiceCaption(
                        item = state?.captionItem,
                        text = state?.caption.orEmpty(),
                        user = state?.captionSpeaker == VoiceSpeaker.USER,
                        message = if (ended && reason != null) s.message(reason) else null,
                        modifier = Modifier.fillMaxWidth().height(104.dp).padding(horizontal = 32.dp),
                    )
                }
            }

            // Mute, speaker and End; after an end, Call again and Close.
            Surface(
                shape = RoundedCornerShape(50),
                color = MaterialTheme.colorScheme.surfaceContainerHigh,
                modifier = Modifier.align(Alignment.CenterHorizontally).padding(bottom = 20.dp),
            ) {
                AnimatedContent(ended, transitionSpec = { fadeIn() togetherWith fadeOut() }, label = "voice-controls") { over ->
                    Row(Modifier.padding(10.dp), horizontalArrangement = Arrangement.spacedBy(14.dp), verticalAlignment = Alignment.CenterVertically) {
                        if (!over) {
                            CallButton(
                                if (s.muted) R.drawable.zi_microphone_off else R.drawable.zi_microphone,
                                if (s.muted) "Unmute microphone" else "Mute microphone",
                                // Inverted plate while muted, like the desktop call bar.
                                container = if (s.muted) MaterialTheme.colorScheme.onSurface else MaterialTheme.colorScheme.surfaceContainerHighest,
                                content = if (s.muted) MaterialTheme.colorScheme.surface else MaterialTheme.colorScheme.onSurface,
                            ) {
                                haptics.performHapticFeedback(if (s.muted) HapticFeedbackType.ToggleOff else HapticFeedbackType.ToggleOn)
                                controller.toggleMute()
                            }
                            CallButton(
                                if (s.route == VoiceRoute.Earpiece) R.drawable.zi_smartphone else R.drawable.zi_volume_loud,
                                when (s.route) {
                                    VoiceRoute.Speaker -> "Speaker on. Switch to the earpiece"
                                    VoiceRoute.Earpiece -> "Earpiece. Switch to the speaker"
                                    VoiceRoute.External -> "Playing on a headset"
                                },
                                container = if (s.speakerOn && s.route != VoiceRoute.External) MaterialTheme.colorScheme.secondaryContainer else MaterialTheme.colorScheme.surfaceContainerHighest,
                                content = if (s.speakerOn && s.route != VoiceRoute.External) MaterialTheme.colorScheme.onSecondaryContainer else MaterialTheme.colorScheme.onSurface,
                            ) {
                                haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
                                controller.toggleSpeaker()
                            }
                            CallButton(
                                R.drawable.zi_phone_hang_up,
                                "End call",
                                container = endRed(),
                                content = Color.White,
                                width = 88.dp,
                            ) {
                                haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                                controller.stop()
                            }
                        } else {
                            PillButton(
                                if (reason == VoiceEndReason.MICROPHONE_DENIED) "Open Settings" else "Call again",
                                container = MaterialTheme.colorScheme.primary,
                                content = Color.White,
                            ) {
                                if (reason == VoiceEndReason.MICROPHONE_DENIED) {
                                    openAppSettings(context)
                                } else if (!controller.start()) {
                                    controller.dismissEndReason()
                                }
                            }
                            PillButton("Close", MaterialTheme.colorScheme.surfaceContainerHighest, MaterialTheme.colorScheme.onSurface) {
                                controller.dismissEndReason()
                                close()
                            }
                        }
                    }
                }
            }
        }
    }
}

/**
 * The tail of the utterance, its newest words fading in through the core's
 * caption veil and the previous turn fading out; repaints every frame until
 * settled. [message] (an end-of-call explanation) replaces it.
 */
@Composable
internal fun VoiceCaption(item: String?, text: String, user: Boolean, message: String?, modifier: Modifier = Modifier) {
    val fader = remember { CaptionFader(160u) }
    DisposableEffect(fader) { onDispose { fader.close() } }
    val reducedMotion = rememberReducedMotion()
    var frame by remember { mutableStateOf<CaptionFrame?>(null) }
    LaunchedEffect(item, text, message == null) {
        if (message != null) return@LaunchedEffect
        var next = fader.frame(item, text, reducedMotion)
        frame = next
        while (next.animating) {
            withFrameNanos { }
            next = fader.frame(item, text, reducedMotion)
            frame = next
        }
    }
    val secondary = MaterialTheme.colorScheme.onSurfaceVariant
    val style = MaterialTheme.typography.bodyLarge.copy(fontSize = 16.sp, textAlign = TextAlign.Center)
    Box(modifier, contentAlignment = Alignment.TopCenter) {
        if (message != null) {
            Text(message, style = style, color = secondary, maxLines = 4, overflow = TextOverflow.Ellipsis)
            return@Box
        }
        val f = frame ?: return@Box
        f.previous?.let { previous ->
            Text(
                previous,
                style = style,
                color = secondary,
                maxLines = 4,
                overflow = TextOverflow.Ellipsis,
                modifier = Modifier.alpha(f.previousAlpha.coerceIn(0f, 1f)).clearAndSetSemantics { },
            )
        }
        val base = if (user) MaterialTheme.colorScheme.outline else secondary
        val caption = buildAnnotatedString {
            append(f.text)
            for (span in f.spans) {
                if (span.alpha >= 1f) continue
                val start = span.start.toInt().coerceIn(0, f.text.length)
                val end = span.end.toInt().coerceIn(start, f.text.length)
                if (start < end) addStyle(SpanStyle(color = base.copy(alpha = base.alpha * span.alpha.coerceIn(0f, 1f))), start, end)
            }
        }
        Text(caption, style = style, color = base, maxLines = 4, overflow = TextOverflow.Ellipsis)
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun CallButton(
    @DrawableRes icon: Int,
    description: String,
    container: Color,
    content: Color,
    width: Dp = 60.dp,
    onClick: () -> Unit,
) {
    FilledIconButton(
        onClick = onClick,
        shapes = IconButtonDefaults.shapes(),
        colors = IconButtonDefaults.filledIconButtonColors(containerColor = container, contentColor = content),
        modifier = Modifier.size(width, 60.dp),
    ) { ZIcon(icon, description, Modifier.size(26.dp)) }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun PillButton(title: String, container: Color, content: Color, onClick: () -> Unit) {
    Button(
        onClick = onClick,
        shapes = ButtonDefaults.shapes(),
        colors = ButtonDefaults.buttonColors(containerColor = container, contentColor = content),
        contentPadding = PaddingValues(horizontal = 22.dp),
        modifier = Modifier.height(52.dp),
    ) { Text(title, style = MaterialTheme.typography.titleMediumEmphasized.copy(fontSize = 16.sp)) }
}

/** Zeron's warning tone (transcript palette `WARNING`). */
@Composable
internal fun warningColor(): Color = if (LocalDarkTheme.current) Color(0xFFFACC15) else Color(0xFFA16207)

/** The hang-up red (iOS `systemRed`). */
@Composable
internal fun endRed(): Color = if (LocalDarkTheme.current) Color(0xFFFF453A) else Color(0xFFFF3B30)

/** This app's system settings page, where the microphone is granted. */
fun openAppSettings(context: Context) {
    runCatching {
        context.startActivity(
            Intent(android.provider.Settings.ACTION_APPLICATION_DETAILS_SETTINGS, Uri.fromParts("package", context.packageName, null))
                .addFlags(Intent.FLAG_ACTIVITY_NEW_TASK),
        )
    }
}
