package sh.zeron.android.tools

import android.content.Context
import android.content.Intent
import android.graphics.Bitmap
import android.graphics.BitmapFactory
import android.graphics.pdf.PdfRenderer
import android.net.Uri
import android.os.ParcelFileDescriptor
import androidx.compose.foundation.Image
import androidx.compose.foundation.background
import androidx.compose.foundation.gestures.detectTapGestures
import androidx.compose.foundation.gestures.awaitEachGesture
import androidx.compose.foundation.gestures.awaitFirstDown
import androidx.compose.foundation.gestures.calculatePan
import androidx.compose.foundation.gestures.calculateZoom
import androidx.compose.foundation.horizontalScroll
import androidx.compose.foundation.layout.height
import androidx.compose.foundation.rememberScrollState
import androidx.compose.ui.input.pointer.positionChanged
import androidx.compose.foundation.layout.Arrangement
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.Column
import androidx.compose.foundation.layout.Row
import androidx.compose.foundation.layout.Spacer
import androidx.compose.foundation.layout.aspectRatio
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.navigationBarsPadding
import androidx.compose.foundation.layout.padding
import androidx.compose.foundation.layout.size
import androidx.compose.foundation.layout.width
import androidx.compose.foundation.lazy.LazyColumn
import androidx.compose.foundation.lazy.rememberLazyListState
import androidx.compose.foundation.shape.RoundedCornerShape
import androidx.compose.material3.HorizontalFloatingToolbar
import androidx.compose.material3.IconButton
import androidx.compose.material3.LoadingIndicator
import androidx.compose.material3.MaterialTheme
import androidx.compose.material3.Surface
import androidx.compose.material3.Text
import androidx.compose.runtime.Composable
import androidx.compose.runtime.DisposableEffect
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.derivedStateOf
import androidx.compose.runtime.getValue
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.mutableStateOf
import androidx.compose.runtime.produceState
import androidx.compose.runtime.remember
import androidx.compose.runtime.rememberCoroutineScope
import androidx.compose.runtime.setValue
import androidx.compose.ui.Alignment
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.clipToBounds
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.asImageBitmap
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.input.pointer.pointerInput
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.dp
import androidx.core.content.FileProvider
import kotlinx.coroutines.Dispatchers
import kotlinx.coroutines.launch
import kotlinx.coroutines.sync.Mutex
import kotlinx.coroutines.sync.withLock
import kotlinx.coroutines.withContext
import sh.zeron.android.core.AppModel
import sh.zeron.android.design.ZIcon
import sh.zeron.android.design.ZIcons
import java.io.File

/**
 * Pinch to zoom (1–6×), pan while zoomed, double-tap to toggle 2.5×. A single
 * finger at 1× passes through (nothing to pan).
 */
@Composable
fun ZoomBox(modifier: Modifier = Modifier, content: @Composable () -> Unit) {
    var scale by remember { mutableFloatStateOf(1f) }
    var offset by remember { mutableStateOf(Offset.Zero) }
    BoxWithConstraints(
        modifier
            .clipToBounds()
            .pointerInput(Unit) {
                detectTapGestures(onDoubleTap = {
                    if (scale > 1.01f) {
                        scale = 1f
                        offset = Offset.Zero
                    } else {
                        scale = 2.5f
                    }
                })
            }
            .pointerInput(Unit) {
                awaitEachGesture {
                    awaitFirstDown(requireUnconsumed = false)
                    do {
                        val event = awaitPointerEvent()
                        val fingers = event.changes.count { it.pressed }
                        if (fingers >= 2 || scale > 1.01f) {
                            scale = (scale * event.calculateZoom()).coerceIn(1f, 6f)
                            val pan = event.calculatePan()
                            val maxX = size.width * (scale - 1) / 2
                            val maxY = size.height * (scale - 1) / 2
                            offset = if (scale <= 1.01f) Offset.Zero else Offset(
                                (offset.x + pan.x).coerceIn(-maxX, maxX),
                                (offset.y + pan.y).coerceIn(-maxY, maxY),
                            )
                            event.changes.forEach { if (it.positionChanged()) it.consume() }
                        }
                    } while (event.changes.any { it.pressed })
                }
            },
        contentAlignment = Alignment.Center,
    ) {
        Box(Modifier.fillMaxSize().graphicsLayer(scaleX = scale, scaleY = scale, translationX = offset.x, translationY = offset.y), contentAlignment = Alignment.Center) {
            content()
        }
    }
}

/** A workspace image (png, jpg, webp, gif, heic…), decoded off the main thread and downsampled to the screen. */
@Composable
fun ImageViewer(model: AppModel, ref: WorkspaceRef, path: String) {
    val state by produceState<Result<Bitmap>?>(null, ref, path) {
        value = runCatching {
            val bytes = model.workspaceApi.bytes(ref, path, limit = 48L shl 20)
            withContext(Dispatchers.Default) { decodeSampled(bytes, 4096) ?: error("This image format can't be shown.") }
        }
    }
    Box(Modifier.fillMaxSize().background(checker()), contentAlignment = Alignment.Center) {
        when (val s = state) {
            null -> LoadingIndicator()
            else -> s.fold(
                onSuccess = { bitmap -> ZoomBox(Modifier.fillMaxSize()) { Image(bitmap.asImageBitmap(), path, Modifier.fillMaxSize(), contentScale = ContentScale.Fit) } },
                onFailure = { Hint(it.message ?: "Couldn't load the image") },
            )
        }
    }
}

@Composable
private fun checker(): Color = MaterialTheme.colorScheme.surfaceContainerLowest

fun decodeSampled(bytes: ByteArray, maxSide: Int): Bitmap? {
    val bounds = BitmapFactory.Options().apply { inJustDecodeBounds = true }
    BitmapFactory.decodeByteArray(bytes, 0, bytes.size, bounds)
    var sample = 1
    while (bounds.outWidth / sample > maxSide || bounds.outHeight / sample > maxSide) sample *= 2
    return BitmapFactory.decodeByteArray(bytes, 0, bytes.size, BitmapFactory.Options().apply { inSampleSize = sample })
}

/** Copy a workspace file into the app cache (streamed), e.g. to render or hand to another app. */
suspend fun cacheCopy(context: Context, model: AppModel, ref: WorkspaceRef, path: String, dir: String): File = withContext(Dispatchers.IO) {
    val folder = File(context.cacheDir, dir).apply { mkdirs() }
    val name = path.substringAfterLast('/').ifEmpty { "file" }
    val file = File(folder, name)
    val partial = File(folder, "$name.part")
    partial.outputStream().buffered(1 shl 16).use { out -> model.workspaceApi.readBytes(ref, path) { chunk, _ -> out.write(chunk) } }
    if (!partial.renameTo(file)) {
        file.delete()
        partial.renameTo(file)
    }
    file
}

/** "Open with…": the file through this app's FileProvider. */
suspend fun openWith(context: Context, model: AppModel, ref: WorkspaceRef, path: String) {
    val file = cacheCopy(context, model, ref, path, "shared")
    val uri: Uri = FileProvider.getUriForFile(context, context.packageName + ".files", file)
    val mime = model.downloads.mime(file.name)
    val intent = Intent(Intent.ACTION_VIEW).setDataAndType(uri, mime).addFlags(Intent.FLAG_GRANT_READ_URI_PERMISSION)
    withContext(Dispatchers.Main) {
        runCatching { context.startActivity(Intent.createChooser(intent, "Open ${file.name}").addFlags(Intent.FLAG_ACTIVITY_NEW_TASK)) }
            .onFailure { toast(context, "No app can open ${file.name}") }
    }
}

/**
 * PDF preview with the platform renderer: pages render lazily at the screen
 * width (sharper as you zoom), pinch/double-tap zoom, a page counter and
 * previous/next jumps.
 */
@Composable
fun PdfViewer(model: AppModel, ref: WorkspaceRef, path: String) {
    val context = androidx.compose.ui.platform.LocalContext.current
    PdfFrom(ref to path) { cacheCopy(context, model, ref, path, "pdf") }
}

/** A PDF from any source (a workspace file, a page's download), fetched into a local file first. */
@Composable
fun PdfFrom(key: Any, fetch: suspend () -> File) {
    val doc by produceState<Result<PdfDoc>?>(null, key) {
        value = runCatching {
            val file = fetch()
            withContext(Dispatchers.IO) { PdfDoc(file) }
        }
    }
    val opened = doc?.getOrNull()
    DisposableEffect(opened) {
        // Close the document this effect saw, not whatever `doc` reads later.
        onDispose { opened?.close() }
    }
    Box(Modifier.fillMaxSize().background(MaterialTheme.colorScheme.surfaceContainerLowest), contentAlignment = Alignment.Center) {
        when (val d = doc) {
            null -> Column(horizontalAlignment = Alignment.CenterHorizontally) {
                LoadingIndicator()
                Text("Loading PDF…", style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
            }
            else -> d.fold(
                onSuccess = { PdfPages(it) },
                onFailure = { Hint(it.message ?: "Couldn't open the PDF") },
            )
        }
    }
}

class PdfDoc(file: File) {
    private val fd = ParcelFileDescriptor.open(file, ParcelFileDescriptor.MODE_READ_ONLY)
    private val renderer = PdfRenderer(fd)
    private val lock = Mutex()
    val pageCount: Int = renderer.pageCount
    val ratios: List<Float> = (0 until pageCount).map { i -> renderer.openPage(i).use { it.height.toFloat() / it.width.coerceAtLeast(1) } }

    /** One page as a white-backed bitmap `width` px wide (PdfRenderer allows one open page at a time). */
    suspend fun render(index: Int, width: Int): Bitmap = lock.withLock {
        withContext(Dispatchers.IO) {
            renderer.openPage(index).use { page ->
                val w = width.coerceIn(64, 3000)
                val h = (w * page.height.toFloat() / page.width.coerceAtLeast(1)).toInt().coerceAtLeast(1)
                val bitmap = Bitmap.createBitmap(w, h, Bitmap.Config.ARGB_8888)
                bitmap.eraseColor(android.graphics.Color.WHITE)
                page.render(bitmap, null, null, PdfRenderer.Page.RENDER_MODE_FOR_DISPLAY)
                bitmap
            }
        }
    }

    fun close() {
        runCatching { renderer.close() }
        runCatching { fd.close() }
    }
}

/**
 * Zoom is layout zoom: pages lay out at `zoom ×` the screen width inside a
 * horizontal scroller, so a zoomed document still scrolls page to page.
 * Pinch (two fingers) changes it; double-tap toggles 2×.
 */
@Composable
private fun PdfPages(doc: PdfDoc) {
    val list = rememberLazyListState()
    val horizontal = rememberScrollState()
    val scope = rememberCoroutineScope()
    var zoom by remember { mutableFloatStateOf(1f) }
    val current by remember { derivedStateOf { list.firstVisibleItemIndex + if (list.firstVisibleItemScrollOffset > 200) 1 else 0 } }
    BoxWithConstraints(Modifier.fillMaxSize()) {
        val viewport = maxWidth
        val height = maxHeight
        val px = with(LocalDensity.current) { (viewport * zoom).roundToPx() }
        Box(
            Modifier
                .fillMaxSize()
                .pointerInput(Unit) {
                    detectTapGestures(onDoubleTap = { zoom = if (zoom > 1.01f) 1f else 2f })
                }
                .pointerInput(Unit) {
                    awaitEachGesture {
                        awaitFirstDown(requireUnconsumed = false)
                        do {
                            val event = awaitPointerEvent(androidx.compose.ui.input.pointer.PointerEventPass.Initial)
                            if (event.changes.count { it.pressed } >= 2) {
                                zoom = (zoom * event.calculateZoom()).coerceIn(1f, 4f)
                                event.changes.forEach { if (it.positionChanged()) it.consume() }
                            }
                        } while (event.changes.any { it.pressed })
                    }
                }
                .horizontalScroll(horizontal),
        ) {
            LazyColumn(
                Modifier.width(viewport * zoom).height(height),
                state = list,
                verticalArrangement = Arrangement.spacedBy(12.dp),
                contentPadding = androidx.compose.foundation.layout.PaddingValues(12.dp, 12.dp, 12.dp, 120.dp),
            ) {
                items(doc.pageCount) { i -> PdfPage(doc, i, (px * 1.5f).toInt().coerceAtMost(3000)) }
            }
        }
        Surface(
            shape = RoundedCornerShape(50),
            color = MaterialTheme.colorScheme.inverseSurface.copy(alpha = 0.85f),
            contentColor = MaterialTheme.colorScheme.inverseOnSurface,
            modifier = Modifier.align(Alignment.TopEnd).padding(12.dp),
        ) {
            val zoomLabel = if (zoom > 1.01f) " · ${(zoom * 100).toInt()}%" else ""
            Text("${(current + 1).coerceAtMost(doc.pageCount)} / ${doc.pageCount}$zoomLabel", style = MaterialTheme.typography.labelLarge, modifier = Modifier.padding(horizontal = 12.dp, vertical = 6.dp))
        }
        if (doc.pageCount > 1) {
            HorizontalFloatingToolbar(expanded = true, modifier = Modifier.align(Alignment.BottomCenter).navigationBarsPadding().padding(bottom = 12.dp)) {
                IconButton(onClick = { scope.launch { list.animateScrollToItem((current - 1).coerceAtLeast(0)) } }, enabled = current > 0) {
                    ZIcon(ZIcons.ChevronUp, "Previous page", Modifier.size(22.dp))
                }
                Text("Page ${current + 1}", style = MaterialTheme.typography.labelLarge, modifier = Modifier.align(Alignment.CenterVertically).padding(horizontal = 8.dp))
                IconButton(onClick = { scope.launch { list.animateScrollToItem((current + 1).coerceAtMost(doc.pageCount - 1)) } }, enabled = current < doc.pageCount - 1) {
                    ZIcon(ZIcons.ChevronDown, "Next page", Modifier.size(22.dp))
                }
            }
        }
    }
}

@Composable
private fun PdfPage(doc: PdfDoc, index: Int, widthPx: Int) {
    val bitmap by produceState<Bitmap?>(null, index, widthPx) {
        value = runCatching { doc.render(index, widthPx) }
            .onFailure { android.util.Log.w("Zeron", "PDF page ${index + 1} failed to render", it) }
            .getOrNull()
    }
    Surface(shape = RoundedCornerShape(6.dp), color = Color.White, shadowElevation = 1.dp, modifier = Modifier.fillMaxWidth().aspectRatio(1f / doc.ratios[index].coerceIn(0.1f, 10f))) {
        bitmap?.let { Image(it.asImageBitmap(), "Page ${index + 1}", Modifier.fillMaxSize(), contentScale = ContentScale.FillWidth) }
    }
}

/** A file the viewer can't show inline: what it is and where it can go. */
@Composable
fun BinaryInfo(name: String, size: Long?, onSave: () -> Unit, onOpenWith: () -> Unit) {
    Column(Modifier.fillMaxSize().padding(32.dp), verticalArrangement = Arrangement.Center, horizontalAlignment = Alignment.CenterHorizontally) {
        FileIcon(name, false, 48.dp)
        Spacer(Modifier.size(12.dp))
        Text(name, style = MaterialTheme.typography.titleLarge)
        if (size != null) Text(formatBytes(size), style = MaterialTheme.typography.bodyMedium, color = MaterialTheme.colorScheme.onSurfaceVariant)
        Spacer(Modifier.size(20.dp))
        Row(horizontalArrangement = Arrangement.spacedBy(12.dp)) {
            androidx.compose.material3.Button(onClick = onSave) {
                ZIcon(ZIcons.Save, null, Modifier.size(18.dp))
                Spacer(Modifier.width(8.dp))
                Text("Save to Downloads")
            }
            androidx.compose.material3.FilledTonalButton(onClick = onOpenWith) { Text("Open with…") }
        }
    }
}
