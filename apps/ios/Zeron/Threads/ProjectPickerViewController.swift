import UIKit

/// Shared native picker for list scope and the composer's execution target.
/// Candidates and search matching come from the Rust workspace projection.
final class ProjectPickerViewController: UIViewController, UITableViewDelegate, UISearchResultsUpdating, UISearchBarDelegate, UIPopoverPresentationControllerDelegate {
    enum Mode { case filter, newSession }
    private let app: AppModel
    private let mode: Mode
    private var selection: SessionScope
    private let onSelect: (SessionScope) -> Void
    private let search = UISearchController(searchResultsController: nil)
    private let table = UITableView(frame: .zero, style: .plain)
    private let newProject = UIButton(type: .system)
    private var dataSource: UITableViewDiffableDataSource<Int, String>!
    private var candidates: [String: ProjectOption] = [:]
    private var repositoryMembers: [String: [ProjectOption]] = [:]
    private var observer: AnyObject?
    private let status = UILabel()

    init(app: AppModel, selection: SessionScope, mode: Mode, onSelect: @escaping (SessionScope) -> Void) {
        self.app = app; self.selection = selection; self.mode = mode; self.onSelect = onSelect
        super.init(nibName: nil, bundle: nil)
        title = "Projects"
        preferredContentSize = CGSize(width: 440, height: 560)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        if traitCollection.preferredContentSizeCategory.isAccessibilityCategory {
            preferredContentSize = CGSize(width: 520, height: 720)
        }
        navigationItem.largeTitleDisplayMode = .never
        navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in self?.close() })
        navigationItem.rightBarButtonItem?.accessibilityIdentifier = "project-picker-close"
        search.searchResultsUpdater = self
        search.searchBar.delegate = self
        search.obscuresBackgroundDuringPresentation = false
        search.hidesNavigationBarDuringPresentation = false
        search.searchBar.placeholder = "Search projects…"
        search.searchBar.searchTextField.accessibilityIdentifier = "project-search"
        navigationItem.searchController = search
        navigationItem.preferredSearchBarPlacement = .stacked
        navigationItem.hidesSearchBarWhenScrolling = false
        definesPresentationContext = true
        table.delegate = self
        table.backgroundColor = .clear
        table.separatorColor = Palette.hairline
        table.keyboardDismissMode = .onDrag
        table.rowHeight = UITableView.automaticDimension
        table.estimatedRowHeight = 100
        table.accessibilityIdentifier = "project-picker"
        table.register(UITableViewCell.self, forCellReuseIdentifier: "project")
        dataSource = UITableViewDiffableDataSource<Int, String>(tableView: table) { [weak self] table, path, id in
            let cell = table.dequeueReusableCell(withIdentifier: "project", for: path)
            self?.configure(cell, id: id)
            return cell
        }
        var c = UIButton.Configuration.plain()
        c.title = "New project…"
        c.image = UIImage(systemName: "folder.badge.plus")
        c.imagePadding = 8
        c.baseForegroundColor = Palette.text
        c.contentInsets = NSDirectionalEdgeInsets(top: 14, leading: 16, bottom: 14, trailing: 16)
        c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { attrs in
            var attrs = attrs
            attrs.font = UIFontMetrics(forTextStyle: .body).scaledFont(for: Fonts.ui(.sansMedium, 16))
            return attrs
        }
        newProject.configuration = c
        newProject.accessibilityIdentifier = "new-project"
        newProject.addAction(UIAction { [weak self] _ in self?.createProject() }, for: .touchUpInside)
        status.font = UIFontMetrics(forTextStyle: .footnote).scaledFont(for: Fonts.ui(.sans, 14))
        status.adjustsFontForContentSizeCategory = true
        status.textColor = Palette.secondary
        status.textAlignment = .center
        status.numberOfLines = 0
        let bottom = UIStackView(arrangedSubviews: [status, newProject])
        bottom.axis = .vertical
        bottom.spacing = 8
        for child in [table, bottom] { child.translatesAutoresizingMaskIntoConstraints = false; view.addSubview(child) }
        NSLayoutConstraint.activate([
            table.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            table.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            table.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            table.bottomAnchor.constraint(equalTo: bottom.topAnchor, constant: -8),
            bottom.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 16),
            bottom.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -16),
            bottom.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -8),
            newProject.heightAnchor.constraint(greaterThanOrEqualToConstant: 44),
        ])
        observer = app.observe { [weak self] in self?.reload() }
        registerForTraitChanges([UITraitPreferredContentSizeCategory.self]) { (self: ProjectPickerViewController, _) in self.reload() }
        reload()
    }

    private func configure(_ cell: UITableViewCell, id: String) {
        var c = UIListContentConfiguration.subtitleCell()
        let scope: SessionScope
        if let p = candidates[id] {
            scope = .project(projectId: p.id)
            c.text = mode == .newSession ? p.groupName : p.name
            if let members = repositoryMembers[p.groupKey] {
                c.secondaryText = Set(members.map(\.deviceName)).sorted {
                    $0.localizedCaseInsensitiveCompare($1) == .orderedAscending
                }.joined(separator: ", ")
            } else {
                c.secondaryText = "\(p.deviceName)\(p.online ? "" : " · Offline")\n\(p.path)"
            }
            c.image = ProjectTile.image(name: c.text ?? p.name, colorIndex: p.colorIndex)
        } else {
            scope = id == "all" ? .all : .projectless
            c.text = app.scopeTitle(scope)
            c.secondaryText = id == "all" ? "Every project and host" : "Sessions in host home folders"
            c.image = app.scopeImage(scope)
        }
        c.textProperties.font = UIFontMetrics(forTextStyle: .body).scaledFont(for: Fonts.ui(.sansMedium, 16))
        c.secondaryTextProperties.font = UIFontMetrics(forTextStyle: .footnote).scaledFont(for: Fonts.ui(.sans, 13))
        c.textProperties.color = Palette.text
        c.secondaryTextProperties.color = Palette.secondary
        c.textProperties.numberOfLines = 0
        c.secondaryTextProperties.numberOfLines = 0
        c.imageProperties.tintColor = Palette.secondary
        c.imageProperties.maximumSize = CGSize(width: 26, height: 26)
        cell.contentConfiguration = c
        cell.backgroundColor = .clear
        cell.tintColor = Palette.accent
        let selected: Bool
        if mode == .newSession, let candidate = candidates[id], case let .project(projectId) = selection {
            selected = app.projectOptions.first { $0.id == projectId }?.groupKey == candidate.groupKey
        } else {
            selected = scope == selection
        }
        cell.accessoryType = selected ? .checkmark : .none
        cell.isAccessibilityElement = true
        cell.contentView.isAccessibilityElement = false
        cell.accessibilityElements = nil
        cell.accessoryView = nil
        cell.accessibilityLabel = [c.text, c.secondaryText].compactMap { $0 }.joined(separator: ", ")
        cell.accessibilityIdentifier = "project-\(id)"
        cell.accessibilityTraits = selected ? [.button, .selected] : [.button]
        if mode == .filter, let project = candidates[id] {
            let options = UIButton(type: .system)
            options.setImage(UIImage(systemName: "ellipsis"), for: .normal)
            options.tintColor = Palette.secondary
            options.accessibilityLabel = "Options for \(project.name)"
            options.accessibilityIdentifier = "project-options-\(project.id)"
            options.showsMenuAsPrimaryAction = true
            options.menu = projectActionsMenu(app: app, projectId: project.id)
            options.widthAnchor.constraint(equalToConstant: 44).isActive = true
            options.heightAnchor.constraint(equalToConstant: 44).isActive = true
            let accessory = UIStackView()
            accessory.alignment = .center
            accessory.spacing = 4
            if selected {
                let check = UIImageView(image: UIImage(systemName: "checkmark"))
                check.tintColor = Palette.accent
                check.isAccessibilityElement = false
                accessory.addArrangedSubview(check)
            }
            accessory.addArrangedSubview(options)
            accessory.frame.size = accessory.systemLayoutSizeFitting(UIView.layoutFittingCompressedSize)
            cell.accessoryType = .none
            cell.accessoryView = accessory
            // Keep both selection and management reachable to VoiceOver.
            cell.isAccessibilityElement = false
            let choice = ProjectChoiceAccessibilityElement(container: cell.contentView) { [weak self] in self?.choose(id) }
            choice.accessibilityLabel = cell.accessibilityLabel
            choice.accessibilityIdentifier = "project-choice-\(project.id)"
            choice.accessibilityTraits = cell.accessibilityTraits
            cell.accessibilityElements = [choice, options]
        }
    }

    private func reload() {
        if mode == .filter, app.workspaceSynced, case let .project(id) = selection, !app.projectOptions.contains(where: { $0.id == id }) { selection = .all }
        let query = search.searchBar.text ?? ""
        let matches = app.projects(matching: query)
        let projects: [ProjectOption]
        if mode == .newSession {
            let matchingRepositories = Set(matches.map(\.groupKey))
            repositoryMembers = Dictionary(grouping: app.projectOptions, by: \.groupKey)
            projects = repositoryMembers.values.compactMap { members in
                members.first.flatMap { matchingRepositories.contains($0.groupKey) ? $0 : nil }
            }.sorted {
                let order = $0.groupName.localizedCaseInsensitiveCompare($1.groupName)
                return order == .orderedSame ? $0.groupKey < $1.groupKey : order == .orderedAscending
            }
        } else {
            repositoryMembers = [:]
            projects = matches
        }
        candidates = Dictionary(uniqueKeysWithValues: projects.map { ("id:" + $0.id, $0) })
        var snapshot = NSDiffableDataSourceSnapshot<Int, String>()
        snapshot.appendSections([0, 1])
        if query.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty {
            snapshot.appendItems(mode == .filter ? ["all", "projectless"] : ["projectless"], toSection: 0)
        }
        snapshot.appendItems(projects.map { "id:" + $0.id }, toSection: 1)
        let old = Set(dataSource.snapshot().itemIdentifiers)
        snapshot.reconfigureItems(snapshot.itemIdentifiers.filter { old.contains($0) })
        dataSource.apply(snapshot, animatingDifferences: false)
        status.text = !app.workspaceSynced && projects.isEmpty ? "Syncing projects…" : projects.isEmpty ? (query.isEmpty ? "No projects yet" : "No matching projects") : nil
        status.isHidden = status.text == nil
    }

    func updateSearchResults(for searchController: UISearchController) {
        if searchController.isActive {
            navigationController?.sheetPresentationController?.animateChanges {
                self.navigationController?.sheetPresentationController?.selectedDetentIdentifier = .large
            }
        }
        reload()
    }

    func searchBarSearchButtonClicked(_ searchBar: UISearchBar) {
        if let id = dataSource.snapshot().itemIdentifiers.first { choose(id) }
    }

    func adaptivePresentationStyle(for controller: UIPresentationController, traitCollection: UITraitCollection) -> UIModalPresentationStyle {
        // A 360pt sidebar is locally compact even in a wide iPad split.
        // Adapt when the shell becomes compact, not just its source column.
        usesProjectPopover(controller.presentingViewController) ? .none : .pageSheet
    }

    func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        if let id = dataSource.itemIdentifier(for: indexPath) { choose(id) }
    }

    func tableView(_ tableView: UITableView, contextMenuConfigurationForRowAt indexPath: IndexPath, point: CGPoint) -> UIContextMenuConfiguration? {
        guard let id = dataSource.itemIdentifier(for: indexPath), let project = candidates[id] else { return nil }
        return UIContextMenuConfiguration(identifier: id as NSString, previewProvider: nil) { [weak self] _ in
            guard let self else { return nil }
            return self.projectActionsMenu(app: self.app, projectId: project.id)
        }
    }

    private func choose(_ id: String) {
        let scope = candidates[id].map { SessionScope.project(projectId: $0.id) } ?? (id == "all" ? .all : .projectless)
        onSelect(scope)
        close()
    }

    private func close() {
        // An active UISearchController is itself presented inside this nav.
        // Dismiss the entire picker from its presenter, including that child.
        navigationController?.presentingViewController?.dismiss(animated: true)
    }

    private func createProject() {
        search.isActive = false
        let vc = NewProjectViewController(app: app)
        let select = onSelect
        vc.onCreated = { id in select(.project(projectId: id)) }
        // The existing browser owns its modal dismissal. Presenting it from
        // our presenter closes both layers after successful creation.
        guard let presenter = presentingViewController ?? navigationController?.presentingViewController else { return }
        presenter.dismiss(animated: true) {
            presenter.present(MainTabController.nav(vc), animated: true)
        }
    }
}

extension UIViewController {
    func presentProjectPicker(app: AppModel, selection: SessionScope, mode: ProjectPickerViewController.Mode, source: UIView, barButtonItem: UIBarButtonItem? = nil, onSelect: @escaping (SessionScope) -> Void) {
        let picker = ProjectPickerViewController(app: app, selection: selection, mode: mode, onSelect: onSelect)
        let nav = MainTabController.nav(picker)
        if usesProjectPopover(self) {
            nav.modalPresentationStyle = .popover
            if let barButtonItem {
                nav.popoverPresentationController?.barButtonItem = barButtonItem
            } else {
                nav.popoverPresentationController?.sourceView = source
                nav.popoverPresentationController?.sourceRect = source.bounds
            }
            nav.popoverPresentationController?.delegate = picker
        } else {
            nav.modalPresentationStyle = .pageSheet
            // Accessibility text needs the full height for multiline names,
            // host/path details and the fixed creation action.
            let largeText = traitCollection.preferredContentSizeCategory.isAccessibilityCategory
            nav.sheetPresentationController?.detents = largeText ? [.large()] : [.medium(), .large()]
            nav.sheetPresentationController?.selectedDetentIdentifier = largeText ? .large : .medium
            nav.sheetPresentationController?.prefersGrabberVisible = true
        }
        present(nav, animated: true)
    }
}

private func usesProjectPopover(_ presenter: UIViewController) -> Bool {
    let context = presenter.splitViewController ?? presenter.view.window?.rootViewController ?? presenter
    if let split = context as? UISplitViewController, split.isCollapsed { return false }
    return presenter.traitCollection.userInterfaceIdiom == .pad && context.traitCollection.horizontalSizeClass == .regular
}

private final class ProjectChoiceAccessibilityElement: UIAccessibilityElement {
    private weak var container: UIView?
    private let activate: () -> Void

    init(container: UIView, activate: @escaping () -> Void) {
        self.container = container
        self.activate = activate
        super.init(accessibilityContainer: container)
    }

    override var accessibilityFrame: CGRect {
        get { container.map { UIAccessibility.convertToScreenCoordinates($0.bounds, in: $0) } ?? .zero }
        set {}
    }

    override func accessibilityActivate() -> Bool {
        activate()
        return true
    }
}
