package sh.zeron.android.ui

import androidx.compose.foundation.Canvas
import androidx.compose.foundation.layout.fillMaxSize
import androidx.compose.runtime.Composable
import androidx.compose.runtime.LaunchedEffect
import androidx.compose.runtime.mutableFloatStateOf
import androidx.compose.runtime.remember
import kotlinx.coroutines.delay
import androidx.compose.ui.Modifier
import androidx.compose.ui.geometry.Offset
import androidx.compose.ui.graphics.BlendMode
import androidx.compose.ui.graphics.Color
import androidx.compose.ui.graphics.Path
import androidx.compose.ui.graphics.StrokeCap
import androidx.compose.ui.graphics.StrokeJoin
import androidx.compose.ui.graphics.drawscope.DrawScope
import androidx.compose.ui.graphics.drawscope.Stroke
import androidx.compose.ui.platform.LocalDensity
import androidx.compose.ui.unit.Dp
import androidx.compose.ui.unit.dp
import sh.zeron.android.design.LocalDarkTheme
import kotlin.math.cos
import kotlin.math.sin
import kotlin.math.sqrt
import kotlin.random.Random

/**
 * A lightning strike as flat segment arrays, so a strike costs no objects per
 * segment and the buffer is reused for every regeneration. [gen] is the branch
 * generation: 0 the trunk, 1 a fork off it, 2 a fork off a fork.
 */
class BoltBuffer(val capacity: Int = Lightning.MAX_SEGMENTS) {
    val x1 = FloatArray(capacity)
    val y1 = FloatArray(capacity)
    val x2 = FloatArray(capacity)
    val y2 = FloatArray(capacity)
    val gen = ByteArray(capacity)
    var count = 0
        private set

    fun clear() {
        count = 0
    }

    /** Appends one segment; false once the buffer is full (the strike is simply cut short). */
    fun add(ax: Float, ay: Float, bx: Float, by: Float, generation: Int): Boolean {
        if (count >= capacity) return false
        x1[count] = ax
        y1[count] = ay
        x2[count] = bx
        y2[count] = by
        gen[count] = generation.toByte()
        count++
        return true
    }
}

/**
 * Procedural branching lightning by recursive midpoint displacement: each
 * segment is split at its midpoint, pushed sideways by a random amount that
 * halves per level, and sometimes sprouts a fork that is itself a (shorter,
 * thinner) bolt. Everything derives from [seed], so the same seed always draws
 * the same strike, and the segment count is bounded by [MAX_SEGMENTS].
 */
object Lightning {
    const val MAX_SEGMENTS = 192

    private const val TRUNK_LEVELS = 6
    private const val FORK_LEVELS = 4
    private const val SUBFORK_LEVELS = 3

    /**
     * Fills [out] with a strike from the anchor ([ax], [ay]) across a panel of
     * [w] x [h]: one trunk toward the far lower-left, sometimes a second one
     * toward the left edge, each with forks.
     */
    fun generate(seed: Long, ax: Float, ay: Float, w: Float, h: Float, out: BoltBuffer) {
        out.clear()
        if (w <= 0f || h <= 0f) return
        val rng = Random(seed)
        val tx = w * (0.04f + 0.5f * rng.nextFloat())
        val ty = h * (0.72f + 0.30f * rng.nextFloat())
        trunk(rng, ax, ay, tx, ty, out)
        if (rng.nextFloat() < 0.65f) {
            val sx = w * (0.0f + 0.28f * rng.nextFloat())
            val sy = h * (0.08f + 0.45f * rng.nextFloat())
            trunk(rng, ax, ay, sx, sy, out)
        }
    }

    private fun trunk(rng: Random, ax: Float, ay: Float, bx: Float, by: Float, out: BoltBuffer) {
        val len = dist(ax, ay, bx, by)
        split(rng, ax, ay, bx, by, len * 0.20f, TRUNK_LEVELS, 0, out)
    }

    private fun split(rng: Random, ax: Float, ay: Float, bx: Float, by: Float, disp: Float, level: Int, gen: Int, out: BoltBuffer) {
        if (out.count >= out.capacity) return
        if (level == 0) {
            out.add(ax, ay, bx, by, gen)
            return
        }
        val dx = bx - ax
        val dy = by - ay
        val len = sqrt(dx * dx + dy * dy).coerceAtLeast(1e-3f)
        // Perpendicular unit vector times a random offset in [-disp, disp].
        val offset = (rng.nextFloat() * 2f - 1f) * disp
        val mx = (ax + bx) / 2f - dy / len * offset
        val my = (ay + by) / 2f + dx / len * offset
        // Forks sprout from the mid points of the larger splits only.
        val maxGen = 2
        if (gen < maxGen && level >= 3 && rng.nextFloat() < FORK_CHANCE) {
            val side = if (rng.nextBoolean()) 1f else -1f
            val spread = (0.35f + 0.5f * rng.nextFloat()) * side
            val c = cos(spread)
            val s = sin(spread)
            val scale = 0.45f + 0.5f * rng.nextFloat()
            val fx = mx + (dx * c - dy * s) * scale
            val fy = my + (dx * s + dy * c) * scale
            val levels = if (gen == 0) FORK_LEVELS else SUBFORK_LEVELS
            split(rng, mx, my, fx, fy, dist(mx, my, fx, fy) * 0.22f, levels, gen + 1, out)
        }
        split(rng, ax, ay, mx, my, disp * 0.55f, level - 1, gen, out)
        split(rng, mx, my, bx, by, disp * 0.55f, level - 1, gen, out)
    }

    private const val FORK_CHANCE = 0.30f

    private fun dist(ax: Float, ay: Float, bx: Float, by: Float): Float {
        val dx = bx - ax
        val dy = by - ay
        return sqrt(dx * dx + dy * dy)
    }
}

/** Where the fast button's centre sits in the settings card, from its top-right corner. */
internal val FastButtonTopPad: Dp = 0.dp
internal val FastAnchorFromRight: Dp = 14.dp + 26.dp
internal val FastAnchorFromTop: Dp = 14.dp + FastButtonTopPad + 26.dp

/**
 * Fast mode's backdrop: when fast mode is switched ON, one branching bolt strikes from the fast button through the
 * panel, blooms (layered strokes of falling alpha, added to the panel in the dark theme), flickers and fades out
 * over [LightningFx.LIFE_SECONDS]. Then nothing: no redraw loop, no timer, zero cost until the next switch-on.
 * Switching it off and on strikes again; opening the picker with fast already on shows nothing. It lives at the
 * bottom of the card's content and never takes touches. With reduced motion it is a single still frame at the
 * flash's peak that disappears after [LightningFx.STILL_MILLIS].
 */
@Composable
fun FastLightning(active: Boolean, modifier: Modifier = Modifier) {
    val reduceMotion = rememberReduceMotion()
    val dark = LocalDarkTheme.current
    val density = LocalDensity.current
    val buffer = remember { BoltBuffer() }
    val paths = remember { arrayOf(Path(), Path(), Path()) }
    val strokes = remember(density) { Strokes(density.density) }
    // 1 = nothing showing. Only moves while a strike is being played.
    val age = remember { mutableFloatStateOf(1f) }
    // The panel's [width, height] and the current strike's seed, shared by the strike and the drawing (resize).
    val panel = remember { floatArrayOf(0f, 0f) }
    val seed = remember { longArrayOf(STILL_SEED) }
    // [first composition, previous value of active, strikes so far]
    val memory = remember { longArrayOf(1L, if (active) 1L else 0L, 0L) }

    LaunchedEffect(active) {
        val first = memory[0] == 1L
        val was = memory[1] == 1L
        memory[0] = 0L
        memory[1] = if (active) 1L else 0L
        if (!LightningFx.strikes(first, was, active)) {
            age.floatValue = 1f
            return@LaunchedEffect
        }
        memory[2]++
        seed[0] = LightningFx.seedFor(memory[2].toInt(), System.nanoTime())
        regenerate(seed[0], panel, buffer, paths, strokes.density)
        if (reduceMotion) {
            age.floatValue = STILL_AGE
            delay(LightningFx.STILL_MILLIS)
            age.floatValue = 1f
            return@LaunchedEffect
        }
        var startedAt = -1L
        do {
            androidx.compose.runtime.withFrameNanos { now ->
                if (startedAt < 0) startedAt = now
                age.floatValue = LightningFx.ageAt(now - startedAt)
            }
        } while (age.floatValue < 1f)
    }
    Canvas(modifier.fillMaxSize()) {
        if (age.floatValue >= 1f) return@Canvas
        if (panel[0] != size.width || panel[1] != size.height) {
            panel[0] = size.width
            panel[1] = size.height
            regenerate(seed[0], panel, buffer, paths, strokes.density)
        }
        val intensity = if (reduceMotion) STILL_INTENSITY else LightningFx.intensity(age.floatValue)
        if (intensity <= 0.003f) return@Canvas
        drawBolt(buffer, paths, strokes, intensity, dark)
    }
}

private const val STILL_AGE = 0.1f
private const val STILL_INTENSITY = 0.55f
private const val STILL_SEED = 0x2eed5L

private class Strokes(val density: Float) {
    val glow = Stroke(18f * density, cap = StrokeCap.Round, join = StrokeJoin.Round)
    val halo = Stroke(9f * density, cap = StrokeCap.Round, join = StrokeJoin.Round)
    val mid = Stroke(4f * density, cap = StrokeCap.Round, join = StrokeJoin.Round)
    val core = Stroke(1.6f * density, cap = StrokeCap.Round, join = StrokeJoin.Round)
}

/** Regenerates the strike for a [size] panel and rebuilds one [Path] per branch generation. */
private fun regenerate(seed: Long, size: FloatArray, buffer: BoltBuffer, paths: Array<Path>, density: Float) {
    val w = size[0]
    val h = size[1]
    Lightning.generate(seed, w - FastAnchorFromRight.value * density, FastAnchorFromTop.value * density, w, h, buffer)
    for (p in paths) p.reset()
    for (i in 0 until buffer.count) {
        val p = paths[buffer.gen[i].toInt().coerceIn(0, 2)]
        p.moveTo(buffer.x1[i], buffer.y1[i])
        p.lineTo(buffer.x2[i], buffer.y2[i])
    }
}

private val GlowDark = Color(0xFF6C7BFF)
private val HaloDark = Color(0xFF8FA2FF)
private val CoreDark = Color(0xFFEAF0FF)
private val GlowLight = Color(0xFF5B5BEA)
private val HaloLight = Color(0xFF4553E0)
private val CoreLight = Color(0xFF2336C8)

private fun DrawScope.drawBolt(buffer: BoltBuffer, paths: Array<Path>, strokes: Strokes, k: Float, dark: Boolean) {
    if (buffer.count == 0) return
    // Dark panels take the light additively (a real bloom); on light panels the same
    // layers are drawn as saturated ink, since adding light to white shows nothing.
    val blend = if (dark) BlendMode.Plus else BlendMode.SrcOver
    val glow = if (dark) GlowDark else GlowLight
    val halo = if (dark) HaloDark else HaloLight
    val core = if (dark) CoreDark else CoreLight
    val gain = if (dark) 1f else 1.5f
    // A flash of light around where the strike leaves the button.
    val ax = size.width - FastAnchorFromRight.toPx()
    val ay = FastAnchorFromTop.toPx()
    drawCircle(glow.copy(alpha = 0.10f * k * gain), radius = 70.dp.toPx(), center = Offset(ax, ay), blendMode = blend)
    drawCircle(halo.copy(alpha = 0.12f * k * gain), radius = 34.dp.toPx(), center = Offset(ax, ay), blendMode = blend)
    // Trunk, then thinner forks (each generation draws at 0.7x the weight).
    for (g in 0..2) {
        val path = paths[g]
        val weight = when (g) { 0 -> 1f; 1 -> 0.7f; else -> 0.5f }
        drawPath(path, glow, alpha = 0.07f * k * gain, style = strokes.glow, blendMode = blend)
        drawPath(path, glow, alpha = 0.14f * k * gain * weight, style = strokes.halo, blendMode = blend)
        drawPath(path, halo, alpha = 0.34f * k * weight, style = strokes.mid, blendMode = blend)
        drawPath(path, core, alpha = (0.55f + 0.45f * k) * weight.coerceAtLeast(0.7f), style = strokes.core, blendMode = BlendMode.SrcOver)
    }
}
