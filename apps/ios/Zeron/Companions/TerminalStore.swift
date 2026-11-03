import UIKit

/// One terminal on a chat's host. The Rust session keeps the emulator and the
/// output stream alive while no screen shows it, so coming back is instant
/// and the session list knows what's still running.
final class TerminalTab: TerminalListener, @unchecked Sendable {
    let chatId: String
    let session: TerminalSession
    /// Position among the chat's terminals ("zsh", "zsh 2", …).
    var ordinal: Int
    private var observers: [UUID: () -> Void] = [:]
    private let lock = NSLock()
    private var scheduled = false
    /// The phase as last seen on main (drives the session's chips).
    private(set) var phase: TerminalPhase = .connecting
    private(set) var title: String?
    /// Riding a direct link to its host (else the relay). Not shown — the
    /// route is plumbing — but the DEBUG terminal probe measures by it.
    private(set) var direct = false
    var onPhaseChange: (() -> Void)?

    init(chatId: String, session: TerminalSession, ordinal: Int) {
        self.chatId = chatId
        self.session = session
        self.ordinal = ordinal
    }

    var ref: TerminalRef { session.info() }

    /// "zsh", or what the running program calls itself.
    var name: String {
        if let title, !title.isEmpty { return title }
        let shell = ref.shell.isEmpty ? "Terminal" : ref.shell
        return ordinal > 1 ? "\(shell) \(ordinal)" : shell
    }

    /// The folder it started in, home-abbreviated.
    var folder: String {
        let cwd = ref.cwd
        for home in ["/Users/", "/home/"] where cwd.hasPrefix(home) {
            let rest = cwd.dropFirst(home.count)
            if let slash = rest.firstIndex(of: "/") { return "~" + rest[slash...] }
            return "~"
        }
        return cwd == "/root" ? "~" : cwd
    }

    var isLive: Bool {
        switch phase {
        case .exited, .gone: false
        default: true
        }
    }

    func observe(_ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = handler
        return Token { [weak self] in self?.observers[id] = nil }
    }

    // MARK: TerminalListener (client threads)

    func changed() {
        lock.lock()
        let schedule = !scheduled
        scheduled = true
        lock.unlock()
        guard schedule else { return }
        DispatchQueue.main.async { [weak self] in
            guard let self else { return }
            lock.lock()
            scheduled = false
            lock.unlock()
            let phase = session.phase()
            let title = session.title()
            direct = session.isDirect()
            if phase != self.phase || title != self.title {
                self.phase = phase
                self.title = title
                onPhaseChange?()
            }
            observers.values.forEach { $0() }
        }
    }

    func bell() {
        DispatchQueue.main.async { UIImpactFeedbackGenerator(style: .soft).impactOccurred(intensity: 0.6) }
    }

    private final class Token {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }
}

/// Every chat's terminals. Their refs persist, so a relaunch reattaches to
/// shells still running on their hosts (their output replays).
@MainActor
final class TerminalStore {
    private weak var app: AppModel?
    private var tabs: [String: [TerminalTab]] = [:]
    private var restored = Set<String>()
    private var observers: [UUID: (String) -> Void] = [:]
    private static let key = "terminals"

    init(app: AppModel) {
        self.app = app
    }

    func observe(_ handler: @escaping (String) -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = handler
        return StoreToken { [weak self] in self?.observers[id] = nil }
    }

    private func notify(_ chatId: String) {
        observers.values.forEach { $0(chatId) }
    }

    func tabs(for chatId: String) -> [TerminalTab] {
        restore(chatId)
        return tabs[chatId] ?? []
    }

    /// Open a shell on the chat's host.
    func open(chatId: String, cols: UInt16, rows: UInt16) async throws -> TerminalTab {
        guard let client = app?.client else { throw CoreError.Closed }
        restore(chatId)
        let ordinal = (tabs[chatId]?.map(\.ordinal).max() ?? 0) + 1
        let relay = ListenerRelay()
        let session = try await client.openTerminal(chatId: chatId, cols: cols, rows: rows, listener: relay)
        let tab = adopt(chatId: chatId, session: session, ordinal: ordinal, relay: relay)
        persist(chatId)
        notify(chatId)
        return tab
    }

    /// End the shell on its host and forget it.
    func close(_ tab: TerminalTab) {
        tab.session.close()
        forget(tab)
    }

    /// Forget a tab whose shell already ended.
    func forget(_ tab: TerminalTab) {
        tab.session.detach()
        tabs[tab.chatId]?.removeAll { $0 === tab }
        persist(tab.chatId)
        notify(tab.chatId)
    }

    private func adopt(chatId: String, session: TerminalSession, ordinal: Int, relay: ListenerRelay) -> TerminalTab {
        let tab = TerminalTab(chatId: chatId, session: session, ordinal: ordinal)
        relay.target = tab
        tab.onPhaseChange = { [weak self, weak tab] in
            guard let self, let tab else { return }
            // A terminal its host forgot can't come back.
            if case .gone = tab.phase { self.forget(tab) } else { self.notify(chatId) }
        }
        tab.session.setDark(dark: UITraitCollection.current.userInterfaceStyle == .dark)
        tabs[chatId, default: []].append(tab)
        return tab
    }

    private func restore(_ chatId: String) {
        guard restored.insert(chatId).inserted, let client = app?.client else { return }
        let saved = (UserDefaults.standard.dictionary(forKey: Self.key)?[chatId] as? [[String: String]]) ?? []
        for (i, dict) in saved.enumerated() {
            guard let id = dict["id"], let device = dict["device"] else { continue }
            let ref = TerminalRef(id: id, deviceId: device, cwd: dict["cwd"] ?? "", shell: dict["shell"] ?? "zsh", hostReplies: dict["replies"] != "0")
            let relay = ListenerRelay()
            let session = client.attachTerminal(terminal: ref, cols: 80, rows: 24, listener: relay)
            _ = adopt(chatId: chatId, session: session, ordinal: i + 1, relay: relay)
        }
    }

    private func persist(_ chatId: String) {
        var all = UserDefaults.standard.dictionary(forKey: Self.key) ?? [:]
        let list = (tabs[chatId] ?? []).map { tab -> [String: String] in
            let r = tab.ref
            return ["id": r.id, "device": r.deviceId, "cwd": r.cwd, "shell": r.shell, "replies": r.hostReplies ? "1" : "0"]
        }
        all[chatId] = list.isEmpty ? nil : list
        UserDefaults.standard.set(all, forKey: Self.key)
    }

    /// Appearance changed: terminals resolve their palette again.
    func setDark(_ dark: Bool) {
        for tab in tabs.values.flatMap({ $0 }) { tab.session.setDark(dark: dark) }
    }

    func signOut() {
        for tab in tabs.values.flatMap({ $0 }) { tab.session.detach() }
        tabs = [:]
        restored = []
        UserDefaults.standard.removeObject(forKey: Self.key)
    }

    private final class StoreToken {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }
}

/// The session is created before its tab (the listener is a constructor
/// argument): this forwards to the tab once it exists.
private final class ListenerRelay: TerminalListener, @unchecked Sendable {
    weak var target: TerminalTab?
    func changed() { target?.changed() }
    func bell() { target?.bell() }
}
