import UIKit

/// Shared list machinery for every session list (front page, folders,
/// search): diffable by id, reconfigure-in-place on content
/// changes, fixed row heights, swipe + context actions.
class SessionListController: UIViewController, UICollectionViewDelegate {
    enum Item: Hashable {
        case folder(String)
        case session(String)
        case header(String)
    }

    let app: AppModel
    var collectionView: UICollectionView!
    var dataSource: UICollectionViewDiffableDataSource<String, Item>!
    private(set) var sessions: [String: SessionRowVM] = [:]
    private(set) var folders: [String: FolderRowVM] = [:]
    private var headers: [String: String] = [:]
    private var token: AnyObject?
    private var displayedViewPreferences: SessionViewPreferences?
    private struct MenuState: Equatable {
        let preferences: SessionViewPreferences
        let scope: SessionScope
        let scopeTitle: String
        let searching: Bool
        let includeArchived: Bool
        let archivedCount: Int
        let sidebar: Bool
    }
    private var displayedMenuState: MenuState?
    private let emptyLabel = UILabel()
    private let emptyAction = UIButton(type: .system)
    private let emptyStack = UIStackView()
    var listScope: SessionScope { app.sessionScope }
    var searching: Bool { false }
    var includeArchived = false { didSet { if isViewLoaded { reload(animated: true) } } }
    func selectScope(_ scope: SessionScope) { app.setSessionScope(scope) }
    /// Show drag handles in edit mode (Pinned).
    var reorderable = false
    /// The Archived list: rows offer Unarchive instead of Archive / Pin / Move.
    var archivedRows = false
    /// The session open beside this list (iPad sidebar): drawn as current.
    var currentChatId: String? {
        didSet {
            guard oldValue != currentChatId, dataSource != nil else { return }
            var s = dataSource.snapshot()
            let ids = Set([oldValue, currentChatId].compactMap { $0 })
            s.reconfigureItems(s.itemIdentifiers.filter { if case let .session(id) = $0 { return ids.contains(id) } else { return false } })
            dataSource.apply(s, animatingDifferences: false)
        }
    }
    /// Headers are disclosure rows that fold their section (front page).
    var collapsible = false
    var collapsed: Set<String> { app.viewPreferences.collapsedSections }
    private var headerStates: [String: SectionHeaderCell.State] = [:]

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        var config = UICollectionLayoutListConfiguration(appearance: .plain)
        config.backgroundColor = .clear
        config.showsSeparators = false
        config.leadingSwipeActionsConfigurationProvider = { [weak self] path in self?.leadingSwipe(path) }
        config.trailingSwipeActionsConfigurationProvider = { [weak self] path in self?.trailingSwipe(path) }
        let layout = UICollectionViewCompositionalLayout.list(using: config)
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)
        emptyLabel.font = UIFontMetrics(forTextStyle: .body).scaledFont(for: Fonts.ui(.sans, 16))
        emptyLabel.adjustsFontForContentSizeCategory = true
        emptyLabel.textColor = Palette.secondary
        emptyLabel.textAlignment = .center
        emptyLabel.numberOfLines = 0
        emptyLabel.accessibilityIdentifier = "session-empty-state"
        emptyAction.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            if self.searching { self.selectScope(.all) } else { self.router?.presentNewSession(prompt: nil) }
        }, for: .touchUpInside)
        emptyAction.accessibilityIdentifier = "session-empty-action"
        emptyAction.heightAnchor.constraint(greaterThanOrEqualToConstant: 44).isActive = true
        emptyStack.axis = .vertical
        emptyStack.spacing = 12
        emptyStack.addArrangedSubview(emptyLabel)
        emptyStack.addArrangedSubview(emptyAction)
        emptyStack.translatesAutoresizingMaskIntoConstraints = false
        collectionView.addSubview(emptyStack)
        NSLayoutConstraint.activate([
            emptyStack.centerXAnchor.constraint(equalTo: collectionView.frameLayoutGuide.centerXAnchor),
            emptyStack.centerYAnchor.constraint(equalTo: collectionView.frameLayoutGuide.centerYAnchor),
            emptyStack.widthAnchor.constraint(equalTo: collectionView.frameLayoutGuide.widthAnchor, constant: -48),
        ])

        let sessionReg = UICollectionView.CellRegistration<SessionCell, String> { [weak self] cell, path, id in
            guard let self, let vm = self.sessions[id] else { return }
            cell.configure(vm, preferences: self.app.viewPreferences)
            cell.isCurrent = id == self.currentChatId
            cell.accessories = self.reorderable ? [.reorder(displayed: .whenEditing)] : []
        }
        let folderReg = UICollectionView.CellRegistration<FolderCell, String> { [weak self] cell, _, id in
            guard let vm = self?.folders[id] else { return }
            cell.configure(vm)
        }
        let headerReg = UICollectionView.CellRegistration<UICollectionViewListCell, String> { [weak self] cell, _, id in
            var c = UIListContentConfiguration.plainHeader()
            c.text = self?.headers[id]
            c.textProperties.font = Fonts.ui(.sansSemibold, 13)
            c.textProperties.color = Palette.secondary
            c.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 18, leading: 20, bottom: 6, trailing: 20)
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.clear()
            bg.backgroundColor = .clear
            cell.backgroundConfiguration = bg
        }
        let sectionReg = UICollectionView.CellRegistration<SectionHeaderCell, String> { [weak self] cell, _, id in
            guard let state = self?.headerStates[id] else { return }
            cell.configure(state)
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { [weak self] cv, path, item in
            switch item {
            case let .session(id): cv.dequeueConfiguredReusableCell(using: sessionReg, for: path, item: id)
            case let .folder(id): cv.dequeueConfiguredReusableCell(using: folderReg, for: path, item: id)
            case let .header(id) where self?.collapsible == true: cv.dequeueConfiguredReusableCell(using: sectionReg, for: path, item: id)
            case let .header(id): cv.dequeueConfiguredReusableCell(using: headerReg, for: path, item: id)
            }
        }
        token = app.observe { [weak self] in self?.reload(animated: true) }
        reload(animated: false)
        // Text size changes: fixed row heights change with the category.
        registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) { (self: SessionListController, _) in
            var s = self.dataSource.snapshot()
            s.reconfigureItems(s.itemIdentifiers)
            self.dataSource.apply(s, animatingDifferences: false)
            self.collectionView.collectionViewLayout.invalidateLayout()
        }
        // Pull to re-probe sync (health + room redial) when a network looks stale.
        // (The control is captured directly: a primary action's `sender` is
        // nil, so ending via `action.sender` left the spinner running forever.)
        let refresh = UIRefreshControl()
        refresh.addAction(UIAction { [weak self, weak refresh] _ in
            guard let self else { return refresh?.endRefreshing() ?? () }
            self.app.refresh { refresh?.endRefreshing() }
        }, for: .valueChanged)
        refresh.accessibilityIdentifier = "pull-to-refresh"
        collectionView.refreshControl = refresh
    }

    /// Subclasses build their sections from the app model.
    func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] { [] }

    func reload(animated: Bool) {
        let appearanceChanged = displayedViewPreferences != app.viewPreferences
        displayedViewPreferences = app.viewPreferences
        navigationItem.subtitle = listScope == .all ? nil : app.scopeTitle(listScope)
        updateOptionsMenu()
        let sections = buildSections()
        let empty = sections.allSatisfy { $0.sessions.isEmpty && $0.folders.isEmpty }
        emptyStack.isHidden = !empty
        emptyLabel.text = searching ? "No matching sessions in \(app.scopeTitle(listScope))" : !app.workspaceSynced ? "Syncing sessions…" : "No sessions in \(app.scopeTitle(listScope))"
        emptyAction.setTitle(searching ? "All projects" : "New session", for: .normal)
        emptyAction.isHidden = archivedRows || (!app.workspaceSynced && !searching) || (searching && listScope == .all)
        var snapshot = NSDiffableDataSourceSnapshot<String, Item>()
        var nextSessions: [String: SessionRowVM] = [:]
        var nextFolders: [String: FolderRowVM] = [:]
        var changed: [Item] = []
        let subtitles = Dictionary(uniqueKeysWithValues: app.frontPage.groups.map { ($0.id, $0.subtitle) })
        for s in sections {
            snapshot.appendSections([s.id])
            var items: [Item] = []
            if let header = s.header {
                headers[s.id] = header
                items.append(.header(s.id))
                if collapsible {
                    let state = SectionHeaderCell.State(
                        id: s.id,
                        title: header,
                        subtitle: subtitles[s.id] ?? nil,
                        count: s.sessions.count,
                        collapsed: collapsed.contains(s.id),
                        live: s.sessions.contains { $0.status == .working } ? .spinner : s.sessions.contains { $0.status == .awaiting } ? .dot(StatusTone.input) : nil
                    )
                    if let old = headerStates[s.id], old != state { changed.append(.header(s.id)) }
                    headerStates[s.id] = state
                    if state.collapsed {
                        snapshot.appendItems(items, toSection: s.id)
                        continue
                    }
                }
            }
            for f in s.folders {
                if folders[f.id] != nil, folders[f.id] != f { changed.append(.folder(f.id)) }
                nextFolders[f.id] = f
                items.append(.folder(f.id))
            }
            for r in s.sessions where nextSessions[r.id] == nil {
                if let old = sessions[r.id], old != r || appearanceChanged { changed.append(.session(r.id)) }
                nextSessions[r.id] = r
                items.append(.session(r.id))
            }
            snapshot.appendItems(items, toSection: s.id)
        }
        sessions = nextSessions
        folders = nextFolders
        let present = Set(snapshot.itemIdentifiers)
        snapshot.reconfigureItems(changed.filter { present.contains($0) })
        dataSource.apply(snapshot, animatingDifferences: animated && view.window != nil)
        if appearanceChanged { collectionView.collectionViewLayout.invalidateLayout() }
    }

    func updateOptionsMenu() {
        let state = MenuState(preferences: app.viewPreferences, scope: listScope,
            scopeTitle: app.scopeTitle(listScope), searching: searching,
            includeArchived: includeArchived, archivedCount: app.archived.count,
            sidebar: self is SessionsViewController && navigationItem.searchController != nil)
        let buttons = (navigationItem.rightBarButtonItems ?? []) + (toolbarItems ?? [])
        for button in buttons where button.accessibilityIdentifier == "sessions-options" || button.accessibilityIdentifier == "search-options" {
            button.accessibilityValue = "\(state.scopeTitle); \(state.preferences.organization.title), \(state.preferences.sort.title)"
            // Time-label updates must not replace the menu while it is open.
            if state != displayedMenuState { button.menu = optionsMenu() }
        }
        displayedMenuState = state
    }

    func chooseProjects(forSessions: Bool = false) {
        let scope = forSessions ? app.sessionScope : listScope
        let anchor = toolbarItems?.first { $0.accessibilityIdentifier == "sessions-options" } ?? navigationItem.rightBarButtonItem
        presentProjectPicker(app: app, selection: scope, mode: .filter, source: view, barButtonItem: anchor) { [weak self] scope in
            guard let self else { return }
            if forSessions { self.app.setSessionScope(scope) } else { self.selectScope(scope) }
        }
    }

    func optionsMenu() -> UIMenu {
        var children = viewOptionsMenu().children
        if listScope != .all {
            children.insert(UIAction(title: "All projects", image: UIImage(systemName: "square.grid.2x2")) { [weak self] _ in self?.selectScope(.all) }, at: 0)
        }
        return UIMenu(children: children)
    }

    func viewOptionsMenu() -> UIMenu {
        let preferences = app.viewPreferences
        var menus: [UIMenuElement] = []
        if self is SessionsViewController, !searching {
            menus.append(UIMenu(title: "Organize", options: .singleSelection,
                children: SessionViewPreferences.Organization.allCases.map { choice in
                    UIAction(title: choice.title, state: preferences.organization == choice ? .on : .off) { [weak self] _ in
                        guard let self else { return }
                        var value = self.app.viewPreferences
                        value.organization = choice
                        self.app.setViewPreferences(value)
                    }
                }))
        }
        if !searching, !reorderable {
            menus.append(UIMenu(title: "Sort", options: .singleSelection,
                children: SessionViewPreferences.Sort.allCases.map { choice in
                    UIAction(title: choice.title, state: preferences.sort == choice ? .on : .off) { [weak self] _ in
                        guard let self else { return }
                        var value = self.app.viewPreferences
                        value.sort = choice
                        self.app.setViewPreferences(value)
                    }
                }))
        }
        let fields: [(String, WritableKeyPath<SessionViewPreferences, Bool>)] = [
            ("Branch", \.showBranch), ("Pull request", \.showPullRequest),
            ("Harness", \.showHarness), ("Project icon", \.showProjectIcon),
            ("Project label", \.showProjectLabel),
        ]
        menus.append(UIMenu(title: "Show", children: fields.map { title, key in
            UIAction(title: title, state: preferences[keyPath: key] ? .on : .off) { [weak self] _ in
                guard let self else { return }
                var value = self.app.viewPreferences
                value[keyPath: key].toggle()
                self.app.setViewPreferences(value)
            }
        }))
        return UIMenu(children: menus)
    }

    /// Subclasses follow the scroll (the front page fades its wallpaper).
    func listDidScroll(_ scrollView: UIScrollView) {}

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        listDidScroll(scrollView)
    }

    // MARK: Navigation

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        switch dataSource.itemIdentifier(for: indexPath) {
        case let .session(id):
            openSession(id)
        case let .folder(id):
            openFolder(id)
        case let .header(id) where collapsible:
            toggleSection(id)
        default:
            break
        }
    }

    func collectionView(_ collectionView: UICollectionView, shouldSelectItemAt indexPath: IndexPath) -> Bool {
        if case .header = dataSource.itemIdentifier(for: indexPath) { return collapsible }
        return true
    }

    func toggleSection(_ id: String) {
        var value = app.viewPreferences
        if value.collapsedSections.remove(id) == nil { value.collapsedSections.insert(id) }
        UISelectionFeedbackGenerator().selectionChanged()
        app.setViewPreferences(value)
    }

    /// Long-press actions for a collapsible section header.
    func headerMenu(_ id: String) -> UIMenu? { nil }

    func openSession(_ id: String) {
        // iPad: sessions open beside the sidebar.
        if let split = splitViewController as? SplitRootController, !split.isCollapsed {
            split.openSession(id)
            return
        }
        guard let session = SessionViewController(app: app, chatId: id) else {
            (tabBarController as? MainTabController)?.showSessionOpenError()
            return
        }
        navigationController?.pushViewController(session, animated: true)
    }

    func openFolder(_ id: String) {
        guard let f = folders[id] ?? app.frontPage.folders.first(where: { $0.id == id }) else { return }
        navigationController?.pushViewController(FolderViewController(app: app, folder: f), animated: true)
    }

    // MARK: Actions

    private func sessionId(_ path: IndexPath) -> String? {
        if case let .session(id) = dataSource.itemIdentifier(for: path) { return id }
        return nil
    }

    private func leadingSwipe(_ path: IndexPath) -> UISwipeActionsConfiguration? {
        guard !archivedRows, let id = sessionId(path), let vm = sessions[id] else { return nil }
        let pin = UIContextualAction(style: .normal, title: vm.pinned ? "Unpin" : "Pin") { [weak self] _, _, done in
            self?.app.setPinned(id, !vm.pinned)
            done(true)
        }
        pin.image = UIImage(systemName: vm.pinned ? "pin.slash.fill" : "pin.fill")
        pin.backgroundColor = Palette.accent
        return UISwipeActionsConfiguration(actions: [pin])
    }

    private func trailingSwipe(_ path: IndexPath) -> UISwipeActionsConfiguration? {
        guard let id = sessionId(path) else { return nil }
        if archivedRows {
            let restore = UIContextualAction(style: .normal, title: "Unarchive") { [weak self] _, _, done in
                self?.app.unarchive(id)
                done(true)
            }
            restore.image = UIImage(systemName: "tray.and.arrow.up.fill")
            restore.backgroundColor = Palette.accent
            return UISwipeActionsConfiguration(actions: [restore])
        }
        let archive = UIContextualAction(style: .destructive, title: "Archive") { [weak self] _, _, done in
            self?.app.archive(id)
            done(true)
        }
        archive.image = UIImage(systemName: "archivebox.fill")
        archive.backgroundColor = Palette.secondary
        let move = UIContextualAction(style: .normal, title: "Move") { [weak self] _, view, done in
            self?.presentMoveMenu(id, from: view)
            done(true)
        }
        move.image = UIImage(systemName: "folder.fill")
        move.backgroundColor = UIColor(hex: 0x5E6AD2)
        return UISwipeActionsConfiguration(actions: [archive, move])
    }

    func collectionView(_ collectionView: UICollectionView, contextMenuConfigurationForItemsAt indexPaths: [IndexPath], point: CGPoint) -> UIContextMenuConfiguration? {
        if let path = indexPaths.first, case let .header(id) = dataSource.itemIdentifier(for: path), let menu = headerMenu(id) {
            return UIContextMenuConfiguration(identifier: nil, previewProvider: nil) { _ in menu }
        }
        guard let path = indexPaths.first, let id = sessionId(path), let vm = sessions[id] else { return nil }
        return UIContextMenuConfiguration(identifier: id as NSString, previewProvider: nil) { [weak self] _ in
            guard let self else { return nil }
            if self.archivedRows {
                return UIMenu(children: [
                    UIAction(title: "Unarchive", image: UIImage(systemName: "tray.and.arrow.up")) { _ in self.app.unarchive(id) },
                    UIAction(title: "Rename…", image: UIImage(systemName: "pencil")) { _ in self.rename(id) },
                ])
            }
            return UIMenu(children: [
                UIAction(title: vm.pinned ? "Unpin" : "Pin", image: UIImage(systemName: vm.pinned ? "pin.slash" : "pin")) { _ in self.app.setPinned(id, !vm.pinned) },
                self.moveMenu(id),
                UIAction(title: "Rename…", image: UIImage(systemName: "pencil")) { _ in self.rename(id) },
                UIAction(title: "Archive", image: UIImage(systemName: "archivebox"), attributes: .destructive) { _ in self.app.archive(id) },
            ])
        }
    }

    func moveMenu(_ id: String) -> UIMenu {
        var items: [UIMenuElement] = app.sectionOptions.map { f in
            UIAction(title: f.name, image: UIImage(systemName: "folder")) { [weak self] _ in self?.app.move(id, toSection: f.id) }
        }
        items.append(UIAction(title: "No Section", image: UIImage(systemName: "tray")) { [weak self] _ in self?.app.move(id, toSection: nil) })
        items.append(UIAction(title: "New Section…", image: UIImage(systemName: "folder.badge.plus")) { [weak self] _ in
            self?.promptForSection { name in self?.app.createSection(name) }
        })
        return UIMenu(title: "Move to Section", image: UIImage(systemName: "folder"), children: items)
    }

    private func presentMoveMenu(_ id: String, from view: UIView) {
        let sheet = UIAlertController(title: "Move to Section", message: nil, preferredStyle: .actionSheet)
        for f in app.sectionOptions {
            sheet.addAction(UIAlertAction(title: f.name, style: .default) { [weak self] _ in self?.app.move(id, toSection: f.id) })
        }
        sheet.addAction(UIAlertAction(title: "No Section", style: .default) { [weak self] _ in self?.app.move(id, toSection: nil) })
        sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        sheet.popoverPresentationController?.sourceView = view
        present(sheet, animated: true)
    }

    func promptForSection(_ done: @escaping (String) -> Void) {
        let alert = UIAlertController(title: "New Section", message: nil, preferredStyle: .alert)
        alert.addTextField { $0.placeholder = "Name"; $0.autocapitalizationType = .words }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Create", style: .default) { _ in
            let name = alert.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            if !name.isEmpty { done(name) }
        })
        present(alert, animated: true)
    }

    private func rename(_ id: String) {
        let alert = UIAlertController(title: "Rename Session", message: nil, preferredStyle: .alert)
        alert.addTextField { [weak self] in $0.text = self?.sessions[id]?.title }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Rename", style: .default) { [weak self] _ in
            let title = alert.textFields?.first?.text?.trimmingCharacters(in: .whitespaces) ?? ""
            if !title.isEmpty { self?.app.rename(id, title) }
        })
        present(alert, animated: true)
    }
}

/// Front page, laid out like the desktop sidebar: Pinned and the user's
/// sections inline as foldable groups, then everything else under Recent.
/// Fold state persists; long-press a header for its actions.
class SessionsViewController: SessionListController {
    override init(app: AppModel) {
        super.init(app: app)
        title = "Sessions"
        navigationItem.largeTitleDisplayMode = .always
    }

    required init?(coder: NSCoder) { fatalError() }

    /// Search from the iPad sidebar: non-empty shows matches instead of sections.
    var query = "" { didSet { if query != oldValue { reload(animated: true) } } }
    override var searching: Bool { !query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty }

    /// The chat wallpaper behind the top of the front page. Fixed behind the
    /// list, fading out as the list scrolls up over it.
    private let wallpaper = WallpaperView()

    override func viewDidLoad() {
        collapsible = true
        super.viewDidLoad()
        let backdrop = UIView()
        backdrop.addSubview(wallpaper)
        collectionView.backgroundView = backdrop
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: optionsMenu())
        navigationItem.rightBarButtonItem?.accessibilityLabel = "Options"
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "sessions-options"
        updateOptionsMenu()
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        if !query.trimmingCharacters(in: .whitespaces).isEmpty {
            return [("results", nil, [], app.search(query, includeArchived: includeArchived))]
        }
        var out: [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] = []
        for f in app.frontPage.folders {
            out.append((f.id, f.name, [], app.sessions(inFolder: f.id)))
        }
        if app.viewPreferences.organization == .inOneList {
            out.append(("recent", out.isEmpty ? nil : "Recent", [], app.frontPage.sessions))
        } else {
            out += app.frontPage.groups.map { ($0.id, $0.title, [], $0.sessions) }
        }
        return out
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        // Desktop hero: 72% of the viewport, at most 760pt.
        wallpaper.frame = CGRect(x: 0, y: 0, width: view.bounds.width, height: min(view.bounds.height * 0.72, 760))
    }

    override func listDidScroll(_ scrollView: UIScrollView) {
        let travel = scrollView.contentOffset.y + scrollView.adjustedContentInset.top
        wallpaper.scrollFade = 1 - max(0, travel) / max(1, wallpaper.bounds.height * 0.6)
    }

    override func headerMenu(_ id: String) -> UIMenu? {
        if let group = app.frontPage.groups.first(where: { $0.id == id }), let projectId = group.projectId {
            return projectActionsMenu(app: app, projectId: projectId)
        }
        if id == "recent" { return nil }
        guard let folder = app.frontPage.folders.first(where: { $0.id == id }) else { return nil }
        let open = UIAction(title: "Open", image: UIImage(systemName: "arrow.up.right")) { [weak self] _ in self?.openFolder(id) }
        if id == "pinned" {
            return UIMenu(children: [
                open,
                UIAction(title: "Reorder…", image: UIImage(systemName: "arrow.up.arrow.down")) { [weak self] _ in self?.openFolder(id) },
            ])
        }
        return UIMenu(children: [
            open,
            UIAction(title: "Rename Section…", image: UIImage(systemName: "pencil")) { [weak self] _ in self?.renameSection(folder) },
            UIAction(title: "Delete Section", image: UIImage(systemName: "trash"), attributes: .destructive) { [weak self] _ in self?.app.deleteSection(id) },
        ])
    }

    private func renameSection(_ folder: FolderRowVM) {
        let alert = UIAlertController(title: "Rename Section", message: nil, preferredStyle: .alert)
        alert.addTextField { $0.text = folder.name }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Rename", style: .default) { [weak self] _ in
            guard let name = alert.textFields?.first?.text?.trimmingCharacters(in: .whitespaces), !name.isEmpty else { return }
            self?.app.renameSection(folder.id, name)
        })
        present(alert, animated: true)
    }

    override func optionsMenu() -> UIMenu {
        var children = super.optionsMenu().children
        if navigationItem.searchController != nil {
            children.insert(UIAction(title: "Filter Sessions…", image: UIImage(systemName: "line.3.horizontal.decrease")) { [weak self] _ in self?.chooseProjects() }, at: 0)
            if searching {
                children.insert(UIAction(title: "Include archived", state: includeArchived ? .on : .off) { [weak self] _ in self?.includeArchived.toggle() }, at: 1)
            }
        }
        children.append(UIMenu(options: .displayInline, children: [
            UIAction(title: "New Section…", image: UIImage(systemName: "folder.badge.plus")) { [weak self] _ in
                self?.promptForSection { name in self?.app.createSection(name) }
            },
            UIAction(title: "Collapse All", image: UIImage(systemName: "rectangle.compress.vertical")) { [weak self] _ in
                guard let self else { return }
                var value = self.app.viewPreferences
                value.collapsedSections.formUnion(self.visibleSectionIds)
                self.app.setViewPreferences(value)
            },
            UIAction(title: "Expand All", image: UIImage(systemName: "rectangle.expand.vertical")) { [weak self] _ in
                guard let self else { return }
                var value = self.app.viewPreferences
                value.collapsedSections.subtract(self.visibleSectionIds)
                self.app.setViewPreferences(value)
            },
            UIAction(title: "Archived (\(app.archived.count))", image: UIImage(systemName: "archivebox")) { [weak self] _ in
                guard let self else { return }
                self.navigationController?.pushViewController(FolderViewController(app: self.app, folder: FolderRowVM(id: "archived", name: "Archived", count: 0, symbol: "archivebox")), animated: true)
            },
        ]))
        return UIMenu(children: children)
    }

    private var visibleSectionIds: [String] {
        app.frontPage.folders.map(\.id) + (app.viewPreferences.organization == .inOneList ? ["recent"] : app.frontPage.groups.map(\.id))
    }
}

/// One folder's sessions (Pinned, a user section, Archived).
final class FolderViewController: SessionListController {
    private let folder: FolderRowVM

    init(app: AppModel, folder: FolderRowVM) {
        self.folder = folder
        super.init(app: app)
        title = folder.name
        navigationItem.largeTitleDisplayMode = .always
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        archivedRows = folder.id == "archived"
        reorderable = folder.id == "pinned"
        super.viewDidLoad()
        let options = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: optionsMenu())
        options.accessibilityLabel = "Options"
        options.accessibilityIdentifier = "sessions-options"
        if folder.id == "pinned" {
            // Pins are an ordered list: drag to reorder (synced to desktop).
            navigationItem.rightBarButtonItems = [editButtonItem, options]
            dataSource.reorderingHandlers.canReorderItem = { _ in true }
            dataSource.reorderingHandlers.didReorder = { [weak self] tx in self?.pinsReordered(tx) }
        } else {
            navigationItem.rightBarButtonItem = options
        }
        updateOptionsMenu()
    }

    override func optionsMenu() -> UIMenu {
        var children = super.optionsMenu().children
        if folder.id != "pinned", folder.id != "archived" {
            children.append(UIMenu(options: .displayInline, children: [
                UIAction(title: "Rename Section…", image: UIImage(systemName: "pencil")) { [weak self] _ in self?.renameSection() },
                UIAction(title: "Delete Section", image: UIImage(systemName: "trash"), attributes: .destructive) { [weak self] _ in self?.deleteSection() },
            ]))
        }
        return UIMenu(children: children)
    }

    override func setEditing(_ editing: Bool, animated: Bool) {
        super.setEditing(editing, animated: animated)
        collectionView.isEditing = editing
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("sessions", nil, [], app.sessions(inFolder: folder.id))]
    }

    private func pinsReordered(_ tx: NSDiffableDataSourceTransaction<String, Item>) {
        let ids = tx.finalSnapshot.itemIdentifiers.compactMap { item -> String? in
            if case let .session(id) = item { return id }
            return nil
        }
        for change in tx.difference.insertions {
            guard case let .insert(offset, item, _) = change, case let .session(id) = item else { continue }
            let after = offset > 0 ? ids[offset - 1] : nil
            let before = offset + 1 < ids.count ? ids[offset + 1] : nil
            app.movePin(id, after: after, before: before)
        }
    }

    private func renameSection() {
        let alert = UIAlertController(title: "Rename Section", message: nil, preferredStyle: .alert)
        alert.addTextField { [folder] in $0.text = folder.name }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        alert.addAction(UIAlertAction(title: "Rename", style: .default) { [weak self] _ in
            guard let self, let name = alert.textFields?.first?.text?.trimmingCharacters(in: .whitespaces), !name.isEmpty else { return }
            self.app.renameSection(self.folder.id, name)
            self.title = name
        })
        present(alert, animated: true)
    }

    private func deleteSection() {
        let alert = UIAlertController(title: "Delete “\(folder.name)”?", message: "Its sessions move back to the main list.", preferredStyle: .actionSheet)
        alert.addAction(UIAlertAction(title: "Delete Section", style: .destructive) { [weak self] _ in
            guard let self else { return }
            self.app.deleteSection(self.folder.id)
            self.navigationController?.popViewController(animated: true)
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        // iPad presents action sheets as popovers: anchor it to the ⋯ button.
        alert.popoverPresentationController?.barButtonItem = navigationItem.rightBarButtonItem
        present(alert, animated: true)
    }
}

/// Search tab: live results as you type (matching runs in the core).
final class SearchViewController: SessionListController, UISearchResultsUpdating {
    private var query = ""
    override var listScope: SessionScope { app.historyScope }
    override var searching: Bool { true }
    override func selectScope(_ scope: SessionScope) { app.setHistoryScope(scope) }

    override init(app: AppModel) {
        super.init(app: app)
        includeArchived = true
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Search"
        navigationItem.largeTitleDisplayMode = .always
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        search.hidesNavigationBarDuringPresentation = false
        search.searchBar.placeholder = "Sessions, projects, branches"
        search.searchBar.searchTextField.accessibilityIdentifier = "history-search"
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false
        navigationItem.rightBarButtonItem = UIBarButtonItem(image: UIImage(systemName: "ellipsis"), menu: optionsMenu())
        navigationItem.rightBarButtonItem?.accessibilityLabel = "Search options"
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "search-options"
        updateOptionsMenu()
    }

    override func optionsMenu() -> UIMenu {
        UIMenu(children: [
            UIAction(title: "Search in project…", image: UIImage(systemName: "line.3.horizontal.decrease")) { [weak self] _ in self?.chooseProjects() },
            UIAction(title: "Include archived", state: includeArchived ? .on : .off) { [weak self] _ in self?.includeArchived.toggle() },
            UIAction(title: "Filter Sessions…", image: UIImage(systemName: "tray")) { [weak self] _ in self?.chooseProjects(forSessions: true) },
            UIMenu(options: .displayInline, children: super.optionsMenu().children),
        ])
    }

    func updateSearchResults(for searchController: UISearchController) {
        query = searchController.searchBar.text ?? ""
        reload(animated: true)
    }

    override func buildSections() -> [(id: String, header: String?, folders: [FolderRowVM], sessions: [SessionRowVM])] {
        [("results", nil, [], query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ? [] : app.search(query, scope: listScope, includeArchived: includeArchived))]
    }
}
