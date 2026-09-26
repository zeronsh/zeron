import UIKit

/// App-wide state owner. Maps core snapshots into view models and fans change
/// notifications out to screens (main thread only).
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

    private(set) var frontPage = FrontPage()
    private(set) var projects: [ProjectVM] = []
    private(set) var pullRequests: [PullRequestGroup] = []
    private var observers: [UUID: () -> Void] = [:]

    init() {
        loadFixture()
    }

    @discardableResult
    func observe(_ handler: @escaping () -> Void) -> AnyObject {
        let id = UUID()
        observers[id] = handler
        return Token { [weak self] in self?.observers[id] = nil }
    }

    private final class Token {
        let cancel: () -> Void
        init(cancel: @escaping () -> Void) { self.cancel = cancel }
        deinit { cancel() }
    }

    private func notify() { observers.values.forEach { $0() } }

    func session(_ id: String) -> SessionRowVM? {
        frontPage.sessions.first { $0.id == id }
            ?? frontPage.sectionSessions.values.lazy.flatMap { $0 }.first { $0.id == id }
    }

    // MARK: Writes (wired to the core)

    func setPinned(_ id: String, _ pinned: Bool) {}
    func archive(_ id: String) {}
    func move(_ id: String, toSection section: String?) {}
    func createSection(_ name: String) {}
    func rename(_ id: String, _ title: String) {}
    func markSeen(_ id: String) {}
    var accountName: String { "Demo" }
    var accountDetail: String { "Offline demo workspace" }
    var onSignOut: (() -> Void)?
    func signOut() { onSignOut?() }
    var onSignedIn: (() -> Void)?
    private(set) var isSignedIn = !ProcessInfo.processInfo.arguments.contains("-signedout")
    func enterDemo() {
        isSignedIn = true
        onSignedIn?()
    }
    func signIn(code: String) async throws {
        isSignedIn = true
        onSignedIn?()
    }
    func signOutLocally() { isSignedIn = false }
    func didEnterBackground() {}
    func willEnterForeground() {}

    // MARK: New session

    var lastDraft = NewSessionDraft()

    var projectOptions: [ProjectOption] {
        projects.map { ProjectOption(id: $0.id, name: $0.name, device: $0.device, online: true, git: true, colorIndex: $0.colorIndex) }
    }

    var hostOptions: [HostOption] { [HostOption(id: "wing-mbp", name: "wing-mbp", online: true)] }

    func models(for deviceId: String) async -> [ModelChoice] {
        [
            ModelChoice(harness: "claude-code", harnessLabel: "Claude Code", id: "opus", label: "Opus 4.5", efforts: ["low", "medium", "high", "max"]),
            ModelChoice(harness: "claude-code", harnessLabel: "Claude Code", id: "sonnet", label: "Sonnet 4.5", efforts: ["low", "medium", "high"]),
            ModelChoice(harness: "codex", harnessLabel: "Codex", id: "gpt-5-codex", label: "GPT-5 Codex", efforts: ["low", "medium", "high"]),
        ]
    }

    func refs(projectId: String) async -> [String] { ["main", "ios-rewrite", "release/0.2"] }

    func createSession(draft: NewSessionDraft, text: String, images: [StagedImage]) -> String? {
        frontPage.sessions.first?.id
    }

    func sessionSource(_ chatId: String) -> SessionSource {
        let vm = session(chatId)
        return FixtureSessionSource(title: vm?.title ?? "Session", subtitle: vm.map { "\($0.projectName) @ wing-mbp" } ?? "")
    }

    /// Front-page preference: pinned sessions as a folder or inline at top.
    var pinnedInline: Bool {
        get { UserDefaults.standard.bool(forKey: "pinnedInline") }
        set {
            UserDefaults.standard.set(newValue, forKey: "pinnedInline")
            notify()
        }
    }

    func sessions(inFolder id: String) -> [SessionRowVM] {
        frontPage.sectionSessions[id] ?? []
    }

    func search(_ query: String) -> [SessionRowVM] {
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return frontPage.sessions }
        return frontPage.sessions.filter { $0.title.lowercased().contains(q) || $0.projectName.lowercased().contains(q) }
    }

    // MARK: Temporary fixture until the core's demo mode lands

    private func loadFixture() {
        func row(_ id: String, _ title: String, _ project: String, _ color: Int, _ status: SessionRowVM.Status, _ time: String, pr: SessionRowVM.PR? = nil, n: UInt64? = nil, harness: String = "claude-code", unseen: Bool = false, pinned: Bool = false) -> SessionRowVM {
            SessionRowVM(id: id, title: title, projectName: project, colorIndex: color, harness: harness, branch: "main", pr: pr, prNumber: n, status: status, timeLabel: time, unseen: unseen, pinned: pinned, sendFailed: false)
        }
        let sessions = [
            row("s1", "Inbox, Threads and Pulls screens", "mobile", 5, .working, "", unseen: true),
            row("s2", "Approve the staging migration", "infra", 6, .awaiting, "6h", harness: "codex"),
            row("s3", "Status glyph with the dot grid", "mobile", 5, .errored, "", pr: .open, n: 49376),
            row("s4", "Native sheet presenter", "mobile", 0, .completed, "7h", pr: .draft, n: 48514),
            row("s5", "Tab bar in UIKit with a search circle", "mobile", 3, .idle, "8h", pr: .merged, n: 48417, unseen: true),
            row("s6", "Mobile design rules", "design", 2, .idle, "9h"),
            row("s7", "Move thread status into app-client", "core", 1, .idle, "10h", pr: .open, n: 48434, harness: "codex"),
            row("s8", "Component inventory for the phone", "design", 4, .idle, "1d"),
            row("s9", "Nightly dependency audit", "infra", 6, .idle, "1d", harness: "cursor"),
            row("s10", "Phase 1 UI review", "mobile", 5, .idle, "2d"),
            row("s11", "List base: sidebar rows", "core", 1, .idle, "4d"),
        ]
        frontPage = FrontPage(
            folders: [
                FolderRowVM(id: "pinned", name: "Pinned", count: 2, symbol: "pin"),
                FolderRowVM(id: "p0", name: "P0", count: 2, symbol: "folder"),
                FolderRowVM(id: "mobile", name: "Mobile", count: 6, symbol: "folder"),
            ],
            sessions: sessions,
            sectionSessions: ["pinned": Array(sessions.prefix(2)), "p0": Array(sessions[2...3]), "mobile": sessions.filter { $0.projectName == "mobile" }]
        )
        let byProject = Dictionary(grouping: sessions, by: \.projectName)
        projects = byProject.keys.sorted().map { name in
            let list = byProject[name]!
            return ProjectVM(id: name, name: name, device: "wing-mbp", colorIndex: list[0].colorIndex, status: list.contains { $0.status == .working } ? .working : .idle, unseen: list.filter(\.unseen).count, timeLabel: list[0].timeLabel, sessions: list)
        }
        pullRequests = [
            PullRequestGroup(title: "Open", sessions: sessions.filter { $0.pr == .open }),
            PullRequestGroup(title: "Draft", sessions: sessions.filter { $0.pr == .draft }),
            PullRequestGroup(title: "Merged", sessions: sessions.filter { $0.pr == .merged }),
        ]
    }
}

class PlaceholderViewController: UIViewController {
    init(app: AppModel, title: String) {
        super.init(nibName: nil, bundle: nil)
        self.title = title
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
    }
}

