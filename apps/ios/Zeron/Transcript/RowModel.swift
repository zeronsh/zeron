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
        self.lines = lines
        self.runsByLayer = runs
        self.boxesByLayer = boxes
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
            if case .fresh = pass {} else {
            for i in boxesByLayer[layer] {
                let b = display.boxes[i]
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
            }
            ctx.textMatrix = .identity
            for i in runsByLayer[layer] {
                guard let line = lines[i] else { continue }
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
