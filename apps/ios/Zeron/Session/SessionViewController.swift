import UIKit

/// One session: virtualized transcript under a glass bottom stack (status
/// pill, queue, composer or question panel) that rides the keyboard.
final class SessionViewController: UIViewController {
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
    private let bottom = UIStackView()
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

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.titleView = titleView
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: sessionMenu())

        list.frame = view.bounds
        list.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        list.accessibilityIdentifier = "transcript"
        list.imageLoader = { [weak self] ref, iv in self?.source.loadImage(ref, into: iv) }
        list.onDistanceFromBottom = { [weak self] d in self?.setJumpVisible(d > 140) }
        view.addSubview(list)
        setContentScrollView(list, for: .top)
        list.topEdgeEffect.style = .soft
        list.bottomEdgeEffect.style = .soft

        composer.attachMenu = { [weak self] in
            guard let self else { return UIMenu() }
            return AttachmentPicker.menu(host: self, limit: 8 - self.composer.images.count) { [weak self] in self?.composer.addImages($0) }
        }
        composer.onSend = { [weak self] text, images, mode in
            guard let self else { return }
            if self.editingQueueId != nil {
                self.endEdit(commit: text)
                return
            }
            self.source.send(text: text, images: images, mode: mode)
            self.list.scrollToBottom(animated: true)
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
        for v in [pillRow, queue, questions, composer] { bottom.addArrangedSubview(v) }
        questions.isHidden = true
        queue.isHidden = true
        // Hidden panels start dematerialized so their first appearance grows in.
        questions.setGlassVisible(false, animated: false)
        queue.setGlassVisible(false, animated: false)
        pillRow.isHidden = true
        view.addSubview(bottom)

        jump.addAction(UIAction { [weak self] _ in self?.list.scrollToBottom(animated: true) }, for: .touchUpInside)
        jump.accessibilityIdentifier = "jump-to-latest"
        jump.accessibilityLabel = "Jump to latest"
        jump.alpha = 0
        jump.transform = CGAffineTransform(scaleX: 0.6, y: 0.6)
        view.addSubview(jump)

        NSLayoutConstraint.activate([
            // Full width on phones; a centered 760pt column on iPad/landscape.
            bottom.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            bottom.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            bottom.widthAnchor.constraint(lessThanOrEqualToConstant: 784),
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
        app.markSeen(chatId)
    }

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        setAccessory(visible: false)
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
        if visible { bottomStack.alpha = 1 } else { bottomStack.alpha = 0 }
        coordinator.animate(alongsideTransition: { _ in
            tabs.setAccessoryVisible(visible, animated: false)
            bottomStack.alpha = visible ? 0 : 1
            if visible { jumpButton.alpha = 0 }
        }, completion: { context in
            // An interactive pop that's cancelled keeps the session on screen.
            if context.isCancelled {
                tabs.setAccessoryVisible(!visible, animated: false)
                bottomStack.alpha = 1
            }
        })
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        list.settleEdgeEffect()
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        Drafts.save(chatId, composer.text)
        if isMovingFromParent {
            setAccessory(visible: true)
            source.detach()
            engine.close()
            app.markSeen(chatId)
        }
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

    private func beginEdit(_ id: String) {
        Task { @MainActor in
            guard let text = await source.beginEdit(id) else {
                let alert = UIAlertController(title: "Can't edit right now", message: "Another device is editing this message, or it was just sent.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "OK", style: .default))
                present(alert, animated: true)
                return
            }
            editingQueueId = id
            stashedDraft = composer.text
            composer.text = text
            composer.placeholder = "Edit queued message"
            pill.banner = .editing
            pill.superview?.isHidden = false
            composer.becomeFirstResponder()
        }
    }

    private func endEdit(commit text: String?) {
        editingQueueId = nil
        composer.text = stashedDraft
        composer.placeholder = shown.placeholder
        Task { await source.finishEdit(text: text) }
        render(animated: true)
    }

    private func applyFrame() {
        let frame = engine.frame()
        list.apply(frame)
        if !reportedOpen, frame.rowCount() > 0 {
            reportedOpen = true
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
        if animated, (old.questions == nil) != (c.questions == nil) {
            questions.setGlassVisible(c.questions != nil, animated: true)
        }
        if animated, old.queue.isEmpty != c.queue.isEmpty {
            queue.setGlassVisible(!c.queue.isEmpty, animated: true)
        }
        if animated, structural {
            UIView.animate(withDuration: 0.38, delay: 0, usingSpringWithDamping: 0.86, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: changes)
        } else {
            changes()
        }
    }

    private func sessionMenu() -> UIMenu {
        UIMenu(children: [UIDeferredMenuElement.uncached { [weak self] done in
            guard let self else { return done([]) }
            let vm = self.app.session(self.chatId)
            let pinned = vm?.pinned ?? false
            done([
                UIAction(title: pinned ? "Unpin" : "Pin", image: UIImage(systemName: pinned ? "pin.slash" : "pin")) { _ in self.app.setPinned(self.chatId, !pinned) },
                UIAction(title: "Copy Transcript", image: UIImage(systemName: "doc.on.doc")) { _ in
                    UIPasteboard.general.string = self.engine.frame().plainText()
                },
                UIAction(title: "Archive", image: UIImage(systemName: "archivebox"), attributes: .destructive) { _ in
                    self.app.archive(self.chatId)
                    self.navigationController?.popViewController(animated: true)
                },
            ])
        }])
    }
}

/// Two-line navigation title: session title over "project @ device".
final class SessionTitleView: UIView {
    private let title = UILabel()
    private let subtitle = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.font = Fonts.ui(.sansSemibold, 16)
        title.textColor = Palette.text
        title.textAlignment = .center
        subtitle.font = Fonts.ui(.sans, 12)
        subtitle.textColor = Palette.secondary
        subtitle.textAlignment = .center
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
    private let grid = DotGridView(style: .working)
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
            grid.style = .working
            let render = { [weak self] in
                let secs = since.map { Int(Date().timeIntervalSince($0)) } ?? 0
                self?.label.text = secs > 0 ? "\(word) · \(Self.elapsed(secs))" : "\(word)…"
            }
            render()
            timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { _ in render() }
        case .offline:
            grid.style = .idle
            label.text = "Offline — sends are saved"
        case let .reconnecting(secs):
            grid.style = .idle
            label.text = "Reconnecting in \(secs)s"
        case .notDelivered:
            grid.style = .errored
            label.text = "Not delivered · Tap to retry"
        case let .uploading(p):
            grid.style = .working
            label.text = "Uploading… \(Int(p * 100))%"
        case let .failed(message):
            grid.style = .errored
            label.text = message
        case .editing:
            grid.style = .idle
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
