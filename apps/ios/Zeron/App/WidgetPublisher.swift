import Foundation

extension WidgetSnapshot {
    private static let sessionLimit = 6
    private static let usageLimit = 16
    private static let voiceCapability = voiceHostCapability()

    /// the front page (pinned, sections, recent) the sessions tab counts,
    /// most urgent first.
    init(workspace ws: WorkspaceSnapshot, usage: [String: [AgentAccount]], publishedAt: Date = Date()) {
        var seen = Set<String>()
        let front = (ws.front.pinned + ws.front.sections.flatMap(\.sessions) + ws.front.recent)
            .filter { seen.insert($0.id).inserted }
        let live = front.compactMap(Self.session)
        let ranked = live.sorted { $0.status != $1.status ? $0.status < $1.status : $0.since > $1.since }
        let hosts = ws.devices.filter { $0.isExecutionHost && !$0.isSelf }
        let hostNames = Dictionary(ws.devices.map { ($0.id, $0.name) }, uniquingKeysWith: { a, _ in a })
        self.init(
            sessions: Array(ranked.prefix(Self.sessionLimit)),
            devices: hosts.map(Self.device).sorted { $0.online != $1.online ? $0.online : $0.name < $1.name },
            usage: Self.usage(usage, hostNames: hostNames),
            voiceAvailable: hosts.contains { $0.capabilities.contains(Self.voiceCapability) },
            awaiting: live.count(where: { $0.status == .awaiting }),
            failed: live.count(where: { $0.status == .failed }),
            working: live.count(where: { $0.status == .working }),
            done: live.count(where: { $0.status == .done }),
            publishedAt: publishedAt
        )
    }

    private static func session(_ row: SessionRow) -> Session? {
        let status: Status
        switch row.indicator {
        case .awaitingInput: status = .awaiting
        case .errored: status = .failed
        case .working: status = .working
        case .completed: status = .done
        case .idle: return nil
        }
        let sinceMs = status == .working ? row.workingSinceMs ?? row.lastActivityMs : row.lastActivityMs
        return Session(
            id: row.id,
            title: row.title,
            place: row.project?.name ?? row.deviceName ?? "No project",
            agent: row.modelLabel ?? row.harnessLabel,
            status: status,
            since: date(ms: sinceMs)
        )
    }

    private static func device(_ device: DeviceView) -> Device {
        Device(
            id: device.id,
            name: device.name,
            platform: device.platform,
            online: device.online,
            sessions: Int(device.sessionCount),
            // an online host's heartbeat would change the snapshot (and reload
            // every widget) each time it ticks.
            lastSeen: device.online ? nil : device.lastSeenMs.map { date(ms: $0) }
        )
    }

    /// accounts without windows (api keys, cursor's byok, never probed) have
    /// nothing to draw unless their probe failed, which is worth showing.
    private static func usage(_ byHost: [String: [AgentAccount]], hostNames: [String: String]) -> [UsageAccount] {
        let accounts = byHost.flatMap { host, accounts in
            accounts
                .filter { Self.supportsUsageSource($0.harness) }
                .filter { !$0.usageWindows.isEmpty || $0.active && $0.usageError != nil }
                .map { card($0, host: host, hostName: hostNames[host]) }
        }
        let ranked = accounts.sorted { a, b in
            if a.active != b.active { return a.active }
            if a.peak != b.peak { return a.peak > b.peak }
            return a.id < b.id
        }
        return Array(ranked.prefix(usageLimit))
    }

    private static func card(_ account: AgentAccount, host: String, hostName: String?) -> UsageAccount {
        UsageAccount(
            id: "\(host)/\(account.id)",
            source: account.provider.map { "\(account.harness)/\($0)" } ?? account.harness,
            agent: agentName(account.harness, provider: account.provider),
            account: account.displayName ?? account.email,
            plan: account.planLabel,
            host: hostName ?? "Unknown device",
            active: account.active,
            windows: account.usageWindows.map {
                UsageWindow(label: $0.label, used: Double($0.usedFraction), resetsAt: $0.resetsAtMs.map { date(ms: $0) })
            },
            fetchedAt: account.usageFetchedAtMs.map { date(ms: $0) },
            error: account.usageError
        )
    }

    private static func agentName(_ harness: String, provider: String?) -> String {
        guard let provider else { return agentName(harness) }
        if provider == "opencode-go" { return "OpenCode Go" }
        return "\(agentName(harness)) · \(providerName(provider))"
    }

    private static func providerName(_ provider: String) -> String {
        switch provider {
        case "openai", "openai-codex": "OpenAI"
        case "anthropic": "Claude"
        case "github-copilot", "copilot": "Copilot"
        case "nous": "Nous"
        case "xai", "xai-oauth": "xAI"
        default: provider.capitalized
        }
    }

    private static func agentName(_ harness: String) -> String {
        switch harness {
        case "claude-code": "Claude Code"
        case "codex": "Codex"
        case "cursor": "Cursor"
        case "devin": "Devin"
        case "grok": "Grok"
        case "hermes": "Hermes"
        case "pi": "Pi"
        case "opencode": "OpenCode"
        case "antigravity": "Antigravity"
        default: harness.capitalized
        }
    }

    private static func date(ms: Int64) -> Date {
        Date(timeIntervalSince1970: TimeInterval(ms) / 1000)
    }
}

/// writes only when the snapshot changes: the workspace refreshes every 30s
/// for relative times, and each write reloads every widget timeline. an
/// unchanged snapshot is still rewritten while connected so an app in use
/// stays fresh without renewing cached data through an outage.
final class WidgetPublisher {
    private static let refreshAfter: TimeInterval = 10 * 60
    private let writeSnapshot: (WidgetSnapshot) -> Void
    private var last: WidgetSnapshot?

    init(writeSnapshot: @escaping (WidgetSnapshot) -> Void = WidgetStore.publish) {
        self.writeSnapshot = writeSnapshot
    }

    func publish(_ ws: WorkspaceSnapshot, usage: [String: [AgentAccount]], connected: Bool, now: Date = Date()) {
        // a registry that hasn't arrived yet would read as "nothing running"
        // and wipe what the widgets showed.
        guard ws.synced, connected else { return }
        let snapshot = WidgetSnapshot(workspace: ws, usage: usage, publishedAt: now)
        if let last, snapshot.sameContent(as: last),
           snapshot.publishedAt.timeIntervalSince(last.publishedAt) < Self.refreshAfter {
            return
        }
        last = snapshot
        writeSnapshot(snapshot)
    }

    func clear() {
        last = nil
        WidgetStore.clear()
    }
}
