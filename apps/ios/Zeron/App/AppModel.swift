import Network
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
        var groups: [SessionGroupVM] = []
    }

    struct LiveCounts: Equatable {
        var working = 0
        var awaiting = 0
    }

    private(set) var client: CoreClient?
    private(set) var frontPage = FrontPage()
    private(set) var archived: [SessionRowVM] = []
    private(set) var connectivity: Connectivity?
    /// Front-page sessions that are working / waiting on the user.
    private(set) var live = LiveCounts()
    private var rows: [String: SessionRow] = [:]
    private var rawProjects: [ProjectView] = []
    private(set) var sessionScope: SessionScope = .all
    private(set) var historyScope: SessionScope = .all
    private(set) var viewPreferences = SessionViewPreferences()
    private(set) var workspaceSynced = false
    private(set) var sectionOptions: [FolderRowVM] = []
    private var preferences: WorkspacePreferences?
    private var scopePreferences: WorkspacePreferences?
    private var lastHosts: [HostOption] = []
    private var workspaceRevision: UInt64 = 0
    private var observers: [UUID: () -> Void] = [:]
    private var sessionObservers: [String: [UUID: () -> Void]] = [:]
    private lazy var bridge = ListenerBridge(app: self)
    private var refreshScheduled = false
    private var clock: Timer?
    private let path = NWPathMonitor()

    var onSignedIn: (() -> Void)?
    var onSignOut: (() -> Void)?
    /// The new-session page's options (project, host, branch, model,
    /// effort), kept across launches.
    var lastDraft = NewSessionDraft() {
        didSet {
            preferences?.draft = lastDraft
        }
    }

    /// The demo (and the UI tests on it) keep the new-session page in memory
    /// only: one run's leftovers must not seed the next.
    private var volatileNewSessionText = ""

    /// What was typed on the new-session page when it was closed unsent.
    var newSessionText: String {
        get { preferences?.text ?? volatileNewSessionText }
        set {
            if let preferences { preferences.text = newValue } else { volatileNewSessionText = newValue }
        }
    }

    /// Images staged there (this launch only).
    var newSessionImages: [StagedImage] = []

    var isSignedIn: Bool { client != nil }
    var isDemo: Bool { client?.isDemo() ?? false }

    init(credentials: Credentials? = nil) {
        // Online/offline + interface changes cut sync backoff short.
        path.pathUpdateHandler = { [weak self] p in
            DispatchQueue.main.async { self?.client?.setNetworkOnline(online: p.status == .satisfied) }
        }
        path.start(queue: DispatchQueue(label: "sh.zeron.path"))
        if let credentials { start(credentials); return }
        let args = ProcessInfo.processInfo.arguments
        #if DEBUG
        // Test hooks (never in release builds): wipe the Keychain, or run
        // against a local dev stack.
        if args.contains("-signedout") {
            Credentials.clearStored()
        }
        if let i = args.firstIndex(of: "-dev"), i + 2 < args.count {
            start(.dev(userId: args[i + 1], orgId: args[i + 2]))
            return
        }
        #endif
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
            fixture: args.contains("-no-projects") ? .noProjects : args.contains("-ios-only") ? .iosOnly : args.contains("-project-filter-fixture") ? .projectFilter : .standard,
            transcriptScale: scale,
            streamSpeed: args.contains("-fast") ? .fast : .realistic,
            longReply: args.contains("-longreply")
        )
    }

    // MARK: Lifecycle

    private static var coreDir: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0].appendingPathComponent("core", isDirectory: true)
    }

    /// Local docs belong to one identity: another account/org starts from an
    /// empty store (it must never see — or sync — the last one's cache).
    private static func claimCoreDir(for credentials: Credentials) {
        let owner: String
        switch credentials {
        case let .workOs(userId, orgId, _), let .dev(userId, orgId): owner = "\(userId)/\(orgId)"
        case .demo: return
        }
        let marker = coreDir.appendingPathComponent(".owner")
        if (try? String(contentsOf: marker, encoding: .utf8)) != owner {
            try? FileManager.default.removeItem(at: coreDir)
        }
        try? FileManager.default.createDirectory(at: coreDir, withIntermediateDirectories: true)
        try? owner.write(to: marker, atomically: true, encoding: .utf8)
    }

    @discardableResult
    private func start(_ credentials: Credentials) -> Bool {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir = credentials.isDemo ? support.appendingPathComponent("demo", isDirectory: true) : Self.coreDir
        if !credentials.isDemo {
            preferences = WorkspacePreferences(userId: credentials.userId, orgId: credentials.orgId)
            if (try? String(contentsOf: Self.coreDir.appendingPathComponent(".owner"), encoding: .utf8)) == "\(credentials.userId)/\(credentials.orgId)" {
                preferences?.migrateLegacyDraft()
                preferences?.migrateLegacyViewPreferences()
            }
            Self.claimCoreDir(for: credentials)
        }
        scopePreferences = preferences
        #if DEBUG
        if credentials.isDemo, ProcessInfo.processInfo.arguments.contains("-persist-demo-scope") {
            scopePreferences = WorkspacePreferences(userId: "demo-test", orgId: "demo-test")
            if ProcessInfo.processInfo.arguments.contains("-reset-demo-scope") {
                scopePreferences?.saveScope(.all, "sessionsScope")
                scopePreferences?.saveScope(.all, "historyScope")
                scopePreferences?.viewPreferences = SessionViewPreferences()
            }
        }
        #endif
        sessionScope = scopePreferences?.scope("sessionsScope") ?? .all
        historyScope = scopePreferences?.scope("historyScope") ?? .all
        viewPreferences = scopePreferences?.viewPreferences ?? SessionViewPreferences()
        lastDraft = preferences?.draft ?? NewSessionDraft()
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-harness"), i + 1 < args.count { lastDraft.harness = args[i + 1] }
        try? FileManager.default.createDirectory(at: dir, withIntermediateDirectories: true)
        let config = CoreConfig(
            edgeUrl: Self.edgeURL,
            dataDir: dir.path,
            deviceId: Self.deviceId,
            deviceName: UIDevice.current.name,
            platform: "ios",
            appVersion: Bundle.main.infoDictionary?["CFBundleShortVersionString"] as? String ?? "0"
        )
        do {
            client = try CoreClient(config: config, credentials: credentials, listener: bridge)
            // Only real accounts persist; dev bearers come from launch args
            // (and would be rejected by the production edge next launch).
            if case .workOs = credentials { Credentials.store(credentials) }
            refreshWorkspace()
            client?.preloadSessions()
            backfillProfile()
            clock?.invalidate()
            // Relative times ("4m") and 45s staleness age without events.
            clock = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in self?.refreshWorkspace() }
            return true
        } catch {
            NSLog("core start failed: \(error)")
            client = nil
            return false
        }
    }

    /// `-edge <url>` points at a local `wrangler dev` edge; otherwise production.
    static var edgeURL: String {
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-edge"), i + 1 < args.count { return args[i + 1] }
        #endif
        return Endpoints.edgeURL.absoluteString
    }

    static var deviceId: String {
        if let id = UserDefaults.standard.string(forKey: "deviceId") { return id }
        let id = "ios-" + UUID().uuidString.lowercased().prefix(8)
        UserDefaults.standard.set(id, forKey: "deviceId")
        return id
    }

    /// WorkOS code → tokens → org (the first, or the only one) → client.
    func signIn(code: String, chooseOrg: @escaping ([AuthOrg]) async -> AuthOrg?) async throws {
        let edge = Self.edgeURL
        let exchange = try await authExchangeCode(edgeUrl: edge, code: code)
        let orgs = try await authListOrgs(edgeUrl: edge, accessToken: exchange.tokens.accessToken)
        guard !orgs.isEmpty else { throw CoreError.Auth(message: "This account isn't in an organization yet.") }
        guard let org = orgs.count == 1 ? orgs[0] : await chooseOrg(orgs) else { return }
        let tokens = try await authRefresh(edgeUrl: edge, refreshToken: exchange.tokens.refreshToken, organizationId: org.organizationId)
        let user = exchange.user
        let name = [user.firstName, user.lastName].compactMap { $0?.trimmingCharacters(in: .whitespaces) }.filter { !$0.isEmpty }.joined(separator: " ")
        AccountProfile(name: name.nonEmpty, email: user.email, orgName: org.name).save()
        let started = await MainActor.run { start(.workOs(userId: exchange.user.id, orgId: org.organizationId, tokens: tokens)) }
        guard started else { throw CoreError.Auth(message: "Couldn't open your workspace. Try signing in again.") }
        await MainActor.run { onSignedIn?() }
    }

    /// The user signing out: this device forgets the account's local docs.
    func signOut() {
        forgetOnSignOut = true
        onSignOut?()
    }

    /// Set only by a sign-out the user asked for. An expired session keeps its
    /// store (the outbox may hold unsent messages); the `.owner` marker still
    /// keeps a different account out of it.
    private var forgetOnSignOut = false

    func signOutLocally() {
        client?.shutdown()
        client = nil
        Credentials.clearStored()
        AccountProfile.clear()
        clock?.invalidate()
        if forgetOnSignOut { try? FileManager.default.removeItem(at: Self.coreDir) }
        forgetOnSignOut = false
        // Nothing of the account stays in memory.
        frontPage = FrontPage()
        archived = []
        rows = [:]
        rawProjects = []
        sectionOptions = []
        lastHosts = []
        connectivity = nil
        live = LiveCounts()
        workspaceRevision = 0
        workspaceSynced = false
        sessionScope = .all
        historyScope = .all
        viewPreferences = SessionViewPreferences()
        preferences = nil
        scopePreferences = nil
        lastDraft = NewSessionDraft()
        newSessionText = ""
        newSessionImages = []
    }

    func didEnterBackground() { client?.onBackground() }
    func willEnterForeground() {
        client?.onForeground()
        refreshWorkspace()
    }

    /// Pull to refresh: redial the registry and rooms and re-probe health (the
    /// same resync as returning to the foreground), then report back once the
    /// workspace has updated — at least `minimum` so the spinner doesn't
    /// flicker, at most `maximum` if nothing changes.
    func refresh(minimum: TimeInterval = 0.6, maximum: TimeInterval = 3, completion: @escaping () -> Void) {
        let started = Date()
        var finished = false
        var token: AnyObject?
        let finish = {
            guard !finished else { return }
            finished = true
            token = nil
            let wait = max(0, minimum - Date().timeIntervalSince(started))
            DispatchQueue.main.asyncAfter(deadline: .now() + wait, execute: completion)
        }
        // A reconnecting/offline blip from the redial isn't the resync landing.
        token = observe { [weak self] in
            if let state = self?.connectivity?.state, state == .reconnecting || state == .offline { return }
            finish()
        }
        client?.onForeground()
        refreshWorkspace()
        DispatchQueue.main.asyncAfter(deadline: .now() + maximum) {
            finish()
            _ = token
        }
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
            onSignOut?()
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
        let projectsChanged = rawProjects != ws.projects
        let syncChanged = workspaceSynced != ws.synced
        rawProjects = ws.projects
        sectionOptions = ws.front.sections.map { FolderRowVM(id: $0.id, name: $0.name, count: $0.sessions.count, symbol: "folder") }
        workspaceSynced = ws.synced
        let projection = client.sessionListWithOptions(scope: sessionScope, options: viewPreferences.core)
        let scopeChanged = sessionScope != projection.scope
        sessionScope = projection.scope
        if scopeChanged { scopePreferences?.saveScope(sessionScope, "sessionsScope") }
        if case let .project(id) = historyScope, ws.synced, !ws.projects.contains(where: { $0.id == id }) {
            historyScope = .all
            scopePreferences?.saveScope(historyScope, "historyScope")
        }
        workspaceRevision = ws.revision
        var all: [String: SessionRow] = [:]
        func vm(_ r: SessionRow) -> SessionRowVM {
            all[r.id] = r
            return Self.vm(r)
        }
        var page = FrontPage()
        let pinned = projection.front.pinned.map(vm)
        if !pinned.isEmpty {
            page.folders.append(FolderRowVM(id: "pinned", name: "Pinned", count: pinned.count, symbol: "pin"))
        }
        page.sectionSessions["pinned"] = pinned
        for s in projection.front.sections {
            let list = s.sessions.map(vm)
            page.folders.append(FolderRowVM(id: s.id, name: s.name, count: list.count, symbol: "folder"))
            page.sectionSessions[s.id] = list
        }
        page.sessions = projection.front.recent.map(vm)
        page.groups = projection.groups.map { group in
            SessionGroupVM(id: group.id, title: group.name,
                subtitle: [group.deviceName == group.name ? nil : group.deviceName, group.path].compactMap { $0 }.joined(separator: " · ").nonEmpty,
                projectId: group.id.hasPrefix("project:") ? String(group.id.dropFirst(8)) : nil,
                sessions: group.sessions.map(vm))
        }
        // Keep global rows available to deep links and the iPad detail column.
        for r in ws.front.pinned + ws.front.sections.flatMap(\.sessions) + ws.front.recent + ws.archived { all[r.id] = r }
        // Sessions reachable only through their project still resolve by id.
        for p in ws.projects {
            for r in p.sessions where all[r.id] == nil { all[r.id] = r }
        }
        let archivedVMs = projection.archived.map(vm)
        var counts = LiveCounts()
        var seen = Set<String>()
        for row in page.sessions + page.sectionSessions.values.flatMap({ $0 }) where seen.insert(row.id).inserted {
            if row.status == .working { counts.working += 1 }
            if row.status == .awaiting { counts.awaiting += 1 }
        }
        rows = all
        // Devices coming and going (Settings, host pickers) count as changes.
        let hosts = hostOptions
        let changed = page != frontPage || archivedVMs != archived || counts != live || hosts != lastHosts || projectsChanged || syncChanged || scopeChanged
        lastHosts = hosts
        live = counts
        frontPage = page
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
            hasProject: r.project != nil,
            // Project-less sessions tone like the desktop's "home" tile.
            colorIndex: Int(r.project?.colorIndex ?? projectColorIndex(spacePath: "home")),
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

    func search(_ query: String, scope: SessionScope? = nil, includeArchived: Bool = false) -> [SessionRowVM] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard let client, !q.isEmpty else { return frontPage.sessions }
        return client.searchScoped(query: q, scope: scope ?? sessionScope, includeArchived: includeArchived, limit: 60).map { Self.vm($0.session) }
    }

    func setSessionScope(_ scope: SessionScope) {
        guard scope != sessionScope else { return }
        sessionScope = scope
        scopePreferences?.saveScope(scope, "sessionsScope")
        refreshWorkspace()
        // Empty projects can yield the same projection; the label still changes.
        observers.values.forEach { $0() }
    }

    func setHistoryScope(_ scope: SessionScope) {
        historyScope = scope
        scopePreferences?.saveScope(scope, "historyScope")
        observers.values.forEach { $0() }
    }

    func setViewPreferences(_ value: SessionViewPreferences) {
        guard value != viewPreferences else { return }
        viewPreferences = value
        scopePreferences?.viewPreferences = value
        refreshWorkspace()
        // Metadata changes must redraw even when the row projection is equal.
        observers.values.forEach { $0() }
    }

    func scopeTitle(_ scope: SessionScope) -> String {
        switch scope {
        case .all: return "All projects"
        case .projectless: return "No project"
        case let .project(id): return rawProjects.first { $0.id == id }?.name ?? "Loading project…"
        }
    }

    func scopeImage(_ scope: SessionScope) -> UIImage? {
        if case let .project(id) = scope, let p = rawProjects.first(where: { $0.id == id }) {
            return ProjectTile.image(name: p.name, colorIndex: Int(p.colorIndex))
        }
        return UIImage(systemName: scope == .projectless ? "tray" : "square.grid.2x2")
    }

    func projects(matching query: String) -> [ProjectOption] {
        (client?.searchProjects(query: query) ?? []).map(projectOption)
    }

    /// A fresh draft follows the list. Saved content/attachments or an
    /// explicit target choice keep the draft's actual execution target.
    func newDraft(scope: SessionScope? = nil) -> NewSessionDraft {
        var draft = lastDraft
        if !newSessionText.isEmpty || !newSessionImages.isEmpty || draft.targetChosen { return draft }
        switch scope ?? sessionScope {
        case .all: break
        case let .project(id): draft.projectId = id; draft.hostId = nil; draft.branch = nil; draft.worktree = false
        case .projectless: draft.projectId = nil; draft.hostId = hostOptions.first(where: \.online)?.id ?? hostOptions.first?.id; draft.branch = nil; draft.worktree = false
        }
        return draft
    }


    // MARK: Writes

    private func attempt(_ what: String, _ body: () throws -> Void) {
        do { try body() } catch { NSLog("\(what) failed: \(error)") }
    }

    func setPinned(_ id: String, _ pinned: Bool) {
        attempt("pin") { pinned ? try client?.pinSession(chatId: id) : try client?.unpinSession(chatId: id) }
    }

    func archive(_ id: String) {
        attempt("archive") { try client?.archiveSession(chatId: id) }
        let window = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
        Toast.show("Archived", action: "Undo", in: window) { [weak self] in self?.unarchive(id) }
    }
    func unarchive(_ id: String) { attempt("unarchive") { try client?.unarchiveSession(chatId: id) } }
    func move(_ id: String, toSection section: String?) { attempt("assign") { try client?.assignSection(chatId: id, sectionId: section) } }
    func movePin(_ id: String, after: String?, before: String?) {
        attempt("move pin") { try client?.movePin(chatId: id, after: after, before: before) }
    }
    func renameSection(_ id: String, _ name: String) { attempt("rename section") { try client?.renameSection(sectionId: id, name: name) } }
    func deleteSection(_ id: String) { attempt("delete section") { try client?.deleteSection(sectionId: id) } }
    func createSection(_ name: String) { attempt("section") { _ = try client?.createSection(name: name) } }
    func rename(_ id: String, _ title: String) { attempt("rename") { try client?.renameSession(chatId: id, title: title) } }
    func markSeen(_ id: String) { attempt("seen") { try client?.markSeen(chatId: id) } }

    // MARK: Sessions

    func sessionSource(_ chatId: String) -> SessionSource? {
        guard let client else { return nil }
        do {
            let handle = try client.openSession(chatId: chatId)
            return CoreSessionSource(app: self, client: client, handle: handle, chatId: chatId)
        } catch {
            NSLog("open session failed: \(error)")
            return nil
        }
    }

    // MARK: New session

    /// Who's signed in, by name (never the WorkOS user / org ids).
    var accountName: String {
        if isDemo { return "Demo" }
        guard client != nil else { return "Signed out" }
        let p = AccountProfile.load()
        return p.name ?? p.email ?? "Signed in"
    }

    var accountDetail: String {
        if isDemo { return "Offline demo workspace" }
        let p = AccountProfile.load()
        return [p.name != nil ? p.email : nil, p.orgName].compactMap { $0 }.joined(separator: " · ").nonEmpty ?? "Zeron account"
    }

    /// Logins from before profiles were saved: recover the org name (the
    /// user's name comes back at the next sign-in).
    private func backfillProfile() {
        guard case let .workOs(_, orgId, tokens)? = Credentials.stored(), AccountProfile.load().orgName == nil else { return }
        let edge = Self.edgeURL
        Task { @MainActor [weak self] in
            guard let orgs = try? await authListOrgs(edgeUrl: edge, accessToken: tokens.accessToken),
                  let org = orgs.first(where: { $0.organizationId == orgId })
            else { return }
            var p = AccountProfile.load()
            p.orgName = org.name
            p.save()
            self?.observers.values.forEach { $0() }
        }
    }

    var projectOptions: [ProjectOption] {
        rawProjects.map(projectOption)
    }

    func projectDeletionSummary(_ id: String) -> ProjectDeletionSummary? {
        client?.projectDeletionSummary(spaceId: id)
    }

    func renameProject(_ id: String, name: String) throws {
        guard let client else { throw ProjectActionError.unavailable }
        try client.renameProject(spaceId: id, name: name)
        refreshWorkspace()
    }

    func deleteProject(_ id: String) throws {
        guard let client else { throw ProjectActionError.unavailable }
        guard client.projectDeletionSummary(spaceId: id) != nil else { throw ProjectActionError.removed }
        try client.deleteProject(spaceId: id)
        refreshWorkspace()
    }

    enum ProjectActionError: LocalizedError {
        case unavailable, removed
        var errorDescription: String? {
            switch self {
            case .unavailable: "The workspace is unavailable. Try again after signing in."
            case .removed: "This project has already been removed."
            }
        }
    }

    private func projectOption(_ p: ProjectView) -> ProjectOption {
        ProjectOption(id: p.id, name: p.name, path: p.path, device: p.deviceId, deviceName: p.deviceName ?? deviceName(p.deviceId), online: p.deviceOnline, git: p.gitDetected, groupKey: p.groupKey, groupName: p.groupName, colorIndex: Int(p.colorIndex))
    }

    var hostOptions: [HostOption] {
        (client?.executionDevices() ?? []).map { HostOption(id: $0.id, name: $0.name, online: $0.online) }
    }

    /// A device's name; never its id.
    func deviceName(_ id: String) -> String {
        client?.devices().first { $0.id == id }?.name ?? "Unknown device"
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

    func listFolders(deviceId: String, path: String?) async -> FolderListing? {
        try? await client?.listFolders(deviceId: deviceId, path: path)
    }

    @MainActor
    func createProject(deviceId: String, path: String, gitDetected: Bool) async -> String? {
        guard let client else { return nil }
        do {
            let id = try await client.createProject(deviceId: deviceId, path: path, gitDetected: gitDetected)
            refreshWorkspace()
            return id
        } catch {
            NSLog("create project failed: \(error)")
            return nil
        }
    }

    func searchFiles(deviceId: String, spaceId: String, query: String) async -> [FileMatch] {
        guard let client else { return [] }
        return (try? await client.searchFiles(deviceId: deviceId, chatId: nil, spaceId: spaceId, query: query)) ?? []
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
            let actualScope: SessionScope = draft.projectId.map { .project(projectId: $0) } ?? .projectless
            if sessionScope != .all, sessionScope != actualScope { setSessionScope(actualScope) }
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
    var userId: String {
        switch self {
        case let .workOs(userId, _, _), let .dev(userId, _): return userId
        case .demo: return "demo"
        }
    }

    var orgId: String {
        switch self {
        case let .workOs(_, orgId, _), let .dev(_, orgId): return orgId
        case .demo: return "demo"
        }
    }

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

/// Display identity of the signed-in account (Keychain, next to the
/// credentials): the client itself only knows ids.
struct AccountProfile: Codable {
    var name: String?
    var email: String?
    var orgName: String?

    private static let service = "sh.zeron.ios"
    private static let account = "profile"

    static func load() -> AccountProfile {
        Keychain.load(service: service, account: account).flatMap { try? JSONDecoder().decode(AccountProfile.self, from: $0) } ?? AccountProfile()
    }

    func save() {
        if let data = try? JSONEncoder().encode(self) { Keychain.save(data, service: Self.service, account: Self.account) }
    }

    static func clear() {
        Keychain.delete(service: service, account: account)
    }
}

extension String {
    var nonEmpty: String? { isEmpty ? nil : self }
}
