import UIKit

/// New Project's Repositories: a project of repositories from anywhere on
/// one computer, each staying where it is (docs/reference/composed-projects.md).
/// The computer's repositories, newest worked-in first, a check on each
/// chosen; a search over them; New Repository makes one there and chooses
/// it. The name, left empty, follows what's chosen (their GitHub owner when
/// they share one, else the first one's name).
///
///   NAME
///   [ acme                                   ]
///   REPOSITORIES ON MACBOOK PRO
///   [🔍 Search                                ]
///   api            ~/code/api · main          ✓
///   web            ~/acme/web · develop       ✓
///   + New Repository…
///   [ Create Project · 2 Repositories ]
final class ComposeRepositoriesController: UIViewController, UICollectionViewDelegate {
    private enum Item: Hashable {
        case name
        case search
        case repository(String)
        case new
        case status
    }

    private let app: AppModel
    private let device: HostOption
    private let onCreated: (String) -> Void

    private var listing: LocalRepositories?
    private var failure: String?
    private var chosen: [String] = []
    private var query = ""

    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, Item>!
    private let nameField = UITextField()
    private let searchField = UISearchTextField()
    private let createButton = UIButton(type: .system)
    private let status = UILabel()

    init(app: AppModel, device: HostOption, onCreated: @escaping (String) -> Void) {
        self.app = app
        self.device = device
        self.onCreated = onCreated
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .clear
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        config.headerMode = .supplementary
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.keyboardDismissMode = .interactive
        collectionView.accessibilityIdentifier = "compose-list"
        view.addSubview(collectionView)

        nameField.font = Fonts.ui(.sansMedium, 17)
        nameField.textColor = Palette.text
        nameField.clearButtonMode = .whileEditing
        nameField.autocapitalizationType = .none
        nameField.autocorrectionType = .no
        nameField.returnKeyType = .done
        nameField.accessibilityIdentifier = "compose-name"
        nameField.addAction(UIAction { [weak self] _ in self?.renderButton() }, for: .editingChanged)
        nameField.addAction(UIAction { [weak self] _ in self?.nameField.resignFirstResponder() }, for: .editingDidEndOnExit)

        searchField.placeholder = "Search"
        searchField.font = Fonts.ui(.sans, 16)
        searchField.backgroundColor = .clear
        searchField.borderStyle = .none
        searchField.autocapitalizationType = .none
        searchField.autocorrectionType = .no
        searchField.accessibilityIdentifier = "compose-search"
        searchField.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.query = self.searchField.text ?? ""
            self.render()
        }, for: .editingChanged)

        let fieldCell = UICollectionView.CellRegistration<UICollectionViewListCell, Item> { [weak self] cell, _, item in
            guard let self else { return }
            let field: UIView = item == .name ? self.nameField : self.searchField
            cell.contentView.subviews.forEach { $0.removeFromSuperview() }
            field.translatesAutoresizingMaskIntoConstraints = false
            cell.contentView.addSubview(field)
            NSLayoutConstraint.activate([
                field.leadingAnchor.constraint(equalTo: cell.contentView.layoutMarginsGuide.leadingAnchor),
                field.trailingAnchor.constraint(equalTo: cell.contentView.layoutMarginsGuide.trailingAnchor),
                field.topAnchor.constraint(equalTo: cell.contentView.topAnchor, constant: 6),
                field.bottomAnchor.constraint(equalTo: cell.contentView.bottomAnchor, constant: -6),
                field.heightAnchor.constraint(greaterThanOrEqualToConstant: 34),
            ])
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
        }
        let repositoryCell = UICollectionView.CellRegistration<UICollectionViewListCell, Item> { [weak self] cell, _, item in
            guard let self, case let .repository(path) = item, let r = self.listing?.repositories.first(where: { $0.path == path }) else { return }
            var c = UIListContentConfiguration.subtitleCell()
            c.text = r.name
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.textProperties.color = Palette.text
            c.secondaryText = self.detail(r)
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.secondaryTextProperties.color = Palette.secondary
            c.secondaryTextProperties.numberOfLines = 1
            c.secondaryTextProperties.lineBreakMode = .byTruncatingMiddle
            c.image = BranchIcon.sized(15)?.withTintColor(Palette.secondary, renderingMode: .alwaysOriginal)
            c.textToSecondaryTextVerticalPadding = 3
            cell.contentConfiguration = c
            let on = self.chosen.contains(path)
            cell.accessories = on ? [.checkmark(options: .init(tintColor: Palette.accent))] : []
            cell.accessibilityIdentifier = "compose-repository-\(r.name)"
            cell.accessibilityTraits = on ? [.button, .selected] : [.button]
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
        }
        let actionCell = UICollectionView.CellRegistration<UICollectionViewListCell, Item> { [weak self] cell, _, item in
            guard let self else { return }
            var c = UIListContentConfiguration.cell()
            if item == .new {
                c.text = "New Repository…"
                c.textProperties.color = Palette.accent
                c.textProperties.font = Fonts.ui(.sansMedium, 16)
                c.image = UIImage(systemName: "plus.circle.fill")
                c.imageProperties.tintColor = Palette.accent
                cell.accessibilityIdentifier = "compose-new-repository"
            } else {
                c.text = self.failure ?? (self.listing == nil ? "Looking for repositories…" : self.query.isEmpty ? "No repositories found on \(self.device.name)." : "Nothing matches “\(self.query)”.")
                c.textProperties.color = Palette.secondary
                c.textProperties.font = Fonts.ui(.sans, 15)
                c.textProperties.numberOfLines = 0
                cell.accessibilityIdentifier = "compose-status"
            }
            cell.contentConfiguration = c
            cell.accessories = []
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, index, item in
            switch item {
            case .name, .search: cv.dequeueConfiguredReusableCell(using: fieldCell, for: index, item: item)
            case .repository: cv.dequeueConfiguredReusableCell(using: repositoryCell, for: index, item: item)
            case .new, .status: cv.dequeueConfiguredReusableCell(using: actionCell, for: index, item: item)
            }
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] cell, _, index in
            guard let self else { return }
            var c = UIListContentConfiguration.groupedHeader()
            c.text = index.section == 0 ? "Name" : "Repositories on \(self.device.name)"
            cell.contentConfiguration = c
        }
        dataSource.supplementaryViewProvider = { cv, _, index in
            cv.dequeueConfiguredReusableSupplementary(using: header, for: index)
        }

        var create = UIButton.Configuration.filled()
        create.cornerStyle = .capsule
        create.baseBackgroundColor = Palette.text
        create.baseForegroundColor = Palette.background
        create.contentInsets = NSDirectionalEdgeInsets(top: 14, leading: 20, bottom: 14, trailing: 20)
        createButton.configuration = create
        createButton.accessibilityIdentifier = "compose-create"
        createButton.addAction(UIAction { [weak self] _ in self?.create() }, for: .touchUpInside)
        status.font = Fonts.ui(.sans, 13)
        status.textColor = Palette.secondary
        status.numberOfLines = 2
        status.textAlignment = .center
        status.isHidden = true
        let bottom = UIStackView(arrangedSubviews: [status, createButton])
        bottom.axis = .vertical
        bottom.spacing = 8
        bottom.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bottom)
        NSLayoutConstraint.activate([
            bottom.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            bottom.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            bottom.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor, constant: -12),
        ])
        collectionView.contentInset.bottom = 96

        render()
        load()
    }

    // MARK: Data

    private func load() {
        Task { @MainActor in
            do {
                listing = try await app.localRepositories(deviceId: device.id)
                failure = nil
            } catch {
                failure = device.online ? AppModel.says(error) : "\(device.name) is offline."
            }
            render()
        }
    }

    /// `~/code/api · main · acme/api`
    private func detail(_ r: LocalRepository) -> String {
        var parts = [tilde(r.path)]
        parts.append(r.hasCommits ? (r.branch ?? "Detached") : "No commits yet")
        if let github = r.github { parts.append(github) }
        return parts.joined(separator: " · ")
    }

    private func tilde(_ path: String) -> String {
        guard let home = listing?.home, !home.isEmpty, path.hasPrefix(home + "/") else { return path }
        return "~" + path.dropFirst(home.count)
    }

    private var shown: [LocalRepository] {
        let all = listing?.repositories ?? []
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return all }
        let byName = all.filter { $0.name.lowercased().hasPrefix(q) }
        let inName = all.filter { !$0.name.lowercased().hasPrefix(q) && $0.name.lowercased().contains(q) }
        let elsewhere = all.filter { !$0.name.lowercased().contains(q) && ($0.path.lowercased().contains(q) || ($0.github?.lowercased().contains(q) ?? false)) }
        return byName + inName + elsewhere
    }

    /// The name a project gets when none is typed.
    private var suggested: String {
        let picked = chosen.compactMap { p in listing?.repositories.first { $0.path == p } }
        guard let first = picked.first else { return "" }
        let owners = Set(picked.map { $0.github?.split(separator: "/").first.map { String($0).lowercased() } ?? "" })
        if picked.count > 1, owners.count == 1, let owner = picked.first?.github?.split(separator: "/").first {
            return String(owner)
        }
        return first.name
    }

    private var projectName: String {
        let typed = (nameField.text ?? "").trimmingCharacters(in: .whitespaces)
        return typed.isEmpty ? suggested : typed
    }

    private func render() {
        var s = NSDiffableDataSourceSnapshot<Int, Item>()
        s.appendSections([0, 1])
        s.appendItems([.name], toSection: 0)
        var rows: [Item] = [.search]
        let list = shown
        if list.isEmpty {
            rows.append(.status)
        } else {
            rows += list.map { .repository($0.path) }
        }
        if listing != nil { rows.append(.new) }
        s.appendItems(rows, toSection: 1)
        s.reconfigureItems(s.itemIdentifiers.filter { if case .repository = $0 { return true }; return $0 == .status })
        dataSource.apply(s, animatingDifferences: false)
        renderButton()
    }

    private func renderButton() {
        nameField.attributedPlaceholder = NSAttributedString(string: suggested.isEmpty ? "Project name" : suggested, attributes: [.foregroundColor: Palette.tertiary])
        let n = chosen.count
        var c = createButton.configuration
        c?.title = n == 0 ? "Choose Repositories" : "Create Project · \(n) \(n == 1 ? "Repository" : "Repositories")"
        createButton.configuration = c
        createButton.isEnabled = n > 0 && !projectName.isEmpty
    }

    // MARK: Actions

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard let item = dataSource.itemIdentifier(for: indexPath) else { return }
        switch item {
        case let .repository(path):
            if let i = chosen.firstIndex(of: path) { chosen.remove(at: i) } else { chosen.append(path) }
            UISelectionFeedbackGenerator().selectionChanged()
            var s = dataSource.snapshot()
            s.reconfigureItems([item])
            dataSource.apply(s, animatingDifferences: false)
            renderButton()
        case .new:
            newRepository()
        case .name:
            nameField.becomeFirstResponder()
        case .search:
            searchField.becomeFirstResponder()
        case .status:
            if failure != nil { failure = nil; listing = nil; render(); load() }
        }
    }

    private func newRepository() {
        let form = NewRepositoryViewController(app: app, device: device, canPublish: listing?.canPublish ?? false, name: query) { [weak self] made in
            guard let self else { return }
            var list = self.listing?.repositories ?? []
            list.removeAll { $0.path == made.path }
            list.insert(made, at: 0)
            self.listing?.repositories = list
            self.chosen.append(made.path)
            self.query = ""
            self.searchField.text = ""
            self.render()
        }
        let nav = UINavigationController(rootViewController: form)
        nav.modalPresentationStyle = .formSheet
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.medium(), .large()]
            sheet.prefersGrabberVisible = true
        }
        present(nav, animated: true)
    }

    private func create() {
        let name = projectName
        guard !chosen.isEmpty, !name.isEmpty else { return }
        createButton.configuration?.showsActivityIndicator = true
        createButton.isEnabled = false
        status.isHidden = true
        Task { @MainActor in
            do {
                let id = try await app.composeProject(deviceId: device.id, name: name, paths: chosen)
                onCreated(id)
            } catch {
                status.text = AppModel.says(error)
                status.isHidden = false
            }
            createButton.configuration?.showsActivityIndicator = false
            renderButton()
        }
    }
}

/// A new repository on a computer: its name, a README as its first commit,
/// and (where the computer's GitHub CLI is signed in) a copy on GitHub.
final class NewRepositoryViewController: UIViewController {
    private let app: AppModel
    private let device: HostOption
    private let canPublish: Bool
    private let onCreated: (LocalRepository) -> Void
    private let nameField = UITextField()
    private let readme = UISwitch()
    private let publish = UISwitch()
    private let visibility = UISegmentedControl(items: ["Private", "Public"])
    private let note = UILabel()
    private var initialName: String

    init(app: AppModel, device: HostOption, canPublish: Bool, name: String, onCreated: @escaping (LocalRepository) -> Void) {
        self.app = app
        self.device = device
        self.canPublish = canPublish
        self.onCreated = onCreated
        initialName = name.trimmingCharacters(in: .whitespaces)
        super.init(nibName: nil, bundle: nil)
        title = "New Repository"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .cancel, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        let createItem = UIBarButtonItem(title: "Create", primaryAction: UIAction { [weak self] _ in self?.create() })
        createItem.style = .done
        createItem.accessibilityIdentifier = "new-repository-create"
        navigationItem.rightBarButtonItem = createItem

        nameField.text = initialName
        nameField.placeholder = "my-repository"
        nameField.font = Fonts.ui(.sansMedium, 17)
        nameField.autocapitalizationType = .none
        nameField.autocorrectionType = .no
        nameField.clearButtonMode = .whileEditing
        nameField.accessibilityIdentifier = "new-repository-name"
        nameField.addAction(UIAction { [weak self] _ in self?.changed() }, for: .editingChanged)
        readme.isOn = true
        readme.onTintColor = Palette.accent
        publish.isOn = false
        publish.onTintColor = Palette.accent
        publish.addAction(UIAction { [weak self] _ in self?.changed() }, for: .valueChanged)
        visibility.selectedSegmentIndex = 0

        let nameBox = card([nameField])
        let readmeRow = row("Start with a README", "Its first commit, on main: worktrees branch from it.", readme)
        var rows: [UIView] = [section("Name"), nameBox, card([readmeRow])]
        if canPublish {
            let publishRow = row("Also create it on GitHub", "With the GitHub CLI signed in on \(device.name).", publish)
            rows.append(card([publishRow, visibility]))
        }
        note.font = Fonts.ui(.sans, 13)
        note.textColor = Palette.secondary
        note.numberOfLines = 0
        rows.append(note)
        let stack = UIStackView(arrangedSubviews: rows)
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 16),
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
        ])
        changed()
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        nameField.becomeFirstResponder()
    }

    private func section(_ title: String) -> UIView {
        let l = UILabel()
        l.text = title.uppercased()
        l.font = Fonts.ui(.sansMedium, 12)
        l.textColor = Palette.tertiary
        return l
    }

    private func card(_ views: [UIView]) -> UIView {
        let s = UIStackView(arrangedSubviews: views)
        s.axis = .vertical
        s.spacing = 12
        s.isLayoutMarginsRelativeArrangement = true
        s.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 12, leading: 16, bottom: 12, trailing: 16)
        s.backgroundColor = Palette.elevated
        s.layer.cornerRadius = 14
        s.layer.cornerCurve = .continuous
        return s
    }

    private func row(_ title: String, _ detail: String, _ control: UIView) -> UIView {
        let t = UILabel()
        t.text = title
        t.font = Fonts.ui(.sansMedium, 16)
        t.textColor = Palette.text
        let d = UILabel()
        d.text = detail
        d.font = Fonts.ui(.sans, 13)
        d.textColor = Palette.secondary
        d.numberOfLines = 0
        let words = UIStackView(arrangedSubviews: [t, d])
        words.axis = .vertical
        words.spacing = 2
        let r = UIStackView(arrangedSubviews: [words, control])
        r.alignment = .center
        r.spacing = 12
        return r
    }

    /// The name as the computer will make it (letters, digits, `.`, `_`, `-`).
    private var clean: String {
        let raw = (nameField.text ?? "").trimmingCharacters(in: .whitespaces)
        let mapped = String(raw.map { $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "." || $0 == "_" || $0 == "-") ? $0 : "-" })
        return mapped.trimmingCharacters(in: CharacterSet(charactersIn: "-."))
    }

    private func changed() {
        visibility.isHidden = !publish.isOn
        navigationItem.rightBarButtonItem?.isEnabled = !clean.isEmpty
        note.text = clean.isEmpty ? "Made where most of \(device.name)'s repositories are." : "Made as “\(clean)” where most of \(device.name)'s repositories are."
        note.textColor = Palette.secondary
    }

    private func create() {
        guard !clean.isEmpty else { return }
        navigationItem.rightBarButtonItem?.isEnabled = false
        let spinner = UIActivityIndicatorView(style: .medium)
        spinner.startAnimating()
        let item = navigationItem.rightBarButtonItem
        navigationItem.rightBarButtonItem = UIBarButtonItem(customView: spinner)
        Task { @MainActor in
            do {
                let made = try await app.createRepository(deviceId: device.id, name: clean, empty: !readme.isOn, publish: canPublish && publish.isOn ? visibility.selectedSegmentIndex == 0 : nil)
                onCreated(made.repository)
                dismiss(animated: true)
                if let warning = made.warning { NSLog("new repository: \(warning)") }
            } catch {
                navigationItem.rightBarButtonItem = item
                item?.isEnabled = true
                note.text = AppModel.says(error)
                note.textColor = Palette.danger
            }
        }
    }
}
