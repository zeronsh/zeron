import UIKit

/// New Cloud Project: the GitHub repositories Cloud reaches. Picking one
/// makes it a project that lives on Cloud alone — every session in it runs
/// on its own Cloud machine, cloning the repository.
final class CloudRepoPickerViewController: UITableViewController, UISearchResultsUpdating {
    private let app: AppModel
    var onCreated: ((String) -> Void)?
    private var repos: [GithubRepoInfo] = []
    private var query = ""
    private var creating: String?
    private var token: AnyObject?
    /// The repository read is in flight (`cloud.repos` stays nil on failure).
    private var loading = false

    init(app: AppModel) {
        self.app = app
        super.init(style: .insetGrouped)
        title = "New Cloud Project"
    }

    required init?(coder: NSCoder) { fatalError() }

    @MainActor private var cloud: CloudStore { app.cloud }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        tableView.backgroundColor = .clear
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .cancel, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        let search = UISearchController(searchResultsController: nil)
        search.searchResultsUpdater = self
        search.obscuresBackgroundDuringPresentation = false
        search.searchBar.placeholder = "Search repositories"
        navigationItem.searchController = search
        navigationItem.hidesSearchBarWhenScrolling = false
        tableView.register(UITableViewCell.self, forCellReuseIdentifier: "repo")
        token = cloud.observe { [weak self] in self?.reloadRepos() }
        load()
    }

    private func load() {
        loading = true
        reloadRepos()
        Task { [weak self] in
            await self?.cloud.refreshGitHub()
            self?.loading = false
            self?.reloadRepos()
        }
    }

    private func reloadRepos() {
        let all = (cloud.repos ?? []).sorted { $0.pushedAtMs > $1.pushedAtMs }
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        repos = q.isEmpty ? all : all.filter { $0.fullName.lowercased().contains(q) }
        tableView.reloadData()
        tableView.backgroundView = repos.isEmpty ? emptyView() : nil
    }

    private func emptyView() -> UIView {
        if cloud.repos == nil {
            // Still reading, or the read failed: say which, never a blank page.
            if loading {
                let spinner = UIActivityIndicatorView(style: .medium)
                spinner.startAnimating()
                return spinner
            }
            let stack = UIStackView(arrangedSubviews: [
                CloudUI.header("Couldn't load repositories", "Check your connection and try again.", alignment: .center),
                CloudUI.secondaryButton("Try again", symbol: "arrow.clockwise") { [weak self] in self?.load() },
            ])
            stack.axis = .vertical
            stack.alignment = .center
            stack.spacing = 16
            let holder = UIView()
            stack.translatesAutoresizingMaskIntoConstraints = false
            holder.addSubview(stack)
            NSLayoutConstraint.activate([
                stack.centerYAnchor.constraint(equalTo: holder.centerYAnchor, constant: -40),
                stack.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 32),
                stack.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: -32),
            ])
            return holder
        }
        let connected = cloud.githubConnected
        let stack = UIStackView(arrangedSubviews: [
            CloudUI.tile(image: CloudUI.githubMark, side: 52),
            CloudUI.header(connected ? "No repositories" : "Connect GitHub", connected ? "Cloud reaches the repositories the Zeron GitHub App is installed on." : "Cloud projects are GitHub repositories. Connect GitHub to pick one.", alignment: .center),
        ])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 16
        if !connected {
            stack.addArrangedSubview(CloudUI.primaryButton("Connect GitHub") { [weak self] in
                guard let self else { return }
                self.present(GitHubConnectViewController.sheet(app: self.app), animated: true)
            })
        }
        let holder = UIView()
        stack.translatesAutoresizingMaskIntoConstraints = false
        holder.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.centerYAnchor.constraint(equalTo: holder.centerYAnchor, constant: -40),
            stack.leadingAnchor.constraint(equalTo: holder.leadingAnchor, constant: 32),
            stack.trailingAnchor.constraint(equalTo: holder.trailingAnchor, constant: -32),
        ])
        return holder
    }

    func updateSearchResults(for searchController: UISearchController) {
        query = searchController.searchBar.text ?? ""
        reloadRepos()
    }

    override func tableView(_ tableView: UITableView, numberOfRowsInSection section: Int) -> Int { repos.count }

    override func tableView(_ tableView: UITableView, titleForFooterInSection section: Int) -> String? {
        repos.isEmpty ? nil : "Each session in a Cloud project runs on its own Cloud machine."
    }

    override func tableView(_ tableView: UITableView, cellForRowAt indexPath: IndexPath) -> UITableViewCell {
        let repo = repos[indexPath.row]
        let cell = tableView.dequeueReusableCell(withIdentifier: "repo", for: indexPath)
        var c = UIListContentConfiguration.subtitleCell()
        c.text = repo.fullName
        c.textProperties.font = Fonts.ui(.sansMedium, 16)
        c.secondaryText = [repo.private ? "Private" : nil, repo.description].compactMap { $0 }.joined(separator: " · ").nonEmpty
        c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
        c.secondaryTextProperties.color = Palette.secondary
        c.secondaryTextProperties.numberOfLines = 1
        c.image = CloudUI.githubMark
        c.imageProperties.tintColor = Palette.text
        c.imageProperties.maximumSize = CGSize(width: 20, height: 20)
        cell.contentConfiguration = c
        cell.backgroundColor = Palette.elevated
        if creating == repo.fullName {
            let spinner = UIActivityIndicatorView(style: .medium)
            spinner.startAnimating()
            cell.accessoryView = spinner
        } else {
            cell.accessoryView = nil
        }
        cell.accessibilityIdentifier = "repo-\(repo.fullName)"
        return cell
    }

    override func tableView(_ tableView: UITableView, didSelectRowAt indexPath: IndexPath) {
        tableView.deselectRow(at: indexPath, animated: true)
        guard creating == nil else { return }
        let repo = repos[indexPath.row]
        creating = repo.fullName
        tableView.reloadRows(at: [indexPath], with: .none)
        Task { @MainActor in
            do {
                let id = try await self.cloud.createProject(repo: repo.fullName)
                UINotificationFeedbackGenerator().notificationOccurred(.success)
                let done = self.onCreated
                self.dismiss(animated: true) { done?(id) }
            } catch {
                self.creating = nil
                self.tableView.reloadData()
                let a = UIAlertController(title: "Couldn't add the project", message: CloudStore.message(error), preferredStyle: .alert)
                a.addAction(UIAlertAction(title: "OK", style: .default))
                self.present(a, animated: true)
            }
        }
    }
}
