import Foundation

/// plan usage of the agent cli logins on every online host, for the usage
/// widget. the engine throttles forced probes per account and backs off on
/// provider errors, so asking every few minutes stays cheap.
final class UsagePoller {
    private static let interval: TimeInterval = 5 * 60
    private static let storeKey = "usagePoller.accounts"
    private let defaults: UserDefaults

    /// last good reply per host; an offline host keeps its stale windows,
    /// which carry their fetch time. persisted so a relaunch while a host is
    /// offline doesn't wipe what the widget showed for it.
    private(set) var accounts: [String: [AgentAccount]] = [:]
    private weak var client: CoreClient?
    private var owner = ""
    private var lastPoll = Date.distantPast
    private var poll: Task<Void, Never>?
    /// bumped on every start/stop: a reply from an earlier session (another
    /// account, or after sign-out) must never land.
    private var generation = 0
    private var timer: Timer?
    private var knownHosts: Set<String>?

    var onChange: (() -> Void)?

    init(defaults: UserDefaults = .standard) {
        self.defaults = defaults
    }

    func start(_ client: CoreClient) {
        halt()
        self.client = client
        owner = "\(client.userId())/\(client.orgId())"
        accounts = restore(owner: owner)
        timer = Timer.scheduledTimer(withTimeInterval: Self.interval, repeats: true) { [weak self] _ in
            self?.pollIfDue()
        }
        pollIfDue()
    }

    /// signing out: forget the account's usage, in memory and on disk.
    func stop() {
        halt()
        client = nil
        owner = ""
        accounts = [:]
        defaults.removeObject(forKey: Self.storeKey)
    }

    private func halt() {
        generation += 1
        poll?.cancel()
        poll = nil
        timer?.invalidate()
        timer = nil
        lastPoll = .distantPast
        knownHosts = nil
    }

    /// slack so the repeating timer never skips a beat it fired just early for.
    @discardableResult
    func pollIfDue() -> Task<Void, Never>? {
        guard let client else { return nil }
        let ready = reconcileHosts(from: client)
        guard ready, poll == nil, Date().timeIntervalSince(lastPoll) >= Self.interval - 5 else { return poll }
        let hosts = client.executionDevices().filter(\.online).map(\.id)
        guard !hosts.isEmpty else { return nil }
        lastPoll = Date()
        let started = generation
        poll = Task { @MainActor [weak self] in
            let replies = await withTaskGroup(of: (String, [AgentAccount]?).self) { group in
                for host in hosts {
                    group.addTask { (host, try? await client.listAgentAccounts(deviceId: host, forceUsage: true)) }
                }
                return await group.reduce(into: [String: [AgentAccount]]()) { $0[$1.0] = $1.1 }
            }
            guard let self, started == generation, !Task.isCancelled else { return }
            poll = nil
            reconcileHosts(from: client)
            merge(replies)
        }
        return poll
    }

    @discardableResult
    private func reconcileHosts(from client: CoreClient) -> Bool {
        let workspace = client.workspace()
        guard workspace.synced, client.connectivity().state == .connected else { return false }
        knownHosts = Set(workspace.devices.map(\.id))
        merge([:])
        return true
    }

    private func merge(_ replies: [String: [AgentAccount]]) {
        let merged = accounts.merging(replies) { _, fresh in fresh }
            .filter { knownHosts?.contains($0.key) ?? true }
        guard merged != accounts else { return }
        accounts = merged
        save()
        onChange?()
    }

    private func save() {
        let stored = Stored(owner: owner, accounts: accounts.mapValues { $0.map(StoredAccount.init) })
        defaults.set(try? JSONEncoder().encode(stored), forKey: Self.storeKey)
    }

    private func restore(owner: String) -> [String: [AgentAccount]] {
        guard let data = defaults.data(forKey: Self.storeKey),
              let stored = try? JSONDecoder().decode(Stored.self, from: data),
              stored.owner == owner
        else { return [:] }
        return stored.accounts.mapValues { $0.map(\.account) }
    }
}

/// `AgentAccount` is a uniffi record, which can't be made `Codable` outside
/// its generated file.
private struct Stored: Codable {
    var owner: String
    var accounts: [String: [StoredAccount]]
}

private struct StoredAccount: Codable {
    var id: String
    var harness: String
    var email: String?
    var displayName: String?
    var planLabel: String?
    var active: Bool
    var windows: [StoredWindow]
    var fetchedAtMs: Int64?
    var error: String?
    var provider: String?

    struct StoredWindow: Codable {
        var label: String
        var usedFraction: Float
        var resetsAtMs: Int64?
    }

    init(_ account: AgentAccount) {
        id = account.id
        harness = account.harness
        email = account.email
        displayName = account.displayName
        planLabel = account.planLabel
        active = account.active
        windows = account.usageWindows.map { StoredWindow(label: $0.label, usedFraction: $0.usedFraction, resetsAtMs: $0.resetsAtMs) }
        fetchedAtMs = account.usageFetchedAtMs
        error = account.usageError
        provider = account.provider
    }

    var account: AgentAccount {
        AgentAccount(
            id: id,
            harness: harness,
            email: email,
            displayName: displayName,
            planLabel: planLabel,
            active: active,
            usageWindows: windows.map { AgentUsageWindow(label: $0.label, usedFraction: $0.usedFraction, resetsAtMs: $0.resetsAtMs) },
            usageFetchedAtMs: fetchedAtMs,
            usageError: error,
            provider: provider
        )
    }
}
