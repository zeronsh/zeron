import XCTest
@testable import Zeron

@MainActor
final class HarnessUpdatesTests: XCTestCase {
    private func decode(_ json: String) throws -> HarnessUpdateStatus {
        try JSONDecoder().decode(HarnessUpdateStatus.self, from: Data(json.utf8))
    }

    func testLegacyAndUnknownStatesCannotTriggerMutation() throws {
        let legacy = try decode(#"{"harness":"codex","phase":"available"}"#)
        XCTAssertTrue(legacy.visible)
        XCTAssertFalse(legacy.actionable)
        let future = try decode(#"{"harness":"codex","phase":"future-phase","canApply":true}"#)
        XCTAssertFalse(future.actionable)
        XCTAssertFalse(future.cancellable)
    }

    func testHostControlsActionsAndCancellationBoundary() throws {
        let available = try decode(#"{"harness":"claude-code","phase":"available","canApply":true}"#)
        XCTAssertEqual(available.name, "Claude Code")
        XCTAssertTrue(available.actionable)
        for phase in ["waiting-for-idle", "preparing", "downloading"] {
            let status = try decode("{\"harness\":\"codex\",\"phase\":\"\(phase)\"}")
            XCTAssertTrue(status.active)
            XCTAssertTrue(status.cancellable)
        }
        for phase in ["installing", "verifying"] {
            let status = try decode("{\"harness\":\"codex\",\"phase\":\"\(phase)\"}")
            XCTAssertTrue(status.active)
            XCTAssertFalse(status.cancellable)
        }
    }

    func testManualChecksStayOutOfHomeNoticesButRemainActionable() throws {
        let manual = try decode(#"{"harness":"cursor","phase":"manual-action-required","canApply":true}"#)
        XCTAssertFalse(manual.visible)
        XCTAssertTrue(manual.actionable)
        let knownRelease = try decode(#"{"harness":"codex","phase":"available","canApply":false,"latestVersion":"2.0.0"}"#)
        XCTAssertTrue(knownRelease.visible)
        XCTAssertFalse(knownRelease.actionable)
    }

    func testDownloadProgressIsBoundedAndUnknownTotalsAreIndeterminate() throws {
        let bounded = try decode(#"{"harness":"codex","phase":"downloading","progress":{"completedBytes":12,"totalBytes":10}}"#)
        XCTAssertEqual(bounded.progress?.fraction, 1)
        let unknown = try decode(#"{"harness":"codex","phase":"downloading","progress":{"completedBytes":12,"totalBytes":0}}"#)
        XCTAssertNil(unknown.progress?.fraction)
    }

    func testCancelIsNotBlockedByAnOutstandingApply() {
        XCTAssertNotEqual(HarnessUpdateAction.apply.pendingKey("codex"),
                          HarnessUpdateAction.cancel.pendingKey("codex"))
        XCTAssertNotEqual(HarnessUpdateAction.apply.pendingKey("codex"),
                          HarnessUpdateAction.apply.pendingKey("claude-code"))
    }

    func testDemoFixtureCoversActionableActiveFailedAndManualStates() throws {
        let demo = DemoDataset.standard()
        let device = try XCTUnwrap(demo.devices.first { $0.id == "dev-mac" })
        XCTAssertTrue(device.supports(EngineCapability.harnessUpdatesV1))
        let rows = demo.harnessUpdates.snapshot(deviceId: device.id)
        XCTAssertEqual(Set(rows.map(\.harness)), ["codex", "claude-code", "opencode", "cursor"])
        XCTAssertEqual(rows.first { $0.harness == "codex" }?.phase, "available")
        XCTAssertEqual(rows.first { $0.harness == "claude-code" }?.phase, "waiting-for-idle")
        XCTAssertEqual(rows.first { $0.harness == "opencode" }?.phase, "failed")
        XCTAssertEqual(rows.first { $0.harness == "cursor" }?.manualCommand, "cursor-agent update")
    }

    func testDemoFixtureStreamsCancellationAndSuccessfulCompletion() async throws {
        let demo = DemoDataset.standard()
        let stream = try await demo.watchHarnessUpdates(deviceId: "dev-mac")
        var iterator = stream.makeAsyncIterator()
        let initial = try await iterator.next()
        XCTAssertEqual(initial?.first { $0.harness == "codex" }?.phase, "available")

        let firstApply = Task { @MainActor in
            try await demo.harnessUpdateAction(.apply, harness: "codex", deviceId: "dev-mac")
        }
        let waiting = try await iterator.next()
        XCTAssertEqual(waiting?.first { $0.harness == "codex" }?.phase, "waiting-for-idle")
        try await demo.harnessUpdateAction(.cancel, harness: "codex", deviceId: "dev-mac")
        let cancelled = try await iterator.next()
        XCTAssertEqual(cancelled?.first { $0.harness == "codex" }?.phase, "available")
        try await firstApply.value

        try await demo.harnessUpdateAction(.apply, harness: "codex", deviceId: "dev-mac")
        let completed = demo.harnessUpdates.snapshot(deviceId: "dev-mac")
            .first { $0.harness == "codex" }
        XCTAssertEqual(completed?.phase, "updated")
        XCTAssertEqual(completed?.installedVersion, completed?.latestVersion)
        XCTAssertNil(completed?.progress)
    }

    func testDemoFailedUpdateRecoversAfterCheck() async throws {
        let demo = DemoDataset.standard()
        try await demo.harnessUpdateAction(.check, harness: "opencode", deviceId: "dev-mac")
        let row = demo.harnessUpdates.snapshot(deviceId: "dev-mac")
            .first { $0.harness == "opencode" }
        XCTAssertEqual(row?.phase, "available")
        XCTAssertTrue(row?.actionable == true)
        XCTAssertNil(row?.error)
    }
}
