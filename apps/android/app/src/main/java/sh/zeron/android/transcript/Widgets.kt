package sh.zeron.android.transcript

import android.graphics.BitmapFactory
import android.util.LruCache
import androidx.compose.animation.core.LinearEasing
import androidx.compose.animation.core.RepeatMode
import androidx.compose.animation.core.animateFloat
import androidx.compose.animation.core.infiniteRepeatable
import androidx.compose.animation.core.rememberInfiniteTransition
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Canvas
import androidx.compose.foundation.Image
import androidx.compose.foundation.clickable
import androidx.compose.foundation.interaction.MutableInteractionSource
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.offset
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.background
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material.icons.Icons
import androidx.compose.material.icons.automirrored.filled.KeyboardArrowRight
import androidx.compose.material.icons.filled.Check
import androidx.compose.material.icons.filled.Close
import androidx.compose.material.icons.outlined.ContentCopy
import androidx.compose.material.icons.outlined.CheckBoxOutlineBlank
import androidx.compose.material.icons.filled.CheckBox
import androidx.compose.material.icons.automirrored.outlined.CallSplit
import androidx.compose.material.icons.outlined.WarningAmber
import androidx.compose.material.icons.automirrored.outlined.HelpOutline
import androidx.compose.material3.CircularProgressIndicator
import androidx.compose.material3.CircularWavyProgressIndicator
import androidx.compose.animation.fadeIn
import androidx.compose.animation.fadeOut
import androidx.compose.material3.ExperimentalMaterial3ExpressiveApi
import androidx.compose.material3.Icon
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableLongStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clip
import androidx.compose.ui.draw.rotate
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ColorFilter
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.toArgb
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.hapticfeedback.HapticFeedbackType
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalClipboardManager
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.platform.LocalHapticFeedback
import androidx.compose.ui.semantics.contentDescription
import androidx.compose.ui.semantics.semantics
import androidx.compose.ui.text.AnnotatedString
import androidx.compose.ui.text.SpanStyle
import androidx.compose.ui.text.buildAnnotatedString
import androidx.compose.ui.text.withStyle
import androidx.compose.ui.unit.dp
import androidx.compose.ui.unit.sp
import kotlinx.coroutines.delay
import sh.zeron.android.design.Geist
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import androidx.compose.foundation.border
import sh.zeron.android.design.TranscriptPalette
import uniffi.zeron_core.ColorRole
import uniffi.zeron_core.Widget
import uniffi.zeron_core.WidgetKind
import kotlin.math.max

/** Native affordances Rust positioned in the row (or one of its scrollers). */
@Composable
fun RowWidgets(state: TranscriptState, model: RowModel, scroller: UInt?, palette: TranscriptPalette, actions: TranscriptActions) {
    val d = model.display
    val live = d.widgets.any { it.kind is WidgetKind.Shimmer }
    for (w in d.widgets) {
        if (w.scroller != scroller) continue
        Box(Modifier.offset(w.x.dp, w.y.dp).size(max(w.w, 0f).dp, max(w.h, 0f).dp)) {
            Widget(state, model, w, palette, actions, live)
        }
    }
}

@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun Widget(state: TranscriptState, model: RowModel, w: Widget, palette: TranscriptPalette, actions: TranscriptActions, live: Boolean) {
    val key = model.display.key
    val haptics = LocalHapticFeedback.current
    when (val kind = w.kind) {
        WidgetKind.CopyCode -> CopyButton(w.payload ?: "", palette)
        is WidgetKind.Disclosure -> Tap(if (kind.expanded) "Collapse" else "Expand") {
            haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
            state.toggle(key)
        }
        is WidgetKind.ToolStatus -> when {
            kind.running -> CircularProgressIndicator(Modifier.fillMaxSize().padding(1.dp), strokeWidth = 1.5.dp, color = Color(palette[ColorRole.TEXT_TERTIARY]))
            else -> ZIcon(
                if (kind.failed) ZIcons.Close else ZIcons.Check,
                null,
                Modifier.fillMaxSize(),
                tint = Color(palette[if (kind.failed) ColorRole.DANGER else ColorRole.TEXT_TERTIARY]),
            )
        }
        is WidgetKind.Image -> RemoteImage(kind.reference, w, actions) { state.uploadProgress }
        WidgetKind.Spinner -> CircularProgressIndicator(Modifier.fillMaxSize().padding(1.dp), strokeWidth = 1.5.dp, color = Color(palette[ColorRole.TEXT_TERTIARY]))
        is WidgetKind.Working -> WorkingIndicator(kind.sinceMs, kind.streaming, palette)
        is WidgetKind.Detail -> Tap("${kind.title} details") { actions.showText(kind.title, w.payload ?: "", true) }
        is WidgetKind.Icon -> AssetIcon(kind.name, Color(palette[kind.color]))
        is WidgetKind.Chevron -> ZIcon(
            ZIcons.ChevronRight,
            null,
            Modifier.fillMaxSize().rotate(if (kind.expanded) 90f else 0f),
            tint = Color(palette[ColorRole.TEXT_TERTIARY]),
        )
        is WidgetKind.ToolRail -> ToolRail(kind, Color(palette[ColorRole.TOOL_RAIL]))
        is WidgetKind.ToolToggle -> Tap(if (kind.open) "Hide details" else "Show details") {
            haptics.performHapticFeedback(HapticFeedbackType.SegmentTick)
            state.toggleDetail(key, kind.detail, kind.open)
        }
        WidgetKind.Shimmer -> Shimmer(state, model, w, palette)
    }
}

@Composable
private fun Tap(label: String, onClick: () -> Unit) {
    Box(
        Modifier
            .fillMaxSize()
            .semantics { contentDescription = label }
            .clickable(interactionSource = remember { MutableInteractionSource() }, indication = null, onClick = onClick),
    )
}

@Composable
private fun CopyButton(payload: String, palette: TranscriptPalette) {
    val clipboard = LocalClipboardManager.current
    val haptics = LocalHapticFeedback.current
    var copied by remember { mutableStateOf(false) }
    LaunchedEffect(copied) {
        if (copied) {
            delay(1400)
            copied = false
        }
    }
    Box(
        Modifier
            .fillMaxSize()
            .clip(RoundedCornerShape(8.dp))
            .semantics { contentDescription = "Copy code" }
            .clickable {
                clipboard.setText(AnnotatedString(payload))
                haptics.performHapticFeedback(HapticFeedbackType.Confirm)
                copied = true
            },
        contentAlignment = Alignment.Center,
    ) {
        ZIcon(
            if (copied) ZIcons.Check else ZIcons.Copy,
            null,
            Modifier.size(16.dp),
            tint = if (copied) MaterialTheme.colorScheme.primary else Color(palette[ColorRole.TEXT_TERTIARY]),
        )
    }
}

/** Tail-of-turn indicator: the expressive loading shape + "Working…" and elapsed time. */
@OptIn(ExperimentalMaterial3ExpressiveApi::class)
@Composable
private fun WorkingIndicator(sinceMs: Long?, streaming: Boolean, palette: TranscriptPalette) {
    var now by remember { mutableLongStateOf(System.currentTimeMillis()) }
    LaunchedEffect(Unit) {
        while (true) {
            delay(1000)
            now = System.currentTimeMillis()
        }
    }
    val secs = sinceMs?.let { max(0L, (now - it) / 1000) } ?: 0L
    Row(Modifier.fillMaxSize(), verticalAlignment = Alignment.CenterVertically, horizontalArrangement = Arrangement.spacedBy(6.dp)) {
        LoadingIndicator(Modifier.size(24.dp), color = MaterialTheme.colorScheme.primary)
        Text(
            buildAnnotatedString {
                append(if (streaming) "Writing…" else "Working…")
                if (secs > 0) withStyle(SpanStyle(color = Color(palette[ColorRole.TEXT_TERTIARY]))) { append("  " + elapsed(secs)) }
            },
            color = Color(palette[ColorRole.TEXT_SECONDARY]),
            fontFamily = Geist,
            fontSize = 13.5.sp,
            style = MaterialTheme.typography.labelLarge.copy(fontWeight = androidx.compose.ui.text.font.FontWeight.Medium),
        )
    }
}

fun elapsed(secs: Long): String = when {
    secs < 60 -> "${secs}s"
    secs < 3600 -> "${secs / 60}m ${secs % 60}s"
    else -> "${secs / 3600}h ${(secs % 3600) / 60}m"
}

/** Desktop tool/file icons (rasterized SVGs); unknown names draw nothing. */
@Composable
private fun AssetIcon(name: String, tint: Color) {
    val context = LocalContext.current
    when (name) {
        "square", "checkmark.square.fill" -> return TaskBox(name != "square", tint)
        "arrow.triangle.branch" -> return ZIcon(ZIcons.Branch, null, Modifier.fillMaxSize(), tint = tint)
        "exclamationmark.triangle" -> return ZIcon(ZIcons.Warning, null, Modifier.fillMaxSize(), tint = tint)
        "questionmark.bubble" -> return ZIcon(ZIcons.Chat, null, Modifier.fillMaxSize(), tint = tint)
    }
    val image = remember(name) { IconAssets.load(context, name) } ?: return
    Image(
        image,
        null,
        Modifier.fillMaxSize(),
        contentScale = ContentScale.Fit,
        colorFilter = if (name.startsWith("tool-")) ColorFilter.tint(tint) else null,
    )
}

/** A markdown task checkbox: a rounded square, filled with a check when done. */
@Composable
private fun TaskBox(checked: Boolean, tint: Color) {
    Box(
        Modifier
            .fillMaxSize()
            .padding(2.dp)
            .clip(RoundedCornerShape(5.dp))
            .then(if (checked) Modifier.background(tint) else Modifier.border(1.5.dp, tint, RoundedCornerShape(5.dp))),
        contentAlignment = Alignment.Center,
    ) {
        if (checked) ZIcon(ZIcons.Check, null, Modifier.fillMaxSize().padding(1.dp), tint = MaterialTheme.colorScheme.onPrimary)
    }
}

object IconAssets {
    private val cache = HashMap<String, ImageBitmap?>()

    fun load(context: android.content.Context, name: String): ImageBitmap? = cache.getOrPut(name) {
        if (!name.startsWith("tool-") && !name.startsWith("fileicon-")) return@getOrPut null
        runCatching { context.assets.open("icons/$name.png").use { BitmapFactory.decodeStream(it)?.asImageBitmap() } }.getOrNull()
    }
}

private val images = LruCache<String, ImageBitmap>(48)

@Composable
private fun RemoteImage(reference: String, w: Widget, actions: TranscriptActions, progress: () -> Double?) {
    val image by produceState(images.get(reference), reference) {
        if (value == null) value = actions.loadImage(reference)?.also { images.put(reference, it) }
    }
    val radius = if (minOf(w.w, w.h) > 120) 14.dp else 12.dp
    Box(Modifier.fillMaxSize().clip(RoundedCornerShape(radius)), contentAlignment = Alignment.Center) {
        image?.let { Image(it, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop) }
            ?: Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerHigh))
        // Still being escorted to the host: a scrim and a wavy ring that fills with the transfer.
        val p = if (reference.startsWith("pending://")) progress() else null
        androidx.compose.animation.AnimatedVisibility(p != null, enter = fadeIn(), exit = fadeOut()) {
            Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = 0.38f)), contentAlignment = Alignment.Center) {
                val side = minOf(w.w, w.h)
                CircularWavyProgressIndicator(
                    progress = { (p ?: 0.0).toFloat().coerceIn(0.02f, 1f) },
                    modifier = Modifier.size(minOf(40f, side * 0.55f).dp),
                    color = Color.White,
                    trackColor = Color.White.copy(alpha = 0.3f),
                )
                Text(
                    "${((p ?: 0.0).coerceIn(0.0, 1.0) * 100).toInt()}%",
                    color = Color.White,
                    fontSize = 10.sp,
                    style = MaterialTheme.typography.labelSmallEmphasized,
                    modifier = Modifier.semantics { contentDescription = "Uploading" },
                )
            }
        }
    }
}

/** The activity rail of an expanded tool group: trunk, elbows, branches. */
@Composable
private fun ToolRail(r: WidgetKind.ToolRail, color: Color) {
    val d = LocalDensity.current.density
    Canvas(Modifier.fillMaxSize()) {
        val path = Path()
        val x = r.trunkX * d
        val bend = r.bend * d
        for (i in r.tops.indices) {
            val top = r.tops[i] * d
            val mid = top + r.rowMid * d
            path.moveTo(x, top)
            path.lineTo(x, mid - bend)
            path.quadraticTo(x, mid, x + bend, mid)
            path.lineTo(r.branchEnd * d, mid)
            if (i + 1 < r.tops.size) {
                path.moveTo(x, mid - bend)
                path.lineTo(x, top + r.heights[i] * d)
            }
        }
        drawPath(path, color, style = Stroke(width = d))
    }
}

/** Shimmer over the live group's title: its runs re-drawn brighter through a moving band. */
@Composable
private fun Shimmer(state: TranscriptState, model: RowModel, w: Widget, palette: TranscriptPalette) {
    val d = LocalDensity.current.density
    val transition = rememberInfiniteTransition(label = "shimmer")
    val phase by transition.animateFloat(0f, 1f, infiniteRepeatable(tween(3400, easing = LinearEasing), RepeatMode.Restart), label = "sweep")
    val bright = MaterialTheme.colorScheme.onSurface
    Canvas(Modifier.fillMaxSize()) {
        drawIntoCanvas { c ->
            val canvas = c.nativeCanvas
            val width = max(w.w, 1f) * d
            val save = canvas.saveLayer(null, null)
            canvas.translate(-w.x * d, -w.y * d)
            model.drawRunsIn(canvas, w.x, w.y, w.w, w.h, bright.toArgb(), d, state.fonts, palette)
            canvas.translate(w.x * d, w.y * d)
            val center = (phase * 1.6f - 0.3f) * width
            val band = width * 0.18f
            val mask = android.graphics.Paint().apply {
                xfermode = android.graphics.PorterDuffXfermode(android.graphics.PorterDuff.Mode.DST_IN)
                shader = android.graphics.LinearGradient(
                    center - band, 0f, center + band, 0f,
                    intArrayOf(0, 0xFF000000.toInt(), 0), floatArrayOf(0f, 0.5f, 1f),
                    android.graphics.Shader.TileMode.CLAMP,
                )
            }
            canvas.drawRect(0f, 0f, width, size.height, mask)
            canvas.restoreToCount(save)
        }
    }
}

