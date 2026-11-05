import UIKit

/// Navigation entry points the scene and screens use without knowing which
/// shell is on screen (iPhone tabs or the iPad split).
protocol AppRouter: AnyObject {
    func openSession(_ chatId: String)
    func presentNewSession(prompt: String?)
    func showSettings()
    func showSearch()
}

/// The app's shell: Zeron mobile with a sidebar. The sidebar is the phone's
/// Sessions page (same rows, sections, wallpaper) over a bottom toolbar;
/// the main column shows the open session or the phone's new-session page.
/// It shows when the window is regular in both directions (iPad, an unfolded
/// iPhone Duo); anything else (iPhone, the Duo folded, Slide Over, narrow
/// Split View) collapses to the iPhone tab shell.
final class SplitRootController: UISplitViewController, UISplitViewControllerDelegate, AppRouter {
    private let app: AppModel
    private let sidebar: SidebarViewController
    private let detail = UINavigationController()
    /// The open session's companions (agents, side chats, terminals) beside
    /// it: the inspector column.
    private let inspector = UINavigationController()
    private var inspectorChatId: String?
    /// The sidebar's mode before the inspector made room for itself.
    private var modeBeforeInspector: UISplitViewController.DisplayMode?
    private lazy var tabs = MainTabController(app: app)
    private(set) var currentChatId: String?

    init(app: AppModel) {
        self.app = app
        self.sidebar = SidebarViewController(app: app)
        super.init(style: .doubleColumn)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        delegate = self
        preferredDisplayMode = .oneBesideSecondary
        preferredSplitBehavior = .tile
        // About a phone's width, so the rows read as they do on iPhone.
        preferredPrimaryColumnWidth = 360
        minimumPrimaryColumnWidth = 320
        maximumPrimaryColumnWidth = 400
        primaryBackgroundStyle = .none
        // No toggle button (the phone has none); ⌘B still hides the sidebar.
        displayModeButtonVisibility = .never
        setViewController(sidebar, for: .primary)
        detail.navigationBar.titleTextAttributes = [.font: Fonts.ui(.sansSemibold, 17), .foregroundColor: Palette.text]
        detail.navigationBar.tintColor = Palette.text
        setViewController(detail, for: .secondary)
        inspector.navigationBar.titleTextAttributes = [.font: Fonts.ui(.sansSemibold, 17), .foregroundColor: Palette.text]
        inspector.navigationBar.tintColor = Palette.text
        inspector.view.backgroundColor = Palette.background
        preferredInspectorColumnWidth = 400
        minimumInspectorColumnWidth = 340
        maximumInspectorColumnWidth = 480
        setViewController(inspector, for: .inspector)
        hide(.inspector)
        setViewController(tabs, for: .compact)
        // A phone in landscape is compact in height but can be regular in
        // width (the larger models); it keeps the tab shell. The split
        // collapses on its own horizontal class, so pin that to compact.
        registerForTraitChanges([UITraitVerticalSizeClass.self]) { (self: SplitRootController, _) in
            self.syncCollapse()
        }
        syncCollapse()
    }

    private func syncCollapse() {
        if traitCollection.verticalSizeClass == .compact {
            traitOverrides.horizontalSizeClass = .compact
        } else if traitOverrides.contains(UITraitHorizontalSizeClass.self) {
            traitOverrides.remove(UITraitHorizontalSizeClass.self)
        }
    }

    /// The main column starts on the new-session page, built only once the
    /// sidebar is actually on screen (an iPhone may never expand).
    override func viewIsAppearing(_ animated: Bool) {
        super.viewIsAppearing(animated)
        if !isCollapsed, detail.viewControllers.isEmpty { showDraft(prompt: nil, focus: false) }
    }

    /// Narrow windows hold the sidebar at its minimum (an iPad mini in
    /// portrait gets 320). The unfolded iPhone Duo is narrower still (669pt
    /// in portrait): it goes lower so the session column keeps a phone's
    /// width for the transcript and composer.
    override func viewWillLayoutSubviews() {
        super.viewWillLayoutSubviews()
        let minimum: CGFloat = view.bounds.width < 700 ? 280 : 320
        if minimumPrimaryColumnWidth != minimum { minimumPrimaryColumnWidth = minimum }
    }

    override var keyCommands: [UIKeyCommand]? {
        [
            UIKeyCommand(title: "New Session", action: #selector(newSessionCommand), input: "n", modifierFlags: .command),
            UIKeyCommand(title: "Search", action: #selector(searchCommand), input: "f", modifierFlags: [.command, .shift]),
            UIKeyCommand(title: "Toggle Sidebar", action: #selector(toggleSidebarCommand), input: "b", modifierFlags: .command),
        ]
    }

    @objc private func newSessionCommand() { presentNewSession(prompt: nil) }
    @objc private func searchCommand() { showSearch() }
    @objc private func toggleSidebarCommand() {
        UIView.animate(withDuration: 0.3) {
            self.preferredDisplayMode = self.displayMode == .secondaryOnly ? .oneBesideSecondary : .secondaryOnly
        }
    }

    // MARK: AppRouter

    func openSession(_ chatId: String) {
        if presentedViewController != nil { dismiss(animated: true) }
        guard !isCollapsed else { return tabs.openSession(chatId) }
        if currentChatId == chatId, detail.viewControllers.first is SessionViewController { return }
        followInspector(to: chatId)
        guard let session = SessionViewController(app: app, chatId: chatId) else {
            return showSessionOpenError()
        }
        currentChatId = chatId
        sidebar.currentChatId = chatId
        detail.setViewControllers([session], animated: false)
    }

    private func showSessionOpenError() {
        if presentedViewController != nil { dismiss(animated: false) }
        view.endEditing(true)
        showDraft(prompt: nil, focus: false)
        sidebar.showSessions()
        show(.primary)
        Toast.show("Couldn't open session. Try again.", in: view.window)
    }

    func presentNewSession(prompt: String?) {
        guard !isCollapsed else { return tabs.presentNewSession(prompt: prompt) }
        showDraft(prompt: prompt, focus: true)
    }

    /// The new-session page in the main column. Launch shows it without the
    /// keyboard (it would cover half the column); asking for one focuses it.
    func showDraft(prompt: String?, focus: Bool) {
        currentChatId = nil
        sidebar.currentChatId = nil
        let draft = NewSessionViewController(app: app, prompt: prompt, embedded: true) { [weak self] chatId, handoff in
            guard let self, let handoff, let window = self.view.window, !self.isCollapsed else {
                self?.openSession(chatId)
                return
            }
            // In place: the chat replaces the draft in the column and the
            // handoff carries the composer and message across.
            guard let session = SessionViewController(app: self.app, chatId: chatId) else {
                return self.showSessionOpenError()
            }
            self.currentChatId = chatId
            self.sidebar.currentChatId = chatId
            UIView.performWithoutAnimation {
                self.detail.setViewControllers([session], animated: false)
                self.view.layoutIfNeeded()
                session.prepareArrival()
            }
            DraftHandoffAnimator.run(handoff, into: session, window: window)
        }
        draft.focusOnAppear = focus
        detail.setViewControllers([draft], animated: false)
    }

    func showSettings() {
        guard !isCollapsed else { return tabs.showSettings() }
        let nav = MainTabController.nav(MoreViewController(app: app))
        nav.modalPresentationStyle = .formSheet
        nav.topViewController?.navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak nav] _ in
            nav?.dismiss(animated: true)
        })
        present(nav, animated: true)
    }

    func showSearch() {
        guard !isCollapsed else { return tabs.showSearch() }
        if displayMode == .secondaryOnly { show(.primary) }
        sidebar.focusSearch()
    }

    /// A session came on screen in either shell (sidebar, tabs, search):
    /// it's the one to keep across a width change.
    func sessionDidAppear(_ chatId: String) {
        currentChatId = chatId
        sidebar.currentChatId = chatId
        followInspector(to: chatId)
    }

    // MARK: Companions (the inspector)

    private var inspectorShown: Bool { isShowing(.inspector) }

    /// The companions panel for `chatId` beside it, or put away when it's
    /// already showing that panel.
    func toggleCompanions(for chatId: String, section: CompanionsViewController.Section?) {
        if inspectorShown, inspectorChatId == chatId, inspector.viewControllers.count == 1 {
            return hideCompanions()
        }
        let root = companionsRoot(for: chatId)
        root.initialSection = section
        inspector.setViewControllers([root], animated: false)
        revealInspector()
    }

    /// A companion screen (terminal, subagent, side chat) in the inspector,
    /// over the session's panel so Back returns to it.
    /// The middle of the sidebar's bottom toolbar while the sidebar shows: a
    /// session's composer rests level with it.
    func sidebarToolbarMidY(in window: UIWindow) -> CGFloat? {
        guard !isCollapsed, displayMode != .secondaryOnly else { return nil }
        return sidebar.toolbarMidY(in: window)
    }

    func showCompanion(_ vc: UIViewController, for chatId: String) {
        let root = companionsRoot(for: chatId)
        if inspectorShown, inspector.viewControllers.first === root {
            inspector.setViewControllers([root, vc], animated: true)
        } else {
            inspector.setViewControllers([root, vc], animated: false)
            revealInspector()
        }
    }

    /// A companion closed itself (its shell ended, its side chat archived).
    func closeCompanion(_ vc: UIViewController) {
        if inspector.viewControllers.count > 1, inspector.viewControllers.last === vc {
            inspector.popViewController(animated: true)
        } else {
            hideCompanions()
        }
    }

    func hideCompanions() {
        view.endEditing(true)
        hide(.inspector)
        if let mode = modeBeforeInspector {
            modeBeforeInspector = nil
            UIView.animate(withDuration: 0.3) { self.preferredDisplayMode = mode }
        }
    }

    private func companionsRoot(for chatId: String) -> CompanionsViewController {
        if inspectorChatId == chatId, let root = inspector.viewControllers.first as? CompanionsViewController { return root }
        inspectorChatId = chatId
        let root = CompanionsViewController(app: app, chatId: chatId, inSheet: false)
        root.close = { [weak self] in self?.hideCompanions() }
        root.showsClose = isCollapsed
        root.open = { [weak self] vc in self?.inspector.pushViewController(vc, animated: true) }
        root.openSideChat = { [weak self] id in
            guard let self, let vc = SessionViewController(app: self.app, chatId: id) else { return }
            vc.isCompanion = true
            self.inspector.pushViewController(vc, animated: true)
        }
        return root
    }

    /// Narrow windows (iPad portrait, the unfolded Duo) can't fit sidebar,
    /// session and inspector: the sidebar steps aside while it shows.
    private func revealInspector() {
        if view.bounds.width < 1100, displayMode != .secondaryOnly, modeBeforeInspector == nil {
            modeBeforeInspector = preferredDisplayMode
            UIView.animate(withDuration: 0.3) { self.preferredDisplayMode = .secondaryOnly }
        }
        show(.inspector)
    }

    /// Another session came up: the open panel follows it.
    private func followInspector(to chatId: String) {
        guard inspectorShown, inspectorChatId != chatId else { return }
        inspector.setViewControllers([companionsRoot(for: chatId)], animated: false)
    }

    /// A session was closed in the tab shell (compact): widening shouldn't
    /// bring it back.
    func sessionDidClose(_ chatId: String) {
        guard isCollapsed, currentChatId == chatId else { return }
        currentChatId = nil
        sidebar.currentChatId = nil
    }

    // MARK: Collapse / expand

    /// Narrowing to compact carries the open session into the tab shell; the
    /// column lets go of its copy (one live view per session). The page
    /// itself moves, at once: rebuilding it showed the list, then a loading
    /// page, under iPhone Duo's fold animation.
    func splitViewController(_ svc: UISplitViewController, topColumnForCollapsingToProposedTopColumn proposed: UISplitViewController.Column) -> UISplitViewController.Column {
        // The companions panel rides along: folded, UIKit shows the
        // inspector as a sheet over the tab shell (with a Close button).
        // The sidebar comes back with the width (it stepped aside for the
        // inspector).
        if let mode = modeBeforeInspector { preferredDisplayMode = mode }
        modeBeforeInspector = nil
        (inspector.viewControllers.first as? CompanionsViewController)?.showsClose = true
        if let session = detail.viewControllers.first as? SessionViewController {
            session.relocating = true
            detail.setViewControllers([], animated: false)
            tabs.adoptSession(session)
            session.relocating = false
            return .compact
        }
        let chatId = currentChatId
        DispatchQueue.main.async {
            if self.detail.viewControllers.first is SessionViewController {
                self.detail.setViewControllers([], animated: false)
            }
            if let chatId { self.tabs.openSession(chatId) }
        }
        return .compact
    }

    /// Widening back brings the session last open in the tab shell into the
    /// main column (or the new-session page when none was).
    func splitViewController(_ svc: UISplitViewController, displayModeForExpandingToProposedDisplayMode proposed: UISplitViewController.DisplayMode) -> UISplitViewController.DisplayMode {
        (inspector.viewControllers.first as? CompanionsViewController)?.showsClose = false
        // The session page moves back as it is (see collapsing above).
        if let session = tabs.releaseSession() {
            currentChatId = session.chatId
            sidebar.currentChatId = session.chatId
            detail.setViewControllers([session], animated: false)
            session.relocating = false
            return proposed
        }
        DispatchQueue.main.async {
            let current = self.currentChatId
            self.tabs.popToFrontPage()
            let shown = (self.detail.viewControllers.first as? SessionViewController)?.chatId
            if let chatId = current {
                if shown != chatId {
                    guard let session = SessionViewController(app: self.app, chatId: chatId) else {
                        return self.showSessionOpenError()
                    }
                    self.detail.setViewControllers([session], animated: false)
                }
                self.sidebar.currentChatId = chatId
            } else if self.detail.viewControllers.isEmpty {
                self.showDraft(prompt: nil, focus: false)
            }
        }
        return proposed
    }
}

/// The iPad sidebar: the phone's Sessions page in its own navigation stack
/// (sections, folders, Archived push inside it), search in the bar, and
/// the actions in a toolbar at the bottom: Settings and options on the
/// left, new session on the right, within thumb reach.
final class SidebarViewController: UIViewController, UISearchResultsUpdating {
    private let app: AppModel
    private let list: SessionsViewController
    private let nav: UINavigationController
    private let search = UISearchController(searchResultsController: nil)
    private var voiceBar: VoiceBarItem?
    /// A zero-size, invisible toolbar item: where it lands is the middle of
    /// the toolbar's row. iOS 26 draws the toolbar outside `nav.toolbar`
    /// (that view isn't even in the window), so the row can't be measured
    /// any other public way.
    private let toolbarProbe = UIView(frame: .zero)
    private var voiceToken: AnyObject?
    private var workspaceToken: AnyObject?

    var currentChatId: String? {
        get { list.currentChatId }
        set { list.currentChatId = newValue }
    }

    /// The vertical middle of the bottom toolbar's buttons, in `window`.
    func toolbarMidY(in window: UIWindow) -> CGFloat? {
        guard !nav.isToolbarHidden, toolbarProbe.window === window else { return nil }
        return toolbarProbe.convert(CGPoint.zero, to: window).y
    }

    init(app: AppModel) {
        self.app = app
        self.list = SessionsViewController(app: app)
        self.nav = MainTabController.nav(list)
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        addChild(nav)
        nav.view.frame = view.bounds
        nav.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(nav.view)
        nav.didMove(toParent: self)

        _ = list.view
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        search.hidesNavigationBarDuringPresentation = false
        search.searchBar.placeholder = "Search"
        search.searchBar.searchTextField.accessibilityIdentifier = "sidebar-search"
        list.navigationItem.searchController = search
        list.navigationItem.hidesSearchBarWhenScrolling = false
        list.navigationItem.preferredSearchBarPlacement = .stacked
        let settings = UIBarButtonItem(image: UIImage(systemName: "gearshape"), primaryAction: UIAction { [weak self] _ in
            self?.router?.showSettings()
        })
        settings.accessibilityLabel = "Settings"
        settings.accessibilityIdentifier = "sidebar-settings"
        let options = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: list.optionsMenu())
        options.accessibilityLabel = "Options"
        let compose = UIBarButtonItem(image: UIImage(systemName: "plus"), primaryAction: UIAction { [weak self] _ in
            self?.router?.presentNewSession(prompt: nil)
        })
        compose.style = .prominent
        compose.tintColor = Palette.accent
        compose.accessibilityLabel = "New session"
        compose.accessibilityIdentifier = "new-session"
        list.navigationItem.rightBarButtonItem = nil
        toolbarProbe.isUserInteractionEnabled = false
        let probe = UIBarButtonItem(customView: toolbarProbe)
        probe.hidesSharedBackground = true
        probe.sharesBackground = false
        probe.isAccessibilityElement = false
        list.toolbarItems = [settings, options, .flexibleSpace(), probe, compose]
        nav.isToolbarHidden = false
        // Voice beside compose, once a host can run it.
        let voiceBar = VoiceBarItem(app: app, host: self)
        self.voiceBar = voiceBar
        let syncVoice = { [weak self, weak voiceBar] in
            guard let self, let voiceBar else { return }
            let items = voiceBar.visible ? [settings, options, .flexibleSpace(), probe, voiceBar.item, compose] : [settings, options, .flexibleSpace(), probe, compose]
            if self.list.toolbarItems?.count != items.count {
                self.list.setToolbarItems(items, animated: self.view.window != nil)
            }
        }
        voiceToken = app.voice.observe(syncVoice)
        workspaceToken = app.observe(syncVoice)
        syncVoice()

        // A hairline between the sidebar and the main column.
        let edge = UIView()
        edge.backgroundColor = Palette.hairline
        edge.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(edge)
        NSLayoutConstraint.activate([
            edge.topAnchor.constraint(equalTo: view.topAnchor),
            edge.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            edge.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            edge.widthAnchor.constraint(equalToConstant: 1 / max(1, traitCollection.displayScale)),
        ])
    }

    /// The split wraps the sidebar in a navigation controller of its own; its
    /// bar would sit empty above the sidebar's, pushing title and search down.
    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        navigationController?.setNavigationBarHidden(true, animated: false)
    }

    func updateSearchResults(for searchController: UISearchController) {
        list.query = searchController.searchBar.text ?? ""
    }

    func focusSearch() {
        nav.popToRootViewController(animated: false)
        search.searchBar.becomeFirstResponder()
    }

    func showSessions() {
        search.isActive = false
        search.searchBar.text = ""
        list.query = ""
        nav.popToRootViewController(animated: false)
    }
}

extension UIViewController {
    /// The shell's router (tabs or split), wherever this controller sits.
    var router: AppRouter? { view.window?.rootViewController as? AppRouter }
}
