import Loro
import XCTest
@testable import Zeron

@MainActor
final class SessionStoreDurabilityTests: XCTestCase {
    private enum WaitError: Error { case timedOut }

    private func waitUntil(_ description: String, timeout: Duration = .seconds(5),
                           file: StaticString = #filePath, line: UInt = #line,
                           _ condition: () -> Bool) async throws {
        let clock = ContinuousClock()
        let deadline = clock.now.advanced(by: timeout)
        while !condition() {
            guard clock.now < deadline else {
                XCTFail("Timed out waiting for \(description)", file: file, line: line)
                throw WaitError.timedOut
            }
            try await clock.sleep(for: .milliseconds(10))
        }
    }

    private func waitForPersistence(_ task: Task<Bool, Never>,
                                    file: StaticString = #filePath, line: UInt = #line) async throws -> Bool {
        let completed = expectation(description: "Stopped store's persistence attempt finished")
        var result: Bool?
        let observer = Task {
            let saved = await task.value
            guard !Task.isCancelled else { return }
            result = saved
            completed.fulfill()
        }
        defer { observer.cancel() }
        await fulfillment(of: [completed], timeout: 5)
        return try XCTUnwrap(result, "Persistence did not finish before the timeout", file: file, line: line)
    }

    private func config() -> AppConfig {
        AppConfig(edgeURL: URL(string: "http://localhost:1")!, mode: .dev,
                  userId: "u", orgId: "o", deviceId: "phone",
                  deviceName: "phone", tokens: nil, devBearer: "u@o")
    }

    private func appendEntry(_ store: SessionStore, id: String, text: String) throws {
        let list = store.doc.getMovableList(id: "messages")
        let map = try list.insertMapContainer(pos: list.len(), child: LoroMap())
        try map.insert(key: "id", v: id)
        try map.insert(key: "role", v: "assistant")
        try map.insert(key: "createdAt", v: Int64(1))
        try map.insert(key: "deviceId", v: "remote")
        let parts = try map.insertContainer(key: "parts", child: LoroList())
        let part = try parts.insertMapContainer(pos: 0, child: LoroMap())
        try part.insert(key: "id", v: "\(id)-part")
        try part.insert(key: "kind", v: "text")
        try part.insert(key: "text", v: text)
    }

    func testCommitNowSavesImmediatelyAndNotifies() {
        var saves = 0
        var callbacks = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000)
        saver.onSaved = { callbacks += 1 }
        saver.poke()

        XCTAssertTrue(saver.commitNow())
        XCTAssertEqual(saves, 1)
        XCTAssertEqual(callbacks, 1)
        XCTAssertFalse(saver.isDirty)
    }

    func testFailedCommitStaysDirtyAndLaterFlushNotifies() {
        var shouldSucceed = false
        var callbacks = 0
        let saver = DocSaver { shouldSucceed }
        saver.onSaved = { callbacks += 1 }
        saver.poke()

        XCTAssertFalse(saver.commitNow())
        XCTAssertTrue(saver.isDirty)
        XCTAssertEqual(callbacks, 0)

        shouldSucceed = true
        saver.flush()
        XCTAssertFalse(saver.isDirty)
        XCTAssertEqual(callbacks, 1)
    }

    func testCommitAsyncWritesAndNotifies() async {
        var callbacks = 0
        var written: Data?
        let saver = DocSaver { true }
        saver.onSaved = { callbacks += 1 }
        saver.poke()

        let result = await saver.commitAsync(
            export: { Data([1, 2, 3]) },
            write: { data in
                written = data
                return true
            }
        )

        XCTAssertTrue(result)
        XCTAssertEqual(written, Data([1, 2, 3]))
        XCTAssertFalse(saver.isDirty)
        XCTAssertEqual(callbacks, 1)
    }

    func testCommitAsyncKeepsConsistentStaleExportDirty() async {
        var writes = 0
        let saver = DocSaver { true }
        saver.poke()

        let result = await saver.commitAsync(
            export: {
                DispatchQueue.main.sync {
                    MainActor.assumeIsolated {
                        saver.poke()
                    }
                }
                return Data([4, 5, 6])
            },
            write: { _ in
                writes += 1
                return true
            }
        )

        XCTAssertFalse(result)
        XCTAssertEqual(writes, 1)
        XCTAssertTrue(saver.isDirty)
    }

    func testCommitAsyncRetiresDebounceWhileExporting() async {
        let scheduler = ManualDocSaverScheduler()
        let started = expectation(description: "detached export started")
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        var saves = 0
        var wrote = false
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000, scheduler: scheduler)
        saver.poke()

        let task = Task { @MainActor in
            await saver.commitAsync(
                export: {
                    started.fulfill()
                    gate.wait()
                    return Data([7])
                },
                write: { _ in
                    wrote = true
                    return true
                }
            )
        }

        await fulfillment(of: [started], timeout: 5)
        await scheduler.advance(by: 2_000_000_000)
        XCTAssertEqual(saves, 0)
        XCTAssertFalse(wrote)

        gate.signal()
        let result = await task.value
        XCTAssertTrue(result)
        XCTAssertTrue(wrote)
        XCTAssertFalse(saver.isDirty)
    }

    func testRetireTimersLeavesDirtyAndCancelsDebounce() async {
        let scheduler = ManualDocSaverScheduler()
        var saves = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000, scheduler: scheduler)
        saver.poke()
        saver.retireTimers()

        await scheduler.advance(by: 11_000_000_000)
        XCTAssertTrue(saver.isDirty)
        XCTAssertEqual(saves, 0)
    }

    func testCommitAsyncRetiresRetryWhileExporting() async {
        let scheduler = ManualDocSaverScheduler()
        let started = expectation(description: "detached export started")
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        var saves = 0
        var shouldSucceed = false
        var wrote = false
        let saver = DocSaver(save: {
            saves += 1
            return shouldSucceed
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000, scheduler: scheduler)
        saver.poke()
        XCTAssertFalse(saver.commitNow())
        XCTAssertEqual(saves, 1)

        let task = Task { @MainActor in
            await saver.commitAsync(
                export: {
                    started.fulfill()
                    gate.wait()
                    return Data([8])
                },
                write: { _ in
                    wrote = true
                    return true
                }
            )
        }

        await fulfillment(of: [started], timeout: 5)
        await scheduler.advance(by: 2_500_000_000)
        XCTAssertEqual(saves, 1)
        XCTAssertFalse(wrote)

        shouldSucceed = true
        gate.signal()
        let result = await task.value
        XCTAssertTrue(result)
        XCTAssertTrue(wrote)
        XCTAssertFalse(saver.isDirty)
    }

    func testAsyncFlushesRetireBothTimers() async {
        let scheduler = ManualDocSaverScheduler()
        let started = expectation(description: "first detached export started")
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        var aSaves = 0
        var bSaves = 0
        var aWrote = false
        var bWrote = false
        let a = DocSaver(save: {
            aSaves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000, scheduler: scheduler)
        let b = DocSaver(save: {
            bSaves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000, scheduler: scheduler)
        a.poke()
        b.poke()
        a.retireTimers()
        b.retireTimers()

        let task = Task { @MainActor in
            _ = await a.commitAsync(
                export: {
                    started.fulfill()
                    gate.wait()
                    return Data([9])
                },
                write: { _ in
                    aWrote = true
                    return true
                }
            )
            _ = await b.commitAsync(
                export: { Data([10]) },
                write: { _ in
                    bWrote = true
                    return true
                }
            )
        }

        await fulfillment(of: [started], timeout: 5)
        await scheduler.advance(by: 11_000_000_000)
        XCTAssertEqual(aSaves, 0)
        XCTAssertEqual(bSaves, 0)

        gate.signal()
        await task.value
        XCTAssertTrue(aWrote)
        XCTAssertTrue(bWrote)
        XCTAssertFalse(a.isDirty)
        XCTAssertFalse(b.isDirty)
    }

    func testCommitAsyncWritesOlderSnapshotAfterNewPoke() async {
        let started = expectation(description: "detached export started")
        let gate = DispatchSemaphore(value: 0)
        var wrote = false
        let saver = DocSaver(save: { true },
                             quietDebounceNs: 1_500_000_000,
                             maxDeferralNs: 10_000_000_000)
        saver.poke()

        let task = Task { @MainActor in
            await saver.commitAsync(
                export: {
                    started.fulfill()
                    gate.wait()
                    return Data([11])
                },
                write: { _ in
                    wrote = true
                    return true
                }
            )
        }

        await fulfillment(of: [started], timeout: 1)
        saver.poke()
        gate.signal()

        let result = await task.value
        XCTAssertFalse(result)
        XCTAssertTrue(wrote)
        XCTAssertTrue(saver.isDirty)
    }

    func testQuietDebounceCoalescesContinuousPokes() async {
        let scheduler = ManualDocSaverScheduler()
        var saves = 0
        var callbacks = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 300_000_000, maxDeferralNs: 10_000_000_000, scheduler: scheduler)
        saver.onSaved = { callbacks += 1 }

        for _ in 0..<40 {
            saver.poke()
            await scheduler.advance(by: 50_000_000)
            XCTAssertEqual(saves, 0, "Every poke must restart the quiet period")
        }
        // The last poke was 50 ms ago: stop one nanosecond before its deadline.
        await scheduler.advance(by: 249_999_999)
        XCTAssertEqual(saves, 0)
        XCTAssertTrue(saver.isDirty)
        await scheduler.advance(by: 1)
        XCTAssertEqual(saves, 1)
        XCTAssertEqual(callbacks, 1)
        XCTAssertFalse(saver.isDirty)

        // Superseded quiet timers and the old maximum deadline cannot save again.
        await scheduler.advance(by: 10_000_000_000)
        XCTAssertEqual(saves, 1)
        XCTAssertEqual(callbacks, 1)
    }

    func testMaxDeferralFlushesDuringContinuousPokes() async {
        let scheduler = ManualDocSaverScheduler()
        var saves = 0
        var backgroundFlushes = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 5_000_000_000, maxDeferralNs: 400_000_000,
        staleRetryNs: 400_000_000, scheduler: scheduler)
        saver.background = {
            backgroundFlushes += 1
            return true
        }

        // A new dirty cycle must get its own bounded deadline.
        for cycle in 1...2 {
            for _ in 0..<7 {
                saver.poke()
                await scheduler.advance(by: 50_000_000)
                XCTAssertEqual(backgroundFlushes, cycle - 1)
            }
            saver.poke()
            await scheduler.advance(by: 49_999_999)
            XCTAssertEqual(backgroundFlushes, cycle - 1)
            await scheduler.advance(by: 1)
            XCTAssertEqual(backgroundFlushes, cycle)
            XCTAssertFalse(saver.isDirty)
        }
        await scheduler.advance(by: 5_000_000_000)
        XCTAssertEqual(backgroundFlushes, 2)
        XCTAssertEqual(saves, 0, "The deadline must use the background hook")
    }

    func testStaleBackgroundExportRearmsBoundedDeadline() async {
        let scheduler = ManualDocSaverScheduler()
        let started = expectation(description: "deadline export started")
        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        var exports = 0
        var writes = 0
        var callbacks = 0
        let saver = DocSaver(save: { true },
                             quietDebounceNs: 5_000_000_000,
                             maxDeferralNs: 400_000_000,
                             staleRetryNs: 300_000_000, scheduler: scheduler)
        saver.onSaved = { callbacks += 1 }
        saver.background = { [weak saver] in
            guard let saver else { return false }
            return await saver.commitAsync(
                export: {
                    exports += 1
                    if exports == 1 {
                        started.fulfill()
                        gate.wait()
                    }
                    return Data([UInt8(exports)])
                },
                write: { _ in
                    writes += 1
                    return true
                }
            )
        }

        saver.poke()
        // Run the due action separately so the test can invalidate its blocked export.
        let deadline = Task { await scheduler.advance(by: 400_000_000) }
        await fulfillment(of: [started], timeout: 5)
        saver.poke()
        gate.signal()
        await deadline.value

        XCTAssertEqual(writes, 1)
        XCTAssertTrue(saver.isDirty)
        XCTAssertEqual(callbacks, 0)

        for _ in 0..<5 {
            saver.poke()
            await scheduler.advance(by: 50_000_000)
            XCTAssertEqual(writes, 1)
        }
        saver.poke()
        await scheduler.advance(by: 49_999_999)
        XCTAssertEqual(writes, 1)
        await scheduler.advance(by: 1)
        XCTAssertEqual(exports, 2)
        XCTAssertEqual(writes, 2)
        XCTAssertEqual(callbacks, 1)
        XCTAssertFalse(saver.isDirty)
    }

    func testDeadlineOverlapsDebounceExportAndPersistsNewestChange() async throws {
        let scheduler = ManualDocSaverScheduler()
        let firstExportStarted = expectation(description: "debounce export is blocked")
        let secondCallbackStarted = expectation(description: "deadline callback overlaps debounce")
        let secondExportStarted = expectation(description: "deadline export is blocked")
        let firstGate = DispatchSemaphore(value: 0)
        let secondGate = DispatchSemaphore(value: 0)
        defer {
            firstGate.signal()
            secondGate.signal()
        }
        let url = FileManager.default.temporaryDirectory.appendingPathComponent("overlapping-save-\(UUID().uuidString)")
        defer { try? FileManager.default.removeItem(at: url) }
        var snapshot = Data([1])
        var writes: [Data] = []
        var results: [Bool] = []
        var callbacks = 0
        var attempts = 0
        var inFlight = 0
        let saver = DocSaver(save: {
            XCTFail("Timer saves must use the background hook")
            return false
        }, quietDebounceNs: 300_000_000, maxDeferralNs: 400_000_000,
        staleRetryNs: 300_000_000, scheduler: scheduler)
        saver.onSaved = { callbacks += 1 }
        saver.background = { [weak saver] in
            guard let saver else { return false }
            attempts += 1
            inFlight += 1
            defer { inFlight -= 1 }
            let attempt = attempts
            let exportedSnapshot = snapshot
            if attempt == 2 {
                XCTAssertEqual(inFlight, 2, "The deadline must enter before the debounce returns")
                secondCallbackStarted.fulfill()
            }
            let result = await saver.commitAsync(
                export: {
                    if attempt == 1 {
                        firstExportStarted.fulfill()
                        firstGate.wait()
                    } else if attempt == 2 {
                        secondExportStarted.fulfill()
                        secondGate.wait()
                    }
                    return exportedSnapshot
                },
                write: { data in
                    do {
                        try data.write(to: url, options: .atomic)
                        writes.append(data)
                        return true
                    } catch {
                        XCTFail("Snapshot write failed: \(error)")
                        return false
                    }
                }
            )
            results.append(result)
            return result
        }

        func waitForTimer(_ task: Task<Void, Never>) async {
            let finished = expectation(description: "timer callback completed")
            let observer = Task {
                await task.value
                guard !Task.isCancelled else { return }
                finished.fulfill()
            }
            defer { observer.cancel() }
            await fulfillment(of: [finished], timeout: 5)
        }

        saver.poke()
        let debounce = try XCTUnwrap(scheduler.startNext()) // 300 ms
        await fulfillment(of: [firstExportStarted], timeout: 5)
        XCTAssertEqual(attempts, 1)
        snapshot = Data([2])
        saver.poke()

        await scheduler.advance(by: 99_999_999)
        XCTAssertEqual(attempts, 1, "The maximum deadline must not fire early")
        let deadline = try XCTUnwrap(scheduler.startNext()) // 400 ms, while debounce is suspended
        await fulfillment(of: [secondCallbackStarted], timeout: 5)
        XCTAssertTrue(writes.isEmpty)
        XCTAssertTrue(saver.isDirty)
        XCTAssertEqual(callbacks, 0)

        firstGate.signal()
        await fulfillment(of: [secondExportStarted], timeout: 5)
        await waitForTimer(debounce)
        XCTAssertEqual(writes, [Data([1])])
        XCTAssertEqual(try Data(contentsOf: url), Data([1]))
        XCTAssertEqual(results, [false], "An older snapshot cannot complete the newer save")
        XCTAssertTrue(saver.isDirty, "The latest change is still waiting for its export")
        XCTAssertEqual(callbacks, 0, "An obsolete export must not announce a completed save")

        secondGate.signal()
        await waitForTimer(deadline)
        XCTAssertEqual(writes, [Data([1]), Data([2])])
        XCTAssertEqual(try Data(contentsOf: url), Data([2]))
        XCTAssertEqual(results, [false, true])
        XCTAssertFalse(saver.isDirty)
        XCTAssertEqual(callbacks, 1)
        XCTAssertEqual(inFlight, 0)

        // The newer poke and the stale export both armed timers. Neither may
        // write or notify again after the overlapping deadline saved the change.
        await scheduler.advance(by: 10_000_000_000)
        XCTAssertEqual(attempts, 2)
        XCTAssertEqual(writes.count, 2)
        XCTAssertEqual(callbacks, 1)
        XCTAssertEqual(try Data(contentsOf: url), Data([2]))
    }

    func testFailedSaveRetriesAfterTwoSeconds() async {
        let scheduler = ManualDocSaverScheduler()
        var saves = 0
        var callbacks = 0
        let saver = DocSaver(save: {
            saves += 1
            return saves > 1
        }, scheduler: scheduler)
        saver.onSaved = { callbacks += 1 }
        saver.poke()
        XCTAssertFalse(saver.commitNow())
        await scheduler.advance(by: 1_999_999_999)
        XCTAssertEqual(saves, 1)
        XCTAssertEqual(callbacks, 0)
        XCTAssertTrue(saver.isDirty)
        await scheduler.advance(by: 1)
        XCTAssertEqual(saves, 2)
        XCTAssertEqual(callbacks, 1)
        XCTAssertFalse(saver.isDirty)
        await scheduler.advance(by: 300_000_000_000)
        XCTAssertEqual(saves, 2)
    }

    func testRealTimerDebounceUsesBackgroundHook() async {
        let saved = expectation(description: "real debounce invokes the background hook")
        var saves = 0
        var backgroundFlushes = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 10_000_000, maxDeferralNs: 10_000_000_000)
        saver.background = {
            backgroundFlushes += 1
            return true
        }
        saver.onSaved = { saved.fulfill() }

        saver.poke()
        await fulfillment(of: [saved], timeout: 5)

        XCTAssertEqual(saves, 0)
        XCTAssertEqual(backgroundFlushes, 1)
        XCTAssertFalse(saver.isDirty)
    }

    func testSnapshotExporterSerializesExports() async {
        let firstStarted = expectation(description: "first export started")
        let gate = DispatchSemaphore(value: 0)
        var secondStarted = false
        let first = Task {
            await SnapshotExporter.shared.export {
                firstStarted.fulfill()
                gate.wait()
                return Data([1])
            }
        }
        await fulfillment(of: [firstStarted], timeout: 1)
        let second = Task {
            await SnapshotExporter.shared.export {
                secondStarted = true
                return Data([2])
            }
        }
        try? await Task.sleep(nanoseconds: 100_000_000)
        XCTAssertFalse(secondStarted)

        gate.signal()
        let firstResult = await first.value
        let secondResult = await second.value
        XCTAssertEqual(firstResult, Data([1]))
        XCTAssertEqual(secondResult, Data([2]))
        XCTAssertTrue(secondStarted)
    }

    func testRealStoreAsyncFlushPersistsOutbox() async throws {
        let id = "async-flush-\(UUID().uuidString)"
        let url = DocDisk.chat2URL(for: id)
        defer {
            try? FileManager.default.removeItem(at: url)
        }
        let store = SessionStore(chatId: id, config: config())
        defer { store.stop() }
        store.start(holdDial: true)
        let baseline = store.outbox.count

        try store.doc.getMap(id: "test").insert(key: "value", v: "async")
        store.doc.commit()

        try await waitUntil("the local update to enter the outbox") {
            store.outbox.count > baseline
        }
        XCTAssertEqual(store.outbox.count, baseline + 1)

        await store.flushToDiskAsync()

        let loaded = try XCTUnwrap(DocDisk.loadChat2(into: LoroDoc(), id: id))
        XCTAssertEqual(loaded.outbox.map(\.batchId), store.outbox.map(\.batchId))
    }

    func testLocalCommitIsOnDiskBeforeAdmission() async throws {
        let id = "durable-store-\(UUID().uuidString)"
        defer { try? FileManager.default.removeItem(at: DocDisk.chat2URL(for: id)) }
        let store = SessionStore(chatId: id, config: config())
        store.start()
        store.updateRoomGen(2)
        let baseline = Set(store.outbox.map(\.batchId))

        try store.doc.getMap(id: "test").insert(key: "value", v: "durable")
        store.doc.commit()

        try await waitUntil("the committed batch to enter the outbox") {
            !Set(store.outbox.map(\.batchId)).subtracting(baseline).isEmpty
        }
        let newBatchIDs = Set(store.outbox.map(\.batchId)).subtracting(baseline)
        XCTAssertEqual(newBatchIDs.count, 1)
        let loaded = try XCTUnwrap(DocDisk.loadChat2(into: LoroDoc(), id: id))
        XCTAssertTrue(newBatchIDs.isSubset(of: Set(loaded.outbox.map(\.batchId))))
        XCTAssertTrue(newBatchIDs.isSubset(of: store.admittedBatchIDs))
        store.stop()
    }

    func testFailedWriteBlocksAdmissionUntilRetrySucceeds() async throws {
        let id = "durable-retry-\(UUID().uuidString)"
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("zeron-durable-retry-\(UUID().uuidString)")
        let blocker = root.appendingPathComponent("blocker")
        let destination = blocker.appendingPathComponent("dir")
        let restoredDirectory = root.appendingPathComponent("restored", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try Data("not a directory".utf8).write(to: blocker)
        defer {
            DocDisk.directoryOverride = nil
            try? FileManager.default.removeItem(at: root)
            try? FileManager.default.removeItem(at: DocDisk.chat2URL(for: id))
        }
        DocDisk.directoryOverride = destination
        XCTAssertFalse(DocDisk.saveChat2(doc: LoroDoc(), id: "probe-\(id)", cursor: 0, verified: false))

        let store = SessionStore(chatId: id, config: config())
        store.start()
        store.updateRoomGen(2)
        let baselineCount = store.outbox.count
        try store.doc.getMap(id: "test").insert(key: "value", v: "retry")
        store.doc.commit()

        try await waitUntil("the failed write's batch to enter the outbox") {
            store.outbox.count > baselineCount
        }
        XCTAssertEqual(store.outbox.count, baselineCount + 1)
        XCTAssertTrue(store.admittedBatchIDs.isEmpty)

        DocDisk.directoryOverride = restoredDirectory
        try await waitUntil("the retried write to admit its durable batches") {
            !store.admittedBatchIDs.isEmpty
        }
        XCTAssertEqual(store.admittedBatchIDs, Set(store.outbox.map(\.batchId)))
        let loaded = try XCTUnwrap(DocDisk.loadChat2(into: LoroDoc(), id: id))
        XCTAssertEqual(loaded.outbox.map(\.batchId), store.outbox.map(\.batchId))
        store.stop()
    }

    func testStoppedStoreCannotWriteAfterWipe() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("zeron-lease-wipe-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        DocDisk.directoryOverride = root
        defer {
            DocDisk.directoryOverride = nil
            try? FileManager.default.removeItem(at: root)
        }

        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        let started = expectation(description: "blocking export started")
        Task {
            await SnapshotExporter.shared.export {
                started.fulfill()
                gate.wait()
                return Data([0])
            }
        }
        await fulfillment(of: [started], timeout: 1)

        let id = "lease-wipe-\(UUID().uuidString)"
        let store = SessionStore(chatId: id, config: config())
        store.start(holdDial: true)
        try appendEntry(store, id: "first", text: "dirty")
        store.doc.commit()
        try await waitUntil("the batch to enter the outbox before wiping") {
            store.outbox.count == 1
        }
        // The local commit saved synchronously. Dirty the snapshot again so
        // stop actually queues a write behind the blocked exporter.
        store.retirePush(batchId: try XCTUnwrap(store.outbox.first?.batchId))
        let persistence = try XCTUnwrap(store.stop())
        DocDisk.wipeAll()
        gate.signal()
        let saved = try await waitForPersistence(persistence)
        XCTAssertFalse(saved, "A revoked lease must reject the stopped store's write")

        XCTAssertFalse(FileManager.default.fileExists(atPath: root.path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: DocDisk.chat2URL(for: id).path))
    }

    func testStoppedStoreWritesAfterDeallocation() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("zeron-lease-deallocated-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let blockerFile = root.appendingPathComponent("blocker")
        let failingDirectory = blockerFile.appendingPathComponent("dir")
        try Data("not a directory".utf8).write(to: blockerFile)
        DocDisk.directoryOverride = failingDirectory
        defer {
            DocDisk.directoryOverride = nil
            try? FileManager.default.removeItem(at: root)
        }

        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        let started = expectation(description: "blocking export started")
        Task {
            await SnapshotExporter.shared.export {
                started.fulfill()
                gate.wait()
                return Data([0])
            }
        }
        await fulfillment(of: [started], timeout: 1)

        let id = "lease-deallocated-\(UUID().uuidString)"
        var batchID = ""
        weak var weakStore: SessionStore?
        let persistence: Task<Bool, Never>
        do {
            let store = SessionStore(chatId: id, config: config())
            weakStore = store
            store.start(holdDial: true)
            try appendEntry(store, id: "deallocated", text: "deallocated")
            store.doc.commit()
            try await waitUntil("the batch to enter the outbox before deallocation") {
                store.outbox.count == 1
            }
            batchID = try XCTUnwrap(store.outbox.first?.batchId)
            persistence = try XCTUnwrap(store.stop())
        }
        XCTAssertNil(weakStore)

        DocDisk.directoryOverride = root
        gate.signal()
        let saved = try await waitForPersistence(persistence)
        XCTAssertTrue(saved, "The final snapshot must persist after the store is released")

        let loaded = try XCTUnwrap(DocDisk.loadChat2(into: LoroDoc(), id: id))
        XCTAssertEqual(loaded.outbox.map(\.batchId), [batchID])
    }

    func testReplacementStoreWinsOverStoppedStoreExport() async throws {
        let root = FileManager.default.temporaryDirectory
            .appendingPathComponent("zeron-lease-replacement-\(UUID().uuidString)", isDirectory: true)
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        let blockerFile = root.appendingPathComponent("blocker")
        let destination = blockerFile.appendingPathComponent("dir")
        try Data("not a directory".utf8).write(to: blockerFile)
        DocDisk.directoryOverride = destination
        defer {
            DocDisk.directoryOverride = nil
            try? FileManager.default.removeItem(at: root)
        }

        let gate = DispatchSemaphore(value: 0)
        defer { gate.signal() }
        let started = expectation(description: "blocking export started")
        Task {
            await SnapshotExporter.shared.export {
                started.fulfill()
                gate.wait()
                return Data([0])
            }
        }
        await fulfillment(of: [started], timeout: 1)

        let id = "lease-replacement-\(UUID().uuidString)"
        let old = SessionStore(chatId: id, config: config())
        old.start(holdDial: true)
        try appendEntry(old, id: "old", text: "old")
        old.doc.commit()
        try await waitUntil("the old store's batch to enter the outbox") {
            old.outbox.count == 1
        }
        let persistence = try XCTUnwrap(old.stop())

        DocDisk.directoryOverride = root
        let replacement = SessionStore(chatId: id, config: config())
        replacement.start(holdDial: true)
        try appendEntry(replacement, id: "new", text: "new")
        replacement.doc.commit()
        try await waitUntil("the replacement store's batch to enter the outbox") {
            replacement.outbox.count == 1
        }
        XCTAssertEqual(replacement.outbox.count, 1)
        replacement.flushToDisk()

        gate.signal()
        let saved = try await waitForPersistence(persistence)
        XCTAssertFalse(saved, "The old store must not overwrite its replacement")
        let loaded = try XCTUnwrap(DocDisk.loadChat2(into: LoroDoc(), id: id))
        XCTAssertEqual(loaded.outbox.map(\.batchId), replacement.outbox.map(\.batchId))
        replacement.stop()
    }

    func testDetachedProjectionRetainsTrailingUpdate() async throws {
        let id = "projection-trailing-\(UUID().uuidString)"
        let store = SessionStore(chatId: id, config: config())
        try appendEntry(store, id: "first", text: "first")
        store.doc.commit()
        store.start(holdDial: true)
        try await waitUntil("the initial projection") {
            store.entries.contains { $0.id == "first" }
        }
        try appendEntry(store, id: "second", text: "second")
        store.doc.commit()
        store.project()
        try await waitUntil("the trailing projection") {
            store.entries.contains { $0.id == "second" }
        }
        XCTAssertTrue(store.entries.contains { entry in
            entry.id == "second"
        })
        store.stop()
    }
}
