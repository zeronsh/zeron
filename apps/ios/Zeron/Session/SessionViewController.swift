import UIKit

/// One session: virtualized transcript under a glass bottom stack (status
/// pill, queue, composer or question panel) that rides the keyboard.
final class SessionViewController: UIViewController, UIGestureRecognizerDelegate {
    private let app: AppModel
    let chatId: String
    private let source: SessionSource
    private lazy var relay = FrameRelay { [weak self] in self?.applyFrame() }
    private lazy var engine = TranscriptView(text: TextEngine.shared, listener: relay)
    private lazy var list = TranscriptListView(engine: engine)
    private let composer = ComposerBar()
    private let questions = QuestionPanel()
    private let queue = QueuePanel()
    private let pill = StatusPill()
    /// What's working alongside: subagents, side chats, terminals.
    private let strip = CompanionStrip()
    private var companionsItem: UIBarButtonItem!
    private var companionTokens: [AnyObject] = []
    /// Shown beside another session (the inspector holds a side chat): it
    /// isn't the split's current session.
    var isCompanion = false
    /// A Cloud chat out of credits says so once, above the composer.
    private let creditsNotice = CloudCreditsNotice()
    private let bottom = UIStackView()
    private var bottomRest: NSLayoutConstraint?
    private let jump = Glass.circleButton(symbol: "arrow.down", size: 40, pointSize: 15)
    private var shown = SessionChrome()
    private let openedAt = CACurrentMediaTime()
    private var reportedOpen = false

    init?(app: AppModel, chatId: String) {
        guard let source = app.sessionSource(chatId) else { return nil }
        self.app = app
        self.chatId = chatId
        self.source = source
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        // The app can be killed in the background: the draft goes to disk first.
        backgroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.saveDraft()
        }
        navigationItem.largeTitleDisplayMode = .never
        // The title and "project @ device" head the ⋯ menu instead of
        // crowding the bar.
        navigationItem.titleView = UIView()
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: sessionMenu(title: "", subtitle: ""))
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "session-menu"
        companionsItem = UIBarButtonItem(image: UIImage(systemName: "square.stack.3d.up"), primaryAction: UIAction { [weak self] _ in
            self?.showCompanions(section: nil)
        })
        companionsItem.accessibilityLabel = "Agents, side chats and terminals"
        companionsItem.accessibilityIdentifier = "session-companions"
        // The session has its own composer, so the tab bar's voice control
        // steps aside with the accessory; the call stays a tap away up here.
        let voiceBar = VoiceBarItem(app: app, host: self)
        self.voiceBar = voiceBar
        // A custom view reads as horizontal-only; on iPhone Duo it joins ⋯ in
        // the vertical bar.
        #if compiler(>=6.4)
        if #available(iOS 27.1, *) { voiceBar.item.axisBehavior = .verticalPreferred }
        #endif
        let menu = navigationItem.rightBarButtonItem!
        let companions = companionsItem!
        let syncVoice = { [weak self, weak voiceBar] in
            guard let self, let voiceBar else { return }
            let items = voiceBar.visible ? [menu, companions, voiceBar.item] : [menu, companions]
            if self.navigationItem.rightBarButtonItems?.count != items.count {
                self.navigationItem.setRightBarButtonItems(items, animated: self.view.window != nil)
            }
        }
        voiceToken = app.voice.observe(syncVoice)
        hostsToken = app.observe(syncVoice)
        syncVoice()

        list.frame = view.bounds
        list.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        list.accessibilityIdentifier = "transcript"
        list.imageLoader = { [weak self] ref, iv in self?.source.loadImage(ref, into: iv) }
        list.imageFetcher = { [weak self] ref in await self?.source.image(ref) }
        // Hidden while following (the runway glide and tail spring travel).
        // An error card's "Log in": that provider's sign-in, started at once.
        list.onAction = { [weak self] action in
            guard let self, action == "sign-in", let target = self.source.signInTarget() else { return }
            let accounts = AgentAccountsViewController(app: self.app, target: target, signIn: true)
            let nav = UINavigationController(rootViewController: accounts)
            if let sheet = nav.sheetPresentationController {
                sheet.detents = [.medium(), .large()]
                sheet.prefersGrabberVisible = true
            }
            self.present(nav, animated: true)
        }
        list.onSubagent = { [weak self] doc, title, running in
            guard let self else { return }
            self.presentCompanion(SubagentViewController(app: self.app, parentId: self.chatId, docId: doc, title: title, running: running))
        }
        list.onDistanceFromBottom = { [weak self] d in
            guard let self else { return }
            self.setJumpVisible(d > 140 && !self.list.following)
        }
        view.addSubview(list)
        // Tapping the transcript puts the composer (and keyboard) away.
        let dismissTap = UITapGestureRecognizer(target: self, action: #selector(dismissComposer))
        dismissTap.cancelsTouchesInView = false
        dismissTap.delegate = self
        list.addGestureRecognizer(dismissTap)
        setContentScrollView(list, for: .top)
        list.topEdgeEffect.style = .soft
        list.bottomEdgeEffect.isHidden = true

        composer.attachMenu = { [weak self] in
            guard let self else { return UIMenu() }
            return AttachmentPicker.menu(host: self, limit: 8 - self.composer.images.count) { [weak self] in self?.composer.addImages($0) }
        }
        // The model chip opens the picker sheet (tabs per provider).
        composer.onChipTap = { [weak self] id, _ in
            guard let self, id == "model", let picker = self.source.modelPicker() else { return }
            self.present(picker, animated: true)
        }
        composer.onSend = { [weak self] text, images, mode in
            guard let self else { return .kept }
            if self.editingQueueId != nil {
                self.endEdit(commit: text)
                return .replaced
            }
            // An immediate send (new turn, steer, stop-and-send) gets the
            // desktop runway; a message queued behind a live turn keeps the
            // live turn's runway and just follows.
            let queued = self.shown.running && mode == .queue
            // Out of credits, a Cloud send can't wake its machine: say so
            // (with a way to add some) instead of a send that never lands.
            if self.cloudOutOfCredits {
                self.creditsNotice.nudge()
                return .kept
            }
            guard self.source.send(text: text, images: images, mode: mode) else { return .kept }
            // Sending puts the keyboard away (the composer rests as the capsule).
            DispatchQueue.main.async { self.composer.resignFirstResponder() }
            if queued { self.list.expectQueuedTurn() } else { self.list.beginOwnTurn() }
            return .sent
        }
        composer.onStop = { [weak self] in self?.source.stop() }
        composer.text = Drafts.load(chatId)
        composer.mentionSearch = { [weak self] q in await self?.source.searchFiles(q) ?? [] }
        composer.onHeightChange = { [weak self] in self?.view.setNeedsLayout() }
        questions.onSubmit = { [weak self] answers in
            guard let self, let id = self.shown.questions?.requestId else { return }
            self.source.answer(requestId: id, answers: answers)
        }
        questions.onHeightChange = { [weak self] in self?.view.setNeedsLayout() }
        queue.onAction = { [weak self] id, action in
            guard let self else { return }
            if action == .edit { self.beginEdit(id) } else { self.source.queueAction(id, action) }
        }
        pill.onTap = { [weak self] in
            guard let self else { return }
            if self.editingQueueId != nil { self.endEdit(commit: nil) }
            if case .notDelivered = self.shown.banner {
                self.source.retryDelivery()
            }
        }
        // Watched whatever the row says now: a just-created Cloud chat's row
        // may not have arrived yet.
        cloudToken = app.cloud.observe { [weak self] in
            guard let self, self.isCloud else { return }
            self.render(animated: true)
        }

        bottom.axis = .vertical
        bottom.spacing = 8
        bottom.alignment = .fill
        bottom.translatesAutoresizingMaskIntoConstraints = false
        strip.onTap = { [weak self] section in self?.showCompanions(section: section) }
        let pillRow = UIStackView(arrangedSubviews: [pill, UIView(), strip])
        pillRow.axis = .horizontal
        pillRow.spacing = 6
        pillRow.alignment = .center
        creditsNotice.onAddCredits = { [weak self] in
            guard let self else { return }
            CloudRoutes.openBilling(app: self.app, from: self)
        }
        creditsNotice.isHidden = true
        for v in [creditsNotice, pillRow, queue, questions, composer] { bottom.addArrangedSubview(v) }
        questions.isHidden = true
        queue.isHidden = true
        // Hidden panels start dematerialized so their first appearance grows in.
        questions.setGlassVisible(false, animated: false)
        queue.setGlassVisible(false, animated: false)
        pillRow.isHidden = true
        view.addSubview(bottom)
        // Transcript fades into the background behind the composer, like the
        // nav bar's edge effect at the top. A gradient overlay (not a mask on
        // the scroll view) so scrolling never renders offscreen.
        bottomFade.translatesAutoresizingMaskIntoConstraints = false
        view.insertSubview(bottomFade, belowSubview: bottom)
        NSLayoutConstraint.activate([
            bottomFade.topAnchor.constraint(equalTo: bottom.topAnchor, constant: -44),
            bottomFade.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            bottomFade.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            bottomFade.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])

        jump.addAction(UIAction { [weak self] _ in self?.list.scrollToBottom(animated: true) }, for: .touchUpInside)
        jump.accessibilityIdentifier = "jump-to-latest"
        jump.accessibilityLabel = "Jump to latest"
        jump.alpha = 0
        jump.transform = CGAffineTransform(scaleX: 0.6, y: 0.6)
        view.addSubview(jump)

        NSLayoutConstraint.activate([
            // Full width on phones; a centered reading column on iPad/landscape.
            bottom.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            bottom.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            bottom.widthAnchor.constraint(lessThanOrEqualToConstant: 768),
            {
                let fill = bottom.widthAnchor.constraint(equalTo: view.safeAreaLayoutGuide.widthAnchor, constant: -24)
                fill.priority = .defaultHigh
                return fill
            }(),
            jump.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            jump.bottomAnchor.constraint(equalTo: bottom.topAnchor, constant: -12),
        ])
        bottomRest = pinAboveKeyboard(bottom)

        source.onChange = { [weak self] in self?.render(animated: true) }
        source.attach(engine)
        render(animated: false)
        app.markSeen(chatId)
        Self.liveSessions[chatId] = WeakSession(self)
        companionTokens = [
            app.observeSession(chatId) { [weak self] in self?.scheduleCompanionSync() },
            app.observe { [weak self] in self?.scheduleCompanionSync() },
            app.terminals.observe { [weak self] chat in
                guard let self, chat == self.chatId else { return }
                self.scheduleCompanionSync()
            },
        ]
        syncCompanions(animated: false)
    }

    // MARK: Companions (subagents, side chats, terminals)

    private final class WeakSession {
        weak var session: SessionViewController?
        init(_ session: SessionViewController) { self.session = session }
    }

    nonisolated(unsafe) private static var liveSessions: [String: WeakSession] = [:]

    /// The session screen showing `chatId`, if one is open.
    static func live(_ chatId: String) -> SessionViewController? {
        liveSessions[chatId]?.session
    }

    /// Text into the composer (terminal output, say), after what's there.
    func appendToComposer(_ text: String) {
        let current = composer.text
        composer.text = current.isEmpty ? text : current + (current.hasSuffix("\n") ? "" : "\n") + text
        saveDraft()
    }

    private var parentId: String? { app.row(chatId)?.parentChatId }

    private var companionSyncScheduled = false

    /// Streaming fires a session change per frame; the chips settle at most
    /// a few times a second (a subagent count doesn't need 120 Hz).
    private func scheduleCompanionSync() {
        guard !companionSyncScheduled else { return }
        companionSyncScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in
            guard let self else { return }
            self.companionSyncScheduled = false
            self.syncCompanions()
        }
    }

    private func syncCompanions(animated: Bool = true) {
        let agents = app.sessionSubagents(chatId)
        let tabs = app.terminals.tabs(for: chatId)
        // A side chat's own siblings live with its parent.
        let sides = parentId == nil ? app.sideChats(of: chatId).count : 0
        var state = CompanionStrip.State()
        state.agents = agents.count
        state.runningAgents = agents.filter { $0.state == .running }.count
        state.sideChats = sides
        state.terminals = tabs.count
        state.liveTerminals = tabs.filter(\.isLive).count
        let wasEmpty = strip.state.isEmpty
        if state != strip.state {
            strip.apply(state)
            if wasEmpty != state.isEmpty { render(animated: animated) }
        }
        // Violet like the working glyphs: a running agent isn't an alert.
        companionsItem.badge = state.runningAgents > 0 ? {
            var badge = UIBarButtonItem.Badge.count(state.runningAgents)
            badge.backgroundColor = Palette.accent
            badge.foregroundColor = .white
            return badge
        }() : nil
        if parentId != nil, navigationItem.subtitle == nil {
            let parent = parentId.flatMap { app.row($0)?.title } ?? ""
            navigationItem.titleView = nil
            navigationItem.title = "Side chat"
            navigationItem.subtitle = parent
        }
    }

    /// A companion screen (subagent, terminal, side chat): beside the
    /// session in the inspector where there's room, else on top of it.
    func presentCompanion(_ vc: UIViewController) {
        if !isCompanion, let split = splitViewController as? SplitRootController, !split.isCollapsed {
            split.showCompanion(vc, for: chatId)
        } else {
            navigationController?.pushViewController(vc, animated: true)
        }
    }

    func openSideChat(_ id: String) {
        guard let vc = SessionViewController(app: app, chatId: id) else {
            return Toast.show("Couldn't open the side chat", in: view.window)
        }
        // Beside or above its parent, never the split's current session.
        vc.isCompanion = true
        presentCompanion(vc)
    }

    /// The companions panel: the inspector beside the session, or a sheet.
    func showCompanions(section: CompanionsViewController.Section?) {
        if !isCompanion, let split = splitViewController as? SplitRootController, !split.isCollapsed {
            split.toggleCompanions(for: chatId, section: section)
            return
        }
        let hub = CompanionsViewController(app: app, chatId: chatId, inSheet: true)
        hub.initialSection = section
        let nav = UINavigationController(rootViewController: hub)
        nav.navigationBar.titleTextAttributes = [.font: Fonts.ui(.sansSemibold, 17), .foregroundColor: Palette.text]
        nav.navigationBar.tintColor = Palette.text
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.medium(), .large()]
            sheet.prefersGrabberVisible = true
            sheet.prefersScrollingExpandsWhenScrolledToEdge = false
        }
        hub.open = { [weak self, weak nav] vc in
            nav?.dismiss(animated: true)
            self?.navigationController?.pushViewController(vc, animated: true)
        }
        hub.openSideChat = { [weak self, weak nav] id in
            nav?.dismiss(animated: true)
            self?.openSideChat(id)
        }
        present(nav, animated: true)
    }

    func openTerminal() {
        let tabs = app.terminals.tabs(for: chatId)
        presentCompanion(TerminalViewController(app: app, chatId: chatId, tab: tabs.first(where: \.isLive)))
    }

    override var keyCommands: [UIKeyCommand]? {
        [
            UIKeyCommand(title: "Terminal", action: #selector(terminalCommand), input: "j", modifierFlags: .command),
            UIKeyCommand(title: "Agents, Side Chats & Terminals", action: #selector(companionsCommand), input: "0", modifierFlags: [.command, .alternate]),
        ]
    }

    @objc private func terminalCommand() { openTerminal() }
    @objc private func companionsCommand() { showCompanions(section: nil) }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        // Coming back on screen (e.g. after a second copy of this chat was
        // popped and detached it): live again.
        if hasAppeared { source.reattach() }
    }

    private var hasAppeared = false
    /// Set by the split for the moment it moves this page between its
    /// shells, so the move doesn't read as closing the session.
    var relocating = false
    private var backgroundObserver: NSObjectProtocol?
    private var voiceBar: VoiceBarItem?
    private var voiceToken: AnyObject?
    private var hostsToken: AnyObject?

    deinit {
        if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver) }
    }

    // MARK: Opening (loader, then the transcript)

    /// Shown while a pushed session loads.
    private let loader = StatusGlyph(.spinner)
    /// The transcript waits for the push to land before showing: the soft
    /// edge effect under the bar can't pick up content that appears mid-push
    /// (it read crisp under the title, then the effect snapped on). The page
    /// slides in with a loader; the transcript fades in once the push is
    /// done and its rows are laid out.
    private var holdingTranscript = false
    private var landed = false

    override func viewIsAppearing(_ animated: Bool) {
        super.viewIsAppearing(animated)
        // First appearance by an animated push (the new-session handoff and
        // unanimated column swaps reveal their own way).
        guard animated, !hasAppeared, transitionCoordinator != nil, list.alpha > 0 else { return }
        holdingTranscript = true
        list.alpha = 0
        if loader.superview == nil {
            loader.translatesAutoresizingMaskIntoConstraints = false
            view.insertSubview(loader, aboveSubview: list)
            NSLayoutConstraint.activate([
                loader.centerXAnchor.constraint(equalTo: view.centerXAnchor),
                loader.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -40),
                loader.widthAnchor.constraint(equalToConstant: 12),
                loader.heightAnchor.constraint(equalToConstant: 12),
            ])
            loader.transform = CGAffineTransform(scaleX: 1.75, y: 1.75)
        }
        loader.alpha = 1
        loader.accessibilityIdentifier = "session-loading"
    }

    /// Both conditions met (push landed, rows in): show the transcript.
    private func showTranscriptIfReady() {
        guard holdingTranscript, landed, reportedOpen else { return }
        holdingTranscript = false
        list.settleEdgeEffect()
        let reveal = {
            self.list.alpha = 1
            self.loader.alpha = 0
        }
        if UIAccessibility.isReduceMotionEnabled { return reveal() }
        UIView.animate(withDuration: 0.18, delay: 0, options: [.curveEaseOut, .allowUserInteraction], animations: reveal)
    }

    #if DEBUG
    private static var debugPickerShown = false
    private static var debugCompanionShown = false

    /// `-companions`, `-terminal`, `-subagent <tool id>`, `-side-chat <id>`.
    private func debugOpenCompanion(_ args: [String]) {
        func value(_ flag: String) -> String? {
            args.firstIndex(of: flag).flatMap { $0 + 1 < args.count ? args[$0 + 1] : nil }
        }
        if args.contains("-companions") {
            showCompanions(section: nil)
        } else if args.contains("-terminal") {
            let vc = TerminalViewController(app: app, chatId: chatId, tab: nil)
            vc.script = args.contains("-terminal-demo") ? ["git log --oneline -5", "cargo test -p zeron-veil"] : []
            presentCompanion(vc)
        } else if let tool = value("-subagent"), let row = app.sessionSubagents(chatId).first(where: { $0.id == tool || $0.docId?.hasSuffix(tool) == true }), let doc = row.docId {
            presentCompanion(SubagentViewController(app: app, parentId: chatId, docId: doc, title: row.title, running: row.state == .running))
        } else if let side = value("-side-chat") {
            openSideChat(side)
        }
    }
    #endif

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        hasAppeared = true
        landed = true
        if !isCompanion { (splitViewController as? SplitRootController)?.sessionDidAppear(chatId) }
        list.settleEdgeEffect()
        #if DEBUG
        // Screenshots: `-model-picker` opens the picker once.
        if ProcessInfo.processInfo.arguments.contains("-model-picker"), !Self.debugPickerShown, let picker = source.modelPicker() {
            Self.debugPickerShown = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.6) { [weak self] in self?.present(picker, animated: true) }
        }
        #endif
        #if DEBUG
        // Screenshots: open a companion once.
        let args = ProcessInfo.processInfo.arguments
        if !Self.debugCompanionShown, !isCompanion, parentId == nil {
            Self.debugCompanionShown = true
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.8) { [weak self] in self?.debugOpenCompanion(args) }
        }
        #endif
        showTranscriptIfReady()
        if holdingTranscript {
            // Nothing to lay out (an empty or unreachable session): stop
            // holding eventually rather than spin forever.
            DispatchQueue.main.asyncAfter(deadline: .now() + 8) { [weak self] in
                guard let self, self.holdingTranscript else { return }
                self.reportedOpen = true
                self.showTranscriptIfReady()
            }
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // Leaving mid-edit: give the queued row back and the draft its place.
        if editingQueueId != nil { endEdit(commit: nil) }
        saveDraft()
    }

    /// Tear down only once the pop has really happened: an interactive
    /// swipe-back reports `isMovingFromParent` in viewWillDisappear too, and
    /// if it's cancelled the session stays on screen — detaching there left it
    /// frozen (no new updates) until you left and came back.
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        // Carried to the other shell (iPhone Duo folding or unfolding): it
        // left one stack for another and stays live. UIKit may report the
        // move late or not at all, so ask where the page is, not how it left.
        if relocating || navigationController?.viewControllers.contains(self) == true { return }
        if isMovingFromParent || navigationController == nil { teardown() }
    }

    /// Taken off a stack while not on screen (a pop-to-root under a pushed
    /// companion, the inspector reset for another chat): no disappear
    /// callback comes, so this is where it ends.
    override func didMove(toParent parent: UIViewController?) {
        super.didMove(toParent: parent)
        if parent == nil, !relocating { teardown() }
    }

    private var tornDown = false

    /// The session closed for good: stop following it (once).
    private func teardown() {
        guard !tornDown else { return }
        tornDown = true
        if Self.liveSessions[chatId]?.session === self { Self.liveSessions[chatId] = nil }
        // A side chat started here and left without a word: gone.
        if app.freshSideChats.contains(chatId), source.isUnused, Drafts.load(chatId).isEmpty, app.row(chatId)?.preview == nil {
            app.forgetFreshSideChat(chatId)
            app.delete(chatId)
        }
        if mainShell != nil, !isCompanion { (splitViewController as? SplitRootController)?.sessionDidClose(chatId) }
        source.detach()
        engine.close()
        app.markSeen(chatId)
    }

    // MARK: Arrival from the new-session draft (DraftHandoffAnimator)

    /// Content hidden until the handoff reveals it; the keyboard goes away
    /// with the draft (sending dismisses it).
    func prepareArrival() {
        loadViewIfNeeded()
        list.beginOwnTurn()
        list.alpha = 0
        bottom.alpha = 0
        view.layoutIfNeeded()
    }

    func arrivalComposerFrame(in window: UIWindow) -> CGRect? {
        view.layoutIfNeeded()
        return composer.convert(composer.bounds, to: window)
    }

    func arrivalBubbleFrame(in window: UIWindow) -> CGRect? {
        list.layoutIfNeeded()
        return list.firstUserBubble(in: window)
    }

    func revealArrivalComposer() { bottom.alpha = 1 }
    func revealArrivalTranscript() { list.alpha = 1 }
    func finishArrival() {
        list.alpha = 1
        bottom.alpha = 1
    }

    @objc private func dismissComposer() {
        if composer.textView.isFirstResponder { composer.resignFirstResponder() }
    }

    /// Controls in the transcript (tool rows, copy, disclosure) keep their taps.
    func gestureRecognizer(_ g: UIGestureRecognizer, shouldReceive touch: UITouch) -> Bool {
        var v = touch.view
        while let view = v, view !== list {
            if view is UIControl { return false }
            v = view.superview
        }
        return true
    }

    /// Putting the composer away rides along with the transcript's own taps
    /// (links, images). Tap recognizers are exclusive by default: this one
    /// won and every link tap was dropped.
    func gestureRecognizer(_ g: UIGestureRecognizer, shouldRecognizeSimultaneouslyWith other: UIGestureRecognizer) -> Bool {
        other.view.map { $0 === list || $0.isDescendant(of: list) } ?? false
    }

    private let bottomFade = EdgeFadeOverlay()

    override func viewWillLayoutSubviews() {
        super.viewWillLayoutSubviews()
        bottomRest?.constant = -composerRestGap
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let covered = view.bounds.maxY - bottom.frame.minY + 8
        let inset = max(0, covered - view.safeAreaInsets.bottom)
        if abs(list.contentInset.bottom - inset) > 0.5 {
            let following = list.following
            list.contentInset.bottom = inset
            list.verticalScrollIndicatorInsets.bottom = inset
            if following { list.contentOffset.y = list.maxOffsetY }
        }
    }

    // MARK: Queue editing (host lease)

    private var editingQueueId: String?
    private var stashedDraft = ""
    private var stashedImages: [StagedImage] = []
    /// The draft is parked in the stash (from the first edit until the last ends).
    private var stashed = false

    /// The user's own draft (never a queued message being edited).
    private func saveDraft() {
        Drafts.save(chatId, stashed ? stashedDraft : composer.text)
    }

    /// An edit lease is being requested (taps on other rows wait).
    private var editPending = false

    private func beginEdit(_ id: String) {
        guard !editPending else { return }
        editPending = true
        let switching = editingQueueId != nil
        if switching {
            // Row A goes back and the user's own draft returns right away, so
            // nothing of A's edit can be sent as a new message meanwhile.
            editingQueueId = nil
            restoreStash()
            composer.placeholder = shown.placeholder
            render(animated: true)
        }
        Task { @MainActor in
            defer { editPending = false }
            if switching { await source.finishEdit(text: nil) }
            guard let text = await source.beginEdit(id) else {
                let alert = UIAlertController(title: "Can't edit right now", message: "Another device is editing this message, or it was just sent.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                present(alert, animated: true)
                return
            }
            // Left the session while waiting: give the row straight back.
            guard view.window != nil else {
                await source.finishEdit(text: nil)
                return
            }
            if !stashed {
                stashedDraft = composer.text
                stashedImages = composer.images
                stashed = true
            }
            editingQueueId = id
            composer.images = []
            composer.text = text
            composer.placeholder = "Edit queued message"
            pill.banner = .editing
            pill.superview?.isHidden = false
            composer.becomeFirstResponder()
        }
    }

    private func restoreStash() {
        guard stashed else { return }
        composer.text = stashedDraft
        composer.images = stashedImages
        stashedImages = []
        stashed = false
    }

    private func endEdit(commit text: String?) {
        editingQueueId = nil
        restoreStash()
        composer.placeholder = shown.placeholder
        Task { await source.finishEdit(text: text) }
        render(animated: true)
    }

    private func applyFrame() {
        let frame = engine.frame()
        list.apply(frame)
        if !reportedOpen, frame.rowCount() > 0 {
            reportedOpen = true
            showTranscriptIfReady()
            // Open latency: push → first measured frame on screen.
            let ms = (CACurrentMediaTime() - openedAt) * 1000
            let json = String(format: "{\"openMs\":%.1f,\"rows\":%d,\"layoutPassMs\":%.2f}", ms, frame.rowCount(), Double(frame.buildMicros()) / 1000)
            NSLog("OPEN %@", json)
            if ProcessInfo.processInfo.arguments.contains("-measureopen") {
                let docs = FileManager.default.urls(for: .documentDirectory, in: .userDomainMask)[0]
                try? json.write(to: docs.appendingPathComponent("open.json"), atomically: true, encoding: .utf8)
            }
        }
    }

    private func setJumpVisible(_ visible: Bool) {
        guard (jump.alpha > 0.5) != visible else { return }
        UIView.animate(withDuration: 0.35, delay: 0, usingSpringWithDamping: 0.8, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState]) {
            self.jump.alpha = visible ? 1 : 0
            self.jump.transform = visible ? .identity : CGAffineTransform(scaleX: 0.6, y: 0.6)
        }
    }

    private var cloudToken: AnyObject?

    private var isCloud: Bool { app.row(chatId)?.isCloud == true }

    /// Out of credits, the chat's Cloud machine can't start or wake.
    private var cloudOutOfCredits: Bool { isCloud && app.cloud.outOfCredits }

    /// A Cloud chat whose send can't land says why instead of a bare "Not
    /// delivered": out of credits, the credits notice already does (the
    /// pill steps aside); a failed machine, the pill names it.
    private func cloudBanner(_ banner: SessionChrome.Banner) -> SessionChrome.Banner {
        guard case .notDelivered = banner, isCloud else { return banner }
        let cloud = app.cloud
        cloud.refreshSessions()
        if cloud.outOfCredits {
            return .none
        }
        if let error = cloud.sessionError(chat: chatId, device: app.row(chatId)?.deviceId) {
            return .failed("Cloud machine: \(error) · Tap to retry")
        }
        return banner
    }

    private func render(animated: Bool) {
        let c = source.chrome
        let old = shown
        shown = c
        if c.title != old.title || c.subtitle != old.subtitle || navigationItem.title == nil {
            navigationItem.title = c.title
            navigationItem.rightBarButtonItem?.menu = sessionMenu(title: c.title, subtitle: c.subtitle)
        }
        list.uploadProgress = c.uploadProgress
        composer.running = c.running
        composer.canSteer = c.canSteer
        composer.placeholder = c.placeholder
        composer.chips = c.chips
        composer.chipMenus = Dictionary(uniqueKeysWithValues: c.chips.map { chip in
            (chip.id, { [weak self] in self?.source.chipMenu(chip.id) })
        })
        // One banner for both the row's animation and its contents (the
        // Cloud reading may turn a "not delivered" into nothing).
        let banner: SessionChrome.Banner = editingQueueId != nil ? .editing : cloudBanner(c.banner)
        let changes = {
            let asking = c.questions != nil
            self.questions.isHidden = !asking
            self.composer.isHidden = asking
            if let q = c.questions { self.questions.configure(q.items) }
            self.queue.isHidden = c.queue.isEmpty || asking
            self.queue.configure(c.queue)
            self.pill.isHidden = banner == .none
            self.pill.superview?.isHidden = banner == .none && self.strip.state.isEmpty
            self.pill.banner = banner
            self.creditsNotice.isHidden = !self.cloudOutOfCredits
            self.bottom.layoutIfNeeded()
            self.view.layoutIfNeeded()
        }
        let stripShown = !(pill.superview?.isHidden ?? true)
        let stripWanted = banner != .none || !strip.state.isEmpty
        let structural = (old.questions == nil) != (c.questions == nil) || old.queue.count != c.queue.count || (old.banner == .none) != (c.banner == .none) || stripShown != stripWanted
        // Panels materialize when their content appears; a first render (the
        // session reopened with a queue or an open question) shows them at
        // once — they used to stay dematerialized, so the queue "vanished"
        // after navigating away and back.
        if (old.questions == nil) != (c.questions == nil) || !animated {
            questions.setGlassVisible(c.questions != nil, animated: animated)
        }
        if old.queue.isEmpty != c.queue.isEmpty || !animated {
            queue.setGlassVisible(!c.queue.isEmpty, animated: animated)
        }
        if animated, structural {
            UIView.animate(withDuration: 0.38, delay: 0, usingSpringWithDamping: 0.86, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: changes)
        } else {
            changes()
        }
    }

    /// Headed by the session's title over "project @ device".
    private func sessionMenu(title: String, subtitle: String) -> UIMenu {
        // The header wraps: title, then where it runs, on its own line (a
        // menu's subtitle isn't drawn in the header).
        let header = subtitle.isEmpty ? title : "\(title)\n\(subtitle)"
        return UIMenu(title: header, children: [UIDeferredMenuElement.uncached { [weak self] done in
            guard let self else { return done([]) }
            let vm = self.app.session(self.chatId)
            let pinned = vm?.pinned ?? false
            let isSide = self.parentId != nil
            let companions = UIMenu(options: .displayInline, children: [
                UIAction(title: "New Side Chat", image: UIImage(systemName: "bubble.left.and.text.bubble.right")) { _ in
                    guard let root = isSide ? self.parentId : self.chatId, let id = self.app.createSideChat(parentId: root) else { return }
                    self.openSideChat(id)
                },
                UIAction(title: "Fork Into Side Chat", image: UIImage(systemName: "arrow.triangle.branch")) { _ in self.fork() },
                UIAction(title: "Terminal", image: UIImage(systemName: "apple.terminal")) { _ in self.openTerminal() },
            ])
            done([
                companions,
                UIAction(title: pinned ? "Unpin" : "Pin", image: UIImage(systemName: pinned ? "pin.slash" : "pin")) { _ in self.app.setPinned(self.chatId, !pinned) },
                UIAction(title: "Copy Transcript", image: UIImage(systemName: "doc.on.doc")) { _ in
                    UIPasteboard.general.string = self.engine.frame().plainText()
                },
                UIAction(title: "Archive", image: UIImage(systemName: "archivebox")) { _ in
                    self.app.archive(self.chatId)
                    self.leave()
                },
                UIAction(title: "Delete", image: UIImage(systemName: "trash"), attributes: .destructive) { _ in
                    self.confirmDelete(app: self.app, self.chatId) { self.leave() }
                },
            ])
        }])
    }

    private func fork() {
        Toast.show("Forking…", in: view.window)
        Task { @MainActor in
            do {
                let id = try await app.forkSideChat(sourceId: chatId)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                openSideChat(id)
            } catch {
                Toast.show(CompanionsViewController.forkError(error), in: view.window)
            }
        }
    }

    /// Off this session once it's gone from the list. Beside the iPad
    /// sidebar there's nothing to pop back to.
    private func leave() {
        if isCompanion {
            if let nav = navigationController, nav.viewControllers.count > 1 {
                nav.popViewController(animated: true)
            } else {
                (splitViewController as? SplitRootController)?.closeCompanion(self)
            }
            return
        }
        if let split = splitViewController as? SplitRootController, !split.isCollapsed {
            split.showDraft(prompt: nil, focus: false)
        } else {
            navigationController?.popViewController(animated: true)
        }
    }
}

extension UIViewController {
    /// "Delete session?" — permanent, like the desktop's dialog; a Cloud
    /// session's machine goes with it. `then` runs after the delete.
    func confirmDelete(app: AppModel?, _ id: String, then: @escaping () -> Void) {
        guard let app else { return }
        let row = app.row(id)
        let title = row.map { $0.hasTitle ? $0.title : "New session" } ?? "This session"
        let message = row?.isCloud == true
            ? "\u{201C}\(title)\u{201D} and its Cloud machine will be permanently deleted, with anything not pushed from it. This can\u{2019}t be undone."
            : "\u{201C}\(title)\u{201D} will be permanently deleted. This can\u{2019}t be undone."
        let alert = UIAlertController(title: "Delete session?", message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Delete", style: .destructive) { _ in
            app.delete(id)
            then()
        })
        present(alert, animated: true)
    }

    /// Pins a composer 8pt above the keyboard; at rest it sits on the safe
    /// area, `composerRestGap` up. Returns the rest constraint.
    func pinAboveKeyboard(_ bar: UIView) -> NSLayoutConstraint {
        view.keyboardLayoutGuide.usesBottomSafeArea = false
        let rest = bar.bottomAnchor.constraint(lessThanOrEqualTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -composerRestGap)
        let down = bar.bottomAnchor.constraint(equalTo: view.bottomAnchor)
        down.priority = .defaultLow
        NSLayoutConstraint.activate([
            bar.bottomAnchor.constraint(lessThanOrEqualTo: view.keyboardLayoutGuide.topAnchor, constant: -8),
            rest,
            down,
        ])
        return rest
    }

    /// Beside the sidebar the composer rests centered on the sidebar's
    /// bottom toolbar, as measured: where the system puts that toolbar
    /// differs by device and OS (on the unfolded Duo its buttons sit ~10pt
    /// below the safe area, on iPad on it), so a fixed gap left the two out
    /// of line. Negative sinks the composer into the safe area, as the
    /// toolbar is. On its own (tab shell, sheet, sidebar hidden) 8pt up.
    var composerRestGap: CGFloat {
        guard mainShell == nil, let split = splitViewController as? SplitRootController,
              let window = view.window, let middle = split.sidebarToolbarMidY(in: window)
        else { return 8 }
        let y = view.convert(CGPoint(x: 0, y: middle), from: window).y
        let safeBottom = view.bounds.maxY - view.safeAreaInsets.bottom
        return (safeBottom - y - ComposerBar.restHeight / 2).rounded()
    }
}

/// "Out of Cloud credits" with its one way forward, above a Cloud chat's
/// composer while the balance is spent. A send nudges it.
final class CloudCreditsNotice: UIView {
    var onAddCredits: (() -> Void)?
    private let glass = Glass.surface()

    override init(frame: CGRect) {
        super.init(frame: frame)
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        let icon = UIImageView(image: CloudUI.symbol("exclamationmark.triangle.fill", 13, weight: .semibold))
        icon.tintColor = Palette.danger
        icon.contentMode = .center
        let label = UILabel()
        label.text = "Out of Cloud credits"
        label.font = Fonts.ui(.sansMedium, 14)
        label.textColor = Palette.text
        label.setContentHuggingPriority(UILayoutPriority(1), for: .horizontal)
        var c = UIButton.Configuration.filled()
        c.title = "Add credits"
        c.baseBackgroundColor = Palette.text
        c.baseForegroundColor = Palette.background
        c.cornerStyle = .capsule
        c.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 12)
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansSemibold, 13)
            return a
        }
        let button = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in self?.onAddCredits?() })
        button.setContentHuggingPriority(.required, for: .horizontal)
        button.setContentCompressionResistancePriority(.required, for: .horizontal)
        button.accessibilityIdentifier = "add-credits"
        let row = UIStackView(arrangedSubviews: [icon, label, button])
        row.alignment = .center
        row.spacing = 8
        row.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(row)
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.topAnchor.constraint(equalTo: glass.contentView.topAnchor, constant: 6),
            row.bottomAnchor.constraint(equalTo: glass.contentView.bottomAnchor, constant: -6),
            row.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -6),
        ])
        accessibilityIdentifier = "cloud-credits-notice"
    }

    required init?(coder: NSCoder) { fatalError() }

    /// A send was refused: a short shake and a warning tap point at it.
    func nudge() {
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
        guard !UIAccessibility.isReduceMotionEnabled else { return }
        let shake = CAKeyframeAnimation(keyPath: "transform.translation.x")
        shake.values = [0, -8, 8, -5, 5, -2, 0]
        shake.duration = 0.4
        layer.add(shake, forKey: "nudge")
    }
}

/// Small glass capsule above the composer: working/elapsed, offline, retry.
final class StatusPill: UIControl {
    var onTap: (() -> Void)?
    var banner: SessionChrome.Banner = .none { didSet { if banner != oldValue { update() } } }
    private let glass = Glass.surface()
    private let grid = StatusGlyph(.trailer)
    private let label = UILabel()
    private var timer: Timer?

    override init(frame: CGRect) {
        super.init(frame: frame)
        glass.isUserInteractionEnabled = false
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        label.font = Fonts.ui(.sansMedium, 13)
        label.textColor = Palette.secondary
        for v in [grid, label] {
            v.translatesAutoresizingMaskIntoConstraints = false
            glass.contentView.addSubview(v)
        }
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            heightAnchor.constraint(equalToConstant: 30),
            grid.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 11),
            grid.centerYAnchor.constraint(equalTo: glass.contentView.centerYAnchor),
            grid.widthAnchor.constraint(equalToConstant: 12),
            grid.heightAnchor.constraint(equalToConstant: 12),
            label.leadingAnchor.constraint(equalTo: grid.trailingAnchor, constant: 8),
            label.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -12),
            label.centerYAnchor.constraint(equalTo: glass.contentView.centerYAnchor),
        ])
        addAction(UIAction { [weak self] _ in self?.onTap?() }, for: .touchUpInside)
    }

    required init?(coder: NSCoder) { fatalError() }

    private func update() {
        timer?.invalidate()
        timer = nil
        switch banner {
        case .none:
            break
        case let .working(since, word):
            grid.kind = .trailer
            let render = { [weak self] in
                let secs = since.map { Int(Date().timeIntervalSince($0)) } ?? 0
                self?.label.text = secs > 0 ? "\(word) · \(Self.elapsed(secs))" : "\(word)…"
            }
            render()
            timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in render() }
        case .sending:
            grid.kind = .trailer
            label.text = "Sending to host…"
        case .offline:
            grid.kind = .dot(StatusTone.idle)
            label.text = "Offline — sends are saved"
        case let .reconnecting(secs):
            grid.kind = .dot(StatusTone.idle)
            label.text = "Reconnecting in \(secs)s"
        case .notDelivered:
            grid.kind = .dot(Palette.danger)
            label.text = "Not delivered · Tap to retry"
        case let .failed(message):
            grid.kind = .dot(Palette.danger)
            label.text = message
        case .editing:
            grid.kind = .dot(StatusTone.input)
            label.text = "Editing queued message · Tap to cancel"
        }
    }

    static func elapsed(_ s: Int) -> String {
        s < 60 ? "\(s)s" : s < 3600 ? "\(s / 60)m \(s % 60)s" : "\(s / 3600)h \(s / 60 % 60)m"
    }

    override func willMove(toWindow newWindow: UIWindow?) {
        super.willMove(toWindow: newWindow)
        if newWindow == nil { timer?.invalidate() } else { update() }
    }
}

/// Unsent composer text per session, kept across navigation and launches.
enum Drafts {
    private static let key = "drafts"

    static func load(_ chatId: String) -> String {
        (UserDefaults.standard.dictionary(forKey: key) as? [String: String])?[chatId] ?? ""
    }

    static func save(_ chatId: String, _ text: String) {
        var all = (UserDefaults.standard.dictionary(forKey: key) as? [String: String]) ?? [:]
        let trimmed = text.trimmingCharacters(in: .whitespacesAndNewlines)
        all[chatId] = trimmed.isEmpty ? nil : text
        UserDefaults.standard.set(all, forKey: key)
    }
}
