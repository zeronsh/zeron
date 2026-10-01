import UIKit

/// Create a project (a device + folder pair): pick a host, browse its folders
/// over the device relay (git repos badged), and use the current folder.
final class NewProjectViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var device: HostOption?
    private var listing: FolderListing?
    private var path: String?
    private var loading = false
    private var submitting = false
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, String>!
    private let status = UILabel()
    private let useButton = UIButton(type: .system)
    private let newFolderButton = UIButton(type: .system)

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
        var newFolder = UIButton.Configuration.plain()
        newFolder.title = "New Folder…"
        newFolder.image = UIImage(systemName: "folder.badge.plus")
        newFolder.imagePadding = 8
        newFolder.baseForegroundColor = Palette.text
        newFolderButton.configuration = newFolder
        newFolderButton.accessibilityIdentifier = "new-folder"
        newFolderButton.addAction(UIAction { [weak self] _ in self?.promptForFolder() }, for: .touchUpInside)
        let bottom = UIStackView(arrangedSubviews: [status, newFolderButton, useButton])
        bottom.axis = .vertical
        bottom.spacing = 8
        bottom.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(bottom)
        NSLayoutConstraint.activate([
            bottom.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            bottom.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            bottom.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -12),
        ])
        collectionView.contentInset.bottom = 160

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
            newFolderButton.isEnabled = false
            return
        }
        loading = true
        status.text = "Loading…"
        // Until this device answers, nothing from the last one can be used.
        useButton.isEnabled = false
        newFolderButton.isEnabled = false
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
                newFolderButton.isEnabled = useButton.isEnabled
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
            await dataSource.apply(s, animatingDifferences: false)
            let name = (listing.path as NSString).lastPathComponent
            var c = useButton.configuration
            c?.title = "Use “\(name.isEmpty ? listing.path : name)”"
            useButton.configuration = c
            useButton.isEnabled = true
            newFolderButton.isEnabled = true
            status.text = listing.path
            title = name.isEmpty ? "New Project" : name
        }
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: true)
        guard !loading, !submitting, listingDevice == device?.id,
              let name = dataSource.itemIdentifier(for: indexPath), let listing else { return }
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

    private func promptForFolder(name: String = "", error: String? = nil) {
        guard !loading, !submitting, let device, let path, listingDevice == device.id else { return }
        // A fast failure can arrive while the original name alert is still
        // dismissing. Finish that transition before presenting the retry.
        if let presented = presentedViewController {
            presented.dismiss(animated: true) { [weak self] in self?.promptForFolder(name: name, error: error) }
            return
        }
        let alert = UIAlertController(title: "New Folder", message: error ?? "Create a folder in \(path) on \(device.name).", preferredStyle: .alert)
        alert.addTextField { field in
            field.placeholder = "Folder name"
            field.text = name
            field.autocapitalizationType = .none
            field.autocorrectionType = .no
            field.accessibilityIdentifier = "new-folder-name"
        }
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        let create = UIAlertAction(title: "Create", style: .default) { [weak self, weak alert] _ in
            guard let self, let name = alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespacesAndNewlines), !name.isEmpty else { return }
            self.setSubmitting(true)
            self.status.text = "Creating folder…"
            Task { @MainActor in
                do {
                    let created = try await self.app.createFolder(deviceId: device.id, parentPath: path, name: name)
                    self.setSubmitting(false)
                    self.load(created)
                } catch {
                    self.setSubmitting(false)
                    self.status.text = path
                    self.promptForFolder(name: name, error: "Couldn't create the folder. \(Self.folderErrorMessage(error))")
                }
            }
        }
        create.isEnabled = !name.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty
        alert.textFields?.first?.addAction(UIAction { [weak alert, weak create] _ in
            create?.isEnabled = !(alert?.textFields?.first?.text?.trimmingCharacters(in: .whitespacesAndNewlines).isEmpty ?? true)
        }, for: .editingChanged)
        alert.addAction(create)
        present(alert, animated: true)
    }

    private static func folderErrorMessage(_ error: Error) -> String {
        guard let error = error as? CoreError else { return error.localizedDescription }
        switch error {
        case .NotFound(let message), .InvalidArgument(let message), .HostUnavailable(let message),
             .Unsupported(let message), .HostError(let message), .Network(let message),
             .Auth(let message), .Storage(let message), .NotImplemented(let message), .Internal(let message):
            return message
        case .Closed:
            return "Zeron is disconnected."
        }
    }

    private func setSubmitting(_ busy: Bool) {
        submitting = busy
        useButton.isEnabled = !busy
        newFolderButton.isEnabled = !busy
        navigationItem.rightBarButtonItem?.isEnabled = !busy
        navigationItem.leftBarButtonItem?.isEnabled = !busy
        isModalInPresentation = busy
    }

    private func create() {
        // The path must be one this device listed.
        guard !loading, !submitting, let device, let path, listingDevice == device.id else { return }
        let git = listing?.entries.contains { $0.name == ".git" } ?? false
        useButton.configuration?.showsActivityIndicator = true
        setSubmitting(true)
        Task { @MainActor in
            let created = await app.createProject(deviceId: device.id, path: path, gitDetected: git)
            let ok = created != nil
            if let created { onCreated?(created) }
            useButton.configuration?.showsActivityIndicator = false
            setSubmitting(false)
            if ok { dismiss(animated: true) } else { status.text = "Couldn't create the project on \(device.name)." }
        }
    }
}
