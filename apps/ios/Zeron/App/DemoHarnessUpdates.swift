import Foundation

/// Offline host fixture for the real harness-update UI. It speaks the same
/// snapshot stream + mutation contract as WorkspaceStore, so demo mode covers
/// reconnect-safe rendering and action lifecycles without a relay or login.
@MainActor
final class DemoHarnessUpdates {
    private typealias Continuation = AsyncThrowingStream<[HarnessUpdateStatus], Error>.Continuation

    private struct ApplyRun {
        var id: UUID
        var task: Task<Void, Never>
    }

    private var rowsByDevice: [String: [HarnessUpdateStatus]]
    private var watchers: [String: [UUID: Continuation]] = [:]
    private var applyRuns: [String: ApplyRun] = [:]

    init(devices: [DeviceRow]) {
        rowsByDevice = Dictionary(uniqueKeysWithValues: devices.compactMap { device in
            guard device.supports(EngineCapability.harnessUpdatesV1) else { return nil }
            let rows = device.id == "dev-mac" ? Self.primaryRows : Self.secondaryRows
            return (device.id, rows)
        })
    }

    func snapshot(deviceId: String) -> [HarnessUpdateStatus] {
        rowsByDevice[deviceId] ?? []
    }

    func watch(deviceId: String) throws
        -> AsyncThrowingStream<[HarnessUpdateStatus], Error> {
        guard let rows = rowsByDevice[deviceId] else {
            throw RelayError.rpc("This device does not support agent updates")
        }
        let id = UUID()
        let pair = AsyncThrowingStream<[HarnessUpdateStatus], Error>.makeStream()
        watchers[deviceId, default: [:]][id] = pair.continuation
        pair.continuation.yield(rows)
        pair.continuation.onTermination = { [weak self] _ in
            Task { @MainActor in self?.watchers[deviceId]?.removeValue(forKey: id) }
        }
        return pair.stream
    }

    func action(_ action: HarnessUpdateAction, harness: String?, deviceId: String) async throws {
        guard rowsByDevice[deviceId] != nil else {
            throw RelayError.rpc("This device does not support agent updates")
        }
        switch action {
        case .check:
            await check(harness: harness, deviceId: deviceId)
        case .apply:
            guard let harness else { throw RelayError.rpc("Choose an agent to update") }
            try await apply(harness: harness, deviceId: deviceId)
        case .cancel:
            guard let harness else { throw RelayError.rpc("Choose an update to cancel") }
            cancel(harness: harness, deviceId: deviceId)
        }
    }

    private func check(harness: String?, deviceId: String) async {
        mutate(deviceId: deviceId) { rows in
            for index in rows.indices where harness == nil || rows[index].harness == harness {
                guard !rows[index].active else { continue }
                rows[index].phase = "checking"
                rows[index].error = nil
                rows[index].progress = nil
            }
        }
        guard await pause(milliseconds: 650) else { return }
        mutate(deviceId: deviceId) { rows in
            for index in rows.indices where harness == nil || rows[index].harness == harness {
                guard rows[index].phase == "checking" else { continue }
                if rows[index].harness == "cursor" {
                    rows[index].phase = "available"
                    rows[index].canApply = false
                    rows[index].manualCommand = "cursor-agent update"
                } else if rows[index].installedVersion == rows[index].latestVersion {
                    rows[index].phase = "current"
                    rows[index].canApply = false
                } else {
                    rows[index].phase = "available"
                    rows[index].canApply = true
                }
            }
        }
    }

    private func apply(harness: String, deviceId: String) async throws {
        guard let row = rowsByDevice[deviceId]?.first(where: { $0.harness == harness }),
              row.actionable else {
            throw RelayError.rpc("This update cannot be installed automatically")
        }
        let key = "\(deviceId)/\(harness)"
        guard applyRuns[key] == nil else { throw RelayError.rpc("Update already in progress") }
        let id = UUID()
        let task = Task { @MainActor [weak self] in
            guard let self else { return }
            await self.runApply(harness: harness, deviceId: deviceId)
        }
        applyRuns[key] = ApplyRun(id: id, task: task)
        await task.value
        if applyRuns[key]?.id == id { applyRuns.removeValue(forKey: key) }
    }

    private func runApply(harness: String, deviceId: String) async {
        setPhase("waiting-for-idle", harness: harness, deviceId: deviceId)
        guard await pause(milliseconds: 800) else { return }
        setPhase("preparing", harness: harness, deviceId: deviceId)
        guard await pause(milliseconds: 650) else { return }

        let total: UInt64 = 64 * 1_024 * 1_024
        for percent in [8, 27, 53, 79, 100] {
            let completed = total * UInt64(percent) / 100
            mutate(harness: harness, deviceId: deviceId) { row in
                row.phase = "downloading"
                row.progress = .init(
                    completedBytes: completed,
                    totalBytes: total,
                    message: "Downloading \(completed / 1_024 / 1_024) MB of 64 MB"
                )
            }
            guard await pause(milliseconds: 420) else { return }
        }

        setPhase("installing", harness: harness, deviceId: deviceId)
        guard await pause(milliseconds: 850) else { return }
        setPhase("verifying", harness: harness, deviceId: deviceId)
        guard await pause(milliseconds: 650) else { return }
        mutate(harness: harness, deviceId: deviceId) { row in
            row.phase = "updated"
            row.installedVersion = row.latestVersion ?? row.installedVersion
            row.canApply = false
            row.progress = nil
            row.error = nil
        }
    }

    private func cancel(harness: String, deviceId: String) {
        guard snapshot(deviceId: deviceId).first(where: { $0.harness == harness })?.cancellable == true else { return }
        let key = "\(deviceId)/\(harness)"
        applyRuns.removeValue(forKey: key)?.task.cancel()
        mutate(harness: harness, deviceId: deviceId) { row in
            guard row.cancellable else { return }
            row.phase = "available"
            row.canApply = true
            row.progress = nil
            row.error = nil
        }
    }

    private func setPhase(_ phase: String, harness: String, deviceId: String) {
        mutate(harness: harness, deviceId: deviceId) { row in
            row.phase = phase
            row.progress = nil
            row.error = nil
            row.canApply = false
        }
    }

    private func mutate(harness: String, deviceId: String,
                        _ update: (inout HarnessUpdateStatus) -> Void) {
        mutate(deviceId: deviceId) { rows in
            guard let index = rows.firstIndex(where: { $0.harness == harness }) else { return }
            update(&rows[index])
        }
    }

    private func mutate(deviceId: String,
                        _ update: (inout [HarnessUpdateStatus]) -> Void) {
        guard var rows = rowsByDevice[deviceId] else { return }
        update(&rows)
        rowsByDevice[deviceId] = rows
        for continuation in watchers[deviceId]?.values ?? [:].values {
            continuation.yield(rows)
        }
    }

    private func pause(milliseconds: UInt64) async -> Bool {
        do {
            try await Task.sleep(nanoseconds: milliseconds * 1_000_000)
            return !Task.isCancelled
        } catch {
            return false
        }
    }

    private static let primaryRows: [HarnessUpdateStatus] = [
        .init(harness: "codex", installedVersion: "0.57.0", latestVersion: "0.58.0",
              phase: "available", canApply: true),
        .init(harness: "claude-code", installedVersion: "2.1.71", latestVersion: "2.1.76",
              phase: "waiting-for-idle", canApply: false),
        .init(harness: "opencode", installedVersion: "1.0.184", latestVersion: "1.0.187",
              phase: "failed", canApply: false,
              error: .init(message: "Checksum verification failed", retryable: true)),
        .init(harness: "cursor", installedVersion: "0.48.2", latestVersion: "0.49.0",
              phase: "available", canApply: false, manualCommand: "cursor-agent update"),
    ]

    private static let secondaryRows: [HarnessUpdateStatus] = [
        .init(harness: "codex", installedVersion: "0.58.0", latestVersion: "0.58.0",
              phase: "current", canApply: false),
        .init(harness: "claude-code", installedVersion: "2.1.76", latestVersion: "2.1.76",
              phase: "current", canApply: false),
        .init(harness: "opencode", installedVersion: "1.0.184", latestVersion: "1.0.187",
              phase: "available", canApply: true),
    ]
}
