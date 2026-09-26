import CoreText
import UIKit

/// The bundled faces. Rust measures the exact bytes CoreText draws with, so
/// measurement and rendering share one source of truth.
enum Fonts {
    static let files: [(FaceRole, String)] = [
        (.sans, "Geist"),
        (.sansMedium, "Geist-Medium"),
        (.sansSemibold, "Geist-SemiBold"),
        (.sansBold, "Geist-Bold"),
        (.sansItalic, "Geist-Italic"),
        (.sansMediumItalic, "Geist-MediumItalic"),
        (.sansSemiboldItalic, "Geist-SemiBoldItalic"),
        (.sansBoldItalic, "Geist-BoldItalic"),
        (.mono, "GeistMono"),
        (.monoMedium, "GeistMono-Medium"),
        (.monoSemibold, "GeistMono-SemiBold"),
        (.monoItalic, "GeistMono-Italic"),
    ]

    private static let registry: (faces: [FaceData], graphics: [FaceRole: CGFont]) = {
        var faces: [FaceData] = []
        var graphics: [FaceRole: CGFont] = [:]
        for (role, name) in files {
            guard let url = Bundle.main.url(forResource: name, withExtension: "ttf"),
                  let data = try? Data(contentsOf: url),
                  let provider = CGDataProvider(data: data as CFData),
                  let font = CGFont(provider)
            else { continue }
            CTFontManagerRegisterGraphicsFont(font, nil)
            faces.append(FaceData(role: role, bytes: data))
            graphics[role] = font
        }
        return (faces, graphics)
    }()

    static var faceData: [FaceData] { registry.faces }

    private static let lock = NSLock()
    nonisolated(unsafe) private static var cache: [FontKey: CTFont] = [:]

    private struct FontKey: Hashable {
        let face: FaceRole
        let centi: Int
    }

    /// Thread-safe: the Rust layout thread calls through `PlatformMeasurer`.
    static func ctFont(_ face: FaceRole, size: Float) -> CTFont {
        let key = FontKey(face: face, centi: Int((size * 100).rounded()))
        lock.lock()
        defer { lock.unlock() }
        if let font = cache[key] { return font }
        let font: CTFont
        if let graphic = registry.graphics[face] ?? registry.graphics[.sans] {
            font = CTFontCreateWithGraphicsFont(graphic, CGFloat(size), nil, nil)
        } else {
            font = CTFontCreateUIFontForLanguage(.system, CGFloat(size), nil)!
        }
        cache[key] = font
        return font
    }

    static func ui(_ face: FaceRole, _ size: CGFloat) -> UIFont {
        ctFont(face, size: Float(size)) as UIFont
    }
}

/// CoreText as ground truth for glyphs the bundled faces don't cover.
final class CoreTextMeasurer: PlatformMeasurer {
    func measure(face: FaceRole, size: Float, ligatures: Bool, text: String) -> Float {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: Fonts.ctFont(face, size: size),
            .ligature: ligatures ? 1 : 0,
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attrs))
        return Float(CTLineGetTypographicBounds(line, nil, nil, nil))
    }

    /// Per-scalar advances of `text` laid out as one CoreText line (fallback
    /// font + kerning chosen in context), folded from UTF-16 glyph indices.
    func measureRun(face: FaceRole, size: Float, ligatures: Bool, text: String) -> [Float] {
        let attrs: [NSAttributedString.Key: Any] = [
            .font: Fonts.ctFont(face, size: size),
            .ligature: ligatures ? 1 : 0,
        ]
        let line = CTLineCreateWithAttributedString(NSAttributedString(string: text, attributes: attrs))
        let units = text.utf16.count
        var perUnit = [Float](repeating: 0, count: units)
        for case let run as CTRun in CTLineGetGlyphRuns(line) as NSArray {
            let n = CTRunGetGlyphCount(run)
            guard n > 0 else { continue }
            var advances = [CGSize](repeating: .zero, count: n)
            var indices = [CFIndex](repeating: 0, count: n)
            CTRunGetAdvances(run, CFRange(location: 0, length: n), &advances)
            CTRunGetStringIndices(run, CFRange(location: 0, length: n), &indices)
            for i in 0..<n where indices[i] >= 0 && indices[i] < units {
                perUnit[indices[i]] += Float(advances[i].width)
            }
        }
        var out: [Float] = []
        out.reserveCapacity(text.unicodeScalars.count)
        var unit = 0
        for scalar in text.unicodeScalars {
            let width = scalar.utf16.count
            out.append(perUnit[unit..<min(units, unit + width)].reduce(0, +))
            unit += width
        }
        return out
    }
}

/// One process-wide text system shared by every transcript.
enum TextEngine {
    static let shared = TextSystem(faces: Fonts.faceData, measurer: CoreTextMeasurer())
}
