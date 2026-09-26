#if DEBUG
import SwiftUI
import XCTest
@testable import Zeron

@MainActor
final class TranscriptLayoutTests: XCTestCase {
    private struct MotionSample: Codable {
        let timestamp: CFTimeInterval
        let targetTimestamp: CFTimeInterval
        let callbackTime: CFTimeInterval
        let toggle: Int
        let layoutHeight: CGFloat
        let presentedHeight: CGFloat?
        let tailGap: CGFloat?
        let tailVisible: Bool
        let pinned: Bool
    }

    private struct GeometrySample: Codable {
        let timestamp: CFTimeInterval
        let targetTimestamp: CFTimeInterval
        let callbackTime: CFTimeInterval
        let position: CGFloat?
        let modelPosition: CGFloat?
        let viewport: CGRect?
        let modelViewport: CGRect?
        let tail: CGRect?

        var gap: CGFloat? {
            guard let viewport, let tail else { return nil }
            return viewport.maxY - tail.maxY
        }
    }

    private func geometrySample(_ link: CADisplayLink, position: CGFloat?, modelPosition: CGFloat?,
                                tailKey: String?) -> GeometrySample {
        TranscriptLayoutProbe.sample()
        return GeometrySample(timestamp: link.timestamp, targetTimestamp: link.targetTimestamp,
            callbackTime: CACurrentMediaTime(), position: position, modelPosition: modelPosition,
            viewport: TranscriptLayoutProbe.presentedFrame(for: key),
            modelViewport: TranscriptLayoutProbe.viewports[key],
            tail: tailKey.flatMap { TranscriptLayoutProbe.presentedFrame(for: $0) })
    }

    private func attachMotion(_ name: String, samples: [GeometrySample], events: [String] = []) throws {
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let data = try encoder.encode(samples)
        let attachment = XCTAttachment(string: "events=\(events)\nsamples=\(String(decoding: data, as: UTF8.self))")
        attachment.name = name
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    @Observable final class Harness {
        let store: SessionStore
        var scroll = ScrollState()
        let folds = ToolGroupFolds()
        var identity = UUID()
        var composerHeight: CGFloat = 64
        var dynamicTypeSize: DynamicTypeSize = .large
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
            .environment(\.dynamicTypeSize, harness.dynamicTypeSize)
        }
    }

    private var window: UIWindow!
    private var harness: Harness!
    private var key: String { harness.store.chatId }

    private func mount(turns: Int, size: CGSize = CGSize(width: 390, height: 844),
                       offline: Bool = true, waitForLayout: Bool = true, useEditor: Bool = false) async {
        TranscriptLayoutProbe.enabled = true
        let config = AppConfig(edgeURL: URL(string: "http://localhost:8787")!, mode: .dev,
                               userId: "test", orgId: "test", deviceId: "test", deviceName: "Test")
        let store = SessionStore(chatId: UUID().uuidString, config: config, offline: offline)
        store.setEntries(BenchRunner.syntheticEntries(turns: turns))
        harness = Harness(store: store)
        harness.useEditor = useEditor
        let scene = UIApplication.shared.connectedScenes.compactMap { $0 as? UIWindowScene }.first!
        window = UIWindow(windowScene: scene)
        window.frame = useEditor ? scene.screen.bounds : CGRect(origin: .zero, size: size)
        window.rootViewController = UIHostingController(rootView: Surface(harness: harness)
            .frame(width: useEditor ? nil : size.width, height: useEditor ? nil : size.height))
        window.makeKeyAndVisible()
        if waitForLayout { await settle() }
    }

    private func settle() async {
        // Let real SwiftUI/UIKit layout and hosted-cell measurement passes run.
        try? await Task.sleep(for: .milliseconds(700))
        window.layoutIfNeeded()
        TranscriptLayoutProbe.sample()
    }

    private func assertTailVisible(file: StaticString = #filePath, line: UInt = #line) {
        TranscriptLayoutProbe.sample()
        let lastRow = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: harness.store.entries, pendingSends: harness.store.pendingSends).last!
        guard let tail = TranscriptLayoutProbe.tails[key + "|" + lastRow.id],
              let viewport = TranscriptLayoutProbe.viewports[key] else {
            let image = UIGraphicsImageRenderer(bounds: window.bounds).image { _ in
                window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
            }
            let attachment = XCTAttachment(image: image)
            attachment.lifetime = .keepAlways
            add(attachment)
            XCTFail("Tail \(lastRow.id) must be realized without a user scroll; distance \(harness.scroll.distanceFromBottom)", file: file, line: line)
            return
        }
        if tail.maxY < viewport.minY {
            let image = UIGraphicsImageRenderer(bounds: window.bounds).image { _ in
                window.drawHierarchy(in: window.bounds, afterScreenUpdates: true)
            }
            let attachment = XCTAttachment(image: image)
            attachment.lifetime = .keepAlways
            add(attachment)
        }
        XCTAssertGreaterThan(tail.height, 0, file: file, line: line)
        XCTAssertGreaterThan(tail.maxY, viewport.minY, file: file, line: line)
        XCTAssertLessThanOrEqual(tail.maxY, viewport.maxY + 2, file: file, line: line)
        XCTAssertLessThan(viewport.maxY - tail.maxY, 40, file: file, line: line)
        XCTAssertTrue(harness.scroll.pinned, file: file, line: line)
    }

    override func tearDown() {
        window?.endEditing(true)
        window?.isHidden = true
        window?.rootViewController = nil
        window = nil
        harness = nil
        TranscriptLayoutProbe.tails.removeAll()
        TranscriptLayoutProbe.viewports.removeAll()
        TranscriptLayoutProbe.enabled = false
        super.tearDown()
    }

    func testGeneratedImageArrivalResizesTheRowAndKeepsTheTailVisible() async {
        await mount(turns: 6)
        let path = "/fixture/generated-\(UUID().uuidString).png"
        let reference = GeneratedImageReference(path: path, name: "generated.png", mimeType: "image/png")
        var entries = harness.store.entries
        entries.append(MessageEntry(id: "generated-reply", role: .assistant,
                                    parts: [.image(id: "image", reference: reference)],
                                    createdAt: nowMs(), deviceId: "image-owner",
                                    status: .complete, continuationOf: nil))
        harness.store.setEntries(entries)
        await settle()
        let format = UIGraphicsImageRendererFormat()
        format.scale = 1
        let data = UIGraphicsImageRenderer(size: CGSize(width: 320, height: 420), format: format)
            .pngData { context in
                UIColor.red.setFill()
                context.fill(CGRect(x: 0, y: 0, width: 320, height: 420))
            }
        AttachmentImageCache.shared.seed(deviceId: "image-owner", path: path,
                                        name: "generated.png", data: data, expectedMimeType: "image/png")
        await settle()
        assertTailVisible()
    }

    func testAccessibilityPagesThroughVirtualizedHistory() async {
        await mount(turns: 600)
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        let viewport = table.superview as! TranscriptViewport
        let initial = table.contentOffset.y
        XCTAssertGreaterThan(viewport.accessibilityElements?.count ?? 0, 0)
        XCTAssertLessThan(viewport.accessibilityElements?.count ?? 0, 40)
        XCTAssertTrue(viewport.accessibilityScroll(.down))
        await settle()
        XCTAssertLessThan(table.contentOffset.y, initial)
        XCTAssertFalse(harness.scroll.pinned)
        XCTAssertLessThan(viewport.accessibilityElements?.count ?? 0, 40)
        XCTAssertTrue(viewport.accessibilityScroll(.up))
        await settle()
        assertTailVisible()
    }

    func testEarlyHistoryScrollDoesNotLeaveBlankSpaceBelowTail() async {
        await mount(turns: 600, waitForLayout: false)
        try? await Task.sleep(for: .milliseconds(35))
        func findScroll(_ view: UIView) -> UIScrollView? {
            if let scroll = view as? UIScrollView { return scroll }
            return view.subviews.lazy.compactMap { findScroll($0) }.first
        }
        let native = findScroll(window)!
        harness.scroll.interactionEpoch &+= 1
        harness.scroll.userScrolling = true
        harness.scroll.userDragging = true
        harness.scroll.pinned = false
        for delta: CGFloat in [-120, -360, 220, -700, 600, -90] {
            native.setContentOffset(CGPoint(x: 0, y: native.contentOffset.y + delta), animated: false)
            await settle()
            let table = native as! UITableView
            let viewport = table.convert(table.bounds, to: window)
            let visibleBottom = table.visibleCells.map { table.convert($0.frame, to: window).maxY }.max() ?? 0
            XCTAssertGreaterThanOrEqual(visibleBottom, viewport.maxY - 100,
                "Reading history must have realized content throughout the viewport")
        }
    }

    func testToolGroupsRevealAndCollapseThroughIntermediateHeights() async throws {
        await mount(turns: 3)
        let rows = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: harness.store.entries, pendingSends: harness.store.pendingSends)
        let index = rows.lastIndex { if case .toolGroup = $0.kind { return true }; return false }!
        let id = rows[index].id
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        let path = IndexPath(row: index, section: 0)
        let tailKey = key + "|" + rows.last!.id
        // Measure the two endpoints independently of the animated samples.
        harness.folds.values[id] = true
        await settle()
        let openedHeight = table.rectForRow(at: path).height
        harness.folds.values[id] = false
        await settle()
        let closedHeight = table.rectForRow(at: path).height
        XCTAssertGreaterThan(openedHeight - closedHeight, 100)
        for open in [true, false, true, false] {
            let start = open ? closedHeight : openedHeight
            let target = open ? openedHeight : closedHeight
            var samples: [GeometrySample] = []
            var stableFrames = 0
            let completed = await DisplaySampler.observe("Tool disclosure reaches its endpoint") { [self] link in
                let sample = geometrySample(link,
                    position: table.cellForRow(at: path)?.layer.presentation()?.bounds.height,
                    modelPosition: table.rectForRow(at: path).height, tailKey: tailKey)
                samples.append(sample)
                if samples.count == 1 {
                    withAnimation(Motion.resize) { harness.folds.values[id] = open }
                    return false
                }
                let atTarget = sample.position.map { abs($0 - target) <= 1 } ?? false
                stableFrames = atTarget && abs(table.rectForRow(at: path).height - target) <= 1
                    ? stableFrames + 1 : 0
                return stableFrames == 3
            }
            try attachMotion(open ? "tool-opening-motion" : "tool-closing-motion", samples: samples)
            let heights = samples.compactMap(\.position)
            let gaps = samples.compactMap(\.gap)
            XCTAssertTrue(completed, "Disclosure did not reach its measured endpoint")
            XCTAssertGreaterThan(samples.count, 1)
            XCTAssertEqual(heights.count, samples.count, "The tool cell must remain presented")
            XCTAssertEqual(gaps.count, samples.count, "Viewport and tail must remain presented")
            XCTAssertTrue(heights.contains {
                $0 > min(start, target) + 1 && $0 < max(start, target) - 1
            }, "Disclosure must render an intermediate height, not snap between endpoints")
            XCTAssertLessThan((gaps.max() ?? .infinity) - (gaps.min() ?? 0), 4)
            assertTailVisible()
        }
    }

    func testToolToggleReversalWhileStreamingKeepsTailAttached() async throws {
        await mount(turns: 3, useEditor: true)
        let rows = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: harness.store.entries, pendingSends: harness.store.pendingSends)
        let index = rows.lastIndex { if case .toolGroup = $0.kind { return true }; return false }!
        let id = rows[index].id
        harness.folds.values[id] = false
        findNativeEditor(window)!.becomeFirstResponder()
        await settle()
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        let path = IndexPath(row: index, section: 0)
        let closedHeight = table.rectForRow(at: path).height
        // Calibrate endpoints before recording; never infer the open endpoint
        // from a partial reversal or from a fixed number of scheduler wake-ups.
        harness.folds.values[id] = true
        await settle()
        let openedHeight = table.rectForRow(at: path).height
        XCTAssertGreaterThan(openedHeight - closedHeight, 100)
        harness.folds.values[id] = false
        await settle()

        let completed = expectation(description: "Six toggles reverse in flight and finish closed")
        var samples: [MotionSample] = []
        var events: [String] = []
        var toggle = 0
        var open = false
        var legStart = closedHeight
        var streamed = false
        var streamedToggles: Set<Int> = []
        var closedFrames = 0
        var finished = false
        var tailID = rows.last!.id
        let sampler = DisplaySampler { [self] link in
            guard !finished else { return }
            let height = table.cellForRow(at: path)?.layer.presentation()?.bounds.height
            let viewport = TranscriptLayoutProbe.presentedFrame(for: key)
            let tail = TranscriptLayoutProbe.presentedFrame(for: key + "|" + tailID)
            let gap = viewport.flatMap { viewport in tail.map { viewport.maxY - $0.maxY } }
            let visible = viewport.flatMap { viewport in tail.map {
                $0.height > 0 && $0.maxY > viewport.minY
                    && $0.maxY <= viewport.maxY + 2 && viewport.maxY - $0.maxY < 40
            } } ?? false
            samples.append(MotionSample(timestamp: link.timestamp, targetTimestamp: link.targetTimestamp,
                callbackTime: CACurrentMediaTime(), toggle: toggle,
                layoutHeight: table.rectForRow(at: path).height, presentedHeight: height,
                tailGap: gap, tailVisible: visible, pinned: harness.scroll.pinned))
            guard let height else { return }

            if toggle == 0 {
                // Capture the closed presentation before starting the first animation.
                toggle = 1
                open = true
                withAnimation(Motion.resize) { harness.folds.values[id] = open }
                events.append("open timestamp=\(link.timestamp) height=\(height)")
                return
            }
            let target = open ? openedHeight : closedHeight
            let progress = (height - legStart) / (target - legStart)
            if !streamed, progress >= 0.15, progress < 1 {
                var entries = harness.store.entries
                entries[entries.count - 1].status = .streaming
                entries[entries.count - 1].parts.append(.text(id: "stream-\(toggle)", text: "More output."))
                harness.store.setEntries(entries)
                tailID = harness.store.transcriptCache.rows(revision: harness.store.revision,
                    entries: entries, pendingSends: harness.store.pendingSends).last!.id
                streamed = true
                streamedToggles.insert(toggle)
                events.append("stream toggle=\(toggle) timestamp=\(link.timestamp) height=\(height)")
                // Give this chunk a display update before reversing the animation.
                return
            }
            if toggle < 6, streamed, progress >= 0.5, progress < 1, abs(target - height) > 1 {
                events.append("reverse toggle=\(toggle) timestamp=\(link.timestamp) height=\(height)")
                legStart = height
                toggle += 1
                open.toggle()
                streamed = false
                withAnimation(Motion.resize) { harness.folds.values[id] = open }
            } else if toggle == 6 {
                closedFrames = abs(height - closedHeight) <= 1 ? closedFrames + 1 : 0
                if closedFrames == 3 {
                    finished = true
                    completed.fulfill()
                }
            }
        }
        defer { sampler.stop() }
        await fulfillment(of: [completed], timeout: 5)
        sampler.stop()

        // Compare presentation samples on the display clock, not Task.sleep's
        // wake-up clock. Retain both geometries and callback times to diagnose
        // delayed layout/commits and missed display updates in CI.
        let frameSteps = zip(samples, samples.dropFirst()).compactMap { previous, current -> CGFloat? in
            guard let before = previous.presentedHeight, let after = current.presentedHeight else { return nil }
            let elapsed = max(current.timestamp - previous.timestamp, 1.0 / 60)
            return abs(after - before) / CGFloat(elapsed * 60)
        }
        let encoder = JSONEncoder()
        encoder.outputFormatting = [.prettyPrinted, .sortedKeys]
        let diagnostic = String(decoding: try encoder.encode(samples), as: UTF8.self)
        let attachment = XCTAttachment(string: "closedHeight=\(closedHeight)\nopenedHeight=\(openedHeight)\n"
            + "events=\(events)\nframeSteps=\(frameSteps)\nsamples=\(diagnostic)")
        attachment.name = "tool-reversal-motion"
        attachment.lifetime = .keepAlways
        add(attachment)

        XCTAssertEqual(toggle, 6, "Every reversal must be observed before the animation reaches its endpoint")
        XCTAssertEqual(streamedToggles, Set(1...6), "Each animated leg must receive streaming output")
        XCTAssertGreaterThan(samples.count, 1, "Missing display samples cannot count as a passing animation")
        XCTAssertTrue(samples.allSatisfy { $0.presentedHeight != nil }, "The tool cell must remain presented")
        XCTAssertTrue(samples.allSatisfy { $0.tailVisible && $0.pinned }, "The presented tail must stay attached")
        XCTAssertLessThan(frameSteps.max() ?? .infinity, 100)
        await settle()
        XCTAssertEqual(table.rectForRow(at: path).height, closedHeight, accuracy: 1)
        assertTailVisible()
    }

    func testToolDisclosureKeepsHistoryHeaderAnchored() async {
        await mount(turns: 120)
        let rows = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: harness.store.entries, pendingSends: harness.store.pendingSends)
        let index = rows.indices.filter { if case .toolGroup = rows[$0].kind { return true }; return false }[110]
        let id = rows[index].id
        harness.folds.values[id] = false
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        let path = IndexPath(row: index, section: 0)
        harness.scroll.pinned = false
        table.scrollToRow(at: path, at: .top, animated: false)
        await settle()
        let cell = table.cellForRow(at: path)!
        func headerY() -> CGFloat {
            let layer = cell.layer.presentation() ?? cell.layer
            return layer.convert(layer.bounds, to: window.layer.presentation() ?? window.layer).minY
        }
        let startY = headerY()
        for open in [true, false] {
            withAnimation(Motion.resize) { harness.folds.values[id] = open }
            for _ in 0..<30 {
                try? await Task.sleep(for: .milliseconds(16))
                XCTAssertEqual(headerY(), startY, accuracy: 4)
                XCTAssertFalse(harness.scroll.pinned)
            }
        }
    }

    func testAppendingBlockAlsoRefreshesTheCompletedParagraph() async {
        await mount(turns: 0)
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        var rendered: [String: UInt64] = [:]
        func row(_ id: String, _ text: String, version: UInt64) -> TranscriptRow {
            TranscriptRow(id: id, version: version, turnStart: false,
                kind: .markdown(block: .paragraph([InlineRun(text: text, style: .plain)]), streaming: true),
                entryId: "reply", timestamp: nil, partKey: "reply#t0")
        }
        func input(_ rows: [TranscriptRow]) -> NativeTranscriptTable {
            NativeTranscriptTable(rows: rows, scroll: harness.scroll, runwayID: nil,
                expansionHeight: 0, bottomSpacing: 24, reduceMotion: true, configurationID: 0) { row in
                    rendered[row.id] = row.version
                    return AnyView(Text("Rendered version \(row.version)"))
                }
        }
        table.update(input([row("paragraph", "Reply on this", version: 1)]))
        table.layoutIfNeeded()
        XCTAssertEqual(rendered["paragraph"], 1)
        // One network chunk finishes a paragraph AND introduces the next block.
        table.update(input([row("paragraph", "Reply on this device:", version: 2),
                            row("next-block", "New block", version: 1)]))
        table.layoutIfNeeded()
        XCTAssertEqual(rendered["paragraph"], 2, "The already-visible paragraph must receive its final words")
        XCTAssertEqual(rendered["next-block"], 1)
    }

    func testRealKeyboardAndComposerMoveWithPinnedTranscript() async {
        await mount(turns: 600, useEditor: true)
        let editor = findNativeEditor(window)!
        let tailKey = key + "|a599#t1.0"
        for showing in [true, false, true, false] {
            let startPosition = TranscriptLayoutProbe.presentedFrame(for: key)!.maxY
            let baseline = startPosition - TranscriptLayoutProbe.presentedFrame(for: tailKey)!.maxY
            if showing { editor.becomeFirstResponder() } else { editor.resignFirstResponder() }
            var errors: [CGFloat] = []
            // Include the pre-animation position: the first async sample can
            // arrive after most of the keyboard transition has completed.
            var positions: [CGFloat] = [startPosition]
            for _ in 0..<100 {
                try? await Task.sleep(for: .milliseconds(16))
                if let viewport = TranscriptLayoutProbe.presentedFrame(for: key),
                   let tail = TranscriptLayoutProbe.presentedFrame(for: tailKey) {
                    positions.append(viewport.maxY)
                    errors.append(abs(viewport.maxY - tail.maxY - baseline))
                }
            }
            let attachment = XCTAttachment(string: "Viewport positions: \(positions)\nGap errors: \(errors)")
            attachment.name = showing ? "keyboard-opening-motion" : "keyboard-closing-motion"
            attachment.lifetime = .keepAlways
            add(attachment)
            XCTAssertEqual(errors.count, 100, "Both views must remain realized on every sample")
            XCTAssertGreaterThan((positions.max() ?? 0) - (positions.min() ?? 0), 150)
            XCTAssertLessThan(errors.max() ?? .infinity, 4)
            assertTailVisible()
        }
    }

    func testInterruptedKeyboardMotionKeepsTranscriptAttached() async throws {
        await mount(turns: 600, useEditor: true)
        let editor = findNativeEditor(window)!
        let tailKey = key + "|a599#t1.0"
        let hiddenPosition = try XCTUnwrap(TranscriptLayoutProbe.presentedFrame(for: key)).maxY
        let baseline = hiddenPosition - (try XCTUnwrap(TranscriptLayoutProbe.presentedFrame(for: tailKey))).maxY
        var samples: [GeometrySample] = []
        var events: [String] = []
        var driver = KeyboardReversalDriver(hiddenPosition: hiddenPosition)
        let completed = await DisplaySampler.observe("Keyboard reverses in flight and finishes hidden") { [self] link in
            let sample = geometrySample(link,
                position: TranscriptLayoutProbe.presentedFrame(for: key)?.maxY,
                modelPosition: TranscriptLayoutProbe.viewports[key]?.maxY, tailKey: tailKey)
            samples.append(sample)
            guard let position = sample.position, let target = sample.modelViewport?.maxY else { return false }
            let previousPhase = driver.phase
            let action = driver.observe(position: position, target: target, isFirstResponder: editor.isFirstResponder)
            if action != nil || driver.phase != previousPhase {
                events.append("timestamp=\(link.timestamp) position=\(position) target=\(target) \(driver.diagnostic) action=\(String(describing: action))")
            }
            switch action {
            case .setShowing(let showing):
                if showing { editor.becomeFirstResponder() } else { editor.resignFirstResponder() }
            case .finished:
                return true
            case nil:
                break
            }
            return false
        }
        let outcome = "samplerCompleted=\(completed) \(driver.diagnostic)"
        events.append(outcome)
        print("interrupted-keyboard-motion: \(outcome)")
        try attachMotion("interrupted-keyboard-motion", samples: samples, events: events)
        let gaps = samples.compactMap(\.gap)
        XCTAssertTrue(completed && driver.phase == .complete,
                      "Keyboard did not complete five observed in-flight reversals: \(outcome)")
        XCTAssertEqual(driver.reversals, 5)
        XCTAssertFalse(editor.isFirstResponder)
        XCTAssertGreaterThan(samples.count, 1)
        XCTAssertEqual(gaps.count, samples.count, "Viewport and tail must remain presented")
        XCTAssertLessThan(gaps.map { abs($0 - baseline) }.max() ?? .infinity, 4)
        assertTailVisible()
    }

    func testKeyboardKeepsStreamingRunwayPromptAnchored() async {
        await mount(turns: 2, useEditor: true)
        let store = harness.store
        store.demoResponder = { [weak store] prompt in
            guard let store else { return }
            var entries = store.entries
            entries.append(MessageEntry(id: "keyboard-prompt", role: .user,
                parts: [.text(id: "t0", text: prompt)], createdAt: nowMs(),
                deviceId: "test", status: .complete, continuationOf: nil))
            entries.append(MessageEntry(id: "keyboard-reply", role: .assistant,
                parts: [.text(id: "t0", text: "A short reply")], createdAt: nowMs(),
                deviceId: "test", status: .streaming, continuationOf: nil))
            store.setEntries(entries)
        }
        store.sendSteer(prompt: "Keep this turn anchored.")
        await settle()
        let editor = findNativeEditor(window)!
        for showing in [true, false, true, false] {
            if showing { editor.becomeFirstResponder() } else { editor.resignFirstResponder() }
            var errors: [CGFloat] = []
            for tick in 0..<70 {
                try? await Task.sleep(for: .milliseconds(16))
                if tick % 5 == 0 {
                    var entries = store.entries
                    entries[entries.count - 1].parts = [.text(id: "t0", text: "A short reply" + String(repeating: ".", count: tick / 5))]
                    store.setEntries(entries)
                }
                if let viewport = TranscriptLayoutProbe.presentedFrame(for: key),
                   let prompt = TranscriptLayoutProbe.presentedFrame(for: key + "|keyboard-prompt") {
                    errors.append(abs(viewport.minY - prompt.minY))
                }
            }
            let attachment = XCTAttachment(string: "Prompt anchor errors: \(errors)")
            attachment.name = "keyboard-streaming-runway"
            attachment.lifetime = .keepAlways
            add(attachment)
            XCTAssertEqual(errors.count, 70, "The streaming prompt must stay realized throughout keyboard motion")
            XCTAssertLessThan(errors.max() ?? .infinity, 4)
        }
    }

    func testKeyboardKeepsHistoryReadingAnchorStill() async {
        await mount(turns: 120, useEditor: true)
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        harness.scroll.pinned = false
        table.setContentOffset(CGPoint(x: 0, y: table.contentOffset.y - 1600), animated: false)
        await settle()
        let top = table.superview!.convert(table.superview!.bounds, to: window).minY
        let cell = table.visibleCells.first {
            let rect = table.convert($0.frame, to: window)
            return rect.minY > top && rect.minY < top + 250
        }!
        func presentedY() -> CGFloat {
            let layer = cell.layer.presentation() ?? cell.layer
            return layer.convert(layer.bounds, to: window.layer.presentation() ?? window.layer).minY
        }
        let start = presentedY()
        let editor = findNativeEditor(window)!
        for showing in [true, false] {
            if showing { editor.becomeFirstResponder() } else { editor.resignFirstResponder() }
            var errors: [CGFloat] = []
            for _ in 0..<100 {
                try? await Task.sleep(for: .milliseconds(16))
                errors.append(abs(presentedY() - start))
            }
            XCTAssertLessThan(errors.max() ?? .infinity, 4)
            XCTAssertFalse(harness.scroll.pinned)
        }
    }

    private func findNativeEditor(_ view: UIView) -> UITextView? {
        if let editor = view as? UITextView { return editor }
        return view.subviews.lazy.compactMap { self.findNativeEditor($0) }.first
    }

    func testAnimatedComposerResizeKeepsTranscriptAttachedThroughoutMotion() async throws {
        await mount(turns: 600)
        let tailKey = key + "|a599#t1.0"
        let startGap = TranscriptLayoutProbe.presentedFrame(for: key)!.maxY
            - TranscriptLayoutProbe.presentedFrame(for: tailKey)!.maxY
        for height: CGFloat in [390, 64, 240, 64] {
            let start = try XCTUnwrap(TranscriptLayoutProbe.presentedFrame(for: key)).maxY
            let target = start - (height - harness.composerHeight)
            var samples: [GeometrySample] = []
            var stableFrames = 0
            let completed = await DisplaySampler.observe("Composer reaches its requested height") { [self] link in
                let sample = geometrySample(link,
                    position: TranscriptLayoutProbe.presentedFrame(for: key)?.maxY,
                    modelPosition: nil, tailKey: tailKey)
                samples.append(sample)
                if samples.count == 1 {
                    withAnimation(.easeInOut(duration: 0.35)) { harness.composerHeight = height }
                    return false
                }
                let atTarget = sample.position.map { abs($0 - target) <= 1 } ?? false
                let modelAtTarget = sample.modelViewport.map { abs($0.maxY - target) <= 1 } ?? false
                stableFrames = atTarget && modelAtTarget ? stableFrames + 1 : 0
                return stableFrames == 3
            }
            try attachMotion("composer-motion-\(Int(height))", samples: samples)
            let positions = samples.compactMap(\.position)
            let gaps = samples.compactMap(\.gap)
            XCTAssertTrue(completed, "Composer did not reach its requested height")
            XCTAssertGreaterThan(samples.count, 1)
            XCTAssertEqual(positions.count, samples.count)
            XCTAssertEqual(gaps.count, samples.count, "Resizing must not temporarily unrealize the tail")
            XCTAssertTrue(positions.contains {
                $0 > min(start, target) + 1 && $0 < max(start, target) - 1
            }, "Composer resize must render an intermediate position")
            XCTAssertLessThan(gaps.map { abs($0 - startGap) }.max() ?? .infinity, 4,
                "Transcript and composer must move together throughout the resize")
            assertTailVisible()
        }
    }

    func testRowsRemainRealizedBeneathTopSafeArea() async {
        await mount(turns: 600)
        let table = harness.scroll.nativeScrollView as! TranscriptTableView
        let viewport = table.superview!
        let visibleTop = viewport.convert(viewport.bounds, to: window).minY
        XCTAssertGreaterThan(visibleTop, 0)
        XCTAssertLessThanOrEqual(table.convert(table.bounds, to: window).minY, 0)
        XCTAssertEqual(table.logicalViewportHeight, viewport.bounds.height, accuracy: 0.01)
        let earliestRow = table.visibleCells.map { table.convert($0.frame, to: window).minY }.min()!
        XCTAssertLessThan(earliestRow, visibleTop,
            "The table must realize rows behind the header, not stop at the safe-area edge")
        assertTailVisible()
    }

    func testPendingSendAnimatesAndRetainsRunwayThroughAdoptionAndReopen() async throws {
        await mount(turns: 600, offline: false)
        let native = try XCTUnwrap(harness.scroll.nativeScrollView)
        var samples: [GeometrySample] = []
        var submittedID: String?
        var stableFrames = 0
        let completed = await DisplaySampler.observe("Pending send reaches the runway anchor") { [self] link in
            let sample = geometrySample(link, position: native.layer.presentation()?.bounds.origin.y,
                modelPosition: native.contentOffset.y, tailKey: submittedID.map { key + "|" + $0 })
            samples.append(sample)
            if samples.count == 1 {
                harness.store.sendSteer(prompt: "Keep this local turn at the top.")
                submittedID = harness.store.lastSubmittedMessageId
                return false
            }
            // Keep exercising unchanged parent geometry during the native send
            // animation, after observing the currently presented frame.
            native.superview?.setNeedsLayout()
            native.superview?.layoutIfNeeded()
            let anchored = sample.viewport.flatMap { viewport in
                sample.tail.map { abs($0.minY - viewport.minY) <= 3 }
            } ?? false
            stableFrames = anchored ? stableFrames + 1 : 0
            return stableFrames == 3
        }
        try attachMotion("pending-send-animation-samples", samples: samples)
        let offsets = samples.compactMap(\.position)
        XCTAssertTrue(completed, "The pending prompt did not reach the top of the viewport")
        XCTAssertGreaterThan(samples.count, 1)
        XCTAssertEqual(offsets.count, samples.count, "The scroll view must remain presented")
        let start = try XCTUnwrap(offsets.first)
        let end = try XCTUnwrap(offsets.last)
        XCTAssertTrue(offsets.contains {
            $0 > min(start, end) + 1 && $0 < max(start, end) - 1
        }, "The send must glide through an intermediate offset, not jump after a delay")
        let id = try XCTUnwrap(submittedID)
        func promptAligned() -> Bool {
            TranscriptLayoutProbe.sample()
            guard let prompt = TranscriptLayoutProbe.tails[key + "|" + id],
                  let viewport = TranscriptLayoutProbe.viewports[key] else { return false }
            return abs(prompt.minY - viewport.minY) <= 3
        }
        XCTAssertTrue(promptAligned())
        var entries = harness.store.entries
        entries.append(MessageEntry(id: id, role: .user,
            parts: [.text(id: "t0", text: "Keep this local turn at the top.")], createdAt: nowMs(),
            deviceId: "test", status: .complete, continuationOf: nil))
        entries.append(MessageEntry(id: "adopted-reply", role: .assistant,
            parts: [.text(id: "t0", text: "Done.")], createdAt: nowMs(),
            deviceId: "test", status: .complete, continuationOf: nil))
        harness.store.setEntries(entries)
        let adoptedTail = harness.store.transcriptCache.rows(revision: harness.store.revision,
            entries: entries, pendingSends: harness.store.pendingSends).last!.id
        let adopted = await waitForTestCondition {
            promptAligned() && TranscriptLayoutProbe.tails[key + "|" + adoptedTail] != nil
        }
        XCTAssertTrue(adopted, "The adopted reply must render with its prompt anchored")
        harness.identity = UUID()
        harness.scroll = ScrollState()
        let reopened = await waitForTestCondition {
            guard let reopenedScroll = harness.scroll.nativeScrollView, reopenedScroll !== native else { return false }
            return promptAligned()
        }
        XCTAssertTrue(reopened, "The new transcript view must realize the retained prompt at the top")
    }

    func testWarm600TurnOpenRealizesTailWithoutScroll() async {
        await mount(turns: 600)
        assertTailVisible()
    }

    func testDelayed600TurnHydrationRealizesTailWithoutScroll() async {
        await mount(turns: 0)
        harness.store.setEntries(BenchRunner.syntheticEntries(turns: 600))
        await settle()
        assertTailVisible()
    }

    func testRepeatedComposerAndKeyboardResizesKeepTailVisible() async {
        await mount(turns: 120)
        for height: CGFloat in [144, 440, 200, 64, 440, 64] {
            harness.composerHeight = height
            await settle()
            assertTailVisible()
        }
    }

    func testLargeTailShrinkClampsWithoutUserScroll() async {
        await mount(turns: 600)
        harness.store.setEntries(BenchRunner.syntheticEntries(turns: 2))
        await settle()
        assertTailVisible()
    }

    func testStreamingAppendKeepsPinnedTailVisible() async {
        await mount(turns: 10)
        for turns in 11...14 {
            harness.store.setEntries(BenchRunner.syntheticEntries(turns: turns))
            await settle()
            assertTailVisible()
        }
    }

    func testStreamingAcrossRepeatedKeyboardResizesKeepsTailVisible() async {
        await mount(turns: 120)
        for tick in 0..<32 {
            var entries = harness.store.entries
            entries[entries.count - 1].status = .streaming
            entries[entries.count - 1].parts = [.text(id: "continuous", text:
                (0...tick).map { "Chunk \($0): streaming text stays visible as the composer changes height." }.joined(separator: "\n\n"))]
            harness.store.setEntries(entries)
            harness.composerHeight = [CGFloat(64), 144, 440, 200][(tick / 4) % 4]
            try? await Task.sleep(for: .milliseconds(90))
            assertTailVisible()
        }
    }

    func testCompactLandscapeViewportStillRealizesTail() async {
        await mount(turns: 120, size: CGSize(width: 844, height: 390))
        assertTailVisible()
    }

    func testAccessibilityTextResizeKeepsTailVisible() async {
        await mount(turns: 120)
        harness.dynamicTypeSize = .accessibility3
        await settle()
        assertTailVisible()
        harness.dynamicTypeSize = .large
        await settle()
        assertTailVisible()
    }

    func testNarrowViewportKeepsTailVisible() async {
        await mount(turns: 120, size: CGSize(width: 320, height: 568))
        assertTailVisible()
    }

    func testLocalRunwaySurvivesCompletionAndResizeThenHandsOffToLongReply() async {
        await mount(turns: 2)
        let store = harness.store
        store.demoResponder = { [weak store] prompt in
            guard let store else { return }
            var entries = store.entries
            entries.append(MessageEntry(id: "local-prompt", role: .user,
                parts: [.text(id: "t0", text: prompt)], createdAt: nowMs(),
                deviceId: "test", status: .complete, continuationOf: nil))
            entries.append(MessageEntry(id: "local-reply", role: .assistant,
                parts: [.text(id: "t0", text: "A short completed reply.")], createdAt: nowMs(),
                deviceId: "test", status: .complete, continuationOf: nil))
            store.setEntries(entries)
        }
        store.sendSteer(prompt: "Inspect this layout.")
        await settle()
        for height: CGFloat in [64, 440, 64] {
            harness.composerHeight = height
            await settle()
            let prompt = TranscriptLayoutProbe.tails[key + "|local-prompt"]!
            let viewport = TranscriptLayoutProbe.viewports[key]!
            XCTAssertEqual(prompt.minY, viewport.minY, accuracy: 3)
        }
        // Leaving and returning must retain the local turn's reservation.
        harness.identity = UUID()
        harness.scroll = ScrollState()
        await settle()
        XCTAssertEqual(TranscriptLayoutProbe.tails[key + "|local-prompt"]!.minY,
                       TranscriptLayoutProbe.viewports[key]!.minY, accuracy: 3)
        var entries = store.entries
        entries[entries.count - 1].parts = [.text(id: "t0", text:
            Array(repeating: "A long response consumes the reserved space and continues following the tail.", count: 30).joined(separator: "\n\n"))]
        store.setEntries(entries)
        await settle()
        assertTailVisible()
    }

    func testRepeatedWarmReopensResetReleasedFollow() async {
        await mount(turns: 600)
        for _ in 0..<5 {
            harness.scroll.pinned = false
            harness.identity = UUID()
            harness.scroll = ScrollState()
            await settle()
            assertTailVisible()
        }
    }
    func testSingleLongTurnStreamsAcrossReusedRows() async {
        await mount(turns: 1)
        for paragraphs in [30, 80, 120] {
            var entries = harness.store.entries
            entries[entries.count - 1].parts = [.text(id: "long", text:
                (0..<paragraphs).map { "Paragraph \($0): a long streamed response must keep its newest block visible." }.joined(separator: "\n\n"))]
            harness.store.setEntries(entries)
            await settle()
            assertTailVisible()
        }
    }

}
#endif
