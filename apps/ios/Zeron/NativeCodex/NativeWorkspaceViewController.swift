import UIKit
import UniformTypeIdentifiers

final class NativeWorkspaceViewController: UITableViewController, UISearchResultsUpdating {
    private let shell: MobileShellRuntime
    private let prefix: String
    private var entries: [NativeWorkspaceEntry] = []
    private var visible: [NativeWorkspaceEntry] = []
    private let search = UISearchController(searchResultsController: nil)
    private let exporter = NativeWorkspaceExporter()

    init(shell: MobileShellRuntime, prefix: String = "/workspace/") {
        self.shell = shell; self.prefix = prefix
        super.init(style: .insetGrouped)
    }
    required init?(coder: NSCoder) { fatalError() }
    override func viewDidLoad() {
        super.viewDidLoad()
        title = prefix == "/workspace/" ? "Workspace" : (String(prefix.dropLast()) as NSString).lastPathComponent
        tableView.accessibilityIdentifier = "native-workspace-files"
        tableView.rowHeight = 72
        search.searchResultsUpdater = self
        search.searchBar.placeholder = "Search this folder"
        search.obscuresBackgroundDuringPresentation = false
        navigationItem.searchController = search
        definesPresentationContext = true
        let save = UIBarButtonItem(image: UIImage(systemName: "square.and.arrow.down"), primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            self.exporter.save(shell: self.shell, path: self.prefix == "/workspace/" ? nil : String(self.prefix.dropLast()), from: self)
        })
        save.accessibilityLabel = "Save folder to Files"
        save.accessibilityIdentifier = "workspace-save-folder"
        navigationItem.rightBarButtonItems = [UIBarButtonItem(systemItem: .add, primaryAction: UIAction { [weak self] _ in self?.newFile() }), save]
        refreshControl = UIRefreshControl()
        refreshControl?.addTarget(self, action: #selector(reloadFiles), for: .valueChanged)
    }
    override func viewWillAppear(_ animated: Bool) { super.viewWillAppear(animated); reloadFiles() }
    @objc private func reloadFiles() {
        Task {
            defer { refreshControl?.endRefreshing() }
            do {
                entries = try await shell.entries().filter { $0.path.hasPrefix(prefix) && !$0.path.dropFirst(prefix.count).contains("/") }
                    .sorted { lhs, rhs in
                        if lhs.type != rhs.type { return lhs.type == "directory" }
                        return lhs.path.localizedStandardCompare(rhs.path) == .orderedAscending
                    }
                updateSearchResults(for: search)
            } catch { workspaceError(error) }
        }
    }
    func updateSearchResults(for searchController: UISearchController) {
        let query = searchController.searchBar.text ?? ""
        visible = entries.filter { query.isEmpty || ($0.path as NSString).lastPathComponent.localizedCaseInsensitiveContains(query) }
        tableView.reloadData()
        var empty = UIContentUnavailableConfiguration.empty()
        empty.image = UIImage(systemName: query.isEmpty ? "folder" : "magnifyingglass")
        empty.text = query.isEmpty ? "Your workspace is empty" : "No matching files"
        empty.secondaryText = query.isEmpty ? "Create a file with +, or import a project from the chat." : "Try a different name."
        contentUnavailableConfiguration = visible.isEmpty ? empty : nil
    }
    override func tableView(_ tableView: UITableView, titleForHeaderInSection section: Int) -> String? { String(prefix.dropLast()) }
    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        let files = entries.filter { $0.type == "file" }.count
        let folders = entries.filter { $0.type == "directory" }.count
        return "\(files) \(files == 1 ? "file" : "files") · \(folders) \(folders == 1 ? "folder" : "folders")\nSaved on this iPhone. Hold an item to save a copy to Files."
    }
    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { visible.count }
    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let entry = visible[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "file") ?? UITableViewCell(style: .subtitle, reuseIdentifier: "file")
        var content = cell.defaultContentConfiguration()
        content.text = (entry.path as NSString).lastPathComponent
        content.textProperties.font = .preferredFont(forTextStyle: .body)
        content.secondaryText = entry.type == "directory" ? "Folder" : NativeWorkspacePresentation.detail(entry)
        content.secondaryTextProperties.color = .secondaryLabel
        content.image = UIImage(systemName: NativeWorkspacePresentation.icon(entry))
        content.imageProperties.tintColor = entry.type == "directory" ? .systemBlue : .secondaryLabel
        content.imageProperties.maximumSize = CGSize(width: 28, height: 32)
        cell.contentConfiguration = content
        cell.accessoryType = .disclosureIndicator
        return cell
    }
    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        let entry = visible[indexPath.row]
        let controller = entry.type == "directory" ? NativeWorkspaceViewController(shell: shell, prefix: entry.path + "/") as UIViewController : NativeWorkspacePreviewController(path: entry.path, shell: shell)
        navigationController?.pushViewController(controller, animated: true)
    }
    override func tableView(_ tableView: UITableView, contextMenuConfigurationForRowAt indexPath: IndexPath, point: CGPoint) -> UIContextMenuConfiguration? {
        let entry = visible[indexPath.row]
        return UIContextMenuConfiguration(identifier: nil, previewProvider: nil) { [weak self] _ in
            UIMenu(children: [UIAction(title: "Save to Files", image: UIImage(systemName: "square.and.arrow.down")) { _ in
                guard let self else { return }
                self.exporter.save(shell: self.shell, path: entry.path, from: self)
            }])
        }
    }
    private func newFile() {
        let alert = UIAlertController(title: "New file", message: "Name the file in this folder.", preferredStyle: .alert)
        alert.addTextField { $0.placeholder = "README.md"; $0.autocapitalizationType = .none }
        alert.addAction(UIAlertAction(title: "Create", style: .default) { [weak self, weak alert] _ in
            guard let self, let name = alert?.textFields?.first?.text, !name.isEmpty, !name.contains("/") else { return }
            Task {
                do {
                    let path = self.prefix + name
                    try await self.shell.importEntries([.init(path: path, type: "file", mode: 420, content: "")])
                    self.navigationController?.pushViewController(NativeCodexFileViewController(path: path, text: "", shell: self.shell), animated: true)
                } catch { self.workspaceError(error) }
            }
        })
        alert.addAction(UIAlertAction(title: "Cancel", style: .cancel)); present(alert, animated: true)
    }
}

enum NativeWorkspacePresentation {
    static func type(_ entry: NativeWorkspaceEntry) -> UTType? { UTType(filenameExtension: (entry.path as NSString).pathExtension) }
    static func detail(_ entry: NativeWorkspaceEntry) -> String {
        let bytes = entry.size ?? Data(base64Encoded: entry.content ?? "")?.count ?? 0
        return "\(type(entry)?.localizedDescription ?? "File") · \(ByteCountFormatter.string(fromByteCount: Int64(bytes), countStyle: .file))"
    }
    static func icon(_ entry: NativeWorkspaceEntry) -> String {
        if entry.type == "directory" { return "folder.fill" }
        let type = type(entry)
        if type?.conforms(to: .image) == true { return "photo" }
        if type?.conforms(to: .pdf) == true { return "doc.richtext" }
        if type?.conforms(to: .audio) == true { return "waveform" }
        if type?.conforms(to: .movie) == true { return "film" }
        if ["js", "ts", "tsx", "jsx", "json", "html", "css", "swift", "rs", "py"].contains((entry.path as NSString).pathExtension.lowercased()) { return "chevron.left.forwardslash.chevron.right" }
        return "doc.text"
    }
}

extension UIViewController {
    func workspaceError(_ error: Error) {
        let alert = UIAlertController(title: "Workspace", message: error.localizedDescription, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "OK", style: .default)); present(alert, animated: true)
    }
}
