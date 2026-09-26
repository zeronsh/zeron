import UIKit

/// Projects as expandable groups (Capy's "Captains"): avatar tile, name,
/// status glyph or unseen badge, time, and the project's sessions indented
/// underneath. Expansion state persists per project.
final class ProjectsViewController: UIViewController, UICollectionViewDelegate {
    enum Item: Hashable {
        case project(String)
        case session(String)
    }

    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, Item>!
    private var projects: [String: AppModel.ProjectVM] = [:]
    private var sessions: [String: SessionRowVM] = [:]
    private var token: AnyObject?
    private var expanded: Set<String> {
        get { Set(UserDefaults.standard.stringArray(forKey: "expandedProjects") ?? []) }
        set { UserDefaults.standard.set(Array(newValue), forKey: "expandedProjects") }
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Projects"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .always
        var config = UICollectionLayoutListConfiguration(appearance: .plain)
        config.backgroundColor = .clear
        config.showsSeparators = false
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)

        let projectReg = UICollectionView.CellRegistration<ProjectCell, String> { [weak self] cell, _, id in
            guard let self, let p = self.projects[id] else { return }
            cell.configure(p)
            cell.accessories = [.outlineDisclosure(options: .init(style: .header, tintColor: Palette.tertiary))]
        }
        let sessionReg = UICollectionView.CellRegistration<SessionCell, String> { [weak self] cell, _, id in
            guard let vm = self?.sessions[id] else { return }
            cell.indent = 30
            cell.configure(vm)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, item in
            switch item {
            case let .project(id): cv.dequeueConfiguredReusableCell(using: projectReg, for: path, item: id)
            case let .session(id): cv.dequeueConfiguredReusableCell(using: sessionReg, for: path, item: id)
            }
        }
        dataSource.sectionSnapshotHandlers.willExpandItem = { [weak self] item in
            if case let .project(id) = item { self?.expanded.insert(id) }
        }
        dataSource.sectionSnapshotHandlers.willCollapseItem = { [weak self] item in
            if case let .project(id) = item { self?.expanded.remove(id) }
        }
        token = app.observe { [weak self] in self?.reload(animated: true) }
        reload(animated: false)
    }

    private func reload(animated: Bool) {
        var section = NSDiffableDataSourceSectionSnapshot<Item>()
        var changed: [Item] = []
        var nextProjects: [String: AppModel.ProjectVM] = [:]
        var nextSessions: [String: SessionRowVM] = [:]
        let open = expanded
        for p in app.projects {
            if let old = projects[p.id], old != p { changed.append(.project(p.id)) }
            nextProjects[p.id] = p
            section.append([.project(p.id)])
            let children = p.sessions.filter { nextSessions[$0.id] == nil }
            for s in children {
                if let old = sessions[s.id], old != s { changed.append(.session(s.id)) }
                nextSessions[s.id] = s
            }
            section.append(children.map { .session($0.id) }, to: .project(p.id))
            if open.contains(p.id) { section.expand([.project(p.id)]) }
        }
        projects = nextProjects
        sessions = nextSessions
        dataSource.apply(section, to: 0, animatingDifferences: animated && view.window != nil)
        if !changed.isEmpty {
            var snap = dataSource.snapshot()
            let present = Set(snap.itemIdentifiers)
            snap.reconfigureItems(changed.filter { present.contains($0) })
            dataSource.apply(snap, animatingDifferences: false)
        }
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let item = dataSource.itemIdentifier(for: path) else { return }
        switch item {
        case let .project(id):
            var s = dataSource.snapshot(for: 0)
            if s.isExpanded(item) { s.collapse([item]); expanded.remove(id) } else { s.expand([item]); expanded.insert(id) }
            dataSource.apply(s, to: 0, animatingDifferences: true)
        case let .session(id):
            navigationController?.pushViewController(SessionViewController(app: app, chatId: id), animated: true)
        }
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt paths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        guard let path = paths.first, case let .project(id) = dataSource.itemIdentifier(for: path) else { return nil }
        return UIContextMenuConfiguration(identifier: nil, previewProvider: nil) { [weak self] _ in
            UIMenu(children: [
                UIAction(title: "New Session Here", image: UIImage(systemName: "plus.bubble")) { _ in
                    guard let self else { return }
                    self.app.lastDraft.projectId = id
                    (self.tabBarController as? MainTabController)?.presentNewSession()
                },
            ])
        }
    }
}

/// Project group header: avatar tile, name, status/badge, time.
final class ProjectCell: UICollectionViewListCell {
    static let height: CGFloat = 58
    private let tile = UILabel()
    private let name = UILabel()
    private let device = UILabel()
    private let time = UILabel()
    private let badge = UILabel()
    private let status = DotGridView(style: .idle)

    override init(frame: CGRect) {
        super.init(frame: frame)
        tile.font = Fonts.ui(.sansSemibold, 15)
        tile.textColor = .white
        tile.textAlignment = .center
        tile.layer.cornerRadius = 8
        tile.layer.cornerCurve = .continuous
        tile.clipsToBounds = true
        name.font = Fonts.ui(.sansMedium, 18)
        name.textColor = Palette.text
        device.font = Fonts.ui(.sans, 13)
        device.textColor = Palette.tertiary
        time.font = Fonts.ui(.sans, 14)
        time.textColor = Palette.secondary
        badge.font = Fonts.ui(.sansSemibold, 13)
        badge.textColor = .white
        badge.backgroundColor = Palette.accent
        badge.textAlignment = .center
        badge.layer.cornerRadius = 10
        badge.clipsToBounds = true
        for v in [tile, name, device, time, badge, status] { contentView.addSubview(v) }
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = Self.height
        return attrs
    }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted ? Palette.chip.withAlphaComponent(0.7) : .clear
        bg.cornerRadius = 14
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }

    func configure(_ p: AppModel.ProjectVM) {
        tile.text = String(p.name.prefix(1)).uppercased()
        tile.backgroundColor = Palette.projectDots[p.colorIndex % Palette.projectDots.count]
        name.text = p.name
        device.text = p.device
        time.text = p.timeLabel
        badge.text = "\(p.unseen)"
        badge.isHidden = p.unseen == 0
        switch p.status {
        case .working: status.style = .working; status.isHidden = false
        case .awaiting: status.style = .awaiting; status.isHidden = false
        case .errored: status.style = .errored; status.isHidden = false
        default: status.isHidden = true
        }
        accessibilityIdentifier = "project-\(p.id)"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        tile.frame = CGRect(x: 20, y: (b.height - 30) / 2, width: 30, height: 30)
        var right = b.width - 8
        let tw = ceil(time.sizeThatFits(b.size).width)
        time.frame = CGRect(x: right - tw, y: (b.height - 18) / 2, width: tw, height: 18)
        right -= tw + 8
        if !badge.isHidden {
            let w = max(22, ceil(badge.sizeThatFits(b.size).width) + 12)
            badge.frame = CGRect(x: right - w, y: (b.height - 22) / 2, width: w, height: 22)
            right -= w + 8
        }
        if !status.isHidden {
            status.frame = CGRect(x: right - 16, y: (b.height - 16) / 2, width: 16, height: 16)
            right -= 24
        }
        let nw = min(ceil(name.sizeThatFits(b.size).width), right - 62)
        name.frame = CGRect(x: 62, y: (b.height - 24) / 2, width: nw, height: 24)
        device.frame = CGRect(x: name.frame.maxX + 8, y: (b.height - 18) / 2 + 1, width: max(0, right - name.frame.maxX - 8), height: 18)
    }
}
