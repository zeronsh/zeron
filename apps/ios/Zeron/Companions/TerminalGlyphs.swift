import UIKit

/// Box drawing (U+2500–257F), block elements (U+2580–259F) and the `⏺`
/// bullet, drawn instead of shaped. Geist Mono has none of them, and a
/// fallback font's glyphs neither fill the roomy line height nor share the
/// cell width: borders broke between rows and Claude Code's block-art mascot
/// fell apart. Painted from the cell's own rect, snapped to device pixels,
/// neighbors meet exactly; `⏺` would otherwise fall back to the color emoji.
enum TerminalGlyphs {
    static func draws(_ scalar: Unicode.Scalar) -> Bool {
        (0x2500...0x259F).contains(scalar.value) || scalar.value == 0x23FA
    }

    /// `cell` in UIKit coordinates (y down); `baseline` and `xHeight` place
    /// the bullet like a text glyph.
    static func draw(_ scalar: Unicode.Scalar, in cell: CGRect, baseline: CGFloat, xHeight: CGFloat, color: UIColor, ctx: CGContext, scale: CGFloat) {
        let snap = { (v: CGFloat) in (v * scale).rounded() / scale }
        let rect = CGRect(x: snap(cell.minX), y: snap(cell.minY), width: snap(cell.maxX) - snap(cell.minX), height: snap(cell.maxY) - snap(cell.minY))
        ctx.saveGState()
        defer { ctx.restoreGState() }
        ctx.setFillColor(color.cgColor)
        ctx.setStrokeColor(color.cgColor)
        let v = scalar.value
        if v == 0x23FA {
            let d = snap(min(rect.width * 0.62, xHeight * 1.15))
            ctx.fillEllipse(in: CGRect(x: rect.midX - d / 2, y: baseline - xHeight / 2 - d / 2, width: d, height: d))
        } else if v >= 0x2580 {
            block(v, rect, ctx, snap)
        } else {
            box(v, rect, ctx, snap, scale)
        }
    }

    // MARK: Block elements

    private static func block(_ v: UInt32, _ r: CGRect, _ ctx: CGContext, _ snap: (CGFloat) -> CGFloat) {
        func fill(_ x0: CGFloat, _ y0: CGFloat, _ x1: CGFloat, _ y1: CGFloat) {
            let a = snap(r.minX + r.width * x0), b = snap(r.minY + r.height * y0)
            ctx.fill(CGRect(x: a, y: b, width: snap(r.minX + r.width * x1) - a, height: snap(r.minY + r.height * y1) - b))
        }
        switch v {
        case 0x2580: fill(0, 0, 1, 0.5)
        case 0x2581...0x2588: fill(0, 1 - CGFloat(v - 0x2580) / 8, 1, 1)
        case 0x2589...0x258F: fill(0, 0, CGFloat(0x2590 - v) / 8, 1)
        case 0x2590: fill(0.5, 0, 1, 1)
        case 0x2591...0x2593:
            ctx.setAlpha(CGFloat(v - 0x2590) / 4)
            fill(0, 0, 1, 1)
        case 0x2594: fill(0, 0, 1, 1.0 / 8)
        case 0x2595: fill(7.0 / 8, 0, 1, 1)
        default:
            // Quadrants: upper left, upper right, lower left, lower right.
            let masks: [UInt32: Int] = [0x2596: 4, 0x2597: 8, 0x2598: 1, 0x2599: 13, 0x259A: 9, 0x259B: 7, 0x259C: 11, 0x259D: 2, 0x259E: 6, 0x259F: 14]
            let mask = masks[v] ?? 0
            if mask & 1 != 0 { fill(0, 0, 0.5, 0.5) }
            if mask & 2 != 0 { fill(0.5, 0, 1, 0.5) }
            if mask & 4 != 0 { fill(0, 0.5, 0.5, 1) }
            if mask & 8 != 0 { fill(0.5, 0.5, 1, 1) }
        }
    }

    // MARK: Box drawing

    /// Per code point from U+2500: the left, up, right and down arms —
    /// 0 none, 1 light, 2 heavy, 3 double. `.` marks the arcs and diagonals.
    private static let arms: [String] = [
        "1010", "2020", "0101", "0202", "1010", "2020", "0101", "0202", // ─━│┃┄┅┆┇
        "1010", "2020", "0101", "0202", "0011", "0021", "0012", "0022", // ┈┉┊┋┌┍┎┏
        "1001", "2001", "1002", "2002", "0110", "0120", "0210", "0220", // ┐┑┒┓└┕┖┗
        "1100", "2100", "1200", "2200", "0111", "0121", "0211", "0112", // ┘┙┚┛├┝┞┟
        "0212", "0221", "0122", "0222", "1101", "2101", "1201", "1102", // ┠┡┢┣┤┥┦┧
        "1202", "2201", "2102", "2202", "1011", "2011", "1021", "2021", // ┨┩┪┫┬┭┮┯
        "1012", "2012", "1022", "2022", "1110", "2110", "1120", "2120", // ┰┱┲┳┴┵┶┷
        "1210", "2210", "1220", "2220", "1111", "2111", "1121", "2121", // ┸┹┺┻┼┽┾┿
        "1211", "1112", "1212", "2211", "1221", "2112", "1122", "2221", // ╀╁╂╃╄╅╆╇
        "2122", "2212", "1222", "2222", "1010", "2020", "0101", "0202", // ╈╉╊╋╌╍╎╏
        "3030", "0303", "0031", "0013", "0033", "3001", "1003", "3003", // ═║╒╓╔╕╖╗
        "0130", "0310", "0330", "3100", "1300", "3300", "0131", "0313", // ╘╙╚╛╜╝╞╟
        "0333", "3101", "1303", "3303", "3031", "1013", "3033", "3130", // ╠╡╢╣╤╥╦╧
        "1310", "3330", "3131", "1313", "3333", ".", ".", ".",          // ╨╩╪╫╬╭╮╯
        ".", ".", ".", ".", "1000", "0100", "0010", "0001",             // ╰╱╲╳╴╵╶╷
        "2000", "0200", "0020", "0002", "1020", "0102", "2010", "0201", // ╸╹╺╻╼╽╾╿
    ]

    private static func box(_ v: UInt32, _ r: CGRect, _ ctx: CGContext, _ snap: (CGFloat) -> CGFloat, _ scale: CGFloat) {
        let light = max(1 / scale, snap(r.width / 9))
        let heavy = max(2 / scale, snap(light * 2))
        let gap = max(1 / scale, light)
        let cx = snap(r.midX - light / 2) + light / 2
        let cy = snap(r.midY - light / 2) + light / 2
        let code = arms[Int(v - 0x2500)]
        guard code != "." else {
            ctx.setLineWidth(light)
            let path = CGMutablePath()
            switch v {
            case 0x256D...0x2570:
                // Arcs: down-right, down-left, up-left, up-right.
                let right = v == 0x256D || v == 0x2570, down = v <= 0x256E
                let radius = min(r.width, r.height) / 2
                let endX = right ? r.maxX : r.minX, endY = down ? r.maxY : r.minY
                path.move(to: CGPoint(x: cx, y: endY))
                path.addArc(tangent1End: CGPoint(x: cx, y: cy), tangent2End: CGPoint(x: endX, y: cy), radius: radius)
                path.addLine(to: CGPoint(x: endX, y: cy))
            default:
                if v != 0x2572 {
                    path.move(to: CGPoint(x: r.maxX, y: r.minY))
                    path.addLine(to: CGPoint(x: r.minX, y: r.maxY))
                }
                if v != 0x2571 {
                    path.move(to: CGPoint(x: r.minX, y: r.minY))
                    path.addLine(to: CGPoint(x: r.maxX, y: r.maxY))
                }
            }
            ctx.addPath(path)
            ctx.strokePath()
            return
        }
        let digits = code.map { Int(String($0))! }
        let (left, up, right, down) = (digits[0], digits[1], digits[2], digits[3])
        let width = { (style: Int) in style == 2 ? heavy : light }
        // How far a straight arm reaches past the center to square off a
        // join with the perpendicular arms (half the thickest, or the outer
        // line of a double one).
        func reach(_ a: Int, _ b: Int) -> CGFloat {
            let style = max(a, b)
            if a == 3 || b == 3 { return gap + light / 2 }
            return style == 0 ? 0 : width(style) / 2
        }
        let vReach = reach(up, down), hReach = reach(left, right)
        func horizontal(_ style: Int, toward end: CGFloat) {
            guard style != 0 else { return }
            let sign: CGFloat = end > cx ? 1 : -1
            if style == 3 {
                for side: CGFloat in [-1, 1] {
                    // The arm on this line's side (up for the top line).
                    let near = side < 0 ? up : down, far = side < 0 ? down : up
                    let start: CGFloat
                    if near != 0 { start = near == 3 ? gap - light / 2 : light / 2 }
                    else if far != 0 { start = -(far == 3 ? gap : 0) - light / 2 }
                    else { start = 0 }
                    let x0 = cx + sign * start
                    let y = cy + side * gap
                    ctx.fill(CGRect(x: min(x0, end), y: y - light / 2, width: abs(end - x0), height: light))
                }
            } else {
                let t = width(style)
                let x0 = cx - sign * vReach
                ctx.fill(CGRect(x: min(x0, end), y: cy - t / 2, width: abs(end - x0), height: t))
            }
        }
        func vertical(_ style: Int, toward end: CGFloat) {
            guard style != 0 else { return }
            let sign: CGFloat = end > cy ? 1 : -1
            if style == 3 {
                for side: CGFloat in [-1, 1] {
                    let near = side < 0 ? left : right, far = side < 0 ? right : left
                    let start: CGFloat
                    if near != 0 { start = near == 3 ? gap - light / 2 : light / 2 }
                    else if far != 0 { start = -(far == 3 ? gap : 0) - light / 2 }
                    else { start = 0 }
                    let y0 = cy + sign * start
                    let x = cx + side * gap
                    ctx.fill(CGRect(x: x - light / 2, y: min(y0, end), width: light, height: abs(end - y0)))
                }
            } else {
                let t = width(style)
                let y0 = cy - sign * hReach
                ctx.fill(CGRect(x: cx - t / 2, y: min(y0, end), width: t, height: abs(end - y0)))
            }
        }
        let dashes: Int? = switch v {
        case 0x2504...0x2507: 3
        case 0x2508...0x250B: 4
        case 0x254C...0x254F: 2
        default: nil
        }
        if let dashes {
            // Dashed lines: evenly spaced segments with a gap between.
            let horizontalLine = left != 0
            let t = width(max(left, up))
            let length = horizontalLine ? r.width : r.height
            let step = length / CGFloat(dashes)
            for i in 0..<dashes {
                let a = CGFloat(i) * step + step * 0.15, b = CGFloat(i + 1) * step - step * 0.15
                if horizontalLine {
                    ctx.fill(CGRect(x: snap(r.minX + a), y: cy - t / 2, width: snap(r.minX + b) - snap(r.minX + a), height: t))
                } else {
                    ctx.fill(CGRect(x: cx - t / 2, y: snap(r.minY + a), width: t, height: snap(r.minY + b) - snap(r.minY + a)))
                }
            }
            return
        }
        horizontal(left, toward: r.minX)
        horizontal(right, toward: r.maxX)
        vertical(up, toward: r.minY)
        vertical(down, toward: r.maxY)
    }
}
