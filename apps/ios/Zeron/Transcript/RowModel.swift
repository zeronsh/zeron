import CoreText
import UIKit

/// A display list plus everything expensive to derive from it (CTLines),
/// built once per (row, version, width) — safely off the main thread.
/// Lines take their color from the context, so theme switches only repaint.
final class RowModel: @unchecked Sendable {
    let display: RowDisplay
    let lines: [CTLine?]
    /// Runs/boxes grouped by scroller (index 0 = the row canvas itself).
    let runsByLayer: [[Int]]
    let boxesByLayer: [[Int]]
    /// Math glyphs per layer.
    let glyphsByLayer: [[Int]]
    /// The math fonts those glyphs name, by table index — resolved here, off
    /// the main thread (a face's first use copies its ~1–2 MB out of the
    /// core), so drawing only reads loaded fonts.
    private let mathFonts: [UInt8: CTFont]
    /// Overflow fades per layer, each with the runs it masks. Those runs are
    /// painted through the fade instead of in the plain pass.
    let fadesByLayer: [[(fade: Int, runs: [Int])]]
    private let fadedRun: Set<Int>

    init(display: RowDisplay, fonts: StyleFonts) {
        self.display = display
        let text = display.text as NSString
        var lines: [CTLine?] = []
        lines.reserveCapacity(display.runs.count)
        let layers = display.scrollers.count + 1
        var runs = Array(repeating: [Int](), count: layers)
        var boxes = Array(repeating: [Int](), count: layers)
        for (i, run) in display.runs.enumerated() {
            let range = NSRange(location: Int(run.start), length: Int(run.len))
            if range.location + range.length <= text.length, let font = fonts.font(run.style) {
                let s = NSAttributedString(string: text.substring(with: range), attributes: [
                    .font: font.font,
                    .ligature: font.ligatures ? 1 : 0,
                    NSAttributedString.Key(kCTForegroundColorFromContextAttributeName as String): true,
                ])
                lines.append(CTLineCreateWithAttributedString(s))
            } else {
                lines.append(nil)
            }
            runs[run.scroller.map { Int($0) + 1 } ?? 0].append(i)
        }
        for (i, box) in display.boxes.enumerated() {
            boxes[box.scroller.map { Int($0) + 1 } ?? 0].append(i)
        }
        var glyphs = Array(repeating: [Int](), count: layers)
        var mathFonts: [UInt8: CTFont] = [:]
        var unknown = Set<UInt8>()
        for (i, g) in display.glyphs.enumerated() {
            glyphs[g.scroller.map { Int($0) + 1 } ?? 0].append(i)
            if mathFonts[g.font] == nil, !unknown.contains(g.font) {
                if let font = Fonts.mathFont(g.font) { mathFonts[g.font] = font } else { unknown.insert(g.font) }
            }
        }
        self.glyphsByLayer = glyphs
        self.mathFonts = mathFonts
        var fades = Array(repeating: [(fade: Int, runs: [Int])](), count: layers)
        var faded = Set<Int>()
        for (fi, f) in display.fades.enumerated() {
            let layer = f.scroller.map { Int($0) + 1 } ?? 0
            let masked = runs[layer].filter { i in
                let r = display.runs[i]
                guard !faded.contains(i), r.baseline > f.y, r.baseline <= f.y + f.h + 0.5 else { return false }
                return f.edge == .bottom || r.x < f.x + f.w
            }
            faded.formUnion(masked)
            fades[layer].append((fi, masked))
        }
        self.lines = lines
        self.runsByLayer = runs
        self.boxesByLayer = boxes
        self.fadesByLayer = fades
        self.fadedRun = faded
    }

    /// Alpha ramp used to erase text across a fade (clear → opaque).
    private static let ramp = CGGradient(colorSpace: CGColorSpaceCreateDeviceGray(), colorComponents: [0, 0, 0, 1], locations: [0, 1], count: 2)!
    private static let rampBottom = CGGradient(colorSpace: CGColorSpaceCreateDeviceGray(), colorComponents: [0, 0, 0, 0.92], locations: [0, 1], count: 2)!

    private func drawRun(_ i: Int, _ line: CTLine, in ctx: CGContext, hairline: CGFloat) {
        let run = display.runs[i]
        let color = Palette.color(run.color).cgColor
        ctx.saveGState()
        ctx.setFillColor(color)
        ctx.translateBy(x: CGFloat(run.x), y: CGFloat(run.baseline))
        ctx.scaleBy(x: 1, y: -1)
        ctx.textPosition = .zero
        CTLineDraw(line, ctx)
        ctx.restoreGState()
        if run.decoration != .none {
            let y = run.decoration == .underline ? CGFloat(run.baseline) + 2 : CGFloat(run.baseline) - 5
            ctx.setFillColor(color)
            ctx.fill(CGRect(x: CGFloat(run.x), y: y, width: CGFloat(run.width), height: max(hairline, 1)))
        }
    }

    /// One math glyph, its origin on (`x`, `y`); flipped like a run, and
    /// scaled from the face's reference size to the glyph's.
    private func drawGlyph(_ i: Int, in ctx: CGContext) {
        let g = display.glyphs[i]
        guard let font = mathFonts[g.font] else { return }
        var glyph = CGGlyph(g.glyph)
        var origin = CGPoint.zero
        let scale = CGFloat(g.size) / Fonts.mathReferenceSize
        ctx.saveGState()
        ctx.setFillColor(Palette.color(g.color).cgColor)
        ctx.translateBy(x: CGFloat(g.x), y: CGFloat(g.y))
        ctx.scaleBy(x: scale, y: -scale)
        // Text matrix and position aren't graphics state: a prior CTLineDraw
        // leaves the position at its line's end, which would shift the glyph.
        ctx.textMatrix = .identity
        ctx.textPosition = .zero
        CTFontDrawGlyphs(font, &glyph, &origin, 1, ctx)
        ctx.restoreGState()
    }

    /// Canvas-layer runs whose line box sits inside `rect`, in one color
    /// (the shimmer highlight re-draws the title this way).
    func drawRuns(in rect: CGRect, color: UIColor, ctx: CGContext, traits: UITraitCollection) {
        traits.performAsCurrent {
            ctx.textMatrix = .identity
            ctx.clip(to: rect)
            let cg = color.cgColor
            for i in runsByLayer[0] {
                guard let line = lines[i] else { continue }
                let r = display.runs[i]
                let baseline = CGFloat(r.baseline)
                guard baseline > rect.minY, baseline <= rect.maxY + 2, CGFloat(r.x) < rect.maxX else { continue }
                ctx.saveGState()
                ctx.setFillColor(cg)
                ctx.translateBy(x: CGFloat(r.x), y: baseline)
                ctx.scaleBy(x: 1, y: -1)
                ctx.textPosition = .zero
                CTLineDraw(line, ctx)
                ctx.restoreGState()
            }
        }
    }

    /// Runs under a fade: painted into a transparency layer, clipped at a
    /// trailing fade's end, then erased along the ramp — the text itself
    /// fades, whatever the background.
    private func drawFades(layer: Int, in ctx: CGContext, hairline: CGFloat) {
        for (fi, runs) in fadesByLayer[layer] where !runs.isEmpty {
            let f = display.fades[fi]
            let rect = CGRect(x: CGFloat(f.x), y: CGFloat(f.y), width: CGFloat(f.w), height: CGFloat(f.h))
            ctx.saveGState()
            if f.edge == .trailing {
                ctx.clip(to: CGRect(x: -100_000, y: rect.minY - 40, width: 100_000 + rect.maxX, height: rect.height + 80))
            }
            ctx.beginTransparencyLayer(auxiliaryInfo: nil)
            for i in runs {
                if let line = lines[i] { drawRun(i, line, in: ctx, hairline: hairline) }
            }
            ctx.setBlendMode(.destinationOut)
            switch f.edge {
            case .trailing:
                ctx.saveGState()
                ctx.clip(to: CGRect(x: rect.minX, y: rect.minY - 40, width: rect.width, height: rect.height + 80))
                ctx.drawLinearGradient(Self.ramp, start: CGPoint(x: rect.minX, y: 0), end: CGPoint(x: rect.maxX, y: 0), options: [])
                ctx.restoreGState()
            case .bottom:
                ctx.saveGState()
                ctx.clip(to: CGRect(x: rect.minX - 40, y: rect.minY, width: rect.width + 80, height: rect.height + 40))
                ctx.drawLinearGradient(Self.rampBottom, start: CGPoint(x: 0, y: rect.minY), end: CGPoint(x: 0, y: rect.maxY), options: [.drawsAfterEndLocation])
                ctx.restoreGState()
            }
            ctx.endTransparencyLayer()
            ctx.restoreGState()
        }
    }

    /// Paint one layer (0 = row canvas, n = scroller n-1) in its coordinates.
    /// Which runs to paint: `veilFrom` splits a streaming row into settled
    /// text (start < veilFrom) and freshly appended text (the fading overlay).
    enum Pass {
        case all
        case settled(veilFrom: UInt32)
        case fresh(veilFrom: UInt32)
    }

    func draw(layer: Int, in ctx: CGContext, traits: UITraitCollection, hairline: CGFloat, pass: Pass = .all) {
        traits.performAsCurrent {
            // A formula's fills (rules, decorations) veil with its glyphs;
            // every other box is settled chrome.
            for i in boxesByLayer[layer] {
                let b = display.boxes[i]
                switch pass {
                case .all: break
                case let .settled(from): if let start = b.start, start >= from { continue }
                case let .fresh(from): guard let start = b.start, start >= from else { continue }
                }
                let rect = CGRect(x: CGFloat(b.x), y: CGFloat(b.y), width: CGFloat(b.w), height: CGFloat(b.h))
                let color = Palette.color(b.color).cgColor
                switch b.style {
                case .fill:
                    let path = CGPath(roundedRect: rect, cornerWidth: CGFloat(b.radius), cornerHeight: CGFloat(b.radius), transform: nil)
                    ctx.setFillColor(color)
                    ctx.addPath(path)
                    ctx.fillPath()
                case .hairline:
                    let inset = rect.insetBy(dx: hairline / 2, dy: hairline / 2)
                    let r = max(0, CGFloat(b.radius) - hairline / 2)
                    ctx.setStrokeColor(color)
                    ctx.setLineWidth(hairline)
                    ctx.addPath(CGPath(roundedRect: inset, cornerWidth: r, cornerHeight: r, transform: nil))
                    ctx.strokePath()
                }
            }
            ctx.textMatrix = .identity
            for i in runsByLayer[layer] {
                guard let line = lines[i], !fadedRun.contains(i) else { continue }
                let run = display.runs[i]
                // Split a run straddling the veil boundary at the glyph edge.
                var clip: CGRect?
                switch pass {
                case .all:
                    break
                case let .settled(from):
                    if run.start >= from { continue }
                    if run.start + run.len > from {
                        let dx = CTLineGetOffsetForStringIndex(line, CFIndex(from - run.start), nil)
                        clip = CGRect(x: CGFloat(run.x), y: -10_000, width: dx, height: 20_000)
                    }
                case let .fresh(from):
                    if run.start + run.len <= from { continue }
                    if run.start < from {
                        let dx = CTLineGetOffsetForStringIndex(line, CFIndex(from - run.start), nil)
                        clip = CGRect(x: CGFloat(run.x) + dx, y: -10_000, width: 10_000, height: 20_000)
                    }
                }
                if let clip {
                    ctx.saveGState()
                    ctx.clip(to: clip)
                }
                defer { if clip != nil { ctx.restoreGState() } }
                drawRun(i, line, in: ctx, hairline: hairline)
            }
            // A formula is one placeholder in the text: it veils whole.
            for i in glyphsByLayer[layer] {
                let start = display.glyphs[i].start
                switch pass {
                case .all: break
                case let .settled(from): if start >= from { continue }
                case let .fresh(from): if start < from { continue }
                }
                drawGlyph(i, in: ctx)
            }
            if case .fresh = pass {} else { drawFades(layer: layer, in: ctx, hairline: hairline) }
        }
    }
}

/// style id → CTFont for one transcript (ids are per layout view).
final class StyleFonts: @unchecked Sendable {
    struct Entry {
        let font: CTFont
        let ligatures: Bool
    }

    private let lock = NSLock()
    private var fonts: [UInt16: Entry] = [:]
    private(set) var count = 0

    func update(_ styles: [StyleDesc]) {
        lock.lock()
        defer { lock.unlock() }
        for s in styles where fonts[s.id] == nil {
            fonts[s.id] = Entry(font: Fonts.ctFont(s.face, size: s.size), ligatures: s.ligatures)
        }
        count = fonts.count
    }

    func font(_ id: UInt16) -> Entry? {
        lock.lock()
        defer { lock.unlock() }
        return fonts[id]
    }
}
