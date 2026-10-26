import UIKit

/// What a new session will be created with.
struct NewSessionDraft: Equatable, Codable {
    var projectId: String?
    /// Projectless sessions run on an explicit host.
    var hostId: String?
    var branch: String?
    var worktree = false
    var harness = "claude-code"
    var model: String?
    var effort: String?
    /// Run on its own Cloud machine (optional: drafts saved before Cloud).
    var cloud: Bool?
}

/// Options the pickers offer (from the core's workspace + host catalogs).
struct ProjectOption: Equatable {
    let id: String
    let name: String
    let path: String
    let device: String
    /// Display name of `device` (never show the id).
    let deviceName: String
    let online: Bool
    let git: Bool
    /// Shared by every checkout of one repository, on any device; with
    /// `groupName` and `colorIndex` they read as one project.
    let groupKey: String
    let groupName: String
    let colorIndex: Int
    /// Lives on Cloud alone.
    var isCloud = false
    /// `owner/name` of its GitHub repository, when known.
    var githubRepo: String?
}

struct HostOption: Equatable {
    let id: String
    let name: String
    let online: Bool
}

struct ModelChoice: Equatable {
    let harness: String
    let harnessLabel: String
    let id: String
    let label: String
    let efforts: [String]
    var detail: String? = nil
}

extension NewSessionDraft {
    /// A listed draft's destination and run options (before its content loads).
    init(row: DraftRow, fallback: NewSessionDraft) {
        self.init()
        projectId = row.spaceId
        hostId = row.spaceId == nil ? row.deviceId : nil
        branch = row.branch
        worktree = row.newWorktree
        harness = row.harness ?? fallback.harness
        model = row.model
        effort = row.reasoning
    }

    /// A prompt draft's destination and run options.
    init(target t: DraftTarget, fallback: NewSessionDraft) {
        self.init()
        projectId = t.spaceId
        // Projectless (or a project removed since): its host.
        hostId = t.spaceId == nil ? t.deviceId : nil
        branch = t.branch
        worktree = t.newWorktree
        harness = t.config?.harness ?? fallback.harness
        model = t.config?.model
        effort = t.config?.reasoning
    }
}

/// "Ask anything" → a composer-first canvas. Context is set with native menus
/// (project, branch/worktree, model, effort) that load lazily; the composer is
/// focused immediately so the common case is: tap, type, send.
///
/// The page is a prompt draft (docs/prompt-drafts.md): every change autosaves
/// through the core; leaving it unsent lists it under Drafts on every device,
/// and the next New Session starts empty. Opening a draft brings back its
/// text, images and options.
final class NewSessionViewController: UIViewController, UIGestureRecognizerDelegate {
    private let app: AppModel
    private let onCreated: (String, DraftHandoff?) -> Void
    private let composer = ComposerBar()
    private let hero = UILabel()
    private let wallpaper = WallpaperView()
    private let mark = UIImageView()
    private var draft: NewSessionDraft
    private var models: [ModelChoice] = []

    /// Embedded in the iPad split's main column (no sheet chrome).
    private let embedded: Bool
    private var composerRest: NSLayoutConstraint?
    private var backgroundObserver: NSObjectProtocol?

    deinit {
        if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver) }
        saveTimer?.invalidate()
    }
    /// Take the keyboard on appearing. The sheet always does; the iPad column
    /// only when opened on purpose (not the launch page).
    var focusOnAppear: Bool

    /// This page's prompt-draft identity.
    private struct Canvas {
        var id = UUID().uuidString.lowercased()
        var revision: String?
        var baseRevision: String?
        var createdAt = Int64(Date().timeIntervalSince1970 * 1000)
        /// Still the open canvas: saved, not listed.
        var deferred = true
        var savedDeferred = true
        /// Listed (here or elsewhere): sending reserves it first, since
        /// another device may send it too.
        var listed = false
        /// A send attempt reserved `revision`: an edit forks a new draft.
        var reserved = false
        /// What the saved revision holds.
        var saved: Content?
    }

    private struct Content: Equatable {
        var text: String
        var images: [String]
        var options: NewSessionDraft
    }

    private var canvas = Canvas()
    /// The draft being reopened, until its content is loaded.
    private var restoring: DraftRow?
    /// Opened from Drafts: its options don't become the next page's defaults.
    private var reopened = false
    /// Desktop attachments the phone can't stage (non-image files), kept
    /// with the draft and sent with it.
    private var keptFiles: [DraftFile] = []
    /// Desktop Appshot context of restored images, kept across saves.
    private var appshots: [String: String] = [:]
    private var saveTimer: Timer?
    /// Reserving a listed draft before sending it.
    private var sending = false

    var draftId: String { canvas.id }

    init(app: AppModel, prompt: String?, restoring row: DraftRow? = nil, embedded: Bool = false, onCreated: @escaping (String, DraftHandoff?) -> Void) {
        self.app = app
        self.embedded = embedded
        self.focusOnAppear = !embedded
        self.onCreated = onCreated
        self.draft = app.lastDraft
        super.init(nibName: nil, bundle: nil)
        if let row {
            restoring = row
            reopened = true
            canvas = Canvas(id: row.id, revision: row.revision, baseRevision: row.baseRevision, createdAt: row.createdAtMs, deferred: false, savedDeferred: false, listed: true)
            draft = NewSessionDraft(row: row, fallback: app.lastDraft)
        } else if let prompt {
            composer.text = prompt
        }
    }

    /// A session was created from this page: nothing to keep.
    private var created = false
    fileprivate var cloudStarting = false
    fileprivate func markCreated() {
        created = true
        saveTimer?.invalidate()
    }
    fileprivate var onCreatedHandler: (String, DraftHandoff?) -> Void { onCreated }

    /// Closed (swipe down, ✕, or replaced in the iPad column) without
    /// sending: the options seed the next page; the text lists as a draft.
    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        guard !created else { return }
        if !reopened { app.lastDraft = draft }
        saveDraft(listing: true)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        // The app can be killed in the background: save the draft first (a
        // page the app dies on lists at the next launch).
        backgroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            guard let self, !self.created else { return }
            if !self.reopened { self.app.lastDraft = self.draft }
            self.saveDraft(listing: false)
        }
        // Tapping the canvas puts the keyboard away (the card stays open here).
        let dismissTap = UITapGestureRecognizer(target: self, action: #selector(dismissKeyboard))
        dismissTap.cancelsTouchesInView = false
        dismissTap.delegate = self
        view.addGestureRecognizer(dismissTap)
        view.backgroundColor = Palette.background
        title = "New Session"
        if !embedded {
            navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in
                self?.dismiss(animated: true)
            })
        }

        mark.image = BrandMarks.image(for: draft.harness, side: 34)
        mark.tintColor = Palette.text
        mark.contentMode = .center
        hero.text = "What are we building?"
        hero.numberOfLines = 2
        hero.font = Fonts.ui(.sansSemibold, 22)
        hero.textColor = Palette.text
        hero.textAlignment = .center
        let heroStack = UIStackView(arrangedSubviews: [mark, hero])
        heroStack.axis = .vertical
        heroStack.spacing = 14
        heroStack.alignment = .center
        heroStack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(wallpaper)
        view.addSubview(heroStack)

        composer.translatesAutoresizingMaskIntoConstraints = false
        composer.placeholder = "Describe the task"
        composer.chipsAlwaysVisible = true
        composer.attachMenu = { [weak self] in
            guard let self else { return UIMenu() }
            return AttachmentPicker.menu(host: self, limit: 8 - self.composer.images.count) { [weak self] in self?.composer.addImages($0) }
        }
        // The model chip opens the picker sheet (tabs per provider).
        composer.onChipTap = { [weak self] id, _ in
            guard let self, id == "model" else { return }
            self.present(self.modelPicker(), animated: true)
        }
        composer.mentionSearch = { [weak self] q in
            guard let self, let p = self.project else { return [] }
            return await self.app.searchFiles(deviceId: p.device, spaceId: p.id, query: q)
        }
        composer.onSend = { [weak self] text, images, _ in
            self?.send(text: text, images: images) ?? .kept
        }
        composer.onEdit = { [weak self] in self?.scheduleSave() }
        view.addSubview(composer)
        heroStack.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24).isActive = true
        // The headline lives in the free space above the composer — from the
        // top of the page to the composer's top edge — centered there, so
        // it's always above the composer (keyboard up or down, one line or
        // two). On iPad the composer keeps the transcript's reading width.
        let free = UILayoutGuide()
        view.addLayoutGuide(free)
        heroStack.setContentCompressionResistancePriority(.required, for: .vertical)
        let fill = composer.widthAnchor.constraint(equalTo: view.safeAreaLayoutGuide.widthAnchor, constant: -24)
        fill.priority = .defaultHigh
        NSLayoutConstraint.activate([
            free.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            free.bottomAnchor.constraint(equalTo: composer.topAnchor, constant: -20),
            free.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            free.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            // Safe area, not view: insets can differ side to side (the
            // iPhone Duo's camera and status bar sit on one edge).
            heroStack.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            heroStack.centerYAnchor.constraint(equalTo: free.centerYAnchor),
            heroStack.topAnchor.constraint(greaterThanOrEqualTo: free.topAnchor),
            heroStack.bottomAnchor.constraint(lessThanOrEqualTo: free.bottomAnchor),
            composer.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            composer.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            composer.widthAnchor.constraint(lessThanOrEqualToConstant: 768),
            fill,
        ])
        composerRest = pinAboveKeyboard(composer)
        // A draft's project removed since: run where it was meant to.
        if let id = draft.projectId, !app.projectOptions.contains(where: { $0.id == id }), let row = restoring {
            draft.projectId = nil
            draft.hostId = row.deviceId
        }
        if draft.projectId == nil, draft.hostId == nil { draft.projectId = app.projectOptions.first?.id }
        if draft.projectId == nil, draft.hostId == nil {
            // No projects yet: run on the first reachable host.
            draft.hostId = app.hostOptions.first(where: \.online)?.id ?? app.hostOptions.first?.id
        }
        #if DEBUG
        // `-cloud-draft`: open on Cloud for the first project Cloud can run.
        if ProcessInfo.processInfo.arguments.contains("-cloud-draft"),
           let p = app.projectOptions.first(where: { $0.isCloud || $0.githubRepo != nil }) {
            draft.projectId = p.id
            draft.hostId = nil
            draft.cloud = true
        }
        #endif
        cloudToken = app.cloud.observe { [weak self] in self?.loadModels() }
        if app.cloud.info == nil { app.cloud.refresh() }
        loadModels()
        if let row = restoring { load(row) }
    }

    // MARK: Prompt draft

    /// Bring a draft's text and images back; the page stays read-only until
    /// they're in (an edit before that would fork from nothing).
    private func load(_ row: DraftRow) {
        composer.textView.isEditable = false
        composer.alpha = 0.5
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                let loaded = try await self.app.loadDraft(revision: row.revision)
                var images: [StagedImage] = []
                for file in loaded.files {
                    if let image = UIImage(data: file.data) {
                        images.append(StagedImage(id: file.id, name: file.name, data: file.data, thumbnail: image.preparingThumbnail(of: CGSize(width: 180, height: 180)) ?? image))
                        if let json = file.appshotJson { self.appshots[file.id] = json }
                    } else {
                        self.keptFiles.append(file)
                    }
                }
                self.draft = NewSessionDraft(target: loaded.target, fallback: self.draft)
                if let id = self.draft.projectId, !self.app.projectOptions.contains(where: { $0.id == id }) {
                    self.draft.projectId = nil
                    self.draft.hostId = loaded.target.deviceId
                }
                self.composer.text = loaded.prompt
                self.composer.images = images
                self.restoring = nil
                self.canvas.saved = self.content
                self.loadModels()
            } catch {
                self.restoring = nil
                self.canvas = Canvas()
                let alert = UIAlertController(title: "Couldn't open the draft", message: "It may have been sent or discarded on another device, or it hasn't reached this one yet.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                self.present(alert, animated: true)
            }
            self.composer.textView.isEditable = true
            self.composer.alpha = 1
        }
    }

    private var content: Content {
        Content(text: composer.text, images: composer.images.map(\.id), options: draft)
    }

    /// Autosave a moment after the last change.
    private func scheduleSave() {
        guard restoring == nil, !created else { return }
        saveTimer?.invalidate()
        saveTimer = Timer.scheduledTimer(withTimeInterval: 0.3, repeats: false) { [weak self] _ in
            self?.saveDraft(listing: false)
        }
    }

    private var target: DraftTarget {
        let project = self.project
        return DraftTarget(
            deviceId: deviceId,
            spaceId: project?.id,
            projectName: project?.groupName,
            config: ChatConfig(harness: draft.harness, model: draft.model, reasoning: draft.effort, modelOptions: [:], sandbox: .workspaceWrite),
            branch: draft.branch,
            newWorktree: draft.worktree
        )
    }

    /// Save what's on the page. `listing`: the page is being left, so the
    /// draft joins the Drafts list (for good: a later save can't hide it).
    /// Emptied, a saved draft is discarded.
    @discardableResult
    private func saveDraft(listing: Bool) -> Bool {
        saveTimer?.invalidate()
        guard restoring == nil, !created, !deviceId.isEmpty else { return false }
        let now = content
        if now.text.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty, now.images.isEmpty, keptFiles.isEmpty {
            if canvas.revision != nil {
                app.discardDraft(canvas.id)
                canvas = Canvas()
            }
            app.openCanvas = nil
            return false
        }
        let changed = now != canvas.saved
        if changed, canvas.reserved {
            // The reserved revision belongs to its send attempt.
            canvas = Canvas()
        }
        if listing { canvas.deferred = false }
        guard changed || canvas.deferred != canvas.savedDeferred else { return true }
        let revision = changed || canvas.revision == nil ? UUID().uuidString.lowercased() : canvas.revision!
        let base = changed ? canvas.revision : canvas.baseRevision
        let files = composer.images.map { DraftFile(id: $0.id, name: $0.name, data: $0.data, appshotJson: appshots[$0.id]) } + keptFiles
        let edit = DraftEdit(id: canvas.id, revision: revision, baseRevision: base, createdAtMs: canvas.createdAt, deferred: canvas.deferred, prompt: now.text, target: target, files: files)
        do {
            try app.saveDraft(edit)
        } catch {
            NSLog("draft save failed: \(error)")
            return false
        }
        canvas.revision = revision
        canvas.baseRevision = base
        canvas.saved = now
        canvas.savedDeferred = canvas.deferred
        canvas.listed = canvas.listed || !canvas.deferred
        app.openCanvas = canvas.deferred ? AppModel.OpenCanvas(id: canvas.id, revision: revision, baseRevision: base, createdAt: canvas.createdAt) : nil
        return true
    }

    private var cloudToken: AnyObject?

    /// The session will run on its own Cloud machine: a Cloud project, or
    /// Cloud picked for a project whose repository Cloud reaches.
    private var runsOnCloud: Bool {
        guard let p = project else { return false }
        return p.isCloud || (draft.cloud == true && app.cloud.reaches(p.githubRepo))
    }

    /// Cloud can run this project's sessions.
    private var cloudOffered: Bool {
        guard let p = project, app.cloud.available else { return false }
        return p.isCloud || app.cloud.reaches(p.githubRepo)
    }

    /// The last catalog each host reported, so the chip opens on a real model
    /// name while a fresh one comes back over the relay.
    private static var modelCache: [String: [ModelChoice]] = [:]
    private var modelsDevice: String?

    /// Models for the draft's host: cached (or the built-in catalog) at once,
    /// then the host's own list.
    private func loadModels() {
        let device = runsOnCloud ? Self.cloudModelsKey : deviceId
        guard device != modelsDevice else { return refreshChips() }
        modelsDevice = device
        let cloud = device == Self.cloudModelsKey
        if cloud {
            // Cloud runs the agents it has accounts for: Claude Code, Codex.
            models = Self.modelCache[device] ?? Self.catalogModels().filter { AppModel.cloudHarnesses.contains($0.harness) }
            if !models.contains(where: { $0.harness == draft.harness }), let first = models.first {
                draft.harness = first.harness
                draft.model = nil
                draft.effort = nil
            }
        } else {
            models = Self.modelCache[device] ?? Self.catalogModels()
        }
        refreshChips()
        Task { [weak self] in
            guard let self else { return }
            let fresh = cloud ? await self.app.cloudModels() : await self.app.models(for: device)
            guard !fresh.isEmpty else { return }
            Self.modelCache[device] = fresh
            guard self.modelsDevice == device else { return }
            self.models = fresh
            self.refreshChips()
        }
    }

    private static let cloudModelsKey = "\u{0}cloud"

    private static func catalogModels() -> [ModelChoice] {
        fallbackHarnesses().filter(\.offered).flatMap { h in
            fallbackModels(harness: h.id).map { ModelChoice(harness: h.id, harnessLabel: h.label, id: $0.id, label: $0.label, efforts: $0.reasoningLevels, detail: $0.description) }
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if focusOnAppear { composer.becomeFirstResponder() }
    }

    override func viewWillLayoutSubviews() {
        super.viewWillLayoutSubviews()
        composerRest?.constant = -composerRestGap
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        // Desktop hero: from the top, 72% of the viewport (≤ 760pt), the
        // artwork softened behind the composer.
        wallpaper.frame = CGRect(x: 0, y: 0, width: view.bounds.width, height: min(view.bounds.height * 0.72, 760))
        wallpaper.cutout = composer.convert(composer.bounds, to: wallpaper)
    }

    @objc private func dismissKeyboard() {
        view.endEditing(true)
    }

    func gestureRecognizer(_ g: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        touch.view.map { !$0.isDescendant(of: composer) } ?? true
    }

    private var project: ProjectOption? { app.projectOptions.first { $0.id == draft.projectId } }
    private var deviceId: String { project?.device ?? draft.hostId ?? "" }

    /// Checkouts of the picked project, one per row of the host menu.
    private var checkouts: [ProjectOption] {
        guard let p = project else { return [] }
        return app.projectOptions
            .filter { $0.groupKey == p.groupKey }
            .sorted { ($0.deviceName.lowercased(), $0.path) < ($1.deviceName.lowercased(), $1.path) }
    }

    private func refreshChips() {
        var chips: [ComposerChip] = []
        if let p = project, runsOnCloud {
            // On its own Cloud machine, from a branch of the repository.
            chips.append(ComposerChip(id: "project", title: p.groupName, symbol: nil, icon: ProjectTile.image(name: p.groupName, colorIndex: p.colorIndex)))
            chips.append(ComposerChip(id: "host", title: "Cloud", symbol: "cloud"))
            chips.append(ComposerChip(id: "branch", title: draft.branch.map { "From \($0)" } ?? "Default branch", symbol: nil, icon: BranchIcon.sized()))
        } else if let p = project {
            // The project, then the host it runs on.
            chips.append(ComposerChip(id: "project", title: p.groupName, symbol: nil, icon: ProjectTile.image(name: p.groupName, colorIndex: p.colorIndex)))
            chips.append(ComposerChip(id: "host", title: p.deviceName, symbol: "desktopcomputer"))
            if p.git {
                chips.append(ComposerChip(id: "branch", title: draft.worktree ? "New worktree" : (draft.branch ?? "Current branch"), symbol: nil, icon: BranchIcon.sized()))
            }
        } else {
            let host = app.hostOptions.first { $0.id == draft.hostId }
            chips.append(ComposerChip(id: "project", title: "No project", symbol: "tray"))
            chips.append(ComposerChip(id: "host", title: host?.name ?? "Choose host", symbol: "desktopcomputer"))
        }
        let model = draftModel
        // Never the harness name in place of a model: a model this host
        // hasn't listed still gets its catalog label.
        let modelTitle = model?.label ?? draft.model.map { modelLabel(harness: draft.harness, model: $0) } ?? fallbackModels(harness: draft.harness).first?.label ?? HarnessNames.label(draft.harness)
        chips.append(ComposerChip(id: "model", title: modelTitle, symbol: nil, icon: BrandMarks.image(for: draft.harness, side: 13)))
        if let efforts = model?.efforts, !efforts.isEmpty {
            chips.append(ComposerChip(id: "effort", title: (draft.effort ?? efforts[efforts.count / 2]).capitalized, symbol: "gauge.with.dots.needle.67percent"))
        }
        composer.chips = chips
        // Picking options is an edit too.
        scheduleSave()
        composer.chipMenus = [
            "project": { [weak self] in self?.projectMenu() },
            "host": { [weak self] in self?.hostMenu() },
            "branch": { [weak self] in self?.branchMenu() },
            "effort": { [weak self] in self?.effortMenu() },
        ]
        mark.image = BrandMarks.image(for: draft.harness, side: 34)
    }

    /// One entry per project — checkouts of one repository, on any device,
    /// are one entry listing their hosts; the host chip then picks which.
    private func projectMenu() -> UIMenu {
        var groups: [[ProjectOption]] = []
        for p in app.projectOptions {
            if let i = groups.firstIndex(where: { $0[0].groupKey == p.groupKey }) {
                groups[i].append(p)
            } else {
                groups.append([p])
            }
        }
        groups.sort { $0[0].groupName.localizedCaseInsensitiveCompare($1[0].groupName) == .orderedAscending }
        let picked = project?.groupKey
        var sections: [UIMenuElement] = [UIMenu(options: .displayInline, children: groups.map { members in
            let first = members[0]
            var hosts: [String] = []
            for m in members where !hosts.contains(m.deviceName) { hosts.append(m.deviceName) }
            return UIAction(title: first.groupName, subtitle: hosts.sorted { $0.localizedCaseInsensitiveCompare($1) == .orderedAscending }.joined(separator: ", "), image: ProjectTile.image(name: first.groupName, colorIndex: first.colorIndex, side: 18), state: first.groupKey == picked ? .on : .off) { [weak self] _ in
                self?.pickProject(members)
            }
        })]
        sections.append(UIMenu(options: .displayInline, children: [
            UIAction(title: "No Project…", image: UIImage(systemName: "tray"), state: draft.projectId == nil ? .on : .off) { [weak self] _ in
                guard let self else { return }
                self.draft.projectId = nil
                self.draft.hostId = self.draft.hostId ?? self.app.hostOptions.first(where: \.online)?.id ?? self.app.hostOptions.first?.id
                self.loadModels()
            },
            UIAction(title: "New Cloud Project…", image: UIImage(systemName: "cloud"), attributes: app.cloud.available ? [] : .hidden) { [weak self] _ in
                guard let self else { return }
                let vc = CloudRepoPickerViewController(app: self.app)
                vc.onCreated = { [weak self] id in
                    self?.draft.projectId = id
                    self?.draft.hostId = nil
                    self?.draft.branch = nil
                    self?.draft.cloud = true
                    self?.loadModels()
                }
                self.present(UINavigationController(rootViewController: vc), animated: true)
            },
            UIAction(title: "New Project…", image: UIImage(systemName: "folder.badge.plus")) { [weak self] _ in
                guard let self else { return }
                let vc = NewProjectViewController(app: self.app)
                vc.onCreated = { [weak self] id in
                    self?.draft.projectId = id
                    self?.draft.hostId = nil
                    self?.draft.branch = nil
                    self?.loadModels()
                }
                self.present(UINavigationController(rootViewController: vc), animated: true)
            },
        ]))
        return UIMenu(title: "Project", children: sections)
    }

    /// Keep the host when it has a checkout of the project, else the first
    /// reachable one.
    private func pickProject(_ members: [ProjectOption]) {
        let pick = members.first { $0.id == draft.projectId }
            ?? members.first { $0.device == deviceId }
            ?? members.first(where: \.online)
            ?? members[0]
        pickCheckout(pick)
    }

    private func pickCheckout(_ p: ProjectOption) {
        if p.id != draft.projectId || draft.cloud == true { draft.branch = nil }
        draft.projectId = p.id
        draft.hostId = nil
        draft.cloud = p.isCloud ? true : nil
        loadModels()
    }

    /// Run the picked project's session on Cloud.
    private func pickCloud() {
        draft.cloud = true
        draft.branch = nil
        draft.worktree = false
        loadModels()
    }

    /// With a project: its checkouts, by host (the path tells apart several
    /// on one host). Without: every host — project-less sessions run in its
    /// home.
    private func hostMenu() -> UIMenu {
        let checkouts = self.checkouts.filter { !$0.isCloud }
        if !checkouts.isEmpty || project != nil {
            var items: [UIMenuElement] = checkouts.map { p in
                let shared = checkouts.filter { $0.device == p.device }.count > 1
                return UIAction(title: p.deviceName, subtitle: shared ? p.path : (p.online ? "Online" : "Offline"), image: UIImage(systemName: "desktopcomputer"), state: p.id == draft.projectId && !runsOnCloud ? .on : .off) { [weak self] _ in
                    self?.pickCheckout(p)
                }
            }
            if cloudOffered {
                let credits = app.cloud.credits.map { "\(CloudUI.credits($0)) credits left" } ?? "Its own machine"
                items.append(UIMenu(options: .displayInline, children: [
                    UIAction(title: "Cloud", subtitle: credits, image: UIImage(systemName: "cloud"), state: runsOnCloud ? .on : .off) { [weak self] _ in
                        self?.pickCloud()
                    },
                ]))
            }
            return UIMenu(title: "Run on", children: items)
        }
        return UIMenu(title: "Run on", children: app.hostOptions.map { h in
            UIAction(title: h.name, subtitle: h.online ? "Online" : "Offline", image: UIImage(systemName: "desktopcomputer"), state: h.id == draft.hostId ? .on : .off) { [weak self] _ in
                self?.draft.hostId = h.id
                self?.loadModels()
            }
        })
    }

    private func branchMenu() -> UIMenu {
        if runsOnCloud, let repo = project?.githubRepo {
            // The machine clones the repository: pick the branch it starts from.
            return UIMenu(title: "Start from", children: [UIDeferredMenuElement { [weak self] done in
                guard let self else { return done([]) }
                Task { @MainActor in
                    let branches = await self.app.cloudBranches(repo: repo)
                    done([UIAction(title: "Default branch", state: self.draft.branch == nil ? .on : .off) { [weak self] _ in
                        self?.draft.branch = nil
                        self?.refreshChips()
                    }] + branches.map { b in
                        UIAction(title: b, state: b == self.draft.branch ? .on : .off) { [weak self] _ in
                            self?.draft.branch = b
                            self?.refreshChips()
                        }
                    })
                }
            }])
        }
        return UIMenu(title: "Checkout", children: [
            UIAction(title: "New worktree", state: draft.worktree ? .on : .off) { [weak self] _ in
                self?.draft.worktree.toggle()
                self?.refreshChips()
            },
            UIMenu(title: "Branch", options: .displayInline, children: [UIDeferredMenuElement { [weak self] done in
                guard let self, let p = self.project else { return done([]) }
                Task { @MainActor in
                    let refs = await self.app.refs(projectId: p.id)
                    done(refs.map { ref in
                        UIAction(title: ref, state: ref == (self.draft.branch ?? refs.first) ? .on : .off) { [weak self] _ in
                            self?.draft.branch = ref
                            self?.refreshChips()
                        }
                    })
                }
            }]),
        ])
    }

    /// A tab per provider the target runs (the draft's first), each listing
    /// the models already loaded for it.
    private func modelPicker() -> UIViewController {
        var order: [String] = [draft.harness]
        for m in models where !order.contains(m.harness) { order.append(m.harness) }
        let tabs = order.compactMap { h -> ModelPickerTab? in
            let choices = models.filter { $0.harness == h }
            guard let first = choices.first else { return nil }
            let rows = choices.map { ModelPickerRow(id: $0.id, label: $0.label, detail: $0.detail) }
            return ModelPickerTab(harness: h, label: first.harnessLabel) { rows }
        }
        let host = runsOnCloud ? (name: "Cloud", symbol: "cloud") : (name: app.deviceName(deviceId), symbol: "desktopcomputer")
        return ModelPickerViewController.sheet(tabs: tabs, harness: draft.harness, model: draft.model, host: host) { [weak self] harness, model in
            guard let self else { return }
            if harness != self.draft.harness { self.draft.effort = nil }
            self.draft.harness = harness
            self.draft.model = model.id
            self.refreshChips()
        }
    }

    /// The draft's own model when it names one (even one this list lacks:
    /// it's what gets sent, so its label and efforts are its own); the
    /// provider's default only when it names none.
    private var draftModel: ModelChoice? {
        draft.model == nil
            ? models.first { $0.harness == draft.harness }
            : models.first { $0.harness == draft.harness && $0.id == draft.model }
    }

    private func effortMenu() -> UIMenu {
        let model = draftModel
        return UIMenu(title: "Reasoning effort", children: (model?.efforts ?? []).map { e in
            UIAction(title: e.capitalized, state: e == draft.effort ? .on : .off) { [weak self] _ in
                self?.draft.effort = e
                self?.refreshChips()
            }
        })
    }

    /// Send the page: a draft only this page knows goes at once (offline
    /// too); a listed one is reserved first, so two devices can't both send
    /// it. Either way the chat id derives from the draft.
    private func send(text: String, images: [StagedImage]) -> ComposerBar.SendResult {
        guard restoring == nil, !sending, !cloudStarting else { return .kept }
        if !reopened { app.lastDraft = draft }
        guard saveDraft(listing: false), let revision = canvas.revision else {
            return create(text: text, images: images, revision: nil) ? .sent : .kept
        }
        guard canvas.listed else {
            return create(text: text, images: images, revision: revision) ? .sent : .kept
        }
        sending = true
        canvas.reserved = true
        let id = canvas.id
        Task { @MainActor [weak self] in
            guard let self else { return }
            do {
                try await self.app.claimDraft(id, revision: revision)
                self.sending = false
                // Still the same draft (no edit while reserving): send it.
                guard self.canvas.id == id, self.canvas.revision == revision else { return }
                if self.create(text: text, images: images, revision: revision) {
                    self.composer.text = ""
                    self.composer.images = []
                }
            } catch {
                self.sending = false
                // A conflict: another device sent this revision first.
                var alreadySent = false
                if case CoreError.InvalidArgument = error { alreadySent = true }
                let alert = UIAlertController(
                    title: alreadySent ? "Already sent" : "Couldn't send the draft",
                    message: alreadySent ? "This draft was sent from another device." : "Couldn't reach Zeron to reserve it. Try again when you're online.",
                    preferredStyle: .alert
                )
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                self.present(alert, animated: true)
            }
        }
        return .kept
    }

    /// False when the session couldn't be created (the prompt stays put).
    private func create(text: String, images: [StagedImage], revision: String?) -> Bool {
        if runsOnCloud {
            createOnCloud(text: text, images: images, revision: revision)
            return false
        }
        let source = revision.map { (id: canvas.id, revision: $0) }
        guard let chatId = app.createSession(draft: draft, text: text, images: images, files: keptFiles, fromDraft: source) else {
            let alert = UIAlertController(title: "Couldn't start the session", message: "Choose a project or a host that can run it.", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
            return false
        }
        created = true
        saveTimer?.invalidate()
        PushNotifications.shared.askAfterFirstSession()
        // Lift the draft out (page + composer + typed text) so the chat can
        // take over in one motion.
        onCreated(chatId, DraftHandoff.capture(from: self, composer: composer, text: text))
        return true
    }
}

extension NewSessionViewController {
    /// Cloud: the edge mints the machine first (a moment), then the chat
    /// takes over as for any session; the text stays put until then.
    fileprivate func createOnCloud(text: String, images: [StagedImage], revision: String?) {
        guard !cloudStarting else { return }
        if app.cloud.outOfCredits {
            return CloudRoutes.showOutOfCredits(app: app, from: self, above: composer)
        }
        cloudStarting = true
        composer.isUserInteractionEnabled = false
        let hud = CloudStartingHUD()
        hud.show(over: composer)
        Task { @MainActor in
            defer {
                self.cloudStarting = false
                self.composer.isUserInteractionEnabled = true
                hud.removeFromSuperview()
            }
            do {
                let source = revision.map { (id: self.canvas.id, revision: $0) }
                let chatId = try await self.app.createCloudSession(draft: self.draft, text: text, images: images, files: self.keptFiles, fromDraft: source)
                self.markCreated()
                PushNotifications.shared.askAfterFirstSession()
                let handoff = DraftHandoff.capture(from: self, composer: self.composer, text: text)
                self.composer.text = ""
                self.composer.images = []
                self.onCreatedHandler(chatId, handoff)
            } catch where CloudRoutes.isOutOfCredits(error) {
                CloudRoutes.showOutOfCredits(app: self.app, from: self, above: self.composer)
            } catch {
                let alert = UIAlertController(title: "Couldn't start on Cloud", message: CloudStore.message(error), preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                self.present(alert, animated: true)
            }
        }
    }
}

/// "Starting on Cloud…" above the composer while the machine is minted.
final class CloudStartingHUD: UIView {
    func show(over anchor: UIView) {
        guard let host = anchor.superview else { return }
        let glass = Glass.surface()
        let spinner = UIActivityIndicatorView(style: .medium)
        spinner.startAnimating()
        let label = UILabel()
        label.text = "Starting on Cloud…"
        label.font = Fonts.ui(.sansMedium, 14)
        label.textColor = Palette.text
        let row = UIStackView(arrangedSubviews: [spinner, label])
        row.spacing = 8
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(row)
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        translatesAutoresizingMaskIntoConstraints = false
        host.addSubview(self)
        NSLayoutConstraint.activate([
            row.topAnchor.constraint(equalTo: glass.contentView.topAnchor, constant: 10),
            row.bottomAnchor.constraint(equalTo: glass.contentView.bottomAnchor, constant: -10),
            row.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 16),
            row.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -16),
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            centerXAnchor.constraint(equalTo: anchor.centerXAnchor),
            bottomAnchor.constraint(equalTo: anchor.topAnchor, constant: -10),
        ])
    }
}

enum HarnessNames {
    /// The core's harness catalog label (same table as desktop).
    static func label(_ id: String) -> String { harnessLabel(harness: id) }
}
