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
    /// Non-default option picks; only choices the model offers are sent.
    /// Optional so drafts saved before this field still decode.
    var modelOptions: [String: String]?

    var modelSelection: ModelSelection {
        get { ModelSelection(harness: harness, model: model, effort: effort, options: modelOptions ?? [:]) }
        set {
            harness = newValue.harness
            model = newValue.model
            effort = newValue.effort
            modelOptions = newValue.options.isEmpty ? nil : newValue.options
        }
    }
}

/// Options the pickers offer (from the core's workspace + host catalogs).
struct ProjectOption: Equatable {
    let id: String
    let name: String
    let device: String
    /// Display name of `device` (never show the id).
    let deviceName: String
    let online: Bool
    let git: Bool
    let colorIndex: Int
}

struct HostOption: Equatable {
    let id: String
    let name: String
    let online: Bool
}

/// "Ask anything" → a composer-first canvas. Context is set with native
/// project/branch menus plus the model picker; the composer is focused
/// immediately so the common case is: tap, type, send.
final class NewSessionViewController: UIViewController, UIGestureRecognizerDelegate {
    private let app: AppModel
    private let onCreated: (String, DraftHandoff?) -> Void
    private let composer = ComposerBar()
    private let hero = UILabel()
    private let wallpaper = WallpaperView()
    private let mark = UIImageView()
    private var draft: NewSessionDraft
    private var catalog = ModelCatalog.fallback()
    /// The last catalog each host reported, so the chip opens on a real model
    /// name while a fresh one comes back over the relay.
    private static var catalogCache: [String: ModelCatalog] = [:]
    private var catalogDevice: String?
    private weak var modelPicker: ModelPickerViewController?

    /// Embedded in the iPad split's main column (no sheet chrome).
    private let embedded: Bool
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
        composer.onChipTap = { [weak self] id, chip in
            if id == "model" { self?.presentModelPicker(from: chip) }
        }
        composer.mentionSearch = { [weak self] q in
            guard let self, let p = self.project else { return [] }
            return await self.app.searchFiles(deviceId: p.device, spaceId: p.id, query: q)
        }
        composer.onSend = { [weak self] text, images, _ in
            self?.create(text: text, images: images) ?? false ? .sent : .kept
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
            heroStack.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            heroStack.centerYAnchor.constraint(equalTo: free.centerYAnchor),
            heroStack.topAnchor.constraint(greaterThanOrEqualTo: free.topAnchor),
            heroStack.bottomAnchor.constraint(lessThanOrEqualTo: free.bottomAnchor),
            composer.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            composer.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            composer.widthAnchor.constraint(lessThanOrEqualToConstant: 768),
            fill,
            composer.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -8),
        ])
        if draft.projectId == nil, draft.hostId == nil { draft.projectId = app.projectOptions.first?.id }
        if draft.projectId == nil, draft.hostId == nil {
            // No projects yet: run on the first reachable host.
            draft.hostId = app.hostOptions.first(where: \.online)?.id ?? app.hostOptions.first?.id
        }
        loadModels()
    }

    /// Models for the draft's host: cached (or built in) at once, then the
    /// host's own list. `refresh` re-lists the same host when the picker opens.
    private func loadModels(refresh: Bool = false) {
        let device = deviceId
        if !refresh, device == catalogDevice {
            refreshChips()
            return
        }
        if device != catalogDevice {
            catalogDevice = device
            catalog = Self.catalogCache[device] ?? .fallback()
            refreshChips()
        }
        Task { [weak self] in
            guard let self else { return }
            let fresh = await self.app.modelCatalog(for: device)
            guard !fresh.providers.isEmpty else { return }
            Self.catalogCache[device] = fresh
            guard self.catalogDevice == device, fresh != self.catalog else { return }
            self.catalog = fresh
            self.refreshChips()
            self.modelPicker?.update(catalog: fresh)
        }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        if focusOnAppear { composer.becomeFirstResponder() }
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

    private func refreshChips() {
        var chips: [ComposerChip] = []
        if let p = project {
            chips.append(ComposerChip(id: "project", title: p.name, symbol: nil, icon: ProjectTile.image(name: p.name, colorIndex: p.colorIndex)))
            if p.git {
                chips.append(ComposerChip(id: "branch", title: draft.worktree ? "New worktree" : (draft.branch ?? "Current branch"), symbol: draft.worktree ? "square.split.bottomrightquarter" : nil, icon: draft.worktree ? nil : BranchIcon.sized()))
            }
        } else {
            let host = app.hostOptions.first { $0.id == draft.hostId }
            chips.append(ComposerChip(id: "project", title: "No project", symbol: "tray"))
            chips.append(ComposerChip(id: "host", title: host?.name ?? "Choose host", symbol: "desktopcomputer"))
        }
        let pick = draft.modelSelection
        // Never the harness name in place of a model: a model this host
        // hasn't listed still gets its catalog label.
        chips.append(ComposerChip(
            id: "model",
            title: catalog.title(for: pick),
            symbol: nil,
            icon: BrandMarks.image(for: pick.harness, side: 13),
            detail: catalog.chipDetail(for: pick)
        ))
        composer.chips = chips
        composer.chipMenus = [
            "project": { [weak self] in self?.projectMenu() },
            "host": { [weak self] in self?.hostMenu() },
            "branch": { [weak self] in self?.branchMenu() },
        ]
        mark.image = BrandMarks.image(for: draft.harness, side: 34)
    }

    private func projectMenu() -> UIMenu {
        let byDevice = Dictionary(grouping: app.projectOptions, by: \.device)
        var sections: [UIMenuElement] = byDevice.keys.sorted { (byDevice[$0]?.first?.deviceName ?? "") < (byDevice[$1]?.first?.deviceName ?? "") }.map { device in
            let items = byDevice[device]!
            let name = items.first?.deviceName ?? "Host"
            return UIMenu(title: name + (items.first?.online == false ? " · offline" : ""), options: .displayInline, children: items.map { p in
                UIAction(title: p.name, image: UIImage(systemName: p.git ? "folder.badge.gearshape" : "folder"), state: p.id == draft.projectId ? .on : .off) { [weak self] _ in
                    self?.draft.projectId = p.id
                    self?.draft.hostId = nil
                    self?.draft.branch = nil
                    self?.loadModels()
                }
            })
        }
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

    private func hostMenu() -> UIMenu {
        UIMenu(title: "Run on", children: app.hostOptions.map { h in
            UIAction(title: h.name, subtitle: h.online ? "Online" : "Offline", image: UIImage(systemName: "desktopcomputer"), state: h.id == draft.hostId ? .on : .off) { [weak self] _ in
                self?.draft.hostId = h.id
                self?.loadModels()
            }
        })
    }

    private func branchMenu() -> UIMenu {
        UIMenu(title: "Checkout", children: [
            UIAction(title: "New worktree", subtitle: "Isolated branch for this session", image: UIImage(systemName: "square.split.bottomrightquarter"), state: draft.worktree ? .on : .off) { [weak self] _ in
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

    /// The model chip opens the picker, not a menu, and revalidates the host's
    /// catalog on every open like desktop.
    private func presentModelPicker(from chip: UIView) {
        guard presentedViewController == nil else { return }
        let picker = ModelPickerViewController(catalog: catalog, selection: draft.modelSelection, locked: false)
        picker.onChange = { [weak self] pick in
            self?.draft.modelSelection = pick
            self?.refreshChips()
        }
        modelPicker = picker
        picker.present(anchoredTo: chip, holding: composer, over: self)
        loadModels(refresh: true)
    }

    /// False when the session couldn't be created (the prompt stays put).
    private func create(text: String, images: [StagedImage]) -> Bool {
        app.lastDraft = draft
        let run = catalog.resolved(draft.modelSelection, keepingUnlistedOptions: false)
        let config = ChatConfig(harness: run.harness, model: run.model, reasoning: run.effort, modelOptions: run.options, sandbox: .workspaceWrite)
        guard let chatId = app.createSession(draft: draft, config: config, text: text, images: images) else {
            let alert = UIAlertController(title: "Couldn't start the session", message: "Choose a project or a host that can run it.", preferredStyle: .alert)
            alert.addAction(UIAlertAction(title: "OK", style: .default))
            present(alert, animated: true)
            return false
        }
        created = true
        app.newSessionText = ""
        app.newSessionImages = []
        // Lift the draft out (page + composer + typed text) so the chat can
        // take over in one motion.
        onCreated(chatId, DraftHandoff.capture(from: self, composer: composer, text: text))
        return true
    }
}
