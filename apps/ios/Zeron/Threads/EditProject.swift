import UIKit

extension UIViewController {
    func editProject(_ id: String, app: AppModel) {
        guard let project = app.projectOptions.first(where: { $0.id == id }) else { return }
        let editor = ProjectEditorViewController(project: project, app: app)
        let navigation = UINavigationController(rootViewController: editor)
        navigation.modalPresentationStyle = .pageSheet
        if let sheet = navigation.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
            sheet.preferredCornerRadius = 24
        }
        present(navigation, animated: true)
    }
}

private final class ProjectEditorViewController: UIViewController, UITextFieldDelegate, UITableViewDataSource, UITableViewDelegate {
    private let project: ProjectOption
    private let app: AppModel
    private let content = UIStackView()
    private let preview = UIImageView()
    private let nameField = UITextField()
    private let iconField = UILabel()
    private let browseButton = UIButton(type: .system)
    private let browser = UIStackView()
    private let searchField = UISearchTextField()
    private let images = UITableView(frame: .zero, style: .plain)
    private let statusLabel = UILabel()
    private var selectedPath: String?
    private var artwork: Data?
    private var automaticArtwork: Data?
    private var paths: [String] = []
    private var thumbnails: [String: UIImage] = [:]
    private var renderedArtwork: [String: Data] = [:]
    private var searchTask: Task<Void, Never>?
    private var previewTask: Task<Void, Never>?
    private var thumbnailTasks: [String: Task<Data?, Never>] = [:]
    private var searchGeneration = UUID()
    private var previewGeneration = UUID()

    init(project: ProjectOption, app: AppModel) {
        self.project = project
        self.app = app
        selectedPath = project.iconPath
        artwork = project.iconData
        automaticArtwork = project.iconPath == nil ? project.iconData : nil
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError("init(coder:) has not been implemented") }

    deinit {
        searchTask?.cancel()
        previewTask?.cancel()
        thumbnailTasks.values.forEach { $0.cancel() }
    }

    override func viewDidLoad() {
        super.viewDidLoad()
        title = "Edit Project"
        view.backgroundColor = Palette.background
        navigationItem.leftBarButtonItem = UIBarButtonItem(title: "Cancel", style: .plain, target: self, action: #selector(cancel))
        navigationItem.rightBarButtonItem = UIBarButtonItem(title: "Save", style: .prominent, target: self, action: #selector(save))

        let scroll = UIScrollView()
        scroll.keyboardDismissMode = .interactive
        scroll.alwaysBounceVertical = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(scroll)
        content.axis = .vertical
        content.spacing = 24
        content.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(content)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.keyboardLayoutGuide.topAnchor),
            content.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 24),
            content.leadingAnchor.constraint(equalTo: scroll.contentLayoutGuide.leadingAnchor, constant: 24),
            content.trailingAnchor.constraint(equalTo: scroll.contentLayoutGuide.trailingAnchor, constant: -24),
            content.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -32),
            content.widthAnchor.constraint(equalTo: scroll.frameLayoutGuide.widthAnchor, constant: -48),
        ])
        buildName()
        buildIcon()
        statusLabel.font = Fonts.ui(.sans, 13)
        statusLabel.textColor = Palette.danger
        statusLabel.numberOfLines = 0
        statusLabel.isHidden = true
        content.addArrangedSubview(statusLabel)
        refreshSelection()
        if artwork == nil { loadPreview() }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (editor: ProjectEditorViewController, _) in
            editor.refreshSelection()
            editor.images.reloadData()
        }
    }

    private func label(_ text: String) -> UILabel {
        let label = UILabel()
        label.text = text
        label.font = Fonts.ui(.sansMedium, 14)
        label.textColor = Palette.secondary
        return label
    }

    @discardableResult
    private func section(_ title: String, body: UIView) -> UIStackView {
        let stack = UIStackView(arrangedSubviews: [label(title), body])
        stack.axis = .vertical
        stack.spacing = 10
        content.addArrangedSubview(stack)
        return stack
    }

    private func buildName() {
        preview.contentMode = .scaleAspectFit
        preview.accessibilityLabel = "Project preview"
        preview.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([preview.widthAnchor.constraint(equalToConstant: 48), preview.heightAnchor.constraint(equalToConstant: 48)])
        nameField.text = project.groupName
        nameField.font = Fonts.ui(.sansMedium, 20)
        nameField.textColor = Palette.text
        nameField.placeholder = "Project name"
        nameField.accessibilityLabel = "Project name"
        nameField.autocorrectionType = .no
        nameField.returnKeyType = .done
        nameField.clearButtonMode = .whileEditing
        nameField.delegate = self
        nameField.addTarget(self, action: #selector(nameChanged), for: .editingChanged)
        let row = UIStackView(arrangedSubviews: [preview, nameField])
        row.spacing = 14
        row.alignment = .center
        section("Name", body: row)
    }

    private func buildIcon() {
        iconField.font = Fonts.ui(.sans, 16)
        iconField.textColor = Palette.text
        iconField.lineBreakMode = .byTruncatingMiddle
        iconField.translatesAutoresizingMaskIntoConstraints = false
        let field = UIView()
        field.backgroundColor = Palette.controlFill
        field.layer.cornerRadius = 12
        field.addSubview(iconField)
        NSLayoutConstraint.activate([
            field.heightAnchor.constraint(equalToConstant: 50),
            iconField.leadingAnchor.constraint(equalTo: field.leadingAnchor, constant: 14),
            iconField.trailingAnchor.constraint(equalTo: field.trailingAnchor, constant: -14),
            iconField.centerYAnchor.constraint(equalTo: field.centerYAnchor),
        ])
        browseButton.accessibilityLabel = "Browse project images"
        browseButton.accessibilityIdentifier = "project-icon-selector"
        browseButton.addTarget(self, action: #selector(toggleBrowser), for: .touchUpInside)
        NSLayoutConstraint.activate([
            browseButton.widthAnchor.constraint(equalToConstant: 50),
            browseButton.heightAnchor.constraint(equalToConstant: 50),
        ])
        let iconRow = UIStackView(arrangedSubviews: [field, browseButton])
        iconRow.spacing = 8
        iconRow.alignment = .center
        let iconSection = section("Icon", body: iconRow)
        browser.axis = .vertical
        browser.spacing = 8
        browser.isHidden = true
        searchField.placeholder = "Search images"
        searchField.accessibilityLabel = "Search project images"
        searchField.autocapitalizationType = .none
        searchField.autocorrectionType = .no
        searchField.returnKeyType = .search
        searchField.delegate = self
        searchField.addTarget(self, action: #selector(searchChanged), for: .editingChanged)
        searchField.heightAnchor.constraint(equalToConstant: 40).isActive = true
        images.dataSource = self
        images.delegate = self
        images.backgroundColor = Palette.elevated
        images.separatorColor = Palette.hairline
        images.rowHeight = 58
        images.layer.cornerRadius = 12
        images.clipsToBounds = true
        images.keyboardDismissMode = .onDrag
        images.accessibilityIdentifier = "project-image-browser"
        images.heightAnchor.constraint(equalToConstant: 232).isActive = true
        browser.addArrangedSubview(searchField)
        browser.addArrangedSubview(images)
        iconSection.addArrangedSubview(browser)
    }

    private func refreshSelection() {
        let name = nameField.text?.trimmingCharacters(in: .whitespacesAndNewlines) ?? ""
        preview.image = ProjectTile.image(name: name.isEmpty ? project.name : name, colorIndex: project.colorIndex, side: 48, artwork: artwork)
        iconField.text = selectedPath.map { ($0 as NSString).lastPathComponent } ?? "Automatic"
        var config = UIButton.Configuration.plain()
        config.image = UIImage(systemName: browser.isHidden ? "folder" : "folder.fill")
        config.baseForegroundColor = Palette.text
        config.background.backgroundColor = browser.isHidden ? .clear : Palette.controlFill
        config.cornerStyle = .medium
        browseButton.configuration = config
    }

    @objc private func nameChanged() { refreshSelection() }

    @objc private func toggleBrowser() {
        browser.isHidden.toggle()
        view.endEditing(true)
        refreshSelection()
        if !browser.isHidden { searchImages() }
    }

    @objc private func searchChanged() { searchImages() }

    private func searchImages() {
        searchTask?.cancel()
        searchGeneration = UUID()
        let generation = searchGeneration
        let query = searchField.text ?? ""
        guard let client = app.client else { return }
        let id = project.id
        searchTask = Task { @MainActor [weak self] in
            do {
                try await Task.sleep(for: .milliseconds(200))
                let found = try await client.projectImages(spaceId: id, query: query)
                guard !Task.isCancelled, let self, self.searchGeneration == generation else { return }
                self.paths = found
                self.statusLabel.isHidden = true
                self.images.reloadData()
            } catch {
                guard !Task.isCancelled, let self, self.searchGeneration == generation else { return }
                self.paths = []
                self.images.reloadData()
                self.showError("Couldn’t load images.")
            }
        }
    }

    func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { paths.count + 1 }

    func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let cell = tableView.dequeueReusableCell(withIdentifier: "image") ?? UITableViewCell(style: .subtitle, reuseIdentifier: "image")
        cell.backgroundColor = .clear
        cell.textLabel?.font = Fonts.ui(.sansMedium, 14)
        cell.textLabel?.textColor = Palette.text
        cell.detailTextLabel?.font = Fonts.ui(.sans, 11)
        cell.detailTextLabel?.textColor = Palette.secondary
        cell.imageView?.contentMode = .scaleAspectFit
        cell.imageView?.layer.cornerRadius = 6
        cell.imageView?.clipsToBounds = true
        let path = indexPath.row == 0 ? nil : paths[indexPath.row - 1]
        cell.textLabel?.text = path.map { ($0 as NSString).lastPathComponent } ?? "Automatic"
        cell.detailTextLabel?.text = path.map { ($0 as NSString).deletingLastPathComponent }
        cell.imageView?.image = path.flatMap { thumbnails[$0] } ?? ProjectTile.image(name: nameField.text ?? project.groupName, colorIndex: project.colorIndex, side: 30, artwork: path == nil ? automaticArtwork : nil)
        cell.accessoryType = path == selectedPath ? .checkmark : .none
        cell.tintColor = Palette.accent
        cell.accessibilityIdentifier = path.map { "project-image:\($0)" } ?? "project-image-automatic"
        if let path, thumbnails[path] == nil { loadThumbnail(path) }
        return cell
    }

    private func loadThumbnail(_ path: String) {
        guard thumbnailTasks[path] == nil, let client = app.client else { return }
        let id = project.id
        thumbnailTasks[path] = Task { @MainActor [weak self] in
            defer { self?.thumbnailTasks[path] = nil }
            guard let data = try? await client.projectImage(spaceId: id, path: path), !Task.isCancelled, let self,
                  let image = UIImage(data: data)?.preparingThumbnail(of: CGSize(width: 64, height: 64)),
                  let png = image.pngData() else { return nil }
            self.renderedArtwork[path] = png
            self.thumbnails[path] = image.preparingThumbnail(of: CGSize(width: 30, height: 30)) ?? image
            if let row = self.paths.firstIndex(of: path), self.images.indexPathsForVisibleRows?.contains(IndexPath(row: row + 1, section: 0)) == true {
                self.images.reloadRows(at: [IndexPath(row: row + 1, section: 0)], with: .none)
            }
            return png
        }
    }

    func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        selectedPath = indexPath.row == 0 ? nil : paths[indexPath.row - 1]
        browser.isHidden = true
        view.endEditing(true)
        artwork = nil
        refreshSelection()
        images.reloadData()
        loadPreview()
    }

    private func loadPreview() {
        previewTask?.cancel()
        previewGeneration = UUID()
        let generation = previewGeneration
        statusLabel.isHidden = true
        guard let client = app.client else { return }
        let id = project.id
        let path = selectedPath
        if let data = path.flatMap({ renderedArtwork[$0] }) ?? (path == nil ? automaticArtwork : nil) {
            artwork = data
            refreshSelection()
            return
        }
        let pending = path.flatMap { thumbnailTasks[$0] }
        previewTask = Task { @MainActor [weak self] in
            do {
                let data: Data
                if let pending {
                    guard let rendered = await pending.value else {
                        throw CoreError.InvalidArgument(message: "Couldn’t load this image.")
                    }
                    data = rendered
                } else {
                    data = try await client.projectImage(spaceId: id, path: path)
                }
                guard !Task.isCancelled, let self, self.previewGeneration == generation else { return }
                self.artwork = data
                if path == nil { self.automaticArtwork = data }
                self.refreshSelection()
            } catch {
                guard !Task.isCancelled, let self, self.previewGeneration == generation else { return }
                // An automatic icon may legitimately have no repository artwork.
                if path != nil { self.showError("Couldn’t load this image.") }
            }
        }
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        textField.resignFirstResponder()
        return true
    }

    private func showError(_ text: String) {
        statusLabel.text = text
        statusLabel.isHidden = false
    }

    @objc private func cancel() { dismiss(animated: true) }

    @objc private func save() {
        view.endEditing(true)
        let name = (nameField.text ?? "").trimmingCharacters(in: .whitespacesAndNewlines)
        do {
            try app.editProject(project.id, name: name == project.groupName ? nil : name, iconPath: selectedPath, artwork: artwork)
            dismiss(animated: true)
        } catch {
            showError(error.localizedDescription)
        }
    }
}
