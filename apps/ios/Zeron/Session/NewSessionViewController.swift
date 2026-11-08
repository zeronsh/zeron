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
    /// Run in a new Scaffold sandbox with this database (a `CrewDatabase`
    /// key) instead of on a device. Project and host are kept for when the
    /// user picks a device again.
    var scaffold: String?
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

/// "Ask anything" → a composer-first canvas. Context is set with native menus
/// (project, branch/worktree, model, effort) that load lazily; the composer is
/// focused immediately so the common case is: tap, type, send.
final class NewSessionViewController: UIViewController, UIGestureRecognizerDelegate, ScaffoldLaunchPresenter {
    private let app: AppModel
    private let onCreated: (String, DraftHandoff?) -> Void
    private let composer = ComposerBar()
    private let hero = UILabel()
    private let wallpaper = WallpaperView()
    private let mark = UIImageView()
    private var draft: NewSessionDraft
    private var models: [ModelChoice] = []
    /// Shown while a sandbox starts (the launch outlives this page).
    private let launchSpinner = UIActivityIndicatorView(style: .medium)
    private var appToken: AnyObject?
    /// The device-side provider and model to return to when the user picks
    /// a device again after a Scaffold destination.
    private var beforeScaffold: (harness: String, model: String?, effort: String?)?
    /// The desktop controller a sandbox would start through, once known.
    private var controllerName: String?

    /// Embedded in the iPad split's main column (no sheet chrome).
    private let embedded: Bool
    private var composerRest: NSLayoutConstraint?
    private var backgroundObserver: NSObjectProtocol?

    deinit {
        if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver) }
    }
    /// Take the keyboard on appearing. The sheet always does; the iPad column
    /// only when opened on purpose (not the launch page).
    var focusOnAppear: Bool

    init(app: AppModel, prompt: String?, embedded: Bool = false, onCreated: @escaping (String, DraftHandoff?) -> Void) {
        self.app = app
        self.embedded = embedded
        self.focusOnAppear = !embedded
        self.onCreated = onCreated
        self.draft = app.lastDraft
        super.init(nibName: nil, bundle: nil)
        // Pick up where the page was left (closed without sending).
        composer.text = prompt ?? app.newSessionText
        composer.images = app.newSessionImages
    }

    /// A session was created from this page: nothing to keep.
    private var created = false

    /// Closed (swipe down, ✕, or replaced in the iPad column) without
    /// sending: keep what was typed and picked for next time.
    private func rememberDraft() {
        guard !created else { return }
        app.lastDraft = draft
        app.newSessionText = composer.text
        app.newSessionImages = composer.images
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        rememberDraft()
        if app.scaffoldLaunchPresenter === self { app.scaffoldLaunchPresenter = nil }
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        app.scaffoldLaunchPresenter = self
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        // The app can be killed in the background: keep the draft first.
        backgroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.rememberDraft()
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
        launchSpinner.hidesWhenStopped = true
        launchSpinner.color = Palette.secondary
        let heroStack = UIStackView(arrangedSubviews: [mark, hero, launchSpinner])
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
            // The picker needs at least one provider tab.
            guard !self.models.isEmpty else { return self.showNoManagedModels() }
            self.present(self.modelPicker(), animated: true)
        }
        composer.mentionSearch = { [weak self] q in
            // A sandbox can't read this computer's files: no @mentions.
            guard let self, self.scaffold == nil, let p = self.project else { return [] }
            return await self.app.searchFiles(deviceId: p.device, spaceId: p.id, query: q)
        }
        composer.onSend = { [weak self] text, images, _ in
            guard let self else { return .kept }
            if let database = self.scaffold { return self.launchScaffold(database, text: text, images: images) }
            return self.create(text: text, images: images) ? .sent : .kept
        }
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
        if draft.projectId == nil, draft.hostId == nil { draft.projectId = app.projectOptions.first?.id }
        if draft.projectId == nil, draft.hostId == nil {
            // No projects yet: run on the first reachable host.
            draft.hostId = app.hostOptions.first(where: \.online)?.id ?? app.hostOptions.first?.id
        }
        if !app.offersScaffold { draft.scaffold = nil }
        loadModels()
        if app.offersScaffold { resolveController() }
        appToken = app.observe { [weak self] in self?.refreshLaunchState() }
        refreshLaunchState()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if focusOnAppear, app.scaffoldLaunch == nil { composer.becomeFirstResponder() }
        // A launch that failed while this page was closed.
        if let failure = app.scaffoldFailure {
            app.scaffoldFailure = nil
            showLaunchFailure(failure)
        }
    }

    /// The last catalog each host reported, so the chip opens on a real model
    /// name while a fresh one comes back over the relay.
    private static var modelCache: [String: [ModelChoice]] = [:]
    private var modelsDevice: String?

    /// Models for the draft's host: cached (or the built-in catalog) at once,
    /// then the host's own list. A Scaffold destination lists the managed
    /// models its desktop controller offers.
    private func loadModels() {
        if scaffold != nil { return loadScaffoldModels() }
        let device = deviceId
        guard device != modelsDevice else { return refreshChips() }
        modelsDevice = device
        models = Self.modelCache[device] ?? Self.catalogModels()
        refreshChips()
        Task { [weak self] in
            guard let self else { return }
            let fresh = await self.app.models(for: device)
            guard !fresh.isEmpty else { return }
            Self.modelCache[device] = fresh
            guard self.modelsDevice == device else { return }
            self.models = fresh
            self.refreshChips()
        }
    }

    private static func catalogModels() -> [ModelChoice] {
        fallbackHarnesses().filter(\.offered).flatMap { h in
            fallbackModels(harness: h.id).map { ModelChoice(harness: h.id, harnessLabel: h.label, id: $0.id, label: $0.label, efforts: $0.reasoningLevels, detail: $0.description) }
        }
    }

    // MARK: Scaffold destinations

    /// The draft's Scaffold destination (Crew accounts only).
    private var scaffold: CrewDatabase? {
        app.offersScaffold ? CrewDatabase(key: draft.scaffold) : nil
    }

    private static let scaffoldModelsKey = "scaffold"

    private func loadScaffoldModels() {
        let key = Self.scaffoldModelsKey
        guard modelsDevice != key else { return refreshChips() }
        modelsDevice = key
        models = Self.modelCache[key] ?? []
        refreshChips()
        Task { [weak self] in
            guard let self else { return }
            let fresh = await self.app.scaffoldModels()
            self.noteController()
            guard !fresh.isEmpty else {
                // Nothing reachable yet: ask again next time.
                if self.modelsDevice == key, self.models.isEmpty { self.modelsDevice = nil }
                return
            }
            Self.modelCache[key] = fresh
            guard self.modelsDevice == key else { return }
            self.models = fresh
            self.refreshChips()
        }
    }

    /// Learn which computer would start the sandbox (the menu names it).
    private func resolveController() {
        Task { [weak self] in
            _ = await self?.app.scaffoldControllerId()
            self?.noteController()
        }
    }

    private func noteController() {
        let name = app.scaffoldController.map { app.deviceName($0.id) }
        guard name != controllerName else { return }
        controllerName = name
        refreshChips()
    }

    /// Like the desktop: a Scaffold destination runs a managed OMP model,
    /// the first one listed until the user picks another.
    private func pickScaffold(_ database: CrewDatabase) {
        if draft.scaffold == nil {
            beforeScaffold = (draft.harness, draft.model, draft.effort)
            draft.harness = "omp"
            draft.model = nil
            draft.effort = nil
        }
        draft.scaffold = database.key
        loadModels()
    }

    /// A device was picked: back to what it ran before the sandbox.
    private func leaveScaffold() {
        guard draft.scaffold != nil else { return }
        draft.scaffold = nil
        if let before = beforeScaffold {
            draft.harness = before.harness
            draft.model = before.model
            draft.effort = before.effort
            beforeScaffold = nil
        }
    }

    /// The page mirrors a launch in flight, even one started before it opened.
    private func refreshLaunchState() {
        // Handed off: the composer is mid-flight into the chat.
        guard !created else { return }
        let launch = app.scaffoldLaunch
        composer.isUserInteractionEnabled = launch == nil
        composer.alpha = launch == nil ? 1 : 0.55
        hero.text = launch?.phase.progressLabel ?? "What are we building?"
        if launch == nil { launchSpinner.stopAnimating() } else { launchSpinner.startAnimating() }
    }

    private func launchScaffold(_ database: CrewDatabase, text: String, images: [StagedImage]) -> ComposerBar.SendResult {
        guard app.scaffoldLaunch == nil else { return .kept }
        // A picked model the list lacks is still the one to send (the core
        // checks it is managed); nothing picked means the list's first.
        guard let model = draft.model ?? draftModel?.id else {
            showNoManagedModels()
            return .kept
        }
        app.lastDraft = draft
        view.endEditing(true)
        let config = ChatConfig(harness: draft.harness, model: model, reasoning: draft.effort, modelOptions: [:], sandbox: .workspaceWrite)
        app.launchScaffoldSession(database: database, config: config, text: text, images: images)
        // The prompt stays in the composer until the sandbox takes it.
        return .kept
    }

    func scaffoldLaunchFinished(chatId: String?, error: String?) {
        refreshLaunchState()
        guard let chatId else {
            showLaunchFailure(error ?? "The sandbox didn't start.")
            return
        }
        created = true
        app.newSessionText = ""
        app.newSessionImages = []
        PushNotifications.shared.askAfterFirstSession()
        onCreated(chatId, DraftHandoff.capture(from: self, composer: composer, text: composer.text))
    }

    private func showLaunchFailure(_ message: String) {
        let alert = UIAlertController(title: "The sandbox didn't start", message: message, preferredStyle: .alert)
        if app.retainedScaffoldLaunch != nil {
            // Its outcome is still owed: the next send checks the same sandbox.
            alert.addAction(UIAlertAction(title: "Stop Waiting", style: .destructive) { [weak self] _ in
                self?.confirmForgetLaunch()
            })
        }
        alert.addAction(UIAlertAction(title: "OK", style: .cancel))
        present(alert, animated: true)
    }

    private func showNoManagedModels() {
        loadModels()
        let alert = UIAlertController(
            title: "No models yet",
            message: scaffold == nil
                ? "This computer hasn't listed its models yet. Try again in a moment."
                : "Crew couldn't list managed models from your computer. Open Crew there and sign in, then try again.",
            preferredStyle: .alert
        )
        alert.addAction(UIAlertAction(title: "OK", style: .cancel))
        present(alert, animated: true)
    }

    private func confirmForgetLaunch() {
        let alert = UIAlertController(
            title: "Stop waiting for this sandbox?",
            message: "Your next send creates a new sandbox. If this one was created, it stays in Settings → Scaffold sandboxes.",
            preferredStyle: .alert
        )
        alert.addAction(UIAlertAction(title: "Stop Waiting", style: .destructive) { [weak self] _ in
            self?.app.forgetScaffoldLaunch()
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        present(alert, animated: true)
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
        if let database = scaffold {
            // A sandbox runs its deployment's own checkout: no project or
            // branch to pick (the desktop hides both too).
            chips.append(ComposerChip(id: "host", title: database.label, symbol: "server.rack"))
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
        draft.cloud = nil
        loadModels()
    }

    /// With a project: its checkouts, by host (the path tells apart several
    /// on one host). Without: every host — project-less sessions run in its
    /// home. Crew accounts also get the desktop's Scaffold destinations.
    private func hostMenu() -> UIMenu {
        let checkouts = self.checkouts
        let onDevice = scaffold == nil
        var items: [UIMenuElement]
        if !checkouts.isEmpty || project != nil {
            items = checkouts.map { p in
                let shared = checkouts.filter { $0.device == p.device }.count > 1
                return UIAction(title: p.deviceName, subtitle: shared ? p.path : (p.online ? "Online" : "Offline"), image: UIImage(systemName: "desktopcomputer"), state: onDevice && p.id == draft.projectId ? .on : .off) { [weak self] _ in
                    self?.leaveScaffold()
                    self?.pickCheckout(p)
                }
            }
        } else {
            items = app.hostOptions.map { h in
                UIAction(title: h.name, subtitle: h.online ? "Online" : "Offline", image: UIImage(systemName: "desktopcomputer"), state: onDevice && h.id == draft.hostId ? .on : .off) { [weak self] _ in
                    self?.leaveScaffold()
                    self?.draft.hostId = h.id
                    self?.loadModels()
                }
            }
        }
        if app.offersScaffold {
            // The desktop's Scaffold destinations, grouped so they fit a phone
            // menu (the chip then reads "Scaffold (Staging DB)" as there).
            // Selecting one provisions nothing; sending creates the sandbox.
            let via = controllerName.map { "via \($0)" } ?? "via Crew on your computer"
            items.append(UIMenu(title: "New Scaffold sandbox", options: .displayInline, children: CrewDatabase.all.map { database in
                UIAction(title: database.shortLabel, subtitle: via, image: UIImage(systemName: "server.rack"), state: scaffold == database ? .on : .off) { [weak self] _ in
                    self?.pickScaffold(database)
                }
            }))
        }
        return UIMenu(title: "Run on", children: items)
    }

    private func branchMenu() -> UIMenu {
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
        let host = scaffold.map { (name: $0.label, symbol: "server.rack") } ?? (name: app.deviceName(deviceId), symbol: "desktopcomputer")
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

    /// False when the session couldn't be created (the prompt stays put).
    private func create(text: String, images: [StagedImage]) -> Bool {
        app.lastDraft = draft
        guard let chatId = app.createSession(draft: draft, text: text, images: images) else {
            let alert = UIAlertController(title: "Couldn't start the session", message: "Choose a project or a host that can run it.", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
            return false
        }
        created = true
        PushNotifications.shared.askAfterFirstSession()
        app.newSessionText = ""
        app.newSessionImages = []
        // Lift the draft out (page + composer + typed text) so the chat can
        // take over in one motion.
        onCreated(chatId, DraftHandoff.capture(from: self, composer: composer, text: text))
        return true
    }
}

enum HarnessNames {
    /// The core's harness catalog label (same table as desktop).
    static func label(_ id: String) -> String { harnessLabel(harness: id) }
}
