import UIKit

/// One session: virtualized transcript under a glass bottom stack (status
/// pill, queue, composer or question panel) that rides the keyboard.
class SessionViewController: UIViewController, UIGestureRecognizerDelegate {
    private let app: AppModel?
    let chatId: String
    private let source: SessionSource
    private lazy var relay = FrameRelay { [weak self] in self?.applyFrame() }
    private lazy var engine = TranscriptView(text: TextEngine.shared, listener: relay)
    private lazy var list = TranscriptListView(engine: engine)
    private let composer = ComposerBar()
    private let questions = QuestionPanel()
    private let queue = QueuePanel()
    private let pill = StatusPill()
    private let bottom = UIStackView()
    private let loadingBar = UIProgressView(progressViewStyle: .bar)
    private let loadingLabel = UILabel()
    private let loadingStack = UIStackView()
    private let jump = Glass.circleButton(symbol: "arrow.down", size: 40, pointSize: 15)
    private let titleView = SessionTitleView()
    private var shown = SessionChrome()
    private let openedAt = CACurrentMediaTime()
    private var reportedOpen = false

    init(app: AppModel, chatId: String) {
        self.app = app
        self.chatId = chatId
        self.source = app.sessionSource(chatId)
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    init(source: SessionSource, chatId: String) {
        self.app = nil
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
        navigationItem.titleView = titleView
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: sessionMenu())
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "session-menu"

        list.frame = view.bounds
        list.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        list.accessibilityIdentifier = "transcript"
        list.imageLoader = { [weak self] ref, iv in self?.source.loadImage(ref, into: iv) }
        // Hidden while following (the runway glide and tail spring travel).
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
            return self.attachmentMenu()
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
            if case .notDelivered = self.shown.banner { self.source.retryDelivery() }
        }

        bottom.axis = .vertical
        bottom.spacing = 8
        bottom.alignment = .fill
        bottom.translatesAutoresizingMaskIntoConstraints = false
        let pillRow = UIStackView(arrangedSubviews: [pill, UIView()])
        pillRow.axis = .horizontal
        loadingLabel.font = Fonts.ui(.sans, 13)
        loadingLabel.textColor = Palette.secondary
        loadingBar.accessibilityIdentifier = "native-codex-loading"
        loadingStack.axis = .vertical
        loadingStack.spacing = 6
        loadingStack.addArrangedSubview(loadingLabel)
        loadingStack.addArrangedSubview(loadingBar)
        loadingStack.isHidden = true
        for v in [loadingStack, pillRow, queue, questions, composer] { bottom.addArrangedSubview(v) }
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
            bottom.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -8),
            jump.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            jump.bottomAnchor.constraint(equalTo: bottom.topAnchor, constant: -12),
        ])

        source.onChange = { [weak self] in self?.render(animated: true) }
        source.attach(engine)
        render(animated: false)
        app?.markSeen(chatId)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        setAccessory(visible: false)
        // Coming back on screen (e.g. after a second copy of this chat was
        // popped and detached it): live again.
        if hasAppeared { source.reattach() }
    }

    private var hasAppeared = false
    private var backgroundObserver: NSObjectProtocol?

    deinit {
        if let backgroundObserver { NotificationCenter.default.removeObserver(backgroundObserver) }
    }

    /// Swap the tab accessory *inside* the push/pop animation so it travels
    /// with the tab bar instead of snapping ahead of it (no empty glass pill,
    /// no composer ghosting over the tab bar).
    private func setAccessory(visible: Bool) {
        guard let tabs = tabBarController as? MainTabController else { return }
        guard let coordinator = transitionCoordinator else {
            return tabs.setAccessoryVisible(visible, animated: false)
        }
        // Our bottom glass (composer stack) and the tab bar's glass never
        // overlap: the stack fades opposite to the tab bar, tracking an
        // interactive swipe-back too.
        let bottomStack = bottom
        let jumpButton = jump
        if visible {
            bottomStack.alpha = 1
            tabs.prepareAccessoryForReveal()
        } else {
            bottomStack.alpha = 0
        }
        coordinator.animate(alongsideTransition: { _ in
            if visible {
                tabs.setAccessoryContentAlpha(1)
                jumpButton.alpha = 0
            } else {
                tabs.setAccessoryVisible(false, animated: false)
            }
            bottomStack.alpha = visible ? 0 : 1
        }, completion: { context in
            // An interactive pop that's cancelled keeps the session on screen.
            // Re-derive the accessory from what's actually on top rather than
            // flipping: a cancelled pop also re-runs viewWillAppear, and two
            // flips left "New session" showing over the session.
            if context.isCancelled { bottomStack.alpha = 1 }
            tabs.syncAccessory()
            tabs.setAccessoryContentAlpha(1)
        })
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

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        hasAppeared = true
        landed = true
        (splitViewController as? SplitRootController)?.sessionDidAppear(chatId)
        list.settleEdgeEffect()
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
        (tabBarController as? MainTabController)?.syncAccessory()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        // Leaving mid-edit: give the queued row back and the draft its place.
        if editingQueueId != nil { endEdit(commit: nil) }
        saveDraft()
        if isMovingFromParent { setAccessory(visible: true) }
    }

    /// Tear down only once the pop has really happened: an interactive
    /// swipe-back reports `isMovingFromParent` in viewWillDisappear too, and
    /// if it's cancelled the session stays on screen — detaching there left it
    /// frozen (no new updates) until you left and came back.
    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent || navigationController == nil {
            if tabBarController != nil { (splitViewController as? SplitRootController)?.sessionDidClose(chatId) }
            source.detach()
            engine.close()
            app?.markSeen(chatId)
        }
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

    private func render(animated: Bool) {
        let c = source.chrome
        let old = shown
        shown = c
        titleView.set(title: c.title, subtitle: c.subtitle)
        loadingStack.isHidden = c.loadingProgress == nil
        loadingLabel.text = c.loadingLabel
        if let progress = c.loadingProgress { loadingBar.setProgress(Float(progress), animated: animated) }
        list.uploadProgress = c.uploadProgress
        composer.running = c.running
        composer.canSteer = c.canSteer
        composer.placeholder = c.placeholder
        composer.chips = c.chips
        composer.chipMenus = Dictionary(uniqueKeysWithValues: c.chips.map { chip in
            (chip.id, { [weak self] in self?.source.chipMenu(chip.id) })
        })
        let changes = {
            let asking = c.questions != nil
            self.questions.isHidden = !asking
            self.composer.isHidden = asking
            if let q = c.questions { self.questions.configure(q.items) }
            self.queue.isHidden = c.queue.isEmpty || asking
            self.queue.configure(c.queue)
            let editing = self.editingQueueId != nil
            self.pill.superview?.isHidden = c.banner == .none && !editing
            self.pill.banner = editing ? .editing : c.banner
            self.bottom.layoutIfNeeded()
            self.view.layoutIfNeeded()
        }
        let structural = (old.questions == nil) != (c.questions == nil) || old.queue.count != c.queue.count || (old.banner == .none) != (c.banner == .none)
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

    func attachmentMenu() -> UIMenu {
        AttachmentPicker.menu(host: self, limit: 8 - composer.images.count) { [weak self] in self?.composer.addImages($0) }
    }

    func sessionMenu() -> UIMenu {
        UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] done in
            guard let self, let app = self.app else { return done([]) }
            let vm = app.session(self.chatId)
            let pinned = vm?.pinned ?? false
            done([
                UIAction(title: pinned ? "Unpin" : "Pin", image: UIImage(systemName: pinned ? "pin.slash" : "pin")) { _ in app.setPinned(self.chatId, !pinned) },
                UIAction(title: "Copy Transcript", image: UIImage(systemName: "doc.on.doc")) { _ in
                    UIPasteboard.general.string = self.engine.frame().plainText()
                },
                UIAction(title: "Archive", image: UIImage(systemName: "archivebox"), attributes: .destructive) { _ in
                    app.archive(self.chatId)
                    // Beside the iPad sidebar there's nothing to pop back to.
                    if let split = self.splitViewController as? SplitRootController, !split.isCollapsed {
                        split.showDraft(prompt: nil, focus: false)
                    } else {
                        self.navigationController?.popViewController(animated: true)
                    }
                },
            ])
        }])
    }
}

/// Two-line navigation title: session title over "project @ device".
final class SessionTitleView: UIView {
    private let title = FadingLabel()
    private let subtitle = FadingLabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.font = Fonts.ui(.sansSemibold, 16)
        title.textColor = Palette.text
        title.fitsAlignment = .center
        subtitle.font = Fonts.ui(.sans, 12)
        subtitle.textColor = Palette.secondary
        subtitle.fitsAlignment = .center
        let stack = UIStackView(arrangedSubviews: [title, subtitle])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 1
        stack.translatesAutoresizingMaskIntoConstraints = false
        addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: topAnchor),
            stack.bottomAnchor.constraint(equalTo: bottomAnchor),
            stack.leadingAnchor.constraint(equalTo: leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: trailingAnchor),
            widthAnchor.constraint(lessThanOrEqualToConstant: 240),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }

    func set(title t: String, subtitle s: String) {
        title.text = t
        subtitle.text = s
        subtitle.isHidden = s.isEmpty
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
