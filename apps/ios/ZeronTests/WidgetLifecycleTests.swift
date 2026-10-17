import XCTest
@testable import Zeron

final class WidgetPublisherTests: XCTestCase {
    func testDisconnectedRefreshesKeepTheLastConfirmedTimestamp() throws {
        var snapshots: [WidgetSnapshot] = []
        let publisher = WidgetPublisher { snapshots.append($0) }
        let workspace = widgetWorkspace()
        let confirmed = Date(timeIntervalSince1970: 1_800_000_000)

        publisher.publish(workspace, usage: [:], connected: true, now: confirmed)
        publisher.publish(workspace, usage: [:], connected: false, now: confirmed.addingTimeInterval(660))
        publisher.publish(workspace, usage: [:], connected: false, now: confirmed.addingTimeInterval(3600))

        XCTAssertEqual(snapshots.count, 1)
        let cached = try XCTUnwrap(snapshots.last)
        XCTAssertEqual(cached.publishedAt, confirmed)
        XCTAssertLessThan(cached.staleAt, confirmed.addingTimeInterval(3600))

        publisher.publish(workspace, usage: [:], connected: true, now: confirmed.addingTimeInterval(3601))

        XCTAssertEqual(snapshots.count, 2)
        XCTAssertEqual(snapshots.last?.publishedAt, confirmed.addingTimeInterval(3601))
    }

    func testColdReplicaCannotReplaceTheSharedSnapshot() {
        var snapshots: [WidgetSnapshot] = []
        let publisher = WidgetPublisher { snapshots.append($0) }
        var workspace = widgetWorkspace()
        workspace.synced = false

        publisher.publish(workspace, usage: [:], connected: true)

        XCTAssertTrue(snapshots.isEmpty)
    }

    func testConnectedUnchangedSnapshotsRefreshWithoutReloadingEveryTick() {
        var snapshots: [WidgetSnapshot] = []
        let publisher = WidgetPublisher { snapshots.append($0) }
        let workspace = widgetWorkspace()
        let confirmed = Date(timeIntervalSince1970: 1_800_000_000)

        publisher.publish(workspace, usage: [:], connected: true, now: confirmed)
        publisher.publish(workspace, usage: [:], connected: true, now: confirmed.addingTimeInterval(30))
        publisher.publish(workspace, usage: [:], connected: true, now: confirmed.addingTimeInterval(600))

        XCTAssertEqual(snapshots.map(\.publishedAt), [confirmed, confirmed.addingTimeInterval(600)])
    }
}

final class WidgetUsageLifecycleTests: XCTestCase {
    @MainActor
    func testRemovingTheOnlyOnlineHostPrunesAndPersistsBeforeTheNextPoll() async throws {
        let suite = "WidgetUsageLifecycleTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let poller = UsagePoller(defaults: defaults)
        defer {
            poller.stop()
            defaults.removePersistentDomain(forName: suite)
        }
        let client = WidgetPollClient(noHandle: CoreClient.NoHandle())
        client.fetch = { _ in [widgetAccount("current")] }

        poller.start(client)
        await poller.pollIfDue()?.value
        XCTAssertEqual(poller.accounts["desktop"]?.first?.id, "current")

        client.snapshot.devices = []
        poller.pollIfDue()

        XCTAssertTrue(poller.accounts.isEmpty)

        let reopened = UsagePoller(defaults: defaults)
        defer { reopened.stop() }
        reopened.start(client)

        XCTAssertTrue(reopened.accounts.isEmpty)
    }

    @MainActor
    func testOfflineAndUnsyncedRegistriesKeepCachedUsageUntilRemovalIsConfirmed() async throws {
        let suite = "WidgetUsageLifecycleTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let poller = UsagePoller(defaults: defaults)
        defer {
            poller.stop()
            defaults.removePersistentDomain(forName: suite)
        }
        let client = WidgetPollClient(noHandle: CoreClient.NoHandle())
        client.fetch = { _ in [widgetAccount("current")] }
        poller.start(client)
        await poller.pollIfDue()?.value

        client.snapshot.devices[0].online = false
        poller.pollIfDue()
        XCTAssertNotNil(poller.accounts["desktop"])

        client.connectionState = .offline
        client.snapshot.devices = []
        poller.start(client)
        XCTAssertNotNil(poller.accounts["desktop"])

        client.connectionState = .connected
        client.snapshot.synced = false
        poller.pollIfDue()
        XCTAssertNotNil(poller.accounts["desktop"])

        client.snapshot.synced = true
        poller.pollIfDue()
        XCTAssertTrue(poller.accounts.isEmpty)
    }

    @MainActor
    func testLateReplyCannotRestoreAHostRemovedWhilePolling() async throws {
        let suite = "WidgetUsageLifecycleTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let poller = UsagePoller(defaults: defaults)
        defer {
            poller.stop()
            defaults.removePersistentDomain(forName: suite)
        }
        let requested = expectation(description: "Usage requested")
        let gate = WidgetReplyGate()
        let client = WidgetPollClient(noHandle: CoreClient.NoHandle())
        client.fetch = { _ in await gate.wait { requested.fulfill() } }
        poller.start(client)
        let pending = try XCTUnwrap(poller.pollIfDue())
        await fulfillment(of: [requested], timeout: 2)

        client.snapshot.devices = []
        poller.pollIfDue()
        client.connectionState = .offline
        await gate.finish([widgetAccount("removed")])
        await pending.value

        XCTAssertTrue(poller.accounts.isEmpty)
    }

    @MainActor
    func testAccountSwitchDiscardsAnEarlierPollReply() async throws {
        let suite = "WidgetUsageLifecycleTests.\(UUID().uuidString)"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        let poller = UsagePoller(defaults: defaults)
        defer {
            poller.stop()
            defaults.removePersistentDomain(forName: suite)
        }
        let requested = expectation(description: "Earlier account requested usage")
        let gate = WidgetReplyGate()
        let earlier = WidgetPollClient(noHandle: CoreClient.NoHandle())
        earlier.fetch = { _ in await gate.wait { requested.fulfill() } }
        poller.start(earlier)
        let pending = try XCTUnwrap(poller.pollIfDue())
        await fulfillment(of: [requested], timeout: 2)

        let current = WidgetPollClient(noHandle: CoreClient.NoHandle())
        current.identity = "other-user"
        current.fetch = { _ in [widgetAccount("current")] }
        poller.start(current)
        await poller.pollIfDue()?.value
        await gate.finish([widgetAccount("earlier")])
        await pending.value

        XCTAssertEqual(poller.accounts["desktop"]?.map(\.id), ["current"])
    }
}

private final class WidgetPollClient: CoreClient, @unchecked Sendable {
    var snapshot = widgetWorkspace(devices: [widgetHost()])
    var connectionState: ConnectivityState = .connected
    var identity = "widget-user"
    var fetch: @Sendable (String) async throws -> [AgentAccount] = { _ in [] }

    override func userId() -> String { identity }
    override func orgId() -> String { "widget-org" }
    override func workspace() -> WorkspaceSnapshot { snapshot }
    override func executionDevices() -> [DeviceView] { snapshot.devices.filter(\.isExecutionHost) }
    override func connectivity() -> Connectivity {
        Connectivity(state: connectionState, retryAtMs: nil, lastFailure: nil, degradedChats: [])
    }
    override func listAgentAccounts(deviceId: String, forceUsage: Bool) async throws -> [AgentAccount] {
        try await fetch(deviceId)
    }
}

private actor WidgetReplyGate {
    private var pending: CheckedContinuation<[AgentAccount], Never>?

    func wait(onRequest: @Sendable () -> Void) async -> [AgentAccount] {
        await withCheckedContinuation { continuation in
            pending = continuation
            onRequest()
        }
    }

    func finish(_ accounts: [AgentAccount]) {
        pending?.resume(returning: accounts)
        pending = nil
    }
}

private func widgetWorkspace(devices: [DeviceView] = []) -> WorkspaceSnapshot {
    WorkspaceSnapshot(
        revision: 1, computedAtMs: 0, pinsReady: true, synced: true,
        front: FrontPage(pinned: [], sections: [], recent: []),
        projects: [], projectless: [], pullRequests: PullRequestGroups(open: [], merged: [], closed: []),
        archived: [], devices: devices
    )
}

private func widgetHost() -> DeviceView {
    DeviceView(
        id: "desktop", name: "MacBook Pro", platform: "macos", online: true,
        lastSeenMs: nil, version: nil, capabilities: [], isExecutionHost: true, isSelf: false, sessionCount: 0
    )
}

private func widgetAccount(_ id: String) -> AgentAccount {
    AgentAccount(
        id: id, harness: "codex", email: nil, displayName: nil, planLabel: nil, active: true,
        usageWindows: [AgentUsageWindow(label: "Week", usedFraction: 0.4, resetsAtMs: nil)],
        usageFetchedAtMs: nil, usageError: nil, provider: nil
    )
}
