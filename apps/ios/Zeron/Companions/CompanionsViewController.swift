import UIKit

/// Everything working alongside one session: the subagents its agent
/// spawned (live first), its side chats, and its terminals — each opens in
/// place, and each section can start another. On iPhone it's a sheet over the
/// session; beside it on iPad and the unfolded iPhone Duo, the inspector.
final class CompanionsViewController: UIViewController, UICollectionViewDelegate {
    enum Section: Int, CaseIterable {
        case agents, sideChats, terminals

        var title: String {
            switch self {
            case .agents: "Agents"
            case .sideChats: "Side chats"
            case .terminals: "Terminals"
            }
        }
    }

    enum Action: Hashable {
        case newSideChat, fork, newTerminal
    }

    private enum Item: Hashable {
        case header(Section)
        case agent(String)
        case sideChat(String)
        case terminal(ObjectIdentifier)
        case action(Action)
        case empty(Section)
    }

    private let app: AppModel
    let chatId: String
    /// Where a picked row opens (the session's stack, or the inspector's).
    var open: ((UIViewController) -> Void)?
    /// A side chat to open by id (sheet: dismiss first, then push).
    var openSideChat: ((String) -> Void)?
    /// Scroll to this section once loaded.
    var initialSection: Section?
    /// Put the inspector away (shown as a Close button while it's a sheet:
    /// the iPhone Duo folded with the panel open).
    var close: (() -> Void)?
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, Item>!
    private var agents: [String: SubagentRow] = [:]
    private var sides: [String: SessionRowVM] = [:]
    private var terminals: [ObjectIdentifier: TerminalTab] = [:]
    private var tokens: [AnyObject] = []
    private var ticker: Timer?
    private var forking = false
    private let inSheet: Bool

    init(app: AppModel, chatId: String, inSheet: Bool) {
        self.app = app
        self.chatId = chatId
        self.inSheet = inSheet
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = inSheet ? .clear : Palette.background
        navigationItem.title = app.row(chatId)?.title ?? "Session"
        navigationItem.subtitle = "Agents, side chats & terminals"
        navigationItem.largeTitleDisplayMode = .never
        if inSheet {
            navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in
                self?.dismiss(animated: true)
            })
        } else {
            syncClose()
        }
        var config = UICollectionLayoutListConfiguration(appearance: .plain)
        config.backgroundColor = .clear
        config.showsSeparators = false
        config.trailingSwipeActionsConfigurationProvider = { [weak self] path in self?.trailingSwipe(path) }
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.accessibilityIdentifier = "companions"
        view.addSubview(collectionView)

        let header = UICollectionView.CellRegistration<CompanionHeaderCell, Section> { [weak self] cell, _, section in
            cell.configure(title: section.title, button: self?.headerButton(section))
        }
        let agent = UICollectionView.CellRegistration<CompanionCell, String> { [weak self] cell, _, id in
            guard let row = self?.agents[id] else { return }
            cell.configure(agent: row)
        }
        let side = UICollectionView.CellRegistration<SessionCell, String> { [weak self] cell, _, id in
            guard let vm = self?.sides[id] else { return }
            cell.configure(vm)
            cell.accessibilityIdentifier = "companion-side-chat"
        }
        let terminal = UICollectionView.CellRegistration<CompanionCell, ObjectIdentifier> { [weak self] cell, _, id in
            guard let self, let tab = self.terminals[id] else { return }
            cell.configure(terminal: tab, host: self.app.deviceName(tab.ref.deviceId))
        }
        let action = UICollectionView.CellRegistration<CompanionCell, Action> { [weak self] cell, _, action in
            cell.configure(action: action, busy: action == .fork && self?.forking == true)
        }
        let empty = UICollectionView.CellRegistration<UICollectionViewListCell, Section> { cell, _, section in
            var c = UIListContentConfiguration.cell()
            c.text = switch section {
            case .agents: "No subagents yet"
            case .sideChats: "Ask something on the side without derailing the main thread."
            case .terminals: "A shell on this session's computer, in its folder."
            }
            c.textProperties.font = Fonts.ui(.sans, 14)
            c.textProperties.color = Palette.tertiary
            c.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 2, leading: 20, bottom: 8, trailing: 20)
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.clear()
            bg.backgroundColor = .clear
            cell.backgroundConfiguration = bg
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, item in
            switch item {
            case let .header(s): cv.dequeueConfiguredReusableCell(using: header, for: path, item: s)
            case let .agent(id): cv.dequeueConfiguredReusableCell(using: agent, for: path, item: id)
            case let .sideChat(id): cv.dequeueConfiguredReusableCell(using: side, for: path, item: id)
            case let .terminal(id): cv.dequeueConfiguredReusableCell(using: terminal, for: path, item: id)
            case let .action(a): cv.dequeueConfiguredReusableCell(using: action, for: path, item: a)
            case let .empty(s): cv.dequeueConfiguredReusableCell(using: empty, for: path, item: s)
            }
        }
        tokens.append(app.observeSession(chatId) { [weak self] in self?.scheduleReload() })
        tokens.append(app.observe { [weak self] in self?.scheduleReload() })
        tokens.append(app.terminals.observe { [weak self] chat in
            guard let self, chat == self.chatId else { return }
            self.scheduleReload()
        })
        reload(animated: false)
    }

    /// The split folded (the inspector shows as a sheet): it gets a Close
    /// button. A column is size-class compact too, so the split says which.
    var showsClose = false { didSet { if showsClose != oldValue, isViewLoaded { syncClose() } } }

    private func syncClose() {
        guard !inSheet else { return }
        navigationItem.rightBarButtonItem = showsClose ? UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in
            self?.close?()
        }) : nil
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        // A terminal may be a tap away: pair with the session's computer now.
        app.client?.warmDirect(chatId: chatId)
        // Running agents' elapsed time ticks on screen only.
        ticker?.invalidate()
        ticker = Timer.scheduledTimer(withTimeInterval: 1, repeats: true) { [weak self] _ in self?.tick() }
        if let section = initialSection {
            initialSection = nil
            if let path = dataSource.indexPath(for: .header(section)) {
                collectionView.scrollToItem(at: path, at: .top, animated: true)
            }
        }
    }

    override func viewWillDisappear(_ animated: Bool) {
        super.viewWillDisappear(animated)
        ticker?.invalidate()
    }

    private func tick() {
        var s = dataSource.snapshot()
        let live = agents.values.filter { $0.state == .running }.map { Item.agent($0.id) }
        guard !live.isEmpty else { return }
        s.reconfigureItems(live.filter { s.indexOfItem($0) != nil })
        dataSource.apply(s, animatingDifferences: false)
    }

    private func headerButton(_ section: Section) -> UIButton? {
        guard section != .agents else { return nil }
        let button = UIButton(configuration: {
            var c = UIButton.Configuration.glass()
            c.image = UIImage(systemName: "plus", withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .bold))
            c.baseForegroundColor = Palette.text
            c.cornerStyle = .capsule
            c.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 6, bottom: 6, trailing: 6)
            return c
        }())
        if section == .sideChats {
            button.menu = UIMenu(children: [
                UIAction(title: "New Side Chat", image: UIImage(systemName: "bubble.left.and.text.bubble.right")) { [weak self] _ in self?.perform(.newSideChat) },
                UIAction(title: "Fork This Chat", image: UIImage(systemName: "arrow.triangle.branch")) { [weak self] _ in self?.perform(.fork) },
            ])
            button.showsMenuAsPrimaryAction = true
            button.accessibilityLabel = "Add side chat"
        } else {
            button.addAction(UIAction { [weak self] _ in self?.perform(.newTerminal) }, for: .touchUpInside)
            button.accessibilityLabel = "New terminal"
        }
        button.accessibilityIdentifier = "companions-add-\(section)"
        return button
    }

    // MARK: Data

    private var reloadScheduled = false

    /// Streaming changes the session every frame; the list settles a few
    /// times a second.
    private func scheduleReload() {
        guard !reloadScheduled else { return }
        reloadScheduled = true
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in
            self?.reloadScheduled = false
            self?.reload()
        }
    }

    private func reload(animated: Bool = true) {
        let agentRows = app.sessionSubagents(chatId)
        // Live first, then newest.
        let ordered = agentRows.enumerated().sorted { a, b in
            let (ra, rb) = (a.element.state == .running, b.element.state == .running)
            return ra != rb ? ra : a.offset > b.offset
        }.map(\.element)
        let sideRows = app.sideChats(of: chatId)
        let tabs = app.terminals.tabs(for: chatId)
        var changed: [Item] = []
        let oldAgents = agents
        agents = Dictionary(agentRows.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        changed += agents.filter { oldAgents[$0.key] != $0.value }.map { .agent($0.key) }
        let oldSides = sides
        sides = Dictionary(sideRows.map { ($0.id, $0) }, uniquingKeysWith: { a, _ in a })
        changed += sides.filter { oldSides[$0.key] != $0.value }.map { .sideChat($0.key) }
        terminals = Dictionary(tabs.map { (ObjectIdentifier($0), $0) }, uniquingKeysWith: { a, _ in a })
        changed += tabs.map { .terminal(ObjectIdentifier($0)) }

        var s = NSDiffableDataSourceSnapshot<Section, Item>()
        if !ordered.isEmpty {
            s.appendSections([.agents])
            s.appendItems([.header(.agents)] + ordered.map { .agent($0.id) }, toSection: .agents)
        }
        s.appendSections([.sideChats])
        s.appendItems([.header(.sideChats)] + (sideRows.isEmpty ? [.empty(.sideChats)] : sideRows.map { .sideChat($0.id) }), toSection: .sideChats)
        if sideRows.isEmpty {
            s.appendItems([.action(.newSideChat), .action(.fork)], toSection: .sideChats)
        }
        s.appendSections([.terminals])
        s.appendItems([.header(.terminals)] + (tabs.isEmpty ? [.empty(.terminals), .action(.newTerminal)] : tabs.map { .terminal(ObjectIdentifier($0)) }), toSection: .terminals)
        let existing = Set(s.itemIdentifiers)
        s.reconfigureItems(changed.filter { existing.contains($0) && dataSource.snapshot().indexOfItem($0) != nil })
        dataSource.apply(s, animatingDifferences: animated && view.window != nil)
        updateSheetTitle(agentRows: agentRows)
    }

    private func updateSheetTitle(agentRows: [SubagentRow]) {
        let running = agentRows.filter { $0.state == .running }.count
        navigationItem.subtitle = running > 0 ? "\(running) agent\(running == 1 ? "" : "s") working" : "Agents, side chats & terminals"
    }

    // MARK: Selection

    func collectionView(_ collectionView: UICollectionView, shouldSelectItemAt indexPath: IndexPath) -> Bool {
        switch dataSource.itemIdentifier(for: indexPath) {
        case .header, .empty, nil: false
        case let .agent(id): agents[id]?.docId != nil
        default: true
        }
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard let item = dataSource.itemIdentifier(for: indexPath) else { return }
        UISelectionFeedbackGenerator().selectionChanged()
        switch item {
        case let .agent(id):
            guard let row = agents[id], let doc = row.docId else { return }
            open?(SubagentViewController(app: app, parentId: chatId, docId: doc, title: row.title, running: row.state == .running))
        case let .sideChat(id):
            openSideChat?(id)
        case let .terminal(id):
            guard let tab = terminals[id] else { return }
            open?(TerminalViewController(app: app, chatId: chatId, tab: tab))
        case let .action(a):
            perform(a)
        case .header, .empty:
            break
        }
    }

    func perform(_ action: Action) {
        switch action {
        case .newSideChat:
            guard let id = app.createSideChat(parentId: chatId) else {
                return Toast.show("Couldn't start a side chat", in: view.window)
            }
            openSideChat?(id)
        case .fork:
            guard !forking else { return }
            forking = true
            reconfigure(.action(.fork))
            Task { @MainActor in
                defer {
                    forking = false
                    reconfigure(.action(.fork))
                }
                do {
                    let id = try await app.forkSideChat(sourceId: chatId)
                    UINotificationFeedbackGenerator().notificationOccurred(.success)
                    openSideChat?(id)
                } catch {
                    Toast.show(Self.forkError(error), in: view.window)
                }
            }
        case .newTerminal:
            open?(TerminalViewController(app: app, chatId: chatId, tab: nil))
        }
    }

    static func forkError(_ error: Error) -> String {
        if case CoreError.HostUnavailable = error { return "Its computer isn't reachable right now" }
        if case CoreError.Unsupported = error { return "Its computer needs a newer Zeron to fork" }
        return "Couldn't fork this chat"
    }

    private func reconfigure(_ item: Item) {
        var s = dataSource.snapshot()
        guard s.indexOfItem(item) != nil else { return }
        s.reconfigureItems([item])
        dataSource.apply(s, animatingDifferences: false)
    }

    private func trailingSwipe(_ path: IndexPath) -> UISwipeActionsConfiguration? {
        switch dataSource.itemIdentifier(for: path) {
        case let .terminal(id):
            guard let tab = terminals[id] else { return nil }
            let close = UIContextualAction(style: .destructive, title: "Close") { [weak self] _, _, done in
                self?.app.terminals.close(tab)
                done(true)
            }
            close.image = UIImage(systemName: "xmark")
            return UISwipeActionsConfiguration(actions: [close])
        case let .sideChat(id):
            let archive = UIContextualAction(style: .normal, title: "Archive") { [weak self] _, _, done in
                self?.app.archive(id)
                done(true)
            }
            archive.image = UIImage(systemName: "archivebox")
            archive.backgroundColor = Palette.secondary
            return UISwipeActionsConfiguration(actions: [archive])
        default:
            return nil
        }
    }
}

/// A companion row, laid out like a session row: a status tile, a title,
/// one line of detail, and what it's doing on the right.
final class CompanionCell: UICollectionViewListCell {
    static var height: CGFloat { (62 * TypeScale.factor).rounded() }

    private let tile = UIView()
    private let glyph = StatusGlyph()
    private let icon = UIImageView()
    private let title = FadingLabel()
    private let detail = FadingLabel()
    private let trailing = UILabel()
    private let chevron = UIImageView(image: UIImage(systemName: "chevron.right", withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold)))
    private var accent = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        tile.layer.cornerRadius = 8
        tile.layer.cornerCurve = .continuous
        icon.contentMode = .center
        chevron.tintColor = Palette.tertiary
        chevron.contentMode = .center
        trailing.textAlignment = .right
        for v in [tile, title, detail, trailing, chevron] as [UIView] { contentView.addSubview(v) }
        tile.addSubview(icon)
        tile.addSubview(glyph)
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : .clear
        bg.cornerRadius = 16
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = accent ? (48 * TypeScale.factor).rounded() : Self.height
        return attrs
    }

    private func styleText(titleColor: UIColor = Palette.text) {
        title.font = Fonts.ui(.sansMedium, TypeScale.size(16))
        title.textColor = titleColor
        detail.font = Fonts.ui(.sans, TypeScale.size(13.5))
        detail.textColor = Palette.secondary
        trailing.font = Fonts.ui(.sansMedium, TypeScale.size(13))
    }

    func configure(agent row: SubagentRow) {
        accent = false
        styleText()
        // The tile says how it's doing: working, done, failed.
        let symbol = row.state == .failed ? "xmark" : "checkmark"
        icon.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .bold))
        icon.tintColor = row.state == .failed ? Palette.danger : StatusTone.done
        tile.backgroundColor = row.state == .running ? Palette.accentSoft : row.state == .failed ? Palette.color(.dangerWash) : Palette.chip
        icon.isHidden = row.state == .running
        glyph.isHidden = row.state != .running
        glyph.kind = .spinner
        title.text = row.title
        let model = [row.model.map(SubagentFormat.model), row.kind].compactMap { $0 }.joined(separator: " · ")
        switch row.state {
        case .running:
            let since = Date(timeIntervalSince1970: Double(row.startedAtMs) / 1000)
            trailing.text = SubagentFormat.elapsed(since: since)
            trailing.textColor = StatusTone.working
            detail.text = row.tail ?? (model.isEmpty ? "Working" : model)
        case .done:
            trailing.text = nil
            detail.text = model.isEmpty ? "Finished" : model
        case .failed:
            trailing.text = nil
            detail.text = model.isEmpty ? "Didn't finish" : "Failed · \(model)"
        }
        chevron.isHidden = row.docId == nil
        accessibilityIdentifier = "companion-agent"
        accessibilityLabel = "\(row.title), \(row.state == .running ? "working" : row.state == .failed ? "failed" : "done")"
        setNeedsLayout()
    }

    func configure(terminal tab: TerminalTab, host: String) {
        accent = false
        styleText()
        tile.backgroundColor = Palette.chip
        icon.image = UIImage(systemName: "apple.terminal", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
        icon.tintColor = Palette.secondary
        icon.isHidden = false
        glyph.isHidden = true
        title.text = tab.name
        detail.text = "\(tab.folder) · \(host)"
        switch tab.phase {
        case .live, .connecting:
            trailing.text = "Live"
            trailing.textColor = StatusTone.done
        case .reconnecting:
            trailing.text = "Reconnecting"
            trailing.textColor = Palette.secondary
        case .exited, .gone:
            trailing.text = "Ended"
            trailing.textColor = Palette.tertiary
        }
        chevron.isHidden = false
        accessibilityIdentifier = "companion-terminal"
        accessibilityLabel = "\(tab.name), \(trailing.text ?? "")"
        setNeedsLayout()
    }

    func configure(action: CompanionsViewController.Action, busy: Bool) {
        accent = true
        styleText(titleColor: Palette.accent)
        tile.backgroundColor = Palette.accentSoft
        let (symbol, text) = switch action {
        case .newSideChat: ("plus", "New side chat")
        case .fork: ("arrow.triangle.branch", busy ? "Forking…" : "Fork this chat")
        case .newTerminal: ("plus", "New terminal")
        }
        icon.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .bold))
        icon.tintColor = Palette.accent
        icon.isHidden = busy
        glyph.isHidden = !busy
        glyph.kind = .spinner
        title.text = text
        detail.text = nil
        trailing.text = nil
        chevron.isHidden = true
        accessibilityIdentifier = "companion-action-\(action)"
        accessibilityLabel = text
        accessibilityTraits = .button
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let left: CGFloat = 20
        let side: CGFloat = accent ? 26 : 30
        tile.frame = CGRect(x: left, y: (b.height - side) / 2, width: side, height: side)
        icon.frame = tile.bounds
        glyph.frame = CGRect(x: (side - 12) / 2, y: (side - 12) / 2, width: 12, height: 12)
        let textX = left + side + 12
        let chevronW: CGFloat = chevron.isHidden ? 0 : 18
        let trailingW = ceil(trailing.intrinsicContentSize.width)
        let right = b.width - 20 - chevronW - (trailingW > 0 ? trailingW + 10 : 0)
        chevron.frame = CGRect(x: b.width - 20 - 12, y: (b.height - 16) / 2, width: 12, height: 16)
        trailing.frame = CGRect(x: right + 10, y: 0, width: trailingW, height: b.height)
        if detail.text?.isEmpty ?? true {
            title.frame = CGRect(x: textX, y: 0, width: max(0, right - textX), height: b.height)
            detail.frame = .zero
        } else {
            let th = ceil(title.font.lineHeight)
            let dh = ceil(detail.font.lineHeight)
            let top = (b.height - th - dh - 3) / 2
            title.frame = CGRect(x: textX, y: top, width: max(0, right - textX), height: th)
            detail.frame = CGRect(x: textX, y: top + th + 3, width: max(0, right - textX), height: dh)
            trailing.frame = CGRect(x: right + 10, y: top, width: trailingW, height: th)
        }
    }
}

/// A section title with its add button on the right.
final class CompanionHeaderCell: UICollectionViewListCell {
    private let title = UILabel()
    private var button: UIButton?

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.font = Fonts.ui(.sansSemibold, 13)
        title.textColor = Palette.secondary
        contentView.addSubview(title)
        var bg = UIBackgroundConfiguration.clear()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = 46
        return attrs
    }

    func configure(title text: String, button: UIButton?) {
        title.text = text
        self.button?.removeFromSuperview()
        self.button = button
        if let button { contentView.addSubview(button) }
        accessibilityTraits = .header
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let side: CGFloat = 28
        button?.frame = CGRect(x: b.width - 18 - side, y: b.height - side - 4, width: side, height: side)
        title.frame = CGRect(x: 20, y: b.height - 22 - 6, width: b.width - 80, height: 22)
    }
}
