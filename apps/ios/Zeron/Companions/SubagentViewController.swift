import UIKit

/// What a subagent is doing (or did): its own transcript, read-only, live
/// while it runs. A glass bar at the bottom says how it's going — working
/// and for how long, done, or failed — and that it runs on its own (it
/// takes no messages). Subagents it spawns open the same way, one level in.
final class SubagentViewController: UIViewController {
    private let app: AppModel
    /// The chat whose transcript spawned it (a chat, or a parent subagent's
    /// doc for a nested spawn).
    let parentId: String
    /// The root chat (whose host runs it, and whose spawn list says how it's
    /// doing).
    let rootChatId: String
    let docId: String
    private let titleText: String
    private var running: Bool
    private lazy var relay = FrameRelay { [weak self] in self?.applyFrame() }
    private lazy var engine = TranscriptView(text: TextEngine.shared, listener: relay)
    private lazy var list = TranscriptListView(engine: engine)
    private let bar = SubagentStatusBar()
    private let loader = StatusGlyph(.spinner)
    private var sessionToken: AnyObject?
    private var seenRows = false

    init(app: AppModel, parentId: String, docId: String, title: String, running: Bool) {
        self.app = app
        self.parentId = parentId
        self.docId = docId
        self.titleText = title
        self.running = running
        self.rootChatId = docId.components(separatedBy: "--sub--").first.flatMap { $0.isEmpty ? nil : $0 } ?? parentId
        super.init(nibName: nil, bundle: nil)
        hidesBottomBarWhenPushed = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.title = titleText
        navigationItem.subtitle = "Subagent"
        let copy = UIBarButtonItem(image: UIImage(systemName: "doc.on.doc"), primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            UIPasteboard.general.string = self.engine.frame().plainText()
            Toast.show("Transcript copied", in: self.view.window)
        })
        copy.accessibilityLabel = "Copy transcript"
        navigationItem.rightBarButtonItem = copy

        list.frame = view.bounds
        list.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        list.accessibilityIdentifier = "subagent-transcript"
        list.onSubagent = { [weak self] doc, title, running in self?.openNested(doc, title: title, running: running) }
        list.alpha = 0
        view.addSubview(list)
        setContentScrollView(list, for: .top)
        list.topEdgeEffect.style = .soft
        list.bottomEdgeEffect.isHidden = true

        loader.translatesAutoresizingMaskIntoConstraints = false
        loader.transform = CGAffineTransform(scaleX: 1.75, y: 1.75)
        view.addSubview(loader)

        bar.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bar)
        NSLayoutConstraint.activate([
            loader.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            loader.centerYAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerYAnchor, constant: -40),
            loader.widthAnchor.constraint(equalToConstant: 12),
            loader.heightAnchor.constraint(equalToConstant: 12),
            bar.centerXAnchor.constraint(equalTo: view.safeAreaLayoutGuide.centerXAnchor),
            bar.leadingAnchor.constraint(greaterThanOrEqualTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 12),
            bar.widthAnchor.constraint(lessThanOrEqualToConstant: 560),
            {
                let fill = bar.widthAnchor.constraint(equalTo: view.safeAreaLayoutGuide.widthAnchor, constant: -24)
                fill.priority = .defaultHigh
                return fill
            }(),
            bar.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -8),
        ])

        if let client = app.client {
            if !engine.attachSubagent(client: client, chatId: parentId, docId: docId, frozen: !running) {
                showUnavailable()
            }
            if let host = app.row(rootChatId)?.deviceId {
                engine.setVisualSource(client: client, deviceId: host, pixelScale: Float(max(2, traitCollection.displayScale)))
            }
        }
        sessionToken = app.observeSession(rootChatId) { [weak self] in self?.syncStatus() }
        syncStatus()
        // Nothing arrives (host offline, doc gone): say so instead of spinning.
        DispatchQueue.main.asyncAfter(deadline: .now() + 10) { [weak self] in
            guard let self, !self.seenRows else { return }
            self.showUnavailable()
        }
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let covered = view.bounds.maxY - bar.frame.minY + 8
        let inset = max(0, covered - view.safeAreaInsets.bottom)
        if abs(list.contentInset.bottom - inset) > 0.5 {
            list.contentInset.bottom = inset
            list.verticalScrollIndicatorInsets.bottom = inset
        }
    }

    override func viewDidDisappear(_ animated: Bool) {
        super.viewDidDisappear(animated)
        if isMovingFromParent || navigationController == nil { engine.close() }
    }

    /// Removed while off screen (a pop-to-root under it): stop following.
    override func didMove(toParent parent: UIViewController?) {
        super.didMove(toParent: parent)
        if parent == nil { engine.close() }
    }

    private func applyFrame() {
        let frame = engine.frame()
        list.apply(frame)
        guard !seenRows, frame.rowCount() > 0 else { return }
        seenRows = true
        UIView.animate(withDuration: 0.2) {
            self.list.alpha = 1
            self.loader.alpha = 0
        }
    }

    private func showUnavailable() {
        guard !seenRows else { return }
        loader.alpha = 0
        bar.configure(.unavailable, model: nil, since: nil)
    }

    /// Its spawn on the parent transcript says how it's doing.
    private func syncStatus() {
        let spawn = app.sessionSubagents(rootChatId).first { $0.docId == docId }
        if let spawn {
            running = spawn.state == .running
            let since = Date(timeIntervalSince1970: Double(spawn.startedAtMs) / 1000)
            let model = [spawn.model.map(SubagentFormat.model), spawn.kind].compactMap { $0 }.joined(separator: " · ")
            bar.configure(spawn.state == .running ? .working : spawn.state == .failed ? .failed : .done, model: model.isEmpty ? nil : model, since: since)
            navigationItem.subtitle = ["Subagent", model.isEmpty ? nil : model].compactMap { $0 }.joined(separator: " · ")
        } else {
            bar.configure(running ? .working : .done, model: nil, since: nil)
        }
    }

    private func openNested(_ doc: String, title: String, running: Bool) {
        let vc = SubagentViewController(app: app, parentId: docId, docId: doc, title: title, running: running)
        navigationController?.pushViewController(vc, animated: true)
    }
}

enum SubagentFormat {
    /// `claude-haiku-5` → "Haiku 5"; Claude's bare aliases capitalized.
    static func model(_ id: String) -> String {
        let lower = id.lowercased()
        if ["haiku", "sonnet", "opus"].contains(lower) { return lower.capitalized }
        let harness = lower.hasPrefix("claude") ? "claude-code" : lower.hasPrefix("gpt") ? "codex" : ""
        let label = modelLabel(harness: harness, model: id)
        guard label == id, lower.hasPrefix("claude-") else { return label }
        // An id the catalog doesn't know yet: `claude-haiku-5` → "Haiku 5".
        let words = lower.dropFirst("claude-".count).split(separator: "-")
        guard let family = words.first else { return label }
        let version = words.dropFirst().prefix { $0.count <= 2 && $0.allSatisfy(\.isNumber) }.joined(separator: ".")
        return version.isEmpty ? family.capitalized : "\(family.capitalized) \(version)"
    }

    static func elapsed(since: Date, now: Date = Date()) -> String {
        StatusPill.elapsed(max(0, Int(now.timeIntervalSince(since))))
    }
}

/// The subagent screen's bottom bar: status glyph, what it's doing, how long.
final class SubagentStatusBar: UIView {
    enum State { case working, done, failed, unavailable }

    private let glass = Glass.surface()
    private let glyph = StatusGlyph(.trailer)
    private let label = UILabel()
    private let detail = UILabel()
    private var timer: Timer?
    private var state: State = .working
    private var since: Date?

    override init(frame: CGRect) {
        super.init(frame: frame)
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        label.font = Fonts.ui(.sansMedium, 14)
        label.textColor = Palette.text
        detail.font = Fonts.ui(.sans, 13)
        detail.textColor = Palette.secondary
        detail.textAlignment = .right
        detail.setContentCompressionResistancePriority(.defaultLow, for: .horizontal)
        let row = UIStackView(arrangedSubviews: [glyph, label, UIView(), detail])
        row.spacing = 10
        row.alignment = .center
        row.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(row)
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            heightAnchor.constraint(equalToConstant: 50),
            glyph.widthAnchor.constraint(equalToConstant: 12),
            glyph.heightAnchor.constraint(equalToConstant: 12),
            row.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 18),
            row.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -18),
            row.centerYAnchor.constraint(equalTo: glass.contentView.centerYAnchor),
        ])
        accessibilityIdentifier = "subagent-status"
        isAccessibilityElement = true
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ state: State, model: String?, since: Date?) {
        self.state = state
        self.since = since
        detail.text = model ?? (state == .working ? "Runs on its own" : nil)
        timer?.invalidate()
        switch state {
        case .working:
            glyph.kind = .trailer
            render()
            timer = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.render() }
        case .done:
            glyph.kind = .check(StatusTone.done)
            label.text = "Finished"
        case .failed:
            glyph.kind = .dot(StatusTone.failed)
            label.text = "Failed"
        case .unavailable:
            glyph.kind = .dot(StatusTone.idle)
            label.text = "Waiting for its computer…"
            detail.text = nil
        }
        accessibilityLabel = [label.text, detail.text].compactMap { $0 }.joined(separator: ", ")
    }

    private func render() {
        guard state == .working else { return }
        label.text = since.map { "Working · \(SubagentFormat.elapsed(since: $0))" } ?? "Working"
    }

    override func willMove(toWindow newWindow: UIWindow?) {
        super.willMove(toWindow: newWindow)
        if newWindow == nil { timer?.invalidate() } else if state == .working { configure(state, model: detail.text, since: since) }
    }
}
