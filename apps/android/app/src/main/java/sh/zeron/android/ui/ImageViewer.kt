package sh.zeron.android.ui

import android.content.ClipData
import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import androidx.compose.animation.core.Animatable
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.safeDrawingPadding
import androidx.compose.runtime.Composable
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.ImageBitmap
import androidx.compose.ui.graphics.asAndroidBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalContext
import androidx.compose.ui.platform.testTag
import androidx.compose.ui.unit.dp
import androidx.compose.ui.window.Dialog
import androidx.compose.ui.window.DialogProperties
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.withContext
import sh.zeron.android.design.ZIcons
import java.io.File
import kotlin.math.abs

/**
 * Full-screen, zoomable image (iOS ImageViewer): pinch or double-tap to
 * zoom, drag down to dismiss, share from the corner.
 */
@Composable
fun ImageViewer(image: ImageBitmap, onDismiss: () -> Unit) {
    Dialog(onDismissRequest = onDismiss, properties = DialogProperties(usePlatformDefaultWidth = false, decorFitsSystemWindows = false)) {
        DialogSystemBars(lightIcons = true)
        val context = LocalContext.current
        val scope = rememberCoroutineScope()
        var scale by remember { mutableFloatStateOf(1f) }
        var offset by remember { mutableStateOf(Offset.Zero) }
        val drag = remember { Animatable(0f) }
        val fade = (1f - abs(drag.value) / 900f).coerceIn(0.3f, 1f)
        Box(Modifier.fillMaxSize().background(Color.Black.copy(alpha = fade)).testTag("image-viewer")) {
            Image(
                image,
                contentDescription = "Image",
                contentScale = ContentScale.Fit,
                modifier = Modifier
                    .fillMaxSize()
                    .pointerInput(Unit) {
                        detectTapGestures(onDoubleTap = { at ->
                            if (scale > 1f) {
                                scale = 1f
                                offset = Offset.Zero
                            } else {
                                scale = 2.5f
                                offset = (Offset(size.width / 2f, size.height / 2f) - at) * 1.5f
                            }
                        })
                    }
                    .pointerInput(Unit) {
                        awaitEachGesture {
                            awaitFirstDown(requireUnconsumed = false)
                            var dismissing = false
                            do {
                                val event = awaitPointerEvent()
                                val zoom = event.calculateZoom()
                                val pan = event.calculatePan()
                                if (event.changes.size > 1 || scale > 1f) {
                                    // Zoomed (or pinching): pan within the image.
                                    scale = (scale * zoom).coerceIn(1f, 5f)
                                    val maxX = size.width * (scale - 1f) / 2f
                                    val maxY = size.height * (scale - 1f) / 2f
                                    offset = Offset(
                                        (offset.x + pan.x * scale).coerceIn(-maxX, maxX),
                                        (offset.y + pan.y * scale).coerceIn(-maxY, maxY),
                                    )
                                    if (scale == 1f) offset = Offset.Zero
                                    event.changes.forEach { if (it.positionChanged()) it.consume() }
                                } else if (abs(pan.y) > abs(pan.x) || dismissing) {
                                    // At rest: a vertical drag pulls the image away.
                                    dismissing = true
                                    scope.launch { drag.snapTo(drag.value + pan.y) }
                                    event.changes.forEach { if (it.positionChanged()) it.consume() }
                                }
                            } while (event.changes.any { it.pressed })
                            if (dismissing) {
                                if (abs(drag.value) > 120.dp.toPx()) onDismiss() else scope.launch { drag.animateTo(0f) }
                            }
                        }
                    }
                    .graphicsLayer {
                        scaleX = scale
                        scaleY = scale
                        translationX = offset.x
                        translationY = offset.y + drag.value
                    },
            )
            Box(Modifier.fillMaxSize().safeDrawingPadding().padding(12.dp)) {
                TonalCircleButton(
                    ZIcons.Close, "Close image preview", onClick = onDismiss,
                    modifier = Modifier.align(Alignment.TopStart), size = 44.dp,
                    container = Color.White.copy(alpha = 0.16f), content = Color.White,
                )
                TonalCircleButton(
                    ZIcons.Share, "Share image", onClick = { scope.launch { share(context, image) } },
                    modifier = Modifier.align(Alignment.TopEnd), size = 44.dp,
                    container = Color.White.copy(alpha = 0.16f), content = Color.White,
                )
            }
        }
    }
}

/** Hand the image to the share sheet through the app's FileProvider. */
private suspend fun share(context: Context, image: ImageBitmap) {
    val file = withContext(Dispatchers.IO) {
        val dir = File(context.cacheDir, "shared").apply { mkdirs() }
        File(dir, "image.png").also { f -> f.outputStream().use { image.asAndroidBitmap().compress(Bitmap.CompressFormat.PNG, 100, it) } }
    }
    val uri = FileProvider.getUriForFile(context, "${context.packageName}.files", file)
    val send = Intent(Intent.ACTION_SEND)
        .setType("image/png")
        .putExtra(Intent.EXTRA_STREAM, uri)
        .addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    send.clipData = ClipData.newRawUri(null, uri)
    context.startActivity(Intent.createChooser(send, null))
}
