import Foundation
import WidgetKit

/// what the home and lock screen widgets show, written by the app into the
/// shared app group container. the extension never links the rust core: it
/// only reads this file.
struct WidgetSnapshot: Codable, Equatable {
    /// most urgent first: waiting on the user, failed, running, finished.
    var sessions: [Session]
    var devices: [Device]
    /// agent plan limits, signed-in logins first, then the most used.
    var usage: [UsageAccount]
    var voiceAvailable: Bool
    var awaiting: Int
    var failed: Int
    var working: Int
    var done: Int
    var publishedAt: Date

    /// the widgets only update while the app runs: past this, a run may have
    /// finished or a host gone offline without them knowing.
    static let staleAfter: TimeInterval = 30 * 60

    var staleAt: Date { publishedAt.addingTimeInterval(Self.staleAfter) }

    func sameContent(as other: WidgetSnapshot) -> Bool {
        var aligned = self
        aligned.publishedAt = other.publishedAt
        return aligned == other
    }

    struct Session: Codable, Equatable, Identifiable {
        var id: String
        var title: String
        var place: String
        var agent: String?
        var status: Status
        /// working: when the run started; otherwise the last activity.
        var since: Date
    }

    enum Status: String, Codable, Comparable {
        case awaiting, failed, working, done

        private var rank: Int {
            switch self {
            case .awaiting: 0
            case .failed: 1
            case .working: 2
            case .done: 3
            }
        }

        static func < (lhs: Status, rhs: Status) -> Bool { lhs.rank < rhs.rank }
    }

    /// an execution host other than this phone.
    struct Device: Codable, Equatable, Identifiable {
        var id: String
        var name: String
        var platform: String
        var online: Bool
        var sessions: Int
        var lastSeen: Date?
    }

    struct UsageAccount: Codable, Equatable, Identifiable {
        /// host and login: the same account can be signed in on two hosts.
        var id: String
        /// the agent, or the agent and provider for agents that keep a login
        /// per model provider (`pi/anthropic`): what the widget's
        /// picker selects, stable across hosts and logins.
        var source: String
        var agent: String
        var account: String?
        var plan: String?
        var host: String
        /// the login the cli runs with now; saved logins are false.
        var active: Bool
        var windows: [UsageWindow]
        var fetchedAt: Date?
        var error: String?

        var peak: Double { windows.map(\.used).max() ?? 0 }
        var isIdleUpstream: Bool { source.contains("/") && windows.isEmpty }
    }

    struct UsageWindow: Codable, Equatable {
        var label: String
        /// 0...1
        var used: Double
        var resetsAt: Date?
    }

    static let empty = WidgetSnapshot(sessions: [], devices: [], usage: [], voiceAvailable: false, awaiting: 0, failed: 0, working: 0, done: 0, publishedAt: .now)

    var needsYou: Int { awaiting + failed }

    static func supportsUsageSource(_ source: String) -> Bool {
        let harness = source.split(separator: "/", maxSplits: 1).first
        return harness != "cursor" && harness != "opencode"
    }

    var supportedUsage: [UsageAccount] {
        usage.filter { Self.supportsUsageSource($0.source) }
    }

    func usageAccounts(showing sources: [String]) -> [UsageAccount] {
        let available = supportedUsage
        let selected = sources.filter(Self.supportsUsageSource)
        guard !selected.isEmpty else {
            let inUse = available.filter { !$0.isIdleUpstream }
            return inUse.isEmpty ? available : inUse
        }

        var seen = Set<String>()
        return selected.flatMap { source in
            available.filter { $0.source == source }
        }
        .filter { seen.insert($0.id).inserted }
    }
}

enum WidgetStore {
    static let appGroup = "group.sh.zeron.ios"
    static let sessionsKind = "sh.zeron.widget.sessions"
    static let devicesKind = "sh.zeron.widget.devices"
    static let usageKind = "sh.zeron.widget.usage"
    static let actionsKind = "sh.zeron.widget.actions"

    private static var fileURL: URL? {
        FileManager.default
            .containerURL(forSecurityApplicationGroupIdentifier: appGroup)?
            .appendingPathComponent("widget-sessions.json")
    }

    /// nil when signed out (or the app hasn't published yet).
    static func load() -> WidgetSnapshot? {
        guard let fileURL, let data = try? Data(contentsOf: fileURL) else { return nil }
        return try? JSONDecoder().decode(WidgetSnapshot.self, from: data)
    }

    static func publish(_ snapshot: WidgetSnapshot) {
        guard let fileURL, let data = try? JSONEncoder().encode(snapshot) else { return }
        // session titles stay readable while the phone is locked, which the
        // lock screen widgets need.
        try? data.write(to: fileURL, options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        WidgetCenter.shared.reloadAllTimelines()
    }

    static func clear() {
        guard let fileURL else { return }
        try? FileManager.default.removeItem(at: fileURL)
        WidgetCenter.shared.reloadAllTimelines()
    }
}

/// `zeron://` routes the widgets and controls open; `SceneDelegate` handles them.
enum ZeronLink {
    static let newSession = URL(string: "zeron://new")!
    static let sessions = URL(string: "zeron://sessions")!
    static let call = URL(string: "zeron://call")!

    static func chat(_ id: String) -> URL {
        var components = URLComponents()
        components.scheme = "zeron"
        components.host = "chat"
        components.path = "/" + id
        return components.url ?? sessions
    }
}
