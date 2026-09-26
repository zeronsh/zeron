// RegistryDoc behavior: the persistence blob round-trip and the monotonic
// HLC clock (task-specific tests beyond the shared conformance vectors).

import XCTest
@testable import Zeron

final class RegistryDocPersistenceTests: XCTestCase {
    func testPersistenceBlobRoundTrips() throws {
        let doc = RegistryDoc(deviceId: "ios-test")
        // Authoritative state from the server…
        doc.applyState(seq: 42, full: true, gcFloor: 7, rows: [
            RegistryRow(kind: "chats", id: "chat-1", seq: 40, deleted: false, delHlc: nil,
                        fields: ["title": .string("hello"), "archived": .bool(false),
                                 "createdAt": .int(1_754_000_000_000)],
                        clocks: ["title": encodeHlc(ms: 1000, counter: 0, device: "dev-a")]),
            RegistryRow(kind: "spaces", id: "sp-1", seq: 41, deleted: true,
                        delHlc: encodeHlc(ms: 2000, counter: 0, device: "dev-b"),
                        fields: [:], clocks: [:]),
        ])
        // …plus a local pending write (null field delete included — the blob
        // must carry null through, not drop the key).
        doc.write(kind: "chats", id: "chat-1", op: .update,
                  set: ["title": .string("renamed"), "branch": .null])

        let data = try doc.toData()
        let loaded = try RegistryDoc.from(data: data, deviceId: "ios-test")

        XCTAssertEqual(loaded.cursor, 42)
        XCTAssertEqual(loaded.gcFloor, 7)
        XCTAssertEqual(loaded.authoritative, doc.authoritative)
        XCTAssertEqual(loaded.pending, doc.pending.map {
            var batch = $0
            batch.inFlight = false  // in-flight is connection state, never persisted
            return batch
        })
        XCTAssertEqual(loaded.pending.first?.ops.first?.set?["branch"], .null)
        // The clock survives: the next HLC after reload still beats every
        // persisted one, even if the wall clock regressed to zero.
        XCTAssertEqual(loaded.clock, doc.clock)
        // The overlay reads identically after reload.
        XCTAssertEqual(loaded.overlayRow(kind: "chats", id: "chat-1")?.fields["title"],
                       .string("renamed"))
        XCTAssertNil(loaded.overlayRow(kind: "spaces", id: "sp-1"))
        // A replica with state never sends a null hello cursor.
        XCTAssertEqual(loaded.helloCursor, 42)
    }

    func testFreshDocSendsNullHelloCursor() {
        XCTAssertNil(RegistryDoc(deviceId: "ios-test").helloCursor)
    }
}

final class HlcMonotonicClockTests: XCTestCase {
    func testNeverEmitsAtOrBelowTheLastClockAcrossWallClockRegression() {
        var clock = HlcClock()
        let first = clock.next(nowMs: 5000, device: "ios-test")
        // Wall-clock regression: the clock holds its ms and bumps the counter.
        let second = clock.next(nowMs: 1000, device: "ios-test")
        XCTAssertTrue(second > first)
        XCTAssertTrue(second.hasPrefix("0000000005000-000001"))
        // Same instant repeatedly: still strictly increasing.
        var last = second
        for _ in 0..<100 {
            let next = clock.next(nowMs: 5000, device: "ios-test")
            XCTAssertTrue(next > last)
            last = next
        }
        // Progressing wall clock resets the counter.
        let advanced = clock.next(nowMs: 6000, device: "ios-test")
        XCTAssertTrue(advanced > last)
        XCTAssertTrue(advanced.hasPrefix("0000000006000-000000"))
    }

    func testCounterOverflowCarriesIntoMs() {
        var clock = HlcClock()
        _ = clock.next(nowMs: 1000, device: "d")
        for _ in 0..<999_999 {
            _ = clock.next(nowMs: 1000, device: "d")
        }
        // Counter exhausted at ms=1000 — the next tick carries into ms=1001.
        let carried = clock.next(nowMs: 1000, device: "d")
        XCTAssertTrue(carried.hasPrefix("0000000001001-000000"))
    }

    func testPersistedClockSurvivesRestartMonotonically() throws {
        var clock = HlcClock()
        let before = clock.next(nowMs: 9000, device: "ios-test")
        let data = try JSONEncoder().encode(clock)
        var restored = try JSONDecoder().decode(HlcClock.self, from: data)
        // Restart onto a regressed wall clock: still strictly newer.
        let after = restored.next(nowMs: 100, device: "ios-test")
        XCTAssertTrue(after > before)
    }
}

@MainActor
final class UnreadSessionTests: XCTestCase {
    private var config: AppConfig {
        AppConfig(edgeURL: URL(string: "http://localhost:1")!, mode: .dev,
                  userId: "unread-tests", orgId: "tests", deviceId: "ios-test",
                  deviceName: "Test phone", devBearer: "unread-tests@tests")
    }

    func testDesktopUnreadProjectsAndIosSeenWinsWithClockSkew() throws {
        let futureClock = encodeHlc(ms: 9_000_000_000_000, counter: 42, device: "desktop")
        let messageAt: Int64 = 9_000_000_000_000
        let doc = RegistryDoc(deviceId: config.deviceId)
        doc.applyState(seq: 1, full: true, gcFloor: 0, rows: [
            RegistryRow(kind: "chats", id: "chat-1", seq: 1, deleted: false, delHlc: nil,
                        fields: ["id": .string("chat-1"), "deviceId": .string("desktop"),
                                 "createdAt": .int(1), "lastMessageAt": .int(messageAt)],
                        // Desktop's `lastSeenAt: null` removed the value but retained its clock.
                        clocks: ["id": futureClock, "deviceId": futureClock,
                                 "createdAt": futureClock, "lastMessageAt": futureClock,
                                 "lastSeenAt": futureClock])
        ])
        let store = WorkspaceStore(config: config, doc: doc)
        XCTAssertTrue(try XCTUnwrap(store.chats.first).unseen)

        store.markSeen(chatId: "chat-1")

        let op = try XCTUnwrap(doc.pending.last?.ops.first)
        XCTAssertEqual(op.set?["lastSeenAt"], .int(messageAt))
        XCTAssertTrue(op.hlc > futureClock)
        XCTAssertFalse(try XCTUnwrap(store.chats.first).unseen)
    }
}
