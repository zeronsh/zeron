import Foundation
import Observation

@MainActor
protocol HarnessUpdatesSource: AnyObject {
    func watchHarnessUpdates(deviceId: String) async throws
        -> AsyncThrowingStream<[HarnessUpdateStatus], Error>
    func harnessUpdateAction(_ action: HarnessUpdateAction, harness: String?,
                             deviceId: String) async throws
}

/// Device-owned CLI state. Unknown phases remain readable but never enable an action.
struct HarnessUpdateStatus: Decodable, Identifiable, Equatable {
    var harness: String
    var installedVersion: String?
    var latestVersion: String?
    var phase: String
    var canApply: Bool?
    var manualCommand: String?
    var progress: Progress?
    var error: Failure?
    var id: String { harness }

    struct Progress: Decodable, Equatable {
        var completedBytes: UInt64?
        var totalBytes: UInt64?
        var message: String?
        var fraction: Double? {
            guard let completedBytes, let totalBytes, totalBytes > 0 else { return nil }
            return min(1, Double(completedBytes) / Double(totalBytes))
        }
    }
    struct Failure: Decodable, Equatable {
        var message: String
        var retryable: Bool?
    }
    var name: String {
        switch harness {
        case "claude-code": "Claude Code"
        case "codex": "Codex"
        case "opencode": "OpenCode"
        default: harness.capitalized
        }
    }
    var active: Bool {
        ["waiting-for-idle", "preparing", "downloading", "installing", "verifying"].contains(phase)
    }
    var cancellable: Bool { ["waiting-for-idle", "preparing", "downloading"].contains(phase) }
    var actionable: Bool {
        ["available", "manual-action-required"].contains(phase) && canApply == true
    }
    var visible: Bool {
        active || ["available", "failed", "updated"].contains(phase)
    }
    var label: String {
        switch phase {
        case "available": latestVersion.map { "Version \($0) available" } ?? "Update available"
        case "waiting-for-idle": "Waiting for the current run to finish"
        case "preparing": "Preparing update"
        case "downloading": progress?.message ?? "Downloading"
        case "installing": "Installing"
        case "verifying": "Verifying installation"
        case "updated": "Updated"
        case "current": "Up to date"
        case "checking": "Checking for updates"
        case "dormant": "Update monitoring off"
        case "failed": error?.message ?? "Update failed"
        case "manual-action-required": manualCommand ?? "Manual update required"
        default: "Update status unavailable"
        }
    }
}

@MainActor
@Observable
final class HarnessUpdatesModel {
    private(set) var statuses: [HarnessUpdateStatus] = []
    private(set) var connected = false
    private(set) var error: String?
    private(set) var pending: Set<String> = []
    private var generation = UUID()

    /// View-owned watch. Every reconnect gets a full host snapshot. A target
    /// switch invalidates late frames and action completions from the old host.
    func watch(deviceId: String, source: any HarnessUpdatesSource) async {
        let token = UUID()
        generation = token
        statuses = []
        pending = []
        connected = false
        error = nil
        defer { if generation == token { connected = false } }
        var delay: UInt64 = 1
        while !Task.isCancelled, generation == token {
            do {
                let stream = try await source.watchHarnessUpdates(deviceId: deviceId)
                for try await rows in stream {
                    guard !Task.isCancelled, generation == token else { return }
                    statuses = rows
                    connected = true
                    error = nil
                    delay = 1
                }
            } catch {
                guard !Task.isCancelled, generation == token else { return }
                self.error = error.localizedDescription
            }
            connected = false
            do { try await Task.sleep(nanoseconds: delay * 1_000_000_000) }
            catch { return }
            delay = min(delay * 2, 15)
        }
    }

    func action(_ action: HarnessUpdateAction, harness: String? = nil,
                deviceId: String, source: any HarnessUpdatesSource) async {
        let key = action.pendingKey(harness)
        guard connected, !pending.contains(key) else { return }
        let token = generation
        pending.insert(key)
        error = nil
        defer { if generation == token { pending.remove(key) } }
        do {
            try await source.harnessUpdateAction(action, harness: harness, deviceId: deviceId)
        } catch {
            guard generation == token else { return }
            // Apply remains in flight while Cancel is a separate RPC. The
            // host reports that successful cancellation through Apply's error.
            if action == .apply, case RelayError.rpc("update cancelled") = error { return }
            self.error = "\(error.localizedDescription). Reconnect to check the device’s status before retrying."
        }
    }
}

enum HarnessUpdateAction {
    case check, apply, cancel
    func pendingKey(_ harness: String?) -> String {
        // Cancellation must remain available while the apply RPC is pending.
        "\(method):\(harness ?? "*")"
    }
    var method: String {
        switch self {
        case .check: "CheckHarnessUpdates"
        case .apply: "ApplyHarnessUpdate"
        case .cancel: "CancelHarnessUpdate"
        }
    }
    var timeout: UInt64 {
        switch self {
        case .check: 240
        case .apply: 1200
        case .cancel: 10
        }
    }
}
