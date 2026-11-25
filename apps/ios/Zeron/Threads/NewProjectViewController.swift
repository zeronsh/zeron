import UIKit

/// Create a project (a device + folder pair): pick a host, browse its folders
/// over the device relay (git repos badged), and use the current folder.
final class NewProjectViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var device: HostOption?
    private var listing: FolderListing?
    private var path: String?
    private var loading = false
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, String>!
    private let status = UILabel()
    /// A folder holding several repositories: that its worktrees can check
    /// them out together (docs/reference/project-worktrees.md).
    private let repositoriesNote = UILabel()
    private let useButton = UIButton(type: .system)

    /// Called with the new project's id (the new-session canvas selects it).
    var onCreated: ((String) -> Void)?

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "New Project"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)
        let reg = UICollectionView.CellRegistration<UICollectionViewListCell, String> { [weak self] cell, _, name in
            guard let entry = self?.listing?.entries.first(where: { $0.name == name }) else {
                var c = UIListContentConfiguration.cell()
                c.text = name == ".." ? "Parent Folder" : name
                c.image = UIImage(systemName: "arrow.turn.left.up")
                cell.contentConfiguration = c
                return
            }
            var c = UIListContentConfiguration.cell()
            c.text = entry.name
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.image = UIImage(systemName: entry.isRepo ? "folder.badge.gearshape" : entry.isDir ? "folder" : "doc")
            c.imageProperties.tintColor = entry.isRepo ? Palette.accent : Palette.secondary
            cell.contentConfiguration = c
            cell.accessories = entry.isDir ? [.disclosureIndicator()] : []
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, name in
            cv.dequeueConfiguredReusableCell(using: reg, for: path, item: name)
        }

        var use = UIButton.Configuration.filled()
        use.cornerStyle = .capsule
        use.baseBackgroundColor = Palette.text
        use.baseForegroundColor = Palette.background
        use.contentInsets = NSDirectionalEdgeInsets(top: 14, leading: 20, bottom: 14, trailing: 20)
        useButton.configuration = use
        useButton.addAction(UIAction { [weak self] _ in self?.create() }, for: .touchUpInside)
        useButton.accessibilityIdentifier = "use-folder"
        status.font = Fonts.ui(.sans, 13)
        status.textColor = Palette.secondary
        status.numberOfLines = 2
        status.textAlignment = .center
        repositoriesNote.numberOfLines = 0
        repositoriesNote.textAlignment = .center
        repositoriesNote.isHidden = true
        repositoriesNote.accessibilityIdentifier = "folder-repositories"
        let bottom = UIStackView(arrangedSubviews: [repositoriesNote, status, useButton])
        bottom.axis = .vertical
        bottom.spacing = 8
        bottom.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bottom)
        NSLayoutConstraint.activate([
            bottom.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            bottom.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            bottom.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -12),
        ])
        collectionView.contentInset.bottom = 110

        // Host picker lives in the navigation bar.
        let hosts = app.hostOptions
        device = hosts.first(where: \.online) ?? hosts.first
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: device?.name ?? "No hosts", menu: UIMenu(title: "Device", children: hosts.map { h in
            UIAction(title: h.name, subtitle: h.online ? "Online" : "Offline", image: UIImage(systemName: "desktopcomputer")) { [weak self] _ in
                self?.device = h
                self?.navigationItem.rightBarButtonItem?.title = h.name
                self?.load(nil)
            }
        }))
        load(nil)
    }

    private var loadGeneration = 0
    /// The device `listing` came from.
    private var listingDevice: String?

    private func load(_ path: String?) {
        guard let device else {
            status.text = "No desktop devices yet — open Zeron on a computer to add one."
            useButton.isEnabled = false
            return
        }
        loading = true
        status.text = "Loading…"
        // Until this device answers, nothing from the last one can be used.
        useButton.isEnabled = false
        loadGeneration += 1
        let generation = loadGeneration
        Task { @MainActor in
            let listing = await app.listFolders(deviceId: device.id, path: path)
            // A newer load (another device or folder) superseded this one.
            guard generation == loadGeneration else { return }
            loading = false
            guard let listing else {
                status.text = device.online ? "Couldn't read that folder." : "\(device.name) is offline."
                // Keep browsing the last good folder only on the same device.
                useButton.isEnabled = self.listing != nil && self.listingDevice == device.id
                return
            }
            self.listingDevice = device.id
            self.listing = listing
            self.path = listing.path
            var items: [String] = []
            if Self.parent(of: listing.path) != nil { items.append("..") }
            items += listing.entries.filter(\.isDir).map(\.name)
            var s = NSDiffableDataSourceSnapshot<Int, String>()
            s.appendSections([0])
            s.appendItems(items)
            // (Reloaded whole: rows are keyed by name, and a folder of the same
            // name elsewhere — a repository here, not there — must not keep
            // the last folder's look.)
            await dataSource.applySnapshotUsingReloadData(s)
            let name = (listing.path as NSString).lastPathComponent
            var c = useButton.configuration
            c?.title = "Use “\(name.isEmpty ? listing.path : name)”"
            useButton.configuration = c
            useButton.isEnabled = true
            status.text = listing.path
            // (As its host finds them: the count a worktree of it offers.)
            showRepositories(0)
            let count = await app.folderRepositories(deviceId: device.id, path: listing.path)
            guard generation == loadGeneration else { return }
            showRepositories(count)
            title = name.isEmpty ? "New Project" : name
        }
    }

    /// "Holds 3 repositories…" over the path, for a folder holding two or
    /// more; nothing otherwise.
    private func showRepositories(_ count: Int) {
        guard count >= 2 else {
            repositoriesNote.isHidden = true
            return
        }
        let glyph = NSTextAttachment(image: BranchIcon.sized(13)?.withTintColor(Palette.accent, renderingMode: .alwaysOriginal) ?? UIImage())
        glyph.bounds = CGRect(x: 0, y: -2, width: 13, height: 13)
        let note = NSMutableAttributedString(attachment: glyph)
        note.append(NSAttributedString(string: "  Holds \(count) repositories", attributes: [.font: Fonts.ui(.sansMedium, 14), .foregroundColor: Palette.text]))
        note.append(NSAttributedString(string: "\nNew worktrees check them out together, on one branch.", attributes: [.font: Fonts.ui(.sans, 13), .foregroundColor: Palette.secondary]))
        repositoriesNote.attributedText = note
        repositoriesNote.isHidden = false
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard !loading, let name = dataSource.itemIdentifier(for: indexPath), let listing else { return }
        if name == ".." {
            load(Self.parent(of: listing.path))
        } else {
            load((listing.path as NSString).appendingPathComponent(name))
        }
    }

    static func parent(of path: String) -> String? {
        let trimmed = path.hasSuffix("/") && path.count > 1 ? String(path.dropLast()) : path
        guard trimmed != "/", trimmed.contains("/") else { return nil }
        let up = (trimmed as NSString).deletingLastPathComponent
        return up.isEmpty ? "/" : up
    }

    private func create() {
        // The path must be one this device listed.
        guard let device, let path, listingDevice == device.id else { return }
        let git = listing?.entries.contains { $0.name == ".git" } ?? false
        useButton.configuration?.showsActivityIndicator = true
        Task { @MainActor in
            let created = await app.createProject(deviceId: device.id, path: path, gitDetected: git)
            let ok = created != nil
            if let created { onCreated?(created) }
            useButton.configuration?.showsActivityIndicator = false
            if ok { dismiss(animated: true) } else { status.text = "Couldn't create the project on \(device.name)." }
        }
    }
}
