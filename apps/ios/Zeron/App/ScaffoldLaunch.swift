import UIKit

/// The desktop's Scaffold destinations on the phone: a new session in a new
/// Scaffold sandbox, created and attached by one of the user's desktop
/// controllers (the phone never holds execution credentials). The Rust core
/// owns the protocol (`crewLaunch`): it journals each command before sending
/// it, reconciles an unknown outcome instead of replaying it, and sends the
/// first message once the sandbox's host is online.
struct ScaffoldLaunch: Equatable {
    let database: CrewDatabase
    let text: String
    var phase: CrewLaunchPhase
}

/// The screen showing a launch's outcome (the new-session page).
@MainActor
protocol ScaffoldLaunchPresenter: AnyObject {
    func scaffoldLaunchFinished(chatId: String?, error: String?)
}

extension CrewDatabase {
    static let all: [CrewDatabase] = [.local, .stagingSnapshot, .productionSnapshot]

    /// How a draft stores it (the `ScaffoldDatabaseEnvironment` wire name).
    var key: String {
        switch self {
        case .local: "local"
        case .stagingSnapshot: "staging_snapshot"
        case .productionSnapshot: "production_snapshot"
        }
    }

    init?(key: String?) {
        guard let match = Self.all.first(where: { $0.key == key }) else { return nil }
        self = match
    }

    /// "Scaffold (Local DB)": the desktop device picker's label.
    var label: String { crewDatabaseLabel(database: self) }

    /// Its row under the "Run on" menu's sandbox section.
    var shortLabel: String {
        switch self {
        case .local: "Local DB"
        case .stagingSnapshot: "Staging DB"
        case .productionSnapshot: "Production DB"
        }
    }
}

extension CrewLaunchPhase {
    var progressLabel: String {
        switch self {
        case .creating: "Creating sandbox…"
        case .starting: "Starting sandbox…"
        case .connecting: "Connecting to sandbox…"
        }
    }
}

/// Core progress arrives on a Rust thread; hop to main.
private final class LaunchProgress: CrewLaunchProgress, @unchecked Sendable {
    private let update: (CrewLaunchPhase) -> Void

    init(_ update: @escaping (CrewLaunchPhase) -> Void) { self.update = update }

    func onPhase(phase: CrewLaunchPhase) {
        DispatchQueue.main.async { [update] in update(phase) }
    }
}

extension AppModel {
    /// Crew accounts start sandboxes; the offline demo can't.
    var offersScaffold: Bool {
        #if DEBUG
        // Screenshots: `-scaffold-destinations` lists them in the demo too
        // (no controller answers there, so no models load).
        if isDemo, ProcessInfo.processInfo.arguments.contains("-scaffold-destinations") { return true }
        #endif
        return isSignedIn && !isDemo && Endpoints.deploymentId != nil
    }

    /// The desktop controller sandboxes start through: one of this account's
    /// online Crew computers, found over the relay and kept for a minute.
    @MainActor
    func scaffoldControllerId() async -> String? {
        if let cached = scaffoldController, Date().timeIntervalSince(cached.at) < 60,
           hostOptions.contains(where: { $0.id == cached.id && $0.online }) {
            return cached.id
        }
        guard offersScaffold, let client, let deployment = Endpoints.deploymentId else { return nil }
        let id = try? await client.crewLaunchController(deployment: deployment)
        guard self.client === client else { return nil }
        scaffoldController = id.map { ($0, Date()) }
        return id
    }

    /// The managed models a sandbox runs, from the controller's own catalog
    /// (OMP and Antigravity routes, the desktop's filter).
    @MainActor
    func scaffoldModels() async -> [ModelChoice] {
        guard let client, let controller = await scaffoldControllerId() else { return [] }
        var out: [ModelChoice] = []
        for h in await client.listHarnesses(deviceId: controller) where h.offered && ["omp", "antigravity"].contains(h.id) {
            let models = await client.listModels(deviceId: controller, harness: h.id)
            out += models.filter { crewManagedModel(harness: h.id, model: $0.id) }.map {
                ModelChoice(harness: h.id, harnessLabel: h.label, id: $0.id, label: $0.label, efforts: $0.reasoningLevels, detail: $0.description)
            }
        }
        return out
    }

    /// Start the session in the background: it outlives the new-session
    /// page. The outcome goes to `scaffoldLaunchPresenter`, else a toast.
    @MainActor
    func launchScaffoldSession(database: CrewDatabase, config: ChatConfig, text: String, images: [StagedImage]) {
        guard scaffoldLaunch == nil, let client, let deployment = Endpoints.deploymentId else { return }
        scaffoldLaunch = ScaffoldLaunch(database: database, text: text, phase: .creating)
        scaffoldFailure = nil
        let first = SendRequest(text: text, attachments: images.map(\.outgoing), worktree: nil, busy: .queue)
        let progress = LaunchProgress { [weak self] phase in self?.scaffoldLaunch?.phase = phase }
        noteDeliveryActivity()
        Task { @MainActor [weak self] in
            var chatId: String?
            var failure: String?
            do {
                chatId = try await client.crewLaunch(database: database, config: config, first: first, deployment: deployment, progress: progress)
            } catch {
                failure = error.localizedDescription
            }
            guard let self, self.client === client else { return }
            self.scaffoldLaunch = nil
            if chatId != nil {
                self.noteDeliveryActivity()
                self.workspaceDidChange()
                // Kept as the page's draft when it closed mid-launch: sent now.
                if self.newSessionText == text {
                    self.newSessionText = ""
                    self.newSessionImages = []
                }
            }
            if let presenter = self.scaffoldLaunchPresenter {
                presenter.scaffoldLaunchFinished(chatId: chatId, error: failure)
                return
            }
            let window = UIApplication.shared.connectedScenes.compactMap { ($0 as? UIWindowScene)?.keyWindow }.first
            let router = window?.rootViewController as? AppRouter
            if let chatId {
                Toast.show("Sandbox session started", action: "Open", in: window) { router?.openSession(chatId) }
            } else {
                self.scaffoldFailure = failure
                Toast.show("The sandbox didn't start", action: "Details", in: window) { router?.presentNewSession(prompt: nil) }
            }
        }
    }

    /// The launch still owed by a desktop controller (it resumes on the
    /// next send), if any.
    var retainedScaffoldLaunch: CrewRetainedLaunch? {
        try? client?.crewLaunchRetained()
    }

    /// Stop waiting on it; a sandbox it created stays in Scaffold sandboxes.
    func forgetScaffoldLaunch() {
        do { try client?.crewLaunchForget() } catch { NSLog("forget sandbox launch failed: \(error)") }
    }
}
