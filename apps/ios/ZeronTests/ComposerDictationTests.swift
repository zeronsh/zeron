import UIKit
import XCTest
@testable import Zeron

@MainActor
private final class FakeComposerTranscriber: ComposerTranscriber {
    var receive: ((DictationEvent) -> Void)?
    var finishCount = 0
    var cancelCount = 0
    func start(receive: @escaping @MainActor (DictationEvent) -> Void) { self.receive = receive }
    func finish() { finishCount += 1 }
    func cancel() { cancelCount += 1 }
}

@MainActor
final class ComposerDictationTests: XCTestCase {
    private func setup(_ text: String, selection: NSRange) -> (
        UITextView, ComposerEditorController, FakeComposerTranscriber, ComposerDictation
    ) {
        let view = UITextView()
        view.text = text
        view.selectedRange = selection
        let editor = ComposerEditorController()
        editor.view = view
        view.delegate = editor
        let fake = FakeComposerTranscriber()
        let dictation = ComposerDictation(transcriber: fake)
        dictation.start(editor: editor)
        return (view, editor, fake, dictation)
    }

    func testPartialsReplaceSelectionWithUTF16AndPreserveSurroundingText() {
        let prefix = "👋🏽 Before "
        let (view, editor, fake, dictation) = setup(prefix + "old after", selection:
            NSRange(location: prefix.utf16.count, length: 3))
        var draft = ""
        editor.textChanged = { draft = $0 }
        XCTAssertEqual(dictation.state, .requestingPermission)
        fake.receive?(.listening)
        XCTAssertEqual(dictation.state, .listening)
        fake.receive?(.transcript("hello", final: false))
        fake.receive?(.transcript("你好 🌍", final: false))
        XCTAssertEqual(view.text, prefix + "你好 🌍 after")
        XCTAssertEqual(draft, view.text)
        XCTAssertEqual(editor.dictatedRange, NSRange(location: prefix.utf16.count, length: "你好 🌍".utf16.count))
        fake.receive?(.transcript("done", final: true))
        XCTAssertEqual(view.text, prefix + "done after")
        XCTAssertNil(editor.dictatedRange)
        XCTAssertEqual(dictation.state, .idle)
    }

    func testManualCorrectionStopsBeforeEditAndRejectsLateResults() {
        let (view, editor, fake, dictation) = setup("Before  after", selection: NSRange(location: 7, length: 0))
        fake.receive?(.listening)
        fake.receive?(.transcript("wrong", final: false))
        let correction = NSRange(location: 7, length: 5)
        XCTAssertTrue(editor.textView(view, shouldChangeTextIn: correction, replacementText: "right"))
        view.textStorage.replaceCharacters(in: correction, with: "right")
        editor.textViewDidChange(view)
        fake.receive?(.transcript("late", final: true))
        XCTAssertEqual(view.text, "Before right after")
        XCTAssertEqual(dictation.state, .idle)
        XCTAssertNil(editor.dictatedRange)
    }

    func testSendFinalizesExactlyOnceAndDoesNotRefillClearedDraft() {
        let (view, editor, fake, dictation) = setup("", selection: NSRange(location: 0, length: 0))
        var draft = ""
        editor.textChanged = { draft = $0 }
        fake.receive?(.listening)
        fake.receive?(.transcript("hel", final: false))
        var sent: [String] = []
        let send = {
            editor.commit()
            if !draft.isEmpty { sent.append(draft) }
            draft = ""
            editor.apply(text: draft)
        }
        dictation.finish(then: send)
        dictation.finish(then: send)
        XCTAssertEqual(dictation.state, .finalizing)
        XCTAssertEqual(fake.finishCount, 1)
        XCTAssertTrue(sent.isEmpty)
        fake.receive?(.transcript("hello", final: true))
        fake.receive?(.transcript("hello again", final: true))
        XCTAssertEqual(sent, ["hello"])
        XCTAssertEqual(view.text, "")
        XCTAssertEqual(dictation.state, .idle)
    }

    func testNavigationCancelsPendingSendAndLatePermissionCallback() {
        let (view, editor, fake, dictation) = setup("draft", selection: NSRange(location: 5, length: 0))
        let stale = fake.receive
        dictation.cancel()
        stale?(.listening)
        stale?(.transcript("late", final: false))
        XCTAssertEqual(view.text, "draft")
        dictation.start(editor: editor)
        fake.receive?(.listening)
        var sends = 0
        dictation.finish { sends += 1 }
        dictation.cancel()
        fake.receive?(.transcript("another chat", final: true))
        XCTAssertEqual(sends, 0)
        XCTAssertEqual(view.text, "draft")
    }

    func testFailurePreservesDraftAndCanRetry() {
        for failure in [DictationFailure.permissionDenied, .unavailable, .failed] {
            let (view, editor, fake, dictation) = setup("draft", selection: NSRange(location: 5, length: 0))
            fake.receive?(.failure(failure))
            XCTAssertEqual(dictation.state, .error(failure))
            XCTAssertEqual(view.text, "draft")
            XCTAssertNil(editor.dictatedRange)
            dictation.start(editor: editor)
            fake.receive?(.listening)
            fake.receive?(.transcript(" again", final: true))
            XCTAssertEqual(view.text, "draft again")
        }
    }

    func testQueueDraftReplacementCancelsDictation() {
        let (view, editor, fake, dictation) = setup("draft", selection: NSRange(location: 5, length: 0))
        fake.receive?(.listening)
        fake.receive?(.transcript(" words", final: false))
        editor.apply(text: "queued text")
        fake.receive?(.transcript("late words", final: true))
        XCTAssertEqual(view.text, "queued text")
        XCTAssertEqual(dictation.state, .idle)
    }

    func testStoppingWithoutSpeechPreservesSelectedText() {
        let (view, editor, fake, dictation) = setup("selected", selection: NSRange(location: 0, length: 8))
        dictation.finish()
        fake.receive?(.listening)
        XCTAssertEqual(view.text, "selected")
        XCTAssertNil(editor.dictatedRange)
        XCTAssertEqual(dictation.state, .idle)
    }

    func testFailureWhileFinalizingDoesNotSendIncompleteDraft() {
        let (view, editor, fake, dictation) = setup("", selection: NSRange(location: 0, length: 0))
        fake.receive?(.listening)
        fake.receive?(.transcript("partial", final: false))
        var sends = 0
        dictation.finish { sends += 1 }
        fake.receive?(.failure(.failed))
        XCTAssertEqual(sends, 0)
        XCTAssertEqual(view.text, "partial")
        XCTAssertNil(editor.dictatedRange)
    }

    func testEmptyFinalDoesNotEraseSelection() {
        let (view, editor, fake, dictation) = setup("keep me", selection: NSRange(location: 0, length: 7))
        fake.receive?(.listening)
        dictation.finish()
        fake.receive?(.transcript("", final: true))
        XCTAssertEqual(view.text, "keep me")
        XCTAssertNil(editor.dictatedRange)
    }

    func testSelectionOutsideDictationTracksReplacementWithoutLosingFocus() {
        let (view, editor, fake, dictation) = setup("start end", selection: NSRange(location: 6, length: 0))
        fake.receive?(.listening)
        fake.receive?(.transcript("hello ", final: false))
        view.selectedRange = NSRange(location: 12, length: 3)
        fake.receive?(.transcript("hi ", final: false))
        XCTAssertEqual(view.selectedRange, NSRange(location: 9, length: 3))
        XCTAssertEqual(view.text, "start hi end")
        dictation.cancel()
        XCTAssertNil(editor.dictatedRange)
    }

    func testTimeoutCommitsLatestPartialOnce() async {
        let (view, editor, fake, dictation) = setup("", selection: NSRange(location: 0, length: 0))
        fake.receive?(.listening)
        fake.receive?(.transcript("latest", final: false))
        var sends = 0
        dictation.finish { sends += 1 }
        try? await Task.sleep(for: .milliseconds(2200))
        XCTAssertEqual(dictation.state, .idle)
        XCTAssertEqual(sends, 1)
        XCTAssertEqual(view.text, "latest")
        XCTAssertNil(editor.dictatedRange)
        fake.receive?(.transcript("late", final: true))
        XCTAssertEqual(sends, 1)
        XCTAssertEqual(view.text, "latest")
    }
}
