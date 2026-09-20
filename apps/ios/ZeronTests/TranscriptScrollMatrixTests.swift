#if DEBUG
import SwiftUI
import XCTest
@testable import Zeron

/// 10 markdown corpora × 5 interaction patterns = 50 scroll scenarios. Each
/// scenario streams the corpus in chunks (so partial fences, tables and
/// emphasis are rendered mid-stream) on top of a short, medium or long history.
@MainActor
final class TranscriptScrollMatrixTests: XCTestCase {
    @Observable final class Harness {
        let store: SessionStore
        var scroll = ScrollState()
        let folds = ToolGroupFolds()
        var identity = UUID()
        var composerHeight: CGFloat = 64
        var useEditor = false
        var draft = ""
        init(store: SessionStore) { self.store = store }
    }

    private struct Surface: View {
        let harness: Harness
        var body: some View {
            VStack(spacing: 0) {
                TranscriptView(store: harness.store, chatId: harness.store.chatId, scroll: harness.scroll, folds: harness.folds)
                    .id(harness.identity)
                if harness.useEditor {
                    ComposerShell(draft: Binding(get: { harness.draft }, set: { harness.draft = $0 }),
                                  sendEnabled: true, showStop: false, onSend: {}) { EmptyView() }
                        .padding(.bottom, 8)
                } else {
                    Color.black.frame(height: harness.composerHeight)
                }
            }
        }
    }

    struct Corpus {
        let name: String
        let historyTurns: Int
        let markdown: String
    }

    static let chunkCount = 12

    static let corpora: [Corpus] = [
        Corpus(name: "short-plain", historyTurns: 2, markdown: "Done — the dropdown now paints before `loadRefs()` resolves."),
        Corpus(name: "long-paragraphs", historyTurns: 40, markdown:
            (0..<40).map { "Paragraph \($0): the incremental parser has to keep the newest block realized while earlier blocks settle, and the table must keep the tail attached without a visible jump. **Bold**, _italic_ and `code` spans stay stable as text arrives." }
                .joined(separator: "\n\n")),
        Corpus(name: "headings-lists", historyTurns: 300, markdown:
            (1...6).map { section in
                """
                # Section \(section)

                ## Findings

                - First finding for section \(section) with a fairly long description that wraps onto a second line on a phone
                - Second finding with `inline code`
                - Third finding

                ### Steps

                1. Open the picker
                2. Choose a model
                3. Send the prompt

                #### Notes

                Trailing notes for section \(section).
                """
            }.joined(separator: "\n\n")),
        Corpus(name: "nested-lists", historyTurns: 2, markdown:
            (0..<8).map { i in
                """
                - Level one item \(i)
                  - Level two item with a longer description so it wraps across the viewport width on a phone
                    - Level three `code` item
                      - Level four item
                  - Another level two
                    1. Ordered inside unordered
                    2. Second ordered
                - Sibling level one
                """
            }.joined(separator: "\n")),
        Corpus(name: "wide-table", historyTurns: 40, markdown:
            "| Stage | Cost | Cached | Owner | Notes | Status |\n| --- | --- | --- | --- | --- | --- |\n"
            + (0..<25).map { "| stage-\($0) | O(refs) | \($0 % 2 == 0 ? "yes" : "no") | dev-mac | a fairly long note cell for row \($0) | ok |" }
                .joined(separator: "\n")),
        Corpus(name: "long-code", historyTurns: 300, markdown:
            "Here is the full diff:\n\n```ts\n"
            + (0..<120).map { "export const line\($0) = useMemo(() => computeRefIndex(refs, { includeRemote: true, cacheKey: 'ref-index-\($0)' }), [refs]);" }
                .joined(separator: "\n")
            + "\n```\n\nThat is everything."),
        Corpus(name: "many-code-blocks", historyTurns: 2, markdown:
            (0..<12).map { i in
                let lang = ["swift", "rust", "ts", "sh", "json", "py"][i % 6]
                return "Block \(i) explanation paragraph before the code.\n\n```\(lang)\nfn block_\(i)() {\n    return \(i);\n}\n```"
            }.joined(separator: "\n\n")),
        Corpus(name: "quotes-rules", historyTurns: 40, markdown:
            (0..<10).map { i in
                """
                > Quote \(i): the fix is to paint first and fill in — the index can arrive late.
                >
                > > Nested quote \(i) with `code` and **emphasis**.

                ---

                Paragraph after rule \(i).
                """
            }.joined(separator: "\n\n")),
        Corpus(name: "mixed-report", historyTurns: 300, markdown:
            (0..<5).map { i in
                """
                ## Pass \(i): where the dropdown stalls

                The dropdown's open handler awaits `loadRefs()` **before** it paints, so
                the menu can't render until the full ref index resolves.

                1. `loadRefs()` walks every ref
                2. The handler `await`s it inline
                3. `useRefIndex` has no cache

                | Stage | Cost | Cached |
                | --- | --- | --- |
                | `loadRefs` | O(refs) | no |
                | paint | O(visible) | n/a |

                > The fix is to paint first and fill in.

                ```ts
                const refs = useRefIndex()
                return <Menu items={refs ?? []} loading={refs == null} />
                ```
                """
            }.joined(separator: "\n\n")),
        Corpus(name: "fragments-unicode", historyTurns: 120, markdown:
            "Unicode 你好 👋🏽 مرحبا and a very long unbroken token: "
            + String(repeating: "abcdefghij", count: 40)
            + "\n\n| a | b |\n| --- | --- |\n| 1 | 2 |\n\n```\nunterminated fence keeps streaming\n"
            + (0..<30).map { "line \($0) *emphasis that never closes" }.joined(separator: "\n")),
    ]

    private var window: UIWindow!
    private var harness: Harness!
    private var key: String { harness.store.chatId }
    private var table: TranscriptTableView { harness.scroll.nativeScrollView as! TranscriptTableView }

    private func mount(_ corpus: Corpus, useEditor: Bool = false) async {
        TranscriptLayoutProbe.enabled = true
        let config = AppConfig(edgeURL: URL(string: "http://localhost:8787")!, mode: .dev,
                               userId: "test", orgId: "test", deviceId: "test", deviceName: "Test")
        let store = SessionStore(chatId: UUID().uuidString, config: config, offline: true)
        store.setEntries(BenchRunner.syntheticEntries(turns: corpus.historyTurns))
        harness = Harness(store: store)
        harness.useEditor = useEditor
        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first!
        window = UIWindow(windowScene: scene)
        let size = CGSize(width: 390, height: 844)
        window.frame = useEditor ? scene.screen.bounds : CGRect(origin: .zero, size: size)
        window.rootViewController = UIHostingController(rootView: Surface(harness: harness)
            .frame(width: useEditor ? nil : size.width, height: useEditor ? nil : size.height))
        window.makeKeyAndVisible()
        await settle()
    }

    private func unmount() {
        window?.endEditing(true)
        window?.isHidden = true
        window?.rootViewController = nil
        window = nil
        harness = nil
        TranscriptLayoutProbe.tails.removeAll()
        TranscriptLayoutProbe.viewports.removeAll()
    }

    override func tearDown() {
        unmount()
        TranscriptLayoutProbe.enabled = false
        super.tearDown()
    }

    private func settle(_ ms: Int = 700) async {
        try? await Task.sleep(for: .milliseconds(ms))
        window.layoutIfNeeded()
        TranscriptLayoutProbe.sample()
    }

    /// Apply chunk `index` (0-based) of the corpus as the streaming reply.
    private func applyChunk(_ corpus: Corpus, index: Int) {
        let chars = Array(corpus.markdown)
        let end = min(chars.count, Int((Double(chars.count) * Double(index + 1) / Double(Self.chunkCount)).rounded(.up)))
        let text = String(chars[0..<end])
        var entries = harness.store.entries
        let final = index == Self.chunkCount - 1
        if let last = entries.indices.last, entries[last].id == "matrix-reply" {
            entries[last].parts = [.text(id: "t0", text: text)]
            entries[last].status = final ? .complete : .streaming
        } else {
            entries.append(MessageEntry(id: "matrix-prompt", role: .user,
                parts: [.text(id: "t0", text: "Stream the \(corpus.name) corpus.")], createdAt: nowMs(),
                deviceId: "test", status: .complete, continuationOf: nil))
            entries.append(MessageEntry(id: "matrix-reply", role: .assistant,
                parts: [.text(id: "t0", text: text)], createdAt: nowMs(),
                deviceId: "test", status: final ? .complete : .streaming, continuationOf: nil))
        }
        harness.store.setEntries(entries)
    }

    private func assertTailVisible(_ context: String, file: StaticString = #filePath, line: UInt = #line) {
        TranscriptLayoutProbe.sample()
        let lastRow = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: harness.store.entries, pendingSends: harness.store.pendingSends).last!
        guard let tail = TranscriptLayoutProbe.tails[key + "|" + lastRow.id],
              let viewport = TranscriptLayoutProbe.viewports[key] else {
            attachScreenshot(context)
            XCTFail("[\(context)] tail \(lastRow.id) must be realized; distance \(harness.scroll.distanceFromBottom)", file: file, line: line)
            return
        }
        if tail.maxY < viewport.minY || viewport.maxY - tail.maxY >= 64 || !harness.scroll.pinned {
            attachScreenshot(context)
        }
        XCTAssertGreaterThan(tail.height, 0, "[\(context)]", file: file, line: line)
        XCTAssertGreaterThan(tail.maxY, viewport.minY, "[\(context)] tail above viewport", file: file, line: line)
        XCTAssertLessThanOrEqual(tail.maxY, viewport.maxY + 2, "[\(context)] tail below viewport", file: file, line: line)
        // 24pt bottom spacing plus a block's own trailing padding (lists, tables, fences).
        XCTAssertLessThan(viewport.maxY - tail.maxY, 64, "[\(context)] blank space under tail", file: file, line: line)
        XCTAssertTrue(harness.scroll.pinned, "[\(context)] must remain pinned", file: file, line: line)
        assertNoOverscroll(context, file: file, line: line)
    }

    private func assertNoOverscroll(_ context: String, file: StaticString = #filePath, line: UInt = #line) {
        let maxOffset = max(-table.contentInset.top, table.contentSize.height - table.bounds.height)
        XCTAssertLessThanOrEqual(table.contentOffset.y, maxOffset + 1, "[\(context)] offset past content end", file: file, line: line)
        XCTAssertGreaterThanOrEqual(table.contentOffset.y, -table.contentInset.top - 1, "[\(context)] offset before content start", file: file, line: line)
    }

    /// Viewport must be filled with realized cells while reading history.
    private func assertViewportRealized(_ context: String, file: StaticString = #filePath, line: UInt = #line) {
        let viewport = table.convert(table.bounds, to: window)
        let visibleBottom = table.visibleCells.map { table.convert($0.frame, to: window).maxY }.max() ?? 0
        XCTAssertGreaterThanOrEqual(visibleBottom, viewport.maxY - 100, "[\(context)] blank space in viewport", file: file, line: line)
    }

    private func attachScreenshot(_ name: String) {
        let image = UIGraphicsImageRenderer(bounds: window.bounds).image { _ in
            window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
        }
        let attachment = XCTAttachment(image: image)
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    /// Synthetic finger drag: the delegate path a real pan takes, minus touch delivery.
    private func drag(by delta: CGFloat, steps: Int = 6) async {
        table.scrollViewWillBeginDragging(table)
        let start = table.contentOffset.y
        let maxOffset = max(-table.contentInset.top, table.contentSize.height - table.bounds.height)
        for step in 1...steps {
            let y = min(max(-table.contentInset.top, start + delta * CGFloat(step) / CGFloat(steps)), maxOffset)
            table.setContentOffset(CGPoint(x: 0, y: y), animated: false)
            try? await Task.sleep(for: .milliseconds(16))
        }
        table.scrollViewDidEndDragging(table, willDecelerate: false)
    }

    private func visibleLayout() -> String {
        let rows = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: harness.store.entries, pendingSends: harness.store.pendingSends)
        let paths = (table.indexPathsForVisibleRows ?? []).sorted()
        let lines = paths.map { path -> String in
            let rect = table.rectForRow(at: path)
            let id = path.row < rows.count ? rows[path.row].id : "?"
            return "  \(path.row) \(id) y=\(rect.minY) h=\(rect.height)"
        }
        return "offset=\(table.contentOffset.y) content=\(table.contentSize.height)\n" + lines.joined(separator: "\n")
    }

    private func anchorCell() -> UITableViewCell? {
        let top = table.superview!.convert(table.superview!.bounds, to: window).minY
        return table.visibleCells.first {
            let rect = table.convert($0.frame, to: window)
            return rect.minY > top && rect.minY < top + 300
        }
    }

    private func presentedY(_ cell: UITableViewCell) -> CGFloat {
        let layer = cell.layer.presentation() ?? cell.layer
        return layer.convert(layer.bounds, to: window.layer.presentation() ?? window.layer).minY
    }

    private func forEachCorpus(_ scenario: String, useEditor: Bool = false,
                               _ body: (Corpus, String) async -> Void) async {
        for corpus in Self.corpora {
            let context = "\(scenario)/\(corpus.name)"
            await mount(corpus, useEditor: useEditor)
            await body(corpus, context)
            unmount()
        }
    }

    // MARK: Scenario 1 — pinned streaming (10 scenarios)

    func testStreamingEveryCorpusKeepsTailAttachedWithoutBackwardJumps() async {
        await forEachCorpus("stream-pinned") { corpus, context in
            assertTailVisible(context + "/open")
            var offsets: [CGFloat] = []
            var heights: [CGFloat] = []
            for chunk in 0..<Self.chunkCount {
                applyChunk(corpus, index: chunk)
                await settle(80)
                offsets.append(table.contentOffset.y)
                heights.append(table.contentSize.height)
                assertTailVisible(context + "/chunk\(chunk)")
            }
            await settle()
            assertTailVisible(context + "/final")
            // A pinned viewport may only move back up by as much as the
            // content itself shrank (a row settling from its estimate).
            for i in 1..<offsets.count {
                let shrink = max(0, heights[i - 1] - heights[i])
                XCTAssertGreaterThanOrEqual(offsets[i], offsets[i - 1] - shrink - 1,
                    "[\(context)] backward jump at chunk \(i): offsets \(offsets) heights \(heights)")
            }
        }
    }

    // MARK: Scenario 2 — reading history while it streams, then jump (10 scenarios)

    func testReadingHistoryStaysStillWhileStreamingThenJumpToLatestPins() async {
        await forEachCorpus("read-history") { corpus, context in
            applyChunk(corpus, index: 0)
            await settle(120)
            await drag(by: -900)
            await settle(120)
            XCTAssertFalse(harness.scroll.pinned, "[\(context)] dragging up must release follow")
            assertViewportRealized(context + "/after-drag")
            guard let cell = anchorCell() else {
                XCTFail("[\(context)] no anchor cell"); return
            }
            let start = presentedY(cell)
            var drift: [CGFloat] = []
            for chunk in 1..<Self.chunkCount {
                let before = visibleLayout()
                applyChunk(corpus, index: chunk)
                for _ in 0..<4 {
                    try? await Task.sleep(for: .milliseconds(16))
                    let error = abs(presentedY(cell) - start)
                    if error >= 4, (drift.max() ?? 0) < 4 {
                        let rows = harness.store.transcriptCache.rows(revision: harness.store.revision,
                            entries: harness.store.entries, pendingSends: harness.store.pendingSends)
                        let diagnostic = XCTAttachment(string: "chunk \(chunk) offset \(table.contentOffset.y) contentHeight \(table.contentSize.height)\nlast rows: \(rows.suffix(6).map { "\($0.id)@\($0.version)" })\nbefore:\n\(before)\nafter:\n\(visibleLayout())")
                        diagnostic.name = context + "/drift-chunk\(chunk)"
                        diagnostic.lifetime = .keepAlways
                        add(diagnostic)
                        attachScreenshot(context + "/drift-chunk\(chunk)")
                    }
                    drift.append(error)
                }
                XCTAssertFalse(harness.scroll.pinned, "[\(context)] streaming must not re-pin a reader")
                assertNoOverscroll(context + "/chunk\(chunk)")
            }
            XCTAssertLessThan(drift.max() ?? .infinity, 4, "[\(context)] reading anchor drifted: \(drift)")
            assertViewportRealized(context + "/after-stream")
            XCTAssertTrue(harness.scroll.showJump || harness.scroll.distanceFromBottom <= TranscriptView.jumpThreshold,
                          "[\(context)] jump affordance must appear when far from bottom")
            // Same sequence as the jump button in TranscriptView.
            harness.scroll.arm()
            harness.scroll.jumpToLatest?(true)
            await settle()
            assertTailVisible(context + "/after-jump")
        }
    }

    // MARK: Scenario 3 — keyboard show/hide during streaming (10 scenarios)

    func testKeyboardShowHideDuringStreamingKeepsTailAttached() async {
        await forEachCorpus("keyboard", useEditor: true) { corpus, context in
            let editor = findNativeEditor(window)!
            for chunk in 0..<Self.chunkCount {
                if chunk == 2 { editor.becomeFirstResponder() }
                if chunk == 7 { editor.resignFirstResponder() }
                applyChunk(corpus, index: chunk)
                await settle(chunk == 2 || chunk == 7 ? 450 : 80)
                assertTailVisible(context + "/chunk\(chunk)")
            }
            editor.becomeFirstResponder()
            await settle()
            assertTailVisible(context + "/keyboard-up-final")
            editor.resignFirstResponder()
            await settle()
            assertTailVisible(context + "/keyboard-down-final")
        }
    }

    // MARK: Scenario 4 — animated composer resize during streaming (10 scenarios)

    func testComposerResizeDuringStreamingKeepsTailAttached() async {
        await forEachCorpus("composer-resize") { corpus, context in
            let heights: [CGFloat] = [64, 144, 440, 200, 64, 300]
            for chunk in 0..<Self.chunkCount {
                applyChunk(corpus, index: chunk)
                withAnimation(.easeInOut(duration: 0.25)) { harness.composerHeight = heights[chunk % heights.count] }
                await settle(120)
                assertNoOverscroll(context + "/chunk\(chunk)")
            }
            harness.composerHeight = 64
            await settle()
            assertTailVisible(context + "/final")
        }
    }

    // MARK: Scenario 5 — warm reopen, re-engage by dragging back, late append (10 scenarios)

    func testReopenThenDragBackToBottomReengagesAndFollowsLateAppends() async {
        await forEachCorpus("reopen-reengage") { corpus, context in
            for chunk in 0..<Self.chunkCount {
                applyChunk(corpus, index: chunk)
                await settle(40)
            }
            await settle()
            await drag(by: -1200)
            await settle(120)
            XCTAssertFalse(harness.scroll.pinned, "[\(context)] precondition: released")
            harness.identity = UUID()
            harness.scroll = ScrollState()
            await settle()
            assertTailVisible(context + "/after-reopen")
            await drag(by: -500)
            await settle(120)
            XCTAssertFalse(harness.scroll.pinned, "[\(context)] drag up releases after reopen")
            assertViewportRealized(context + "/reading-after-reopen")
            await drag(by: 5000)
            await settle()
            XCTAssertTrue(harness.scroll.pinned, "[\(context)] dragging to the bottom must re-pin")
            assertTailVisible(context + "/reengaged")
            var entries = harness.store.entries
            entries.append(MessageEntry(id: "late-append", role: .assistant,
                parts: [.text(id: "t0", text: "One more block after re-engaging.\n\n- with a list\n- of two items")],
                createdAt: nowMs(), deviceId: "test", status: .complete, continuationOf: nil))
            harness.store.setEntries(entries)
            await settle()
            assertTailVisible(context + "/late-append")
        }
    }

    private func findNativeEditor(_ view: UIView) -> UITextView? {
        if let editor = view as? UITextView { return editor }
        return view.subviews.lazy.compactMap { self.findNativeEditor($0) }.first
    }
}
#endif
