import Loro
import XCTest
@testable import Zeron

@MainActor
final class SessionStoreDurabilityTests: XCTestCase {
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
        let started = expectation(description: "detached export started")
        let gate = DispatchSemaphore(value: 0)
        var saves = 0
        var wrote = false
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000)
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

        await fulfillment(of: [started], timeout: 1)
        try? await Task.sleep(nanoseconds: 2_000_000_000)
        XCTAssertEqual(saves, 0)
        XCTAssertFalse(wrote)

        gate.signal()
        let result = await task.value
        XCTAssertTrue(result)
        XCTAssertTrue(wrote)
        XCTAssertFalse(saver.isDirty)
    }

    func testRetireTimersLeavesDirtyAndCancelsDebounce() async {
        var saves = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000)
        saver.poke()
        saver.retireTimers()

        try? await Task.sleep(nanoseconds: 2_000_000_000)
        XCTAssertTrue(saver.isDirty)
        XCTAssertEqual(saves, 0)
    }

    func testCommitAsyncRetiresRetryWhileExporting() async {
        let started = expectation(description: "detached export started")
        let gate = DispatchSemaphore(value: 0)
        var saves = 0
        var shouldSucceed = false
        var wrote = false
        let saver = DocSaver(save: {
            saves += 1
            return shouldSucceed
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000)
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

        await fulfillment(of: [started], timeout: 1)
        try? await Task.sleep(nanoseconds: 2_500_000_000)
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
        let started = expectation(description: "first detached export started")
        let gate = DispatchSemaphore(value: 0)
        var aSaves = 0
        var bSaves = 0
        var aWrote = false
        var bWrote = false
        let a = DocSaver(save: {
            aSaves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000)
        let b = DocSaver(save: {
            bSaves += 1
            return true
        }, quietDebounceNs: 1_500_000_000, maxDeferralNs: 10_000_000_000)
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

        await fulfillment(of: [started], timeout: 1)
        try? await Task.sleep(nanoseconds: 2_000_000_000)
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
        var saves = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 300_000_000, maxDeferralNs: 10_000_000_000)

        for _ in 0..<40 {
            saver.poke()
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        try? await Task.sleep(nanoseconds: 500_000_000)

        XCTAssertEqual(saves, 1)
    }

    func testMaxDeferralFlushesDuringContinuousPokes() async {
        var saves = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 5_000_000_000, maxDeferralNs: 400_000_000,
        staleRetryNs: 400_000_000)
        var backgroundFlushes = 0
        saver.background = {
            backgroundFlushes += 1
            saves += 1
            return true
        }

        for _ in 0..<20 {
            saver.poke()
            try? await Task.sleep(nanoseconds: 50_000_000)
        }

        XCTAssertGreaterThanOrEqual(saves, 1)
        XCTAssertGreaterThanOrEqual(backgroundFlushes, 1)
    }

    func testStaleBackgroundExportRearmsBoundedDeadline() async {
        let started = expectation(description: "deadline export started")
        let gate = DispatchSemaphore(value: 0)
        var exports = 0
        var writes = 0
        var callbacks = 0
        let saver = DocSaver(save: { true },
                             quietDebounceNs: 5_000_000_000,
                             maxDeferralNs: 100_000_000,
                             staleRetryNs: 300_000_000)
        saver.onSaved = { callbacks += 1 }
        saver.background = {
            await saver.commitAsync(
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
        await fulfillment(of: [started], timeout: 1)
        saver.poke()
        gate.signal()
        try? await Task.sleep(nanoseconds: 100_000_000)

        XCTAssertEqual(writes, 1)
        XCTAssertTrue(saver.isDirty)
        XCTAssertEqual(callbacks, 0)

        let deadline = Date().addingTimeInterval(1.5)
        while Date() < deadline, writes < 2 {
            saver.poke()
            try? await Task.sleep(nanoseconds: 50_000_000)
        }
        XCTAssertGreaterThanOrEqual(writes, 2)
    }

    func testDebounceUsesBackgroundHook() async {
        var saves = 0
        var backgroundFlushes = 0
        let saver = DocSaver(save: {
            saves += 1
            return true
        }, quietDebounceNs: 100_000_000, maxDeferralNs: 10_000_000_000)
        saver.background = {
            backgroundFlushes += 1
            return true
        }

        saver.poke()
        try? await Task.sleep(nanoseconds: 300_000_000)

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

        for _ in 0..<20 {
            await Task.yield()
            if store.outbox.count > baseline { break }
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

        var newBatchIDs: Set<String> = []
        for _ in 0..<20 {
            await Task.yield()
            newBatchIDs = Set(store.outbox.map(\.batchId)).subtracting(baseline)
            if !newBatchIDs.isEmpty { break }
        }
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

        for _ in 0..<20 { await Task.yield() }
        XCTAssertEqual(store.outbox.count, baselineCount + 1)
        XCTAssertTrue(store.admittedBatchIDs.isEmpty)

        DocDisk.directoryOverride = restoredDirectory
        for _ in 0..<35 {
            if !store.admittedBatchIDs.isEmpty { break }
            try await Task.sleep(for: .milliseconds(100))
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
        let blockerFile = root.appendingPathComponent("blocker")
        let failingDirectory = blockerFile.appendingPathComponent("dir")
        try Data("not a directory".utf8).write(to: blockerFile)
        DocDisk.directoryOverride = failingDirectory
        defer {
            DocDisk.directoryOverride = nil
            try? FileManager.default.removeItem(at: root)
        }

        let gate = DispatchSemaphore(value: 0)
        let started = expectation(description: "blocking export started")
        let blocker = Task {
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
        for _ in 0..<20 {
            await Task.yield()
            if store.outbox.count == 1 { break }
        }
        store.stop()
        DocDisk.directoryOverride = root
        DocDisk.wipeAll()
        gate.signal()
        _ = await blocker.value
        try await Task.sleep(for: .milliseconds(100))

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
        let started = expectation(description: "blocking export started")
        let exportBlocker = Task {
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
        do {
            let store = SessionStore(chatId: id, config: config())
            weakStore = store
            store.start(holdDial: true)
            try appendEntry(store, id: "deallocated", text: "deallocated")
            store.doc.commit()
            for _ in 0..<20 {
                await Task.yield()
                if store.outbox.count == 1 { break }
            }
            batchID = try XCTUnwrap(store.outbox.first?.batchId)
            store.stop()
        }
        XCTAssertNil(weakStore)

        DocDisk.directoryOverride = root
        gate.signal()
        _ = await exportBlocker.value
        try await Task.sleep(for: .milliseconds(100))

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
        let started = expectation(description: "blocking export started")
        let exportBlocker = Task {
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
        for _ in 0..<20 {
            await Task.yield()
            if old.outbox.count == 1 { break }
        }
        old.stop()

        DocDisk.directoryOverride = root
        let replacement = SessionStore(chatId: id, config: config())
        replacement.start(holdDial: true)
        try appendEntry(replacement, id: "new", text: "new")
        replacement.doc.commit()
        for _ in 0..<20 {
            await Task.yield()
            if replacement.outbox.count == 1 { break }
        }
        XCTAssertEqual(replacement.outbox.count, 1)
        replacement.flushToDisk()

        gate.signal()
        _ = await exportBlocker.value
        try await Task.sleep(for: .milliseconds(100))
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
        for _ in 0..<50 where !store.entries.contains(where: { $0.id == "first" }) {
            await Task.yield()
        }
        try appendEntry(store, id: "second", text: "second")
        store.doc.commit()
        store.project()
        try await Task.sleep(for: .milliseconds(1_300))
        XCTAssertTrue(store.entries.contains { entry in
            entry.id == "second"
        })
        store.stop()
    }
}
