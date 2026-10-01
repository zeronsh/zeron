package sh.zeron.android.ui

import kotlin.math.abs
import kotlin.math.max
import kotlin.math.min
import kotlin.math.sqrt

/**
 * The pure geometry of the count badges: where a label sits inside a shape and
 * how big it can be. No Compose, no Android: everything is computed from the
 * shape's outline, in the unit square (y down), and unit-tested on the JVM.
 *
 * A badge is a polygon scaled UNIFORMLY so its larger side spans the badge's
 * footprint and centred in it ([Outline.normalized]); nothing is ever stretched.
 * The label is then the largest axis-aligned box of the label's own aspect
 * ratio that fits inside the shape with some clearance ([fit]): its centre is
 * the shape's optical centre for that label (a triangle's number sits in the
 * wide lower body, a heart's below the lobes, a symmetric shape's in the middle),
 * and its height is the digit height, so every shape carries the same share of
 * label.
 */
class Outline(val xs: FloatArray, val ys: FloatArray) {
    val size: Int get() = xs.size

    /** Left, top, right, bottom. */
    fun bounds(): FloatArray {
        var l = Float.MAX_VALUE; var t = Float.MAX_VALUE; var r = -Float.MAX_VALUE; var b = -Float.MAX_VALUE
        for (i in 0 until size) { l = min(l, xs[i]); r = max(r, xs[i]); t = min(t, ys[i]); b = max(b, ys[i]) }
        return floatArrayOf(l, t, r, b)
    }

    /** Scaled uniformly so the larger side is 1, then centred in the unit square. */
    fun normalization(): Normalization {
        val (l, t, r, b) = bounds().toList()
        val side = max(r - l, b - t)
        return Normalization(l, t, side, (1f - (r - l) / side) / 2f, (1f - (b - t) / side) / 2f)
    }

    fun normalized(): Outline {
        val n = normalization()
        return Outline(FloatArray(size) { n.x(xs[it]) }, FloatArray(size) { n.y(ys[it]) })
    }

    /** Signed shoelace area (positive for either winding: absolute). */
    fun area(): Float {
        var a = 0.0
        for (i in 0 until size) {
            val j = (i + 1) % size
            a += xs[i].toDouble() * ys[j] - xs[j].toDouble() * ys[i]
        }
        return abs(a / 2).toFloat()
    }

    /** The area centroid. */
    fun centroid(): Pair<Float, Float> {
        var a = 0.0; var cx = 0.0; var cy = 0.0
        for (i in 0 until size) {
            val j = (i + 1) % size
            val cross = xs[i].toDouble() * ys[j] - xs[j].toDouble() * ys[i]
            a += cross
            cx += (xs[i] + xs[j]) * cross
            cy += (ys[i] + ys[j]) * cross
        }
        return if (a == 0.0) 0.5f to 0.5f else (cx / (3 * a)).toFloat() to (cy / (3 * a)).toFloat()
    }

    /** Even-odd point in polygon. */
    fun contains(px: Float, py: Float): Boolean {
        var inside = false
        var j = size - 1
        for (i in 0 until size) {
            if ((ys[i] > py) != (ys[j] > py) && px < (xs[j] - xs[i]) * (py - ys[i]) / (ys[j] - ys[i]) + xs[i]) inside = !inside
            j = i
        }
        return inside
    }

    /** Shortest distance from the point to the outline's edges. */
    fun distanceToEdge(px: Float, py: Float): Float {
        var best = Float.MAX_VALUE
        for (i in 0 until size) {
            val j = (i + 1) % size
            val dx = xs[j] - xs[i]; val dy = ys[j] - ys[i]
            val len2 = dx * dx + dy * dy
            val t = if (len2 == 0f) 0f else (((px - xs[i]) * dx + (py - ys[i]) * dy) / len2).coerceIn(0f, 1f)
            val qx = xs[i] + t * dx - px; val qy = ys[i] + t * dy - py
            best = min(best, sqrt(qx * qx + qy * qy))
        }
        return best
    }

    /** Whether the whole box (centre, half extents) lies inside the shape. */
    fun containsBox(cx: Float, cy: Float, hw: Float, hh: Float): Boolean {
        val l = cx - hw; val r = cx + hw; val t = cy - hh; val b = cy + hh
        val steps = 12
        for (i in 0..steps) {
            val u = i / steps.toFloat()
            val x = l + (r - l) * u; val y = t + (b - t) * u
            if (!contains(x, t) || !contains(x, b) || !contains(l, y) || !contains(r, y)) return false
        }
        // A notch of a non-convex shape (a clover's, a heart's cleft) may reach into the box without touching its sides.
        for (i in 0 until size) if (xs[i] > l && xs[i] < r && ys[i] > t && ys[i] < b) return false
        return true
    }
}

/** The one uniform scale and shift that fits an outline into the unit square. */
class Normalization(private val left: Float, private val top: Float, val side: Float, private val offsetX: Float, private val offsetY: Float) {
    fun x(v: Float) = (v - left) / side + offsetX
    fun y(v: Float) = (v - top) / side + offsetY
}

/**
 * Where a label goes: [cx], [cy] its centre and [height] its ink height (the digits' cap height),
 * all in the footprint's unit square.
 */
data class LabelFit(val cx: Float, val cy: Float, val height: Float, val width: Float)

object BadgeGeometry {
    /** Space kept free around the label inside the shape, as a share of the footprint. */
    const val CLEARANCE = 0.05f

    /** The overflow icon is open line work, so it can sit closer to the edge than a digit. */
    const val ICON_CLEARANCE = 0.035f

    /** No label is taller than this share of the footprint, however roomy the shape. */
    const val MAX_HEIGHT = 0.5f

    /**
     * How much of its footprint a shape fills. The arch (2) is the one Material shape that is bulky around a single
     * digit, so it is drawn 20% smaller; the label keeps its size, so it sits snugger in it.
     */
    fun shapeScale(shape: BadgeShape): Float = if (shape == BadgeShape.Arch) 0.8f else 1f

    /** The digits' height as a share of the footprint: one size per digit count, whatever the shape. */
    fun digitHeight(digits: Int): Float = if (digits <= 1) 0.44f else 0.34f

    private data class Candidate(val x: Float, val y: Float, val score: Float)

    /**
     * Places a label box of [aspect] (ink width / ink height) in [outline] (already normalized).
     *
     * With a [height] it keeps that height and looks for the centre with the most air around the box
     * (the shape's optical centre for this label). Without one it takes the largest box that keeps
     * [clearance] around it. Either way, among near-equal places the one nearest the area centroid wins,
     * so a roomy shape's label sits at its middle and a symmetric shape's exactly on its axis.
     */
    fun fit(outline: Outline, aspect: Float, height: Float? = null, clearance: Float = CLEARANCE): LabelFit {
        val (gx, gy) = outline.centroid()

        /** The box height (when [height] is null) or the margin (when it is not) possible at a centre. */
        fun score(cx: Float, cy: Float): Float {
            if (!outline.contains(cx, cy)) return 0f
            var lo = 0f
            var hi = if (height == null) MAX_HEIGHT else 0.3f
            fun fits(v: Float) = if (height == null) outline.containsBox(cx, cy, aspect * v / 2 + clearance, v / 2 + clearance)
            else outline.containsBox(cx, cy, aspect * height / 2 + v, height / 2 + v)
            if (fits(hi)) return hi
            repeat(14) {
                val mid = (lo + hi) / 2
                if (fits(mid)) lo = mid else hi = mid
            }
            return lo
        }

        // Coarse pass over a grid, then refine around the winners.
        val coarse = ArrayList<Candidate>()
        val n = 28
        for (iy in 1 until n) for (ix in 1 until n) {
            val s = score(ix / n.toFloat(), iy / n.toFloat())
            if (s > 0f) coarse += Candidate(ix / n.toFloat(), iy / n.toFloat(), s)
        }
        val coarseBest = coarse.maxOfOrNull { it.score } ?: return LabelFit(gx, gy, 0f, 0f)
        val fine = ArrayList<Candidate>()
        val step = 1f / n
        for (c in coarse) {
            if (c.score < coarseBest * 0.8f) continue
            for (dy in -3..3) for (dx in -3..3) {
                val x = c.x + dx * step / 4; val y = c.y + dy * step / 4
                val s = score(x, y)
                if (s > 0f) fine += Candidate(x, y, s)
            }
        }
        val best = fine.maxOf { it.score }
        val tolerance = if (height == null) best * 0.03f else 0.006f
        val pick = fine.filter { it.score >= best - tolerance }.minBy { (it.x - gx) * (it.x - gx) + (it.y - gy) * (it.y - gy) }
        val h = height ?: pick.score
        return LabelFit(pick.x, pick.y, h, h * aspect)
    }

    /** The margin around a [height] label centred at its fit: how much air it has (negative: it does not fit). */
    fun margin(outline: Outline, fit: LabelFit): Float {
        if (!outline.containsBox(fit.cx, fit.cy, fit.width / 2, fit.height / 2)) return -1f
        var lo = 0f
        var hi = 0.3f
        repeat(16) {
            val mid = (lo + hi) / 2
            if (outline.containsBox(fit.cx, fit.cy, fit.width / 2 + mid, fit.height / 2 + mid)) lo = mid else hi = mid
        }
        return lo
    }
}

/** The Material shape a running-subagent count is drawn in. */
enum class BadgeShape {
    Pill, Arch, Triangle, Diamond, Pentagon, Gem, Cookie7Sided, Clover8Leaf, PuffyDiamond, ClamShell, Puffy, Heart;

    companion object {
        /** 1 Pill, 2 Arch, 3 Triangle, 4 Diamond, 5 Pentagon, 6 Gem, 7 Cookie, 8 Clover, 9 Puffy diamond, 10-20 Clam shell, 21-99 Puffy, 100+ Heart. */
        fun of(count: UInt): BadgeShape = when (count) {
            0u, 1u -> Pill
            2u -> Arch
            3u -> Triangle
            4u -> Diamond
            5u -> Pentagon
            6u -> Gem
            7u -> Cookie7Sided
            8u -> Clover8Leaf
            9u -> PuffyDiamond
            in 10u..20u -> ClamShell
            in 21u..99u -> Puffy
            else -> Heart
        }
    }
}
