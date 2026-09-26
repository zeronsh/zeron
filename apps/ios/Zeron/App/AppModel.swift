import Security
import UIKit

/// App-wide state owner: holds the Rust `CoreClient`, maps its snapshots
/// into view models, and fans change notifications out to screens. All
/// members are main-thread; core events hop here through `ListenerBridge`.
final class AppModel {
    struct FrontPage: Equatable {
        var folders: [FolderRowVM] = []
        var sessions: [SessionRowVM] = []
        var sectionSessions: [String: [SessionRowVM]] = [:]
    }

    struct ProjectVM: Hashable {
        let id: String
        var name: String
        var device: String
        var colorIndex: Int
        var status: SessionRowVM.Status
        var unseen: Int
        var timeLabel: String
        var sessions: [SessionRowVM]
    }

    struct PullRequestGroup: Hashable {
        let title: String
        var sessions: [SessionRowVM]
    }

    private(set) var client: CoreClient?
    private(set) var frontPage = FrontPage()
    private(set) var projects: [ProjectVM] = []
    private(set) var pullRequests: [PullRequestGroup] = []
    private(set) var archived: [SessionRowVM] = []
    private(set) var connectivity: Connectivity?
    private var rows: [String: SessionRow] = [:]
    private var rawProjects: [ProjectView] = []
    private var workspaceRevision: UInt64 = 0
    private var observers: [UUID: () -> Void] = [:]
    private var sessionObservers: [String: [UUID: () -> Void]] = [:]
    private lazy var bridge = ListenerBridge(app: self)
    private var refreshScheduled = false
    private var clock: Timer?

    var onSignedIn: (() -> Void)?
    var onSignOut: (() -> Void)?
    var lastDraft = NewSessionDraft()

    var isSignedIn: Bool { client != nil }
    var isDemo: Bool { client?.isDemo() ?? false }

    init() {
        let args = ProcessInfo.processInfo.arguments
        if args.contains("-signedout") {
            Credentials.clearStored()
        }
        if args.contains("-demo") || args.contains("-route") && Credentials.stored() == nil {
            start(.demo(options: Self.demoOptions()))
        } else if let stored = Credentials.stored() {
            start(stored)
        }
    }

    static func demoOptions() -> DemoOptions {
        let args = ProcessInfo.processInfo.arguments
        let scale: TranscriptScale = args.contains("-huge") ? .huge : args.contains("-big") ? .big : .normal
        return DemoOptions(
            fixture: args.contains("-no-projects") ? .noProjects : args.contains("-ios-only") ? .iosOnly : .standard,
            transcriptScale: scale,
            streamSpeed: args.contains("-fast") ? .fast : .realistic,
            longReply: args.contains("-longreply")
        )
    }

    // MARK: Lifecycle

    private func start(_ credentials: Credentials) {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = support.appendingPathComponent(credentials.isDemo ? "demo" : "core", isDirectory: true)
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let config = CoreConfig(
            edgeUrl: Endpoints.edgeURL.absoluteString,
            dataDir: dir.path,
            deviceId: Self.deviceId,
            deviceName: UIDevice.current.name,
            platform: "ios",
            appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0"
        )
        do {
            client = try CoreClient(config: config, credentials: credentials, listener: bridge)
            if !credentials.isDemo { Credentials.store(credentials) }
            refreshWorkspace()
            client?.preloadSessions()
            clock?.invalidate()
            // Relative times ("4m") and 45s staleness age without events.
            clock = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in self?.refreshWorkspace() }
        } catch {
            NSLog("core start failed: \(error)")
            client = nil
        }
    }

    static var deviceId: String {
        if let id = UserDefaults.standard.string(forKey: "deviceId") { return id }
        let id = "ios-" + UUID().uuidString.lowercased().prefix(8)
        UserDefaults.standard.set(id, forKey: "deviceId")
        return id
    }

    func enterDemo() {
        start(.demo(options: Self.demoOptions()))
        onSignedIn?()
    }

    /// WorkOS code → tokens → org (the first, or the only one) → client.
    func signIn(code: String) async throws {
        let edge = Endpoints.edgeURL.absoluteString
        let exchange = try await authExchangeCode(edgeUrl: edge, code: code)
        let orgs = try await authListOrgs(edgeUrl: edge, accessToken: exchange.tokens.accessToken)
        guard let org = orgs.first else { throw CoreError.Auth(message: "This account isn't in an organization yet.") }
        let tokens = try await authRefresh(edgeUrl: edge, refreshToken: exchange.tokens.refreshToken, organizationId: org.organizationId)
        await MainActor.run {
            start(.workOs(userId: exchange.user.id, orgId: org.organizationId, tokens: tokens))
            onSignedIn?()
        }
    }

    func signOut() { onSignOut?() }

    func signOutLocally() {
        client?.shutdown()
        client = nil
        Credentials.clearStored()
        frontPage = FrontPage()
        projects = []
        pullRequests = []
        clock?.invalidate()
    }

    func didEnterBackground() { client?.onBackground() }
    func willEnterForeground() {
        client?.onForeground()
        refreshWorkspace()
    }

    // MARK: Observation

    @discardableResult
    func observe(_ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = handler
        return Token { [weak self] in self?.observers[id] = nil }
    }

    func observeSession(_ chatId: String, _ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        sessionObservers[chatId, default: [:]][id] = handler
        return Token { [weak self] in self?.sessionObservers[chatId]?[id] = nil }
    }

    fileprivate final class Token {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }

    fileprivate func handle(_ event: ClientEvent) {
        switch event {
        case .workspaceChanged:
            scheduleRefresh()
        case let .sessionChanged(chatId, _), let .composerChanged(chatId, _):
            sessionObservers[chatId]?.values.forEach { $0() }
        case let .connectivityChanged(c):
            connectivity = c
            observers.values.forEach { $0() }
        case let .authRefreshed(tokens):
            Credentials.updateTokens(tokens)
        case .authExpired:
            signOut()
        }
    }

    /// Coalesce bursts of registry frames into one rebuild per runloop turn.
    private func scheduleRefresh() {
        guard !refreshScheduled else { return }
        refreshScheduled = true
        DispatchQueue.main.async { [weak self] in
            self?.refreshScheduled = false
            self?.refreshWorkspace()
        }
    }

    private func refreshWorkspace() {
        guard let client else { return }
        let ws = client.workspace()
        workspaceRevision = ws.revision
        var all: [String: SessionRow] = [:]
        func vm(_ r: SessionRow) -> SessionRowVM {
            all[r.id] = r
            return Self.vm(r)
        }
        var page = FrontPage()
        let pinned = ws.front.pinned.map(vm)
        let inline = pinnedInline
        if !pinned.isEmpty, !inline {
            page.folders.append(FolderRowVM(id: "pinned", name: "Pinned", count: pinned.count, symbol: "pin"))
        }
        page.sectionSessions["pinned"] = pinned
        for s in ws.front.sections {
            let list = s.sessions.map(vm)
            page.folders.append(FolderRowVM(id: s.id, name: s.name, count: list.count, symbol: "folder"))
            page.sectionSessions[s.id] = list
        }
        page.sessions = (inline ? pinned : []) + ws.front.recent.map(vm)
        rawProjects = ws.projects
        let projectVMs = ws.projects.map { p in
            ProjectVM(
                id: p.id,
                name: p.name,
                device: p.deviceName ?? p.deviceId,
                colorIndex: Int(p.colorIndex),
                status: Self.status(p.indicator),
                unseen: Int(p.unseenCount),
                timeLabel: p.sessions.first?.timeLabel ?? "",
                sessions: p.sessions.map(vm)
            )
        }
        let pr = ws.pullRequests
        let prGroups = [
            PullRequestGroup(title: "Open", sessions: pr.open.map(vm)),
            PullRequestGroup(title: "Merged", sessions: pr.merged.map(vm)),
            PullRequestGroup(title: "Closed", sessions: pr.closed.map(vm)),
        ]
        let archivedVMs = ws.archived.map(vm)
        rows = all
        let changed = page != frontPage || projectVMs != projects || prGroups != pullRequests || archivedVMs != archived
        frontPage = page
        projects = projectVMs
        pullRequests = prGroups
        archived = archivedVMs
        if changed { observers.values.forEach { $0() } }
    }

    static func status(_ i: ChatIndicator) -> SessionRowVM.Status {
        switch i {
        case .working: .working
        case .awaitingInput: .awaiting
        case .errored: .errored
        case .completed: .completed
        case .idle: .idle
        }
    }

    static func vm(_ r: SessionRow) -> SessionRowVM {
        SessionRowVM(
            id: r.id,
            title: r.title,
            projectName: r.project?.name ?? r.deviceName ?? "No project",
            colorIndex: Int(r.project?.colorIndex ?? 0),
            harness: r.harness,
            branch: r.branch,
            pr: r.pullRequest.map { pr in
                switch pr.state {
                case .open: .open
                case .merged: .merged
                case .closed: .closed
                }
            },
            prNumber: r.pullRequest?.number,
            status: status(r.indicator),
            timeLabel: r.timeLabel,
            unseen: r.unseen,
            pinned: r.pinned,
            sendFailed: r.sendState == .failed
        )
    }

    func session(_ id: String) -> SessionRowVM? {
        rows[id].map(Self.vm) ?? client?.sessionRow(chatId: id).map(Self.vm)
    }

    func row(_ id: String) -> SessionRow? {
        rows[id] ?? client?.sessionRow(chatId: id)
    }

    func sessions(inFolder id: String) -> [SessionRowVM] {
        id == "archived" ? archived : frontPage.sectionSessions[id] ?? []
    }

    func search(_ query: String) -> [SessionRowVM] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard let client, !q.isEmpty else { return frontPage.sessions }
        return client.search(query: q, limit: 60).map { Self.vm($0.session) }
    }

    /// Front-page preference: pinned sessions as a folder or inline at top.
    var pinnedInline: Bool {
        get { UserDefaults.standard.bool(forKey: "pinnedInline") }
        set {
            UserDefaults.standard.set(newValue, forKey: "pinnedInline")
            refreshWorkspace()
            observers.values.forEach { $0() }
        }
    }

    // MARK: Writes

    private func attempt(_ what: String, _ body: () throws -> Void) {
        do { try body() } catch { NSLog("\(what) failed: \(error)") }
    }

    func setPinned(_ id: String, _ pinned: Bool) {
        attempt("pin") { pinned ? try client?.pinSession(chatId: id) : try client?.unpinSession(chatId: id) }
    }

    func archive(_ id: String) { attempt("archive") { try client?.archiveSession(chatId: id) } }
    func unarchive(_ id: String) { attempt("unarchive") { try client?.unarchiveSession(chatId: id) } }
    func move(_ id: String, toSection section: String?) { attempt("assign") { try client?.assignSection(chatId: id, sectionId: section) } }
    func createSection(_ name: String) { attempt("section") { _ = try client?.createSection(name: name) } }
    func rename(_ id: String, _ title: String) { attempt("rename") { try client?.renameSession(chatId: id, title: title) } }
    func markSeen(_ id: String) { attempt("seen") { try client?.markSeen(chatId: id) } }

    // MARK: Sessions

    func sessionSource(_ chatId: String) -> SessionSource {
        guard let client, let handle = try? client.openSession(chatId: chatId) else {
            return FixtureSessionSource(title: "Unavailable", subtitle: "")
        }
        return CoreSessionSource(app: self, client: client, handle: handle, chatId: chatId)
    }

    // MARK: New session

    var accountName: String { isDemo ? "Demo" : (client?.userId() ?? "Signed out") }
    var accountDetail: String { isDemo ? "Offline demo workspace" : "Organization \(client?.orgId() ?? "")" }

    var projectOptions: [ProjectOption] {
        rawProjects.map { ProjectOption(id: $0.id, name: $0.name, device: $0.deviceId, online: $0.deviceOnline, git: $0.gitDetected, colorIndex: Int($0.colorIndex)) }
    }

    var hostOptions: [HostOption] {
        (client?.executionDevices() ?? []).map { HostOption(id: $0.id, name: $0.name, online: $0.online) }
    }

    func deviceName(_ id: String) -> String {
        client?.devices().first { $0.id == id }?.name ?? id
    }

    func models(for deviceId: String) async -> [ModelChoice] {
        guard let client else { return [] }
        let harnesses = (try? await client.listHarnesses(deviceId: deviceId)) ?? fallbackHarnesses()
        var out: [ModelChoice] = []
        for h in harnesses where h.offered {
            let models = (try? await client.listModels(deviceId: deviceId, harness: h.id)) ?? fallbackModels(harness: h.id)
            out += models.map { ModelChoice(harness: h.id, harnessLabel: h.label, id: $0.id, label: $0.label, efforts: $0.reasoningLevels) }
        }
        return out
    }

    func refs(projectId: String) async -> [String] {
        guard let client, let p = rawProjects.first(where: { $0.id == projectId }) else { return [] }
        let refs = (try? await client.listRefs(deviceId: p.deviceId, repoPath: p.path)) ?? []
        return refs.sorted { $0.current && !$1.current }.map(\.name)
    }

    /// Create the chat, open it, and send the first message.
    func createSession(draft: NewSessionDraft, text: String, images: [StagedImage]) -> String? {
        guard let client else { return nil }
        let target: SessionTarget
        if let p = draft.projectId {
            target = .project(spaceId: p)
        } else if let h = draft.hostId {
            target = .projectless(deviceId: h)
        } else {
            return nil
        }
        let config = ChatConfig(harness: draft.harness, model: draft.model, reasoning: draft.effort, modelOptions: [:], sandbox: .workspaceWrite)
        do {
            let chatId = try client.createSession(newSession: NewSession(target: target, config: config, branch: draft.worktree ? nil : draft.branch, cwd: nil, title: nil))
            let handle = try client.openSession(chatId: chatId)
            let project = rawProjects.first { $0.id == draft.projectId }
            let worktree = draft.worktree ? project.map { WorktreeSpec(repoPath: $0.path, base: draft.branch ?? "HEAD", spaceId: $0.id) } : nil
            _ = try handle.send(request: SendRequest(text: text, attachments: images.map(\.outgoing), worktree: worktree, busy: .queue))
            refreshWorkspace()
            return chatId
        } catch {
            NSLog("create session failed: \(error)")
            return nil
        }
    }
}

extension StagedImage {
    var outgoing: OutgoingAttachment { OutgoingAttachment(name: name, mimeType: "image/jpeg", data: data) }
}

/// Core events arrive on Rust runtime threads; hop to main.
private final class ListenerBridge: ClientListener, @unchecked Sendable {
    weak var app: AppModel?

    init(app: AppModel) { self.app = app }

    func onEvent(event: ClientEvent) {
        DispatchQueue.main.async { [weak self] in self?.app?.handle(event) }
    }
}

// MARK: - Credential persistence (Keychain)

extension Credentials {
    var isDemo: Bool {
        if case .demo = self { return true }
        return false
    }

    private static let service = "sh.zeron.ios"
    private static let account = "credentials"

    static func stored() -> Credentials? {
        guard let data = Keychain.load(service: service, account: account),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: String]
        else { return nil }
        switch dict["kind"] {
        case "workos":
            guard let u = dict["userId"], let o = dict["orgId"], let a = dict["access"], let r = dict["refresh"] else { return nil }
            return .workOs(userId: u, orgId: o, tokens: AuthTokens(accessToken: a, refreshToken: r))
        case "dev":
            guard let u = dict["userId"], let o = dict["orgId"] else { return nil }
            return .dev(userId: u, orgId: o)
        default:
            return nil
        }
    }

    static func store(_ c: Credentials) {
        var dict: [String: String] = [:]
        switch c {
        case let .workOs(userId, orgId, tokens):
            dict = ["kind": "workos", "userId": userId, "orgId": orgId, "access": tokens.accessToken, "refresh": tokens.refreshToken]
        case let .dev(userId, orgId):
            dict = ["kind": "dev", "userId": userId, "orgId": orgId]
        case .demo:
            return
        }
        if let data = try? JSONSerialization.data(withJSONObject: dict) {
            Keychain.save(data, service: service, account: account)
        }
    }

    static func updateTokens(_ tokens: AuthTokens) {
        guard case let .workOs(userId, orgId, _) = stored() else { return }
        store(.workOs(userId: userId, orgId: orgId, tokens: tokens))
    }

    static func clearStored() {
        Keychain.delete(service: service, account: account)
    }
}

enum Keychain {
    static func save(_ data: Data, service: String, account: String) {
        delete(service: service, account: account)
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlock,
        ]
        SecItemAdd(q as CFDictionary, nil)
    }

    static func load(service: String, account: String) -> Data? {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
            kSecReturnData as String: true,
            kSecMatchLimit as String: kSecMatchLimitOne,
        ]
        var out: AnyObject?
        return SecItemCopyMatching(q as CFDictionary, &out) == errSecSuccess ? out as? Data : nil
    }

    static func delete(service: String, account: String) {
        let q: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        SecItemDelete(q as CFDictionary)
    }
}
