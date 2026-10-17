import UIKit

/// Everything hanging off one session, in a sheet over it: the subagents
/// its agent is running (live tail), its side chats (forked, started by hand,
/// or spawned through the Zeron MCP server), and the subagents that finished.
/// Picking a row opens it on top of the session. Each section folds under
/// its header (remembered across sheets).
final class SideChatsViewController: UIViewController, UICollectionViewDelegate {
    private enum Section: String {
        case running, chats, finished
    }

    private enum Item: Hashable {
        case header(Section)
        case agent(String)
        case chat(String)
    }

    private let app: AppModel
    private let parentId: String
    private let onOpen: (String) -> Void
    /// A subagent row: its transcript (read-only).
    var onOpenAgent: ((SubagentInfo) -> Void)?
    /// "+": a new empty side chat, or a fork of the session.
    var onNew: (() -> Void)?
    var onFork: (() -> Void)?

    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, Item>!
    private var chats: [String: SessionRowVM] = [:]
    private var agents: [String: SubagentInfo] = [:]
    private var headers: [Section: SectionHeaderCell.State] = [:]
    private static let collapsedKey = "collapsedSideChatSections"
    private var collapsed = Set((UserDefaults.standard.stringArray(forKey: collapsedKey) ?? []).compactMap(Section.init(rawValue:)))
    private var workspaceToken: AnyObject?
    private var sessionToken: AnyObject?

    init(app: AppModel, parentId: String, onOpen: @escaping (String) -> Void) {
        self.app = app
        self.parentId = parentId
        self.onOpen = onOpen
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        title = "Side Chats & Agents"
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        let add = UIBarButtonItem(image: UIImage(systemName: "plus"), menu: UIMenu(children: [
            UIAction(title: "New Side Chat", image: UIImage(systemName: "plus.bubble")) { [weak self] _ in self?.onNew?() },
            UIAction(title: "Fork into Side Chat", image: UIImage(systemName: "arrow.triangle.branch")) { [weak self] _ in self?.onFork?() },
        ]))
        add.accessibilityLabel = "Add side chat"
        add.accessibilityIdentifier = "side-chats-add"
        navigationItem.leftBarButtonItem = add

        var config = UICollectionLayoutListConfiguration(appearance: .plain)
        config.backgroundColor = .clear
        config.showsSeparators = false
        config.trailingSwipeActionsConfigurationProvider = { [weak self] path in self?.trailingSwipe(path) }
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.accessibilityIdentifier = "side-chats-list"
        view.addSubview(collectionView)

        let chatReg = UICollectionView.CellRegistration<SessionCell, String> { [weak self] cell, _, id in
            guard let vm = self?.chats[id] else { return }
            cell.configure(vm)
        }
        let agentReg = UICollectionView.CellRegistration<SubagentCell, String> { [weak self] cell, _, id in
            guard let agent = self?.agents[id] else { return }
            cell.configure(agent)
        }
        let headerReg = UICollectionView.CellRegistration<SectionHeaderCell, Section> { [weak self] cell, _, section in
            guard let state = self?.headers[section] else { return }
            cell.configure(state)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, item in
            switch item {
            case let .header(section): cv.dequeueConfiguredReusableCell(using: headerReg, for: path, item: section)
            case let .chat(id): cv.dequeueConfiguredReusableCell(using: chatReg, for: path, item: id)
            case let .agent(id): cv.dequeueConfiguredReusableCell(using: agentReg, for: path, item: id)
            }
        }

        reload(animated: false)
        // Side chats come with workspace revisions; subagents (and their live
        // tails) with the session's transcript.
        workspaceToken = app.observeWorkspace { [weak self] in self?.reload(animated: true) }
        sessionToken = app.observeSession(parentId) { [weak self] in self?.reload(animated: true) }
    }

    private func reload(animated: Bool) {
        let chatRows = app.children(of: parentId)
        let agentRows = app.subagents(of: parentId)
        if chatRows.isEmpty, agentRows.isEmpty, view.window != nil {
            return dismiss(animated: true)
        }
        // Running first, newest spawn on top; finished ones newest first too.
        let running = agentRows.filter { $0.state == .running }.reversed()
        let finished = agentRows.filter { $0.state != .running }.reversed()
        var changed: [Item] = []
        var nextChats: [String: SessionRowVM] = [:]
        var nextAgents: [String: SubagentInfo] = [:]
        for r in chatRows {
            if let old = chats[r.id], old != r { changed.append(.chat(r.id)) }
            nextChats[r.id] = r
        }
        for a in agentRows {
            if let old = agents[a.docId], old != a { changed.append(.agent(a.docId)) }
            nextAgents[a.docId] = a
        }
        var snapshot = NSDiffableDataSourceSnapshot<Section, Item>()
        for (section, items) in [(Section.running, running.map { Item.agent($0.docId) }), (.chats, chatRows.map { Item.chat($0.id) }), (.finished, finished.map { Item.agent($0.docId) })] where !items.isEmpty {
            let title = switch section {
            case .running: "Running"
            case .chats: "Side chats"
            case .finished: "Finished agents"
            }
            // Folded, Running still shows its spinner: something is live inside.
            let header = SectionHeaderCell.State(id: section.rawValue, title: title, count: items.count, collapsed: collapsed.contains(section), live: section == .running ? .spinner : nil)
            if let old = headers[section], old != header { changed.append(.header(section)) }
            headers[section] = header
            snapshot.appendSections([section])
            snapshot.appendItems([.header(section)] + (header.collapsed ? [] : items), toSection: section)
        }
        let present = Set(snapshot.itemIdentifiers)
        snapshot.reconfigureItems(changed.filter { present.contains($0) })
        chats = nextChats
        agents = nextAgents
        dataSource.apply(snapshot, animatingDifferences: animated && view.window != nil)
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        switch dataSource.itemIdentifier(for: indexPath) {
        case let .header(section)?: toggle(section)
        case let .chat(id)?: onOpen(id)
        case let .agent(id)?: if let agent = agents[id] { onOpenAgent?(agent) }
        case nil: break
        }
    }

    private func toggle(_ section: Section) {
        if collapsed.remove(section) == nil { collapsed.insert(section) }
        UserDefaults.standard.set(collapsed.map(\.rawValue), forKey: Self.collapsedKey)
        UISelectionFeedbackGenerator().selectionChanged()
        reload(animated: true)
    }

    /// Side chats archive away; subagents belong to the transcript.
    private func trailingSwipe(_ path: IndexPath) -> UISwipeActionsConfiguration? {
        guard case let .chat(id)? = dataSource.itemIdentifier(for: path) else { return nil }
        let archive = UIContextualAction(style: .destructive, title: "Archive") { [weak self] _, _, done in
            self?.app.archive(id)
            done(true)
        }
        archive.image = UIImage(systemName: "archivebox.fill")
        archive.backgroundColor = Palette.secondary
        return UISwipeActionsConfiguration(actions: [archive])
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        guard let path = indexPaths.first, case let .chat(id)? = dataSource.itemIdentifier(for: path) else { return nil }
        return UIContextMenuConfiguration(identifier: id as NSString, previewProvider: nil) { [weak self] _ in
            UIMenu(children: [
                UIAction(title: "Rename…", image: UIImage(systemName: "pencil")) { _ in self?.rename(id) },
                UIAction(title: "Archive", image: UIImage(systemName: "archivebox"), attributes: .destructive) { _ in self?.app.archive(id) },
            ])
        }
    }

    private func rename(_ id: String) {
        let alert = UIAlertController(title: "Rename Session", message: nil, preferredStyle: .alert)
        alert.addTextField { [weak self] in $0.text = self?.chats[id]?.title }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Rename", style: .default) { [weak self] _ in
            let title = alert.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            if !title.isEmpty { self?.app.rename(id, title) }
        })
        present(alert, animated: true)
    }
}
