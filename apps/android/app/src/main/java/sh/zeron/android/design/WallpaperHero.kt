package sh.zeron.android.design

import androidx.compose.animation.core.Animatable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.remember
import androidx.compose.animation.core.tween
import androidx.compose.foundation.Image
import androidx.compose.foundation.layout.Box
import androidx.compose.foundation.layout.BoxWithConstraints
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.foundation.layout.fillMaxWidth
import androidx.compose.foundation.layout.height
import androidx.compose.runtime.Composable
import androidx.compose.runtime.collectAsState
import androidx.compose.runtime.getValue
import androidx.compose.runtime.produceState
import androidx.compose.ui.Modifier
import androidx.compose.ui.draw.drawWithContent
import androidx.compose.ui.geometry.Rect
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Brush
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.CompositingStrategy
import androidx.compose.ui.graphics.drawscope.drawIntoCanvas
import androidx.compose.ui.graphics.graphicsLayer
import androidx.compose.ui.graphics.nativeCanvas
import androidx.compose.ui.layout.ContentScale
import androidx.compose.ui.unit.dp
import sh.zeron.android.core.WallpaperStore
import kotlin.math.max
import kotlin.math.min

/**
 * The wallpaper hero: aspect-filled artwork at the top of a page that fades
 * out downward (alpha, never a tinted overlay — the page shows through), at
 * the opacity the contrast guard allows. An optional cutout (the new-session
 * composer, in root coordinates) softens the artwork behind it.
 */
@Composable
fun WallpaperHero(store: WallpaperStore, modifier: Modifier = Modifier, scrollFade: () -> Float = { 1f }, cutout: () -> Rect? = { null }) {
    val state by store.state.collectAsState()
    val dark = LocalDarkTheme.current
    val render by produceState<WallpaperStore.Render?>(null, state, dark) { value = store.render(dark) }
    val r = render ?: return
    // Artwork arriving late fades in (desktop: 120 ms).
    val appear = remember(r) { Animatable(0f) }
    LaunchedEffect(r) { appear.animateTo(1f, tween(120)) }
    BoxWithConstraints(modifier.fillMaxWidth()) {
        // Desktop hero: 72% of the viewport, at most 760dp.
        val height = min(maxHeight.value * 0.72f, 760f).dp
        Box(
            Modifier
                .fillMaxWidth()
                .height(height)
                .graphicsLayer {
                    compositingStrategy = CompositingStrategy.Offscreen
                    alpha = r.opacity * appear.value * scrollFade().coerceIn(0f, 1f)
                }
                .drawWithContent {
                    drawContent()
                    // Smoothstep fade from opaque at the top to clear at the bottom.
                    val stops = (0..24).map { i ->
                        val t = i / 24f
                        t to Color.Black.copy(alpha = 1f - t * t * (3 - 2 * t))
                    }.toTypedArray()
                    drawRect(Brush.verticalGradient(*stops), blendMode = BlendMode.DstIn)
                    cutout()?.let { c ->
                        val feather = min(280f, max(120f, size.height / density * 0.52f)) / 3f * density
                        drawIntoCanvas { canvas ->
                            val paint = android.graphics.Paint(android.graphics.Paint.ANTI_ALIAS_FLAG).apply {
                                color = android.graphics.Color.argb(128, 0, 0, 0)
                                xfermode = android.graphics.PorterDuffXfermode(android.graphics.PorterDuff.Mode.DST_OUT)
                                maskFilter = android.graphics.BlurMaskFilter(feather, android.graphics.BlurMaskFilter.Blur.NORMAL)
                            }
                            val inset = 8 * density
                            canvas.nativeCanvas.drawRoundRect(c.left - inset, c.top - inset, c.right + inset, c.bottom + inset, 26 * density, 26 * density, paint)
                        }
                    }
                },
        ) {
            Image(r.image, null, Modifier.fillMaxSize(), contentScale = ContentScale.Crop)
        }
    }
}
