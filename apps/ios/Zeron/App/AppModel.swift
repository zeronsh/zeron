import Network
import CryptoKit
import Security
import UIKit
#if DEBUG && targetEnvironment(simulator)
import Darwin
#endif

/// App-wide state owner: holds the Rust `CoreClient`, maps its snapshots
/// into view models, and fans change notifications out to screens. All
/// members are main-thread; core events hop here through `ListenerBridge`.
final class AppModel {
    struct FrontPage: Equatable {
        var folders: [FolderRowVM] = []
        var sessions: [SessionRowVM] = []
        var sectionSessions: [String: [SessionRowVM]] = [:]
        /// `sessions` grouped by repository project, groups in recency order.
        var projectGroups: [ProjectGroupVM] = []
    }

    struct ProjectGroupVM: Equatable {
        /// Section id: `project:<group key>` (`home:<device>` without a project).
        let id: String
        var name: String
        var sessions: [SessionRowVM]
    }

    /// The Sessions page groups its unsectioned sessions by project
    /// (the desktop sidebar's "By project"). Local to this phone.
    var groupByProject: Bool {
        get { UserDefaults.standard.bool(forKey: "groupByProject") }
        set {
            guard newValue != groupByProject else { return }
            UserDefaults.standard.set(newValue, forKey: "groupByProject")
            observers.values.forEach { $0() }
        }
    }

    struct LiveCounts: Equatable {
        var working = 0
        var awaiting = 0
    }

    @MainActor lazy var voice = RemoteVoiceController(app: self)
    /// Every chat's terminals (shells on their hosts).
    @MainActor lazy var terminals = TerminalStore(app: self)
    /// Retained for the shared upstream ABI. Crew does not expose Cloud,
    /// billing, or WorkOS product surfaces.
    @MainActor lazy var cloud = CloudStore(app: self)
    private(set) var client: CoreClient?
    private(set) var frontPage = FrontPage()
    private(set) var archived: [SessionRowVM] = []
    private(set) var connectivity: Connectivity?
    /// Front-page sessions that are working / waiting on the user.
    private(set) var live = LiveCounts()
    private var rows: [String: SessionRow] = [:]
    private var rawProjects: [ProjectView] = []
    private var lastHosts: [HostOption] = []
    private var workspaceRevision: UInt64 = 0
    private var observers: [UUID: () -> Void] = [:]
    private var sessionObservers: [String: [UUID: () -> Void]] = [:]
    private lazy var bridge = ListenerBridge(app: self)
    private var refreshScheduled = false
    private var clock: Timer?
    private let path = NWPathMonitor()
    /// The network path's shape last seen (interfaces in order), so only a
    /// real change re-pairs direct connections.
    private var pathShape: String?
    private var profileKey: String?
    private(set) var restoreError: String?
    private var restoreInFlight = false
    private var authGeneration: UInt64 = 0
    private var restoreSuppressed = false
    var crewStorageKey: String? { profileKey }

    var onSignedIn: (() -> Void)?
    var onSignOut: (() -> Void)?
    /// The new-session page's options (project, host, branch, model,
    /// effort), kept across launches.
    var lastDraft: NewSessionDraft = {
        var d = NewSessionDraft()
        // `-harness mock`: live-stack tests must never start a real agent.
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-harness"), i + 1 < args.count { d.harness = args[i + 1] }
        return d
    }() {
        didSet {
            guard Self.persistsNewSession, let profileKey else { return }
            UserDefaults.standard.set(try? JSONEncoder().encode(lastDraft), forKey: "newSessionDraft.\(profileKey)")
        }
    }

    /// The demo (and the UI tests on it) keep the new-session page in memory
    /// only: one run's leftovers must not seed the next.
    private static let persistsNewSession = !ProcessInfo.processInfo.arguments.contains("-demo")
    private var volatileNewSessionText = ""

    /// What was typed on the new-session page when it was closed unsent.
    var newSessionText: String {
        get { Self.persistsNewSession ? Drafts.load(Self.newSessionDraftKey) : volatileNewSessionText }
        set {
            if Self.persistsNewSession { Drafts.save(Self.newSessionDraftKey, newValue) } else { volatileNewSessionText = newValue }
        }
    }

    /// Images staged there (this launch only).
    var newSessionImages: [StagedImage] = []

    private static let newSessionDraftKey = "new-session"

    /// The new session starting in a Scaffold sandbox, if any (one at a
    /// time; see ScaffoldLaunch.swift).
    var scaffoldLaunch: ScaffoldLaunch? {
        didSet { if scaffoldLaunch != oldValue { observers.values.forEach { $0() } } }
    }
    /// The desktop controller sandboxes start through, as last resolved.
    var scaffoldController: (id: String, at: Date)?
    /// Shows a finished launch's outcome; without one, a toast does.
    weak var scaffoldLaunchPresenter: ScaffoldLaunchPresenter?
    /// Why the last launch failed, kept for the new-session page when it
    /// finished while that page was closed.
    var scaffoldFailure: String?

    var isSignedIn: Bool { client != nil }
    var isDemo: Bool { client?.isDemo() ?? false }

    init() {
        // Online/offline + interface changes cut sync backoff short.
        path.pathUpdateHandler = { [weak self] p in
            // What a direct connection's path depends on: the interfaces in
            // use and their order (Wi-Fi first, or cellular), and whether
            // there's a route at all. Re-delivered paths with the same shape
            // must not tear down a healthy direct link.
            let shape = p.status == .satisfied
                ? p.availableInterfaces.map { "\($0.type)/\($0.name)" }.joined(separator: ",") + (p.isExpensive ? "!" : "")
                : ""
            DispatchQueue.main.async {
                guard let self else { return }
                self.client?.setNetworkOnline(online: p.status == .satisfied)
                if let last = self.pathShape, last != shape, !shape.isEmpty {
                    self.client?.networkPathChanged()
                }
                self.pathShape = shape
                if p.status == .satisfied {
                    self.restoreSavedAccount()
                    if self.isSignedIn, !self.isDemo {
                        PushNotifications.shared.signedIn()
                    }
                }
            }
        }
        path.start(queue: DispatchQueue(label: "sh.zeron.path"))
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
        } else {
            Task { @MainActor in restoreSavedAccount() }
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

    static var profilesDir: URL {
        FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
            .appendingPathComponent("profiles/scaffold", isDirectory: true)
    }

    @discardableResult
    private func start(_ credentials: Credentials) -> Bool {
        let support = FileManager.default.urls(for: .applicationSupportDirectory, in: .userDomainMask)[0]
        let dir: URL
        if credentials.isDemo {
            dir = support.appendingPathComponent("demo", isDirectory: true)
        } else if let profileKey {
            dir = Self.profilesDir.appendingPathComponent(profileKey, isDirectory: true)
        } else if case let .dev(userId, orgId) = credentials {
            let identity = SHA256.hash(data: Data("\(Self.edgeURL)/\(userId)/\(orgId)".utf8))
                .map { String(format: "%02x", $0) }.joined()
            dir = Self.profilesDir.appendingPathComponent(identity, isDirectory: true)
        } else {
            return false
        }
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
            if case .scaffold = credentials, !Credentials.store(credentials) {
                throw CoreError.Storage(message: "Could not save Crew credentials to Keychain.")
            }
            refreshWorkspace()
            client?.preloadSessions()
            Drafts.profile = profileKey ?? (credentials.isDemo ? "demo" : dir.lastPathComponent)
            if let profileKey {
                lastDraft = UserDefaults.standard.data(forKey: "newSessionDraft.\(profileKey)")
                    .flatMap { try? JSONDecoder().decode(NewSessionDraft.self, from: $0) } ?? NewSessionDraft()
            }
            clock?.invalidate()
            // Relative times ("4m") and 45s staleness age without events.
            clock = Timer.scheduledTimer(withTimeInterval: 30, repeats: true) { [weak self] _ in self?.refreshWorkspace() }
            return true
        } catch {
            NSLog("core start failed: \(error)")
            client?.shutdown()
            client = nil
            return false
        }
    }

    /// `-edge <url>` points at a local development edge; otherwise Crew Staging.
    static var edgeURL: String {
        #if DEBUG
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: "-edge"), i + 1 < args.count { return args[i + 1] }
        #endif
        return Endpoints.edgeURL?.absoluteString ?? ""
    }

    static var deviceId: String {
        if let id = UserDefaults.standard.string(forKey: "deviceId"), !id.isEmpty { return id }
        let id = "ios-" + UUID().uuidString.lowercased().prefix(8)
        UserDefaults.standard.set(id, forKey: "deviceId")
        return id
    }

    @MainActor func cancelPendingRestore() {
        authGeneration &+= 1
        restoreSuppressed = true
    }

    @MainActor private func restoreSavedAccount() {
        guard client == nil, !restoreInFlight, !restoreSuppressed else { return }
        var sessionJson: String?
        if case let .scaffold(_, _, _, saved)? = Credentials.stored() { sessionJson = saved }
        #if DEBUG && targetEnvironment(simulator)
        if ProcessInfo.processInfo.arguments.contains("-import-staging-session") {
            // Consume once; path changes must not retry or fall back to another account.
            restoreSuppressed = true
            do {
                sessionJson = try Self.consumeSimulatorSession(at: FileManager.default.temporaryDirectory
                    .appendingPathComponent("crew-scaffold-session.json"))
            } catch {
                restoreError = error.localizedDescription
                observers.values.forEach { $0() }
                return
            }
        }
        #endif
        guard let sessionJson else { return }
        let discardRejectedCredential = !restoreSuppressed
        restoreInFlight = true
        let generation = authGeneration
        Task { @MainActor in
            defer { restoreInFlight = false }
            do {
                let account = try await Endpoints.login().restore(sessionJson: sessionJson)
                guard generation == authGeneration, client == nil else { return }
                try await signIn(account: account)
            } catch {
                guard generation == authGeneration else { return }
                restoreError = error.localizedDescription
                // Rejecting an import must not discard a previously saved account.
                if case CoreError.Auth = error, discardRejectedCredential { Credentials.clearStored() }
                observers.values.forEach { $0() }
            }
        }
    }

    #if DEBUG && targetEnvironment(simulator)
    static func consumeSimulatorSession(at url: URL) throws -> String {
        let fd = open(url.path, O_RDONLY | O_NOFOLLOW | O_NONBLOCK | O_CLOEXEC)
        guard fd >= 0 else { throw CoreError.Storage(message: "Cannot open the private simulator session file.") }
        let file = FileHandle(fileDescriptor: fd, closeOnDealloc: true)
        defer { try? file.close() }
        var info = stat()
        guard fstat(fd, &info) == 0, info.st_mode & S_IFMT == S_IFREG else {
            throw CoreError.Storage(message: "Simulator session import requires a regular file.")
        }
        // Remove the transfer file before parsing, network access or Keychain storage.
        guard unlink(url.path) == 0 else { throw CoreError.Storage(message: "Cannot remove the simulator session transfer file.") }
        guard info.st_uid == getuid(), info.st_nlink == 1, info.st_mode & 0o777 == 0o600,
              info.st_size > 0, info.st_size <= 65_536 else {
            throw CoreError.Storage(message: "Simulator session file must be private (0600), owned, unlinked and at most 64 KB.")
        }
        guard let data = try file.read(upToCount: 65_537), data.count == info.st_size,
              let session = String(data: data, encoding: .utf8) else {
            throw CoreError.Storage(message: "Simulator session file is incomplete or not UTF-8.")
        }
        return session
    }
    #endif

    @MainActor func signIn(account: CrewAccount) async throws {
        if let client { PushNotifications.shared.signingOut(client: client) }
        client?.shutdown()
        client = nil
        // The legacy app's `CometDocs` cache stays untouched and is not
        // imported: this rollout is fresh-only, and the importer holds the
        // whole cache in memory, so phones updated in place were killed at
        // their memory limit right after sign-in. Sessions re-sync from their
        // hosts. `crewPrepareLegacyBackup` remains for an authorized migration.
        profileKey = account.profileKey
        AccountProfile(name: account.name, email: account.email, orgName: account.projectScope).save()
        guard start(account.credentials) else {
            throw CoreError.Storage(message: "Could not open this Crew profile. Its local data was retained.")
        }
        authGeneration &+= 1
        restoreError = nil
        onSignedIn?()
    }

    /// Signing out forgets credentials, never cached-only history or an outbox.
    @MainActor func signOut() {
        voice.stop()
        onSignOut?()
    }

    @MainActor func signOutLocally() {
        PushNotifications.shared.signingOut(client: client)
        authGeneration &+= 1
        voice.stop()
        terminals.signOut()
        endDeliveryBackgroundTask()
        lastDeliveryActivity = .distantPast
        client?.shutdown()
        client = nil
        Credentials.clearStored()
        AccountProfile.clear()
        clock?.invalidate()
        profileKey = nil
        Drafts.profile = "signed-out"
        // Nothing of the account stays in memory.
        frontPage = FrontPage()
        archived = []
        rows = [:]
        rawProjects = []
        lastHosts = []
        connectivity = nil
        live = LiveCounts()
        workspaceRevision = 0
        lastDraft = NewSessionDraft()
        newSessionText = ""
        newSessionImages = []
        scaffoldLaunch = nil
        scaffoldController = nil
        scaffoldFailure = nil
    }

    private var lastDeliveryActivity = Date.distantPast

    func noteDeliveryActivity() { lastDeliveryActivity = Date() }

    private var deliveryBackgroundTask: UIBackgroundTaskIdentifier = .invalid
    private var deliveryBackgroundDeadline: DispatchWorkItem?

    private func endDeliveryBackgroundTask() {
        deliveryBackgroundDeadline?.cancel()
        deliveryBackgroundDeadline = nil
        guard deliveryBackgroundTask != .invalid else { return }
        UIApplication.shared.endBackgroundTask(deliveryBackgroundTask)
        deliveryBackgroundTask = .invalid
    }

    func didEnterBackground() {
        // Give the already-durable room outbox and host wake time to reach
        // the edge when the user leaves immediately after tapping Send.
        endDeliveryBackgroundTask()
        if client != nil, !isDemo,
           Date().timeIntervalSince(lastDeliveryActivity) < 120 || rows.values.contains(where: { $0.sendState == .sending || $0.sendState == .queued }) {
            deliveryBackgroundTask = UIApplication.shared.beginBackgroundTask(withName: "Finish message delivery") { [weak self] in
                self?.endDeliveryBackgroundTask()
            }
            let deadline = DispatchWorkItem { [weak self] in self?.endDeliveryBackgroundTask() }
            deliveryBackgroundDeadline = deadline
            DispatchQueue.main.asyncAfter(deadline: .now() + 25, execute: deadline)
        }
        client?.onBackground()
    }
    func willEnterForeground() {
        endDeliveryBackgroundTask()
        client?.onForeground()
        refreshWorkspace()
        Task { @MainActor in restoreSavedAccount() }
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

    /// Something outside the registry stream changed the workspace (a Cloud
    /// project written here): rebuild now rather than on the next frame.
    func workspaceDidChange() { refreshWorkspace() }

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
        if !pinned.isEmpty {
            page.folders.append(FolderRowVM(id: "pinned", name: "Pinned", count: pinned.count, symbol: "pin"))
        }
        page.sectionSessions["pinned"] = pinned
        for s in ws.front.sections {
            let list = s.sessions.map(vm)
            page.folders.append(FolderRowVM(id: s.id, name: s.name, count: list.count, symbol: "folder"))
            page.sectionSessions[s.id] = list
        }
        page.sessions = ws.front.recent.map(vm)
        page.projectGroups = Self.projectGroups(ws.front.recent, page.sessions, projects: ws.projects)
        rawProjects = ws.projects
        // Sessions reachable only through their project still resolve by id.
        for p in ws.projects {
            for r in p.sessions where all[r.id] == nil { all[r.id] = r }
        }
        let archivedVMs = ws.archived.map(vm)
        var counts = LiveCounts()
        var seen = Set<String>()
        for row in page.sessions + page.sectionSessions.values.flatMap({ $0 }) where seen.insert(row.id).inserted {
            if row.status == .working { counts.working += 1 }
            if row.status == .awaiting { counts.awaiting += 1 }
        }
        // Open session chrome also reads fields from the row itself, so a
        // content revision is observable even when list membership is stable.
        let rowsChanged = all.count != rows.count || all.contains {
            rows[$0.key]?.revision != $0.value.revision
        }
        rows = all
        // Devices coming and going (Settings, host pickers) count as changes.
        let hosts = hostOptions
        let changed = rowsChanged || page != frontPage || archivedVMs != archived || counts != live || hosts != lastHosts
        lastHosts = hosts
        live = counts
        if !freshSideChats.isEmpty { sweepFreshSideChats() }
        frontPage = page
        archived = archivedVMs
        if changed { observers.values.forEach { $0() } }
    }

    /// Like the desktop's project grouping: checkouts of one repository
    /// share a group (named for its representative), project-less sessions
    /// group per device. Groups and rows keep the incoming recency order.
    private static func projectGroups(_ rows: [SessionRow], _ vms: [SessionRowVM], projects: [ProjectView]) -> [ProjectGroupVM] {
        let byId = Dictionary(projects.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        var groups: [ProjectGroupVM] = []
        var index: [String: Int] = [:]
        for (row, vm) in zip(rows, vms) {
            let id: String
            let name: String
            if let ref = row.project {
                let project = byId[ref.id]
                id = "project:" + (project?.groupKey ?? ref.id)
                name = project?.groupName ?? ref.name
            } else {
                id = "home:" + row.deviceId
                name = row.deviceName.map { "Home · \($0)" } ?? "Home"
            }
            if let i = index[id] {
                groups[i].sessions.append(vm)
            } else {
                index[id] = groups.count
                groups.append(ProjectGroupVM(id: id, name: name, sessions: [vm]))
            }
        }
        return groups
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

    func search(_ query: String) -> [SessionRowVM] {
        let q = query.trimmingCharacters(in: .whitespaces)
        guard let client, !q.isEmpty else { return frontPage.sessions }
        return client.search(query: q, limit: 60).map { Self.vm($0.session) }
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
    func delete(_ id: String) { attempt("delete") { try client?.deleteSession(chatId: id) } }
    func move(_ id: String, toSection section: String?) { attempt("assign") { try client?.assignSection(chatId: id, sectionId: section) } }
    func movePin(_ id: String, after: String?, before: String?) {
        attempt("move pin") { try client?.movePin(chatId: id, after: after, before: before) }
    }
    /// Pin between neighbouring rows (`nil` at either edge), preserving the
    /// exact drop position chosen in the Threads list.
    func pin(_ id: String, after: String?, before: String?) {
        attempt("pin") { try client?.pinSessionAt(chatId: id, after: after, before: before) }
    }
    /// Move into a custom section between its neighbouring rows.
    func place(_ id: String, inSection section: String, after: String?, before: String?) {
        attempt("assign") {
            try client?.placeSession(
                chatId: id,
                sectionId: section,
                after: after,
                before: before
            )
        }
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

    // MARK: Companions

    func sideChats(of chatId: String) -> [SessionRowVM] {
        (client?.childSessions(parentId: chatId) ?? [])
            .filter { !$0.archived }
            .map(Self.vm)
    }

    func sessionSubagents(_ chatId: String) -> [SubagentRow] {
        guard let client, !chatId.contains("--sub--") else { return [] }
        return (client.session(chatId: chatId) ?? (try? client.openSession(chatId: chatId)))?
            .subagents() ?? []
    }

    private(set) var freshSideChats = Set(
        UserDefaults.standard.stringArray(forKey: "freshSideChats") ?? []
    ) {
        didSet {
            UserDefaults.standard.set(Array(freshSideChats), forKey: "freshSideChats")
        }
    }

    func forgetFreshSideChat(_ id: String) {
        freshSideChats.remove(id)
    }

    private func sweepFreshSideChats() {
        let now = Int64(Date().timeIntervalSince1970 * 1000)
        for id in freshSideChats {
            guard let row = rows[id] ?? client?.sessionRow(chatId: id) else { continue }
            if row.preview != nil {
                freshSideChats.remove(id)
            } else if SessionViewController.live(id) == nil,
                      now - row.createdAtMs > 60_000 {
                freshSideChats.remove(id)
                delete(id)
            }
        }
    }

    func createSideChat(parentId: String) -> String? {
        do {
            let id = try client?.createSideChat(parentId: parentId)
            if let id { freshSideChats.insert(id) }
            refreshWorkspace()
            return id
        } catch {
            NSLog("side chat failed: \(error)")
            return nil
        }
    }

    func forkSideChat(sourceId: String) async throws -> String {
        guard let client else { throw CoreError.Closed }
        let id = try await client.forkSideChat(sourceId: sourceId)
        await MainActor.run { refreshWorkspace() }
        return id
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
        return [p.name != nil ? p.email : nil, p.orgName].compactMap { $0 }.joined(separator: " · ").nonEmpty ?? "Crew account"
    }


    var projectOptions: [ProjectOption] {
        rawProjects.map { ProjectOption(id: $0.id, name: $0.name, path: $0.path, device: $0.deviceId, deviceName: $0.deviceName ?? deviceName($0.deviceId), online: $0.deviceOnline, git: $0.gitDetected, groupKey: $0.groupKey, groupName: $0.groupName, colorIndex: Int($0.colorIndex)) }
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
            noteDeliveryActivity()
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

    private static let service = "ai.ashler.crew.staging"
    private static var account: String { "credentials.\(Endpoints.storageScope)" }

    static func stored() -> Credentials? {
        guard let data = Keychain.load(service: service, account: account),
              let dict = try? JSONSerialization.jsonObject(with: data) as? [String: String]
        else { return nil }
        switch dict["kind"] {
        case "scaffold":
            guard let u = dict["userId"], let o = dict["orgId"],
                  let origin = dict["controlOrigin"], let session = dict["session"] else { return nil }
            return .scaffold(userId: u, orgId: o, controlOrigin: origin, sessionJson: session)
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

    @discardableResult static func store(_ c: Credentials) -> Bool {
        var dict: [String: String] = [:]
        switch c {
        case let .scaffold(userId, orgId, controlOrigin, sessionJson):
            dict = ["kind": "scaffold", "userId": userId, "orgId": orgId,
                    "controlOrigin": controlOrigin, "session": sessionJson]
        case let .workOs(userId, orgId, tokens):
            dict = ["kind": "workos", "userId": userId, "orgId": orgId, "access": tokens.accessToken, "refresh": tokens.refreshToken]
        case let .dev(userId, orgId):
            dict = ["kind": "dev", "userId": userId, "orgId": orgId]
        case .demo:
            return true
        }
        guard let data = try? JSONSerialization.data(withJSONObject: dict) else { return false }
        return Keychain.save(data, service: service, account: account)
    }

    static func updateTokens(_ tokens: AuthTokens) {
        guard case let .workOs(userId, orgId, _)? = stored() else { return }
        store(.workOs(userId: userId, orgId: orgId, tokens: tokens))
    }

    static func clearStored() {
        Keychain.delete(service: service, account: account)
    }
}

enum Keychain {
    @discardableResult static func save(_ data: Data, service: String, account: String) -> Bool {
        var query: [String: Any] = [
            kSecClass as String: kSecClassGenericPassword,
            kSecAttrService as String: service,
            kSecAttrAccount as String: account,
        ]
        let attributes: [String: Any] = [
            kSecValueData as String: data,
            kSecAttrAccessible as String: kSecAttrAccessibleAfterFirstUnlockThisDeviceOnly,
        ]
        let result = SecItemUpdate(query as CFDictionary, attributes as CFDictionary)
        if result == errSecSuccess { return true }
        guard result == errSecItemNotFound else { return false }
        query.merge(attributes) { _, value in value }
        return SecItemAdd(query as CFDictionary, nil) == errSecSuccess
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

    private static let service = "ai.ashler.crew.staging"
    private static var account: String { "profile.\(Endpoints.storageScope)" }

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
