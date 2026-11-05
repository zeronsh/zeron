import CoreText
import XCTest
@testable import Zeron

/// Pretext's accuracy methodology with CoreText as ground truth: lay out a
/// corpus at many widths with Rust (`debug_line_starts`) and with
/// CTFramesetter using the *same font bytes*, and compare line starts.
final class LineBreakAccuracyTests: XCTestCase {
    static let corpus: [String] = {
        var c = [
            "The transcript is laid out analytically: every row's height is known before it is shown, so scrolling never guesses.",
            "Inline code spans get chips, links are tappable, and old ideas are struck through when they no longer apply to the plan.",
            "Nested bullet with a very/long/path/that/must/wrap/somewhere/in/the/middle/because/it/is/too/wide.rs and then more words.",
            "See https://example.com/a/very/long/url/that/keeps/going/and/going?query=parameters&more=stuff for the details.",
            "Run cargo test -p zeron-text --release -- --nocapture, then compare the numbers against the previous baseline run.",
            "CJK: 日本語のテキストも正しく折り返されます。中文也可以正确换行，不需要空格。한국어 문장도 줄바꿈이 됩니다.",
            "Emoji sequences 👩‍💻 🧑🏽‍🚀 🇯🇵 1️⃣ never split, even when a line is tight 🚀✨🔥 around them.",
            "Numbers like 3,100 and 1.5×, dates like 2026-09-26, and times like 12:48 stay intact; so do e.g. and i.e. abbreviations.",
            "A sentence with an em—dash, an en–dash, “smart quotes”, and (parentheses) [brackets] {braces} that should break sensibly.",
            "supercalifragilisticexpialidocious_is_a_single_identifier_that_is_longer_than_any_phone_screen_is_wide_by_itself",
        ]
        for p in layoutFixtureMarkdown().components(separatedBy: "\n\n") where p.count > 40 && !p.hasPrefix("```") && !p.hasPrefix("|") {
            c.append(p.replacingOccurrences(of: "\n", with: " "))
        }
        return c
    }()

    func coreTextLineStarts(_ text: String, face: FaceRole, size: Float, width: CGFloat) -> [UInt32] {
        let attr = NSAttributedString(string: text, attributes: [.font: Fonts.ctFont(face, size: size)])
        let setter = CTFramesetterCreateWithAttributedString(attr)
        let path = CGPath(rect: CGRect(x: 0, y: 0, width: width, height: 100_000), transform: nil)
        let frame = CTFramesetterCreateFrame(setter, CFRange(location: 0, length: 0), path, nil)
        let lines = CTFrameGetLines(frame) as! [CTLine]
        return lines.map { UInt32(CTLineGetStringRange($0).location) }
    }

    func testRustLineBreaksMatchCoreText() {
        var cases = 0
        var exact = 0
        var lineDiff = 0
        var report: [String] = []
        for face in [FaceRole.sans, .sansSemibold, .mono] {
            for size: Float in [14, 16.5, 19] {
                for width in stride(from: CGFloat(180), through: 430, by: 9) {
                    for text in Self.corpus {
                        let rust = debugLineStarts(textSystem: TextEngine.shared, face: face, size: size, width: Float(width), text: text)
                        let ct = coreTextLineStarts(text, face: face, size: size, width: width)
                        cases += 1
                        if rust == ct {
                            exact += 1
                        } else {
                            if rust.count != ct.count { lineDiff += 1 }
                            if report.count < 12 {
                                report.append("\(face) \(size)pt w=\(width): rust \(rust) vs ct \(ct) — \(text.prefix(48))")
                            }
                        }
                    }
                }
            }
        }
        let rate = Double(exact) / Double(cases)
        print("LINEBREAK ACCURACY exact=\(exact)/\(cases) (\(String(format: "%.2f", rate * 100))%) lineCountDiffs=\(lineDiff)")
        report.forEach { print("  MISMATCH \($0)") }
        // Heights (line counts) must agree exactly; line starts nearly always.
        XCTAssertEqual(lineDiff, 0)
        XCTAssertGreaterThanOrEqual(rate, 0.999)
    }
}
