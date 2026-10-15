import XCTest
@testable import Zeron

/// The dictation's contract with the draft, as the desktop editor keeps it
/// (`crates/ui/src/composer_dictation_tests.rs`), in UTF-16 as UIKit counts.
final class DictationDraftTests: XCTestCase {
    func testWordsLandInTheSelectionAndJoinWhatIsThere() {
        var draft = DictationDraft()
        var content = "👩🏽‍💻 replace café"
        let ns = content as NSString
        draft.begin(content: content, selection: ns.range(of: "replace"))
        draft.listen(now: Date())
        XCTAssertEqual(draft.replace(in: &content, with: "你好"), (("👩🏽‍💻 你好" as NSString).length))
        XCTAssertEqual(content, "👩🏽‍💻 你好 café")
        // Each partial replaces the last; whitespace-only ones change nothing.
        XCTAssertNil(draft.replace(in: &content, with: "  "))
        XCTAssertNotNil(draft.replace(in: &content, with: "bonjour 🌍"))
        XCTAssertEqual(content, "👩🏽‍💻 bonjour 🌍 café")
        XCTAssertTrue(draft.hasPartial)

        // At the end of a word the dictation adds the space itself.
        var tail = "hello"
        draft.begin(content: tail, selection: NSRange(location: 5, length: 0))
        draft.listen(now: Date())
        XCTAssertEqual(draft.replace(in: &tail, with: "world"), 11)
        XCTAssertEqual(tail, "hello world")
        XCTAssertNotNil(draft.replace(in: &tail, with: "world again"))
        XCTAssertEqual(tail, "hello world again")
    }

    func testAChangedDraftOrAStaleEventNeverOverwritesAndNeverSends() {
        var draft = DictationDraft()
        draft.begin(content: "old", selection: NSRange(location: 0, length: 3))
        draft.listen(now: Date())
        let generation = draft.generation
        XCTAssertTrue(draft.finish(send: true, now: Date()))
        XCTAssertFalse(draft.finish(send: true, now: Date()), "finishing twice closes the microphone once")
        var current = "new draft"
        XCTAssertNil(draft.replace(in: &current, with: "late"))
        XCTAssertEqual(current, "new draft")
        XCTAssertNotEqual(draft.generation, generation, "a mismatch invalidates late events")
        XCTAssertFalse(draft.complete(.idle), "a pending send dies with the dictation")
    }

    func testSendWaitsForTheFinalWordsAndInterruptionDropsIt() {
        var draft = DictationDraft()
        let start = Date()
        draft.begin(content: "", selection: NSRange(location: 0, length: 0))
        draft.listen(now: start)
        XCTAssertTrue(draft.finish(send: false, now: start))
        XCTAssertFalse(draft.finish(send: true, now: start + 1), "Send during transcription only asks to send")
        XCTAssertEqual(draft.elapsed(now: start + 30), 0, "the clock holds where capture ended")
        XCTAssertFalse(draft.timedOut(now: start + DictationDraft.finalizeTimeout - 1))
        XCTAssertTrue(draft.timedOut(now: start + DictationDraft.finalizeTimeout))
        XCTAssertTrue(draft.complete(.idle))
        XCTAssertFalse(draft.complete(.idle), "sends once")

        draft.begin(content: "", selection: NSRange(location: 0, length: 0))
        draft.listen(now: start)
        _ = draft.finish(send: true, now: start)
        draft.dropPendingSend()
        XCTAssertFalse(draft.complete(.idle), "an interrupted capture is never sent on its own")
        XCTAssertEqual(draft.phase, .idle)

        draft.begin(content: "", selection: NSRange(location: 0, length: 0))
        _ = draft.finish(send: true, now: start)
        XCTAssertFalse(draft.complete(.noSpeech), "silence keeps the draft and does not send")
        XCTAssertEqual(draft.phase, .noSpeech)
    }
}
