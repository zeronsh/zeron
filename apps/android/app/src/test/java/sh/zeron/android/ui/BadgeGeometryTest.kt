package sh.zeron.android.ui

import androidx.compose.material3.MaterialShapes
import androidx.graphics.shapes.RoundedPolygon
import java.io.File
import org.junit.Assert.*
import org.junit.Test

/** The pure geometry of the count badges, on the real Material polygons. */
class BadgeGeometryTest {
    // Geist Bold's widest ink over digit height (measured from the font file): one digit, two digits.
    private val aspect1 = 0.852f
    private val aspect2 = 1.804f
    private val iconAspect = 2.2f

    private fun polygon(shape: BadgeShape): RoundedPolygon = when (shape) {
        BadgeShape.Pill -> MaterialShapes.Pill
        BadgeShape.Arch -> MaterialShapes.Arch
        BadgeShape.Triangle -> MaterialShapes.Triangle
        BadgeShape.Diamond -> MaterialShapes.Diamond
        BadgeShape.Pentagon -> MaterialShapes.Pentagon
        BadgeShape.Gem -> MaterialShapes.Gem
        BadgeShape.Cookie7Sided -> MaterialShapes.Cookie7Sided
        BadgeShape.Clover8Leaf -> MaterialShapes.Clover8Leaf
        BadgeShape.PuffyDiamond -> MaterialShapes.PuffyDiamond
        BadgeShape.ClamShell -> MaterialShapes.ClamShell
        BadgeShape.Puffy -> MaterialShapes.Puffy
        BadgeShape.Heart -> MaterialShapes.Heart
    }

    private fun outline(shape: BadgeShape) = polygon(shape).art().outline
    private fun fitDigits(shape: BadgeShape, digits: Int): LabelFit =
        BadgeGeometry.fit(outline(shape), if (digits == 1) aspect1 else aspect2, BadgeGeometry.digitHeight(digits))

    @Test fun shapesMapToTheRequestedCounts() {
        assertEquals(BadgeShape.Pill, BadgeShape.of(1u))
        assertEquals(BadgeShape.Arch, BadgeShape.of(2u))
        assertEquals(BadgeShape.Triangle, BadgeShape.of(3u))
        assertEquals(BadgeShape.Diamond, BadgeShape.of(4u))
        assertEquals(BadgeShape.Pentagon, BadgeShape.of(5u))
        assertEquals(BadgeShape.Gem, BadgeShape.of(6u))
        assertEquals(BadgeShape.Cookie7Sided, BadgeShape.of(7u))
        assertEquals(BadgeShape.Clover8Leaf, BadgeShape.of(8u))
        assertEquals(BadgeShape.PuffyDiamond, BadgeShape.of(9u))
        for (n in 10u..20u) assertEquals(BadgeShape.ClamShell, BadgeShape.of(n))
        for (n in 21u..99u) assertEquals(BadgeShape.Puffy, BadgeShape.of(n))
        assertEquals(BadgeShape.Heart, BadgeShape.of(100u))
        assertEquals(BadgeShape.Heart, BadgeShape.of(UInt.MAX_VALUE))
    }

    @Test fun everyShapeIsScaledUniformlyIntoTheSameSquare() {
        for (shape in BadgeShape.entries) {
            val b = outline(shape).bounds()
            val w = b[2] - b[0]
            val h = b[3] - b[1]
            // The larger side spans the footprint exactly; nothing is stretched, so the smaller side is whatever it naturally is.
            assertEquals("$shape larger side", 1f, maxOf(w, h), 0.01f)
            // Centred in the footprint on both axes.
            assertEquals("$shape x centre", 0.5f, (b[0] + b[2]) / 2, 0.01f)
            assertEquals("$shape y centre", 0.5f, (b[1] + b[3]) / 2, 0.01f)
        }
    }

    @Test fun normalizationKeepsAspectRatio() {
        val raw = Outline(floatArrayOf(10f, 30f, 30f, 10f), floatArrayOf(0f, 0f, 10f, 10f)) // 20 x 10
        val n = raw.normalized().bounds()
        assertEquals(1f, n[2] - n[0], 1e-5f)
        assertEquals(0.5f, n[3] - n[1], 1e-5f)
        assertEquals(0.25f, n[1], 1e-5f)
    }

    @Test fun areaCentroidAndContainmentOfASquare() {
        val sq = Outline(floatArrayOf(0f, 1f, 1f, 0f), floatArrayOf(0f, 0f, 1f, 1f))
        assertEquals(1f, sq.area(), 1e-6f)
        assertEquals(0.5f to 0.5f, sq.centroid())
        assertTrue(sq.contains(0.5f, 0.5f)); assertFalse(sq.contains(1.5f, 0.5f))
        assertEquals(0.25f, sq.distanceToEdge(0.25f, 0.5f), 1e-6f)
        assertTrue(sq.containsBox(0.5f, 0.5f, 0.4f, 0.4f)); assertFalse(sq.containsBox(0.5f, 0.5f, 0.6f, 0.4f))
    }

    @Test fun everyLabelFitsEveryShapeItIsUsedInWithRoomToSpare() {
        for (shape in listOf(BadgeShape.Pill, BadgeShape.Arch, BadgeShape.Triangle, BadgeShape.Diamond, BadgeShape.Pentagon, BadgeShape.Gem, BadgeShape.Cookie7Sided, BadgeShape.Clover8Leaf, BadgeShape.PuffyDiamond)) {
            val fit = fitDigits(shape, 1)
            assertEquals(BadgeGeometry.digitHeight(1), fit.height, 1e-6f)
            assertTrue("$shape one digit margin ${BadgeGeometry.margin(outline(shape), fit)}", BadgeGeometry.margin(outline(shape), fit) >= 0.05f)
        }
        for (shape in listOf(BadgeShape.ClamShell, BadgeShape.Puffy)) {
            val fit = fitDigits(shape, 2)
            assertTrue("$shape two digits margin ${BadgeGeometry.margin(outline(shape), fit)}", BadgeGeometry.margin(outline(shape), fit) >= 0.05f)
        }
        val heart = BadgeGeometry.fit(outline(BadgeShape.Heart), iconAspect, clearance = BadgeGeometry.ICON_CLEARANCE)
        assertTrue(BadgeGeometry.margin(outline(BadgeShape.Heart), heart) >= BadgeGeometry.ICON_CLEARANCE - 0.005f)
        assertTrue("icon is not a speck", heart.width > 0.4f)
    }

    @Test fun symmetricShapesCentreTheLabelOnTheirAxes() {
        for (shape in BadgeShape.entries) {
            val digits = if (shape == BadgeShape.ClamShell || shape == BadgeShape.Puffy) 2 else 1
            assertEquals("$shape x", 0.5f, fitDigits(shape, digits).cx, 0.02f)
        }
        for (shape in listOf(BadgeShape.Diamond, BadgeShape.Clover8Leaf, BadgeShape.PuffyDiamond, BadgeShape.Pill)) {
            assertEquals("$shape y", 0.5f, fitDigits(shape, 1).cy, 0.02f)
        }
        for (shape in listOf(BadgeShape.ClamShell, BadgeShape.Puffy)) assertEquals("$shape y", 0.5f, fitDigits(shape, 2).cy, 0.02f)
    }

    @Test fun theTrianglesLabelSitsInItsWideLowerBody() {
        val o = outline(BadgeShape.Triangle)
        val fit = fitDigits(BadgeShape.Triangle, 1)
        assertTrue("below the middle: ${fit.cy}", fit.cy > 0.58f)
        // ...and at the area centroid's height or lower, where the triangle is widest.
        assertTrue(fit.cy >= o.centroid().second - 0.02f)
    }

    @Test fun theHeartsLabelSitsBelowItsLobesButAboveItsPoint() {
        val o = outline(BadgeShape.Heart)
        val fit = BadgeGeometry.fit(o, iconAspect, clearance = BadgeGeometry.ICON_CLEARANCE)
        val b = o.bounds()
        assertTrue(fit.cy > b[1] + 0.25f * (b[3] - b[1]))
        assertTrue(fit.cy < b[1] + 0.6f * (b[3] - b[1]))
    }

    @Test fun theLabelCentreIsInsideTheShapeAndNearTheAreaCentroid() {
        for (shape in BadgeShape.entries) {
            val o = outline(shape)
            val digits = if (shape == BadgeShape.ClamShell || shape == BadgeShape.Puffy) 2 else 1
            val fit = fitDigits(shape, digits)
            assertTrue("$shape centre inside", o.contains(fit.cx, fit.cy))
            val (gx, gy) = o.centroid()
            assertEquals("$shape x vs centroid", gx, fit.cx, 0.05f)
            assertEquals("$shape y vs centroid", gy, fit.cy, 0.13f)
        }
    }

    @Test fun digitHeightsAreOneSizePerDigitCount() {
        assertEquals(BadgeGeometry.digitHeight(1), BadgeGeometry.digitHeight(1))
        assertTrue(BadgeGeometry.digitHeight(1) in 0.40f..0.46f)
        assertTrue(BadgeGeometry.digitHeight(2) < BadgeGeometry.digitHeight(1))
        // Every shape that carries a one-digit label gets that one height, whatever its area.
        for (shape in BadgeShape.entries.take(9)) assertEquals(BadgeGeometry.digitHeight(1), fitDigits(shape, 1).height, 0f)
    }

    @Test fun theFitMaximisesAirAroundTheLabel() {
        val o = outline(BadgeShape.Pentagon)
        val best = fitDigits(BadgeShape.Pentagon, 1)
        val bestMargin = BadgeGeometry.margin(o, best)
        for ((dx, dy) in listOf(0.08f to 0f, -0.08f to 0f, 0f to 0.08f, 0f to -0.08f)) {
            val moved = best.copy(cx = best.cx + dx, cy = best.cy + dy)
            assertTrue("moving by ($dx, $dy) must not give more air", BadgeGeometry.margin(o, moved) <= bestMargin + 0.01f)
        }
    }

    @Test fun dumpFitsForTheMeasuringScript() {
        // scripts/android/measure-badges.py compares screenshots against these (the optical centre per shape).
        val out = System.getenv("ZERON_BADGE_FITS") ?: return
        val sb = StringBuilder("{")
        for (shape in BadgeShape.entries) {
            val o = outline(shape)
            val f1 = BadgeGeometry.fit(o, aspect1, BadgeGeometry.digitHeight(1))
            val f2 = BadgeGeometry.fit(o, aspect2, BadgeGeometry.digitHeight(2))
            val f3 = BadgeGeometry.fit(o, iconAspect, clearance = BadgeGeometry.ICON_CLEARANCE)
            fun a(f: LabelFit) = "[${f.cx},${f.cy},${f.height}]"
            sb.append("\"${shape.name}\":{\"f1\":${a(f1)},\"f2\":${a(f2)},\"f3\":${a(f3)}},")
        }
        sb.setLength(sb.length - 1); sb.append("}")
        File(out).writeText(sb.toString())
    }
}
