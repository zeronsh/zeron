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

    func testApplyCancellationIsBenignButOtherFailuresRemainVisible() async throws {
        final class Source: HarnessUpdatesSource {
            let applyStarted = XCTestExpectation(description: "apply started")
            var apply: CheckedContinuation<Void, Error>?
            let failure: String
            init(_ failure: String) { self.failure = failure }
            func watchHarnessUpdates(deviceId: String) async throws -> AsyncThrowingStream<[HarnessUpdateStatus], Error> {
                AsyncThrowingStream { $0.yield([]) }
            }
            func harnessUpdateAction(_ action: HarnessUpdateAction, harness: String?, deviceId: String) async throws {
                switch action {
                case .apply:
                    try await withCheckedThrowingContinuation { continuation in
                        apply = continuation
                        applyStarted.fulfill()
                    }
                case .cancel:
                    apply?.resume(throwing: RelayError.rpc(failure))
                    apply = nil
                case .check: break
                }
            }
        }
        for failure in ["update cancelled", "permission denied"] {
            let source = Source(failure)
            let model = HarnessUpdatesModel()
            let watch = Task { await model.watch(deviceId: "host", source: source) }
            defer { watch.cancel() }
            for _ in 0..<100 where !model.connected {
                try await Task.sleep(for: .milliseconds(10))
            }
            XCTAssertTrue(model.connected)
            let apply = Task { await model.action(.apply, harness: "codex", deviceId: "host", source: source) }
            await fulfillment(of: [source.applyStarted], timeout: 2)
            await model.action(.cancel, harness: "codex", deviceId: "host", source: source)
            await apply.value
            XCTAssertTrue(model.pending.isEmpty)
            if failure == "update cancelled" { XCTAssertNil(model.error) }
            else { XCTAssertTrue(model.error?.contains(failure) == true) }
        }
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

        let secondApply = Task { @MainActor in
            try await demo.harnessUpdateAction(.apply, harness: "codex", deviceId: "dev-mac")
        }
        while let rows = try await iterator.next() {
            if rows.first(where: { $0.harness == "codex" })?.phase == "installing" { break }
        }
        // An out-of-phase/programmatic Cancel must not strand an installation.
        try await demo.harnessUpdateAction(.cancel, harness: "codex", deviceId: "dev-mac")
        try await secondApply.value
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
