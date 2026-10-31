import SafariServices
import UIKit

/// Settings → Cloud, as on the desktop: GitHub and the repositories Cloud
/// sessions can open. Agents' accounts are under Providers; plans and credits
/// under Billing.
final class CloudSettingsViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<String, Row>!
    private var token: AnyObject?

    struct Row: Hashable {
        let id: String
        let title: String
        var subtitle: String?
        var image: UIImage?
        var tint: UIColor = Palette.text
        var accessory: String? = nil
        var disclosure = true
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Cloud"
    }

    required init?(coder: NSCoder) { fatalError() }

    @MainActor private var cloud: CloudStore { app.cloud }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        config.headerMode = .supplementary
        config.footerMode = .supplementary
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        collectionView.refreshControl = UIRefreshControl(frame: .zero, primaryAction: UIAction { [weak self] _ in
            self?.cloud.refresh()
        })
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { cell, _, row in
            var c = UIListContentConfiguration.subtitleCell()
            c.text = row.title
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.textProperties.color = Palette.text
            c.secondaryText = row.subtitle
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.secondaryTextProperties.color = Palette.secondary
            c.image = row.image
            c.imageProperties.tintColor = row.tint
            c.imageProperties.maximumSize = CGSize(width: 22, height: 22)
            c.imageProperties.reservedLayoutSize = CGSize(width: 28, height: 28)
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
            if let accessory = row.accessory {
                let label = UILabel()
                label.text = accessory
                label.font = Fonts.ui(.sansMedium, 14)
                label.textColor = Palette.accent
                cell.accessories = [.customView(configuration: .init(customView: label, placement: .trailing()))] + (row.disclosure ? [.disclosureIndicator()] : [])
            } else {
                cell.accessories = row.disclosure ? [.disclosureIndicator()] : []
            }
            cell.accessibilityIdentifier = "cloud-\(row.id)"
        }
        let headerReg = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            var c = UIListContentConfiguration.groupedHeader()
            let id = self?.dataSource.snapshot().sectionIdentifiers[path.section] ?? ""
            c.text = id == "status" ? nil : id
            view.contentConfiguration = c
        }
        let footerReg = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionFooter) { [weak self] view, _, path in
            var c = UIListContentConfiguration.groupedFooter()
            switch self?.dataSource.snapshot().sectionIdentifiers[path.section] {
            case "GitHub": c.text = "Cloud agents clone and push to the repositories the Zeron GitHub App is installed on."
            default: c.text = nil
            }
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, row in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: row)
        }
        dataSource.supplementaryViewProvider = { cv, kind, path in
            kind == UICollectionView.elementKindSectionHeader
                ? cv.dequeueConfiguredReusableSupplementary(using: headerReg, for: path)
                : cv.dequeueConfiguredReusableSupplementary(using: footerReg, for: path)
        }
        token = cloud.observe { [weak self] in self?.reload() }
        reload()
        cloud.refresh()
    }

    private func reload() {
        if !cloud.loading { collectionView.refreshControl?.endRefreshing() }
        var s = NSDiffableDataSourceSnapshot<String, Row>()
        guard cloud.info == nil || cloud.available else {
            s.appendSections(["status"])
            s.appendItems([Row(id: "unavailable", title: "Cloud isn't available", subtitle: cloud.error ?? "This account can't use Cloud yet.", image: CloudUI.symbol("cloud"), tint: Palette.secondary, accessory: nil, disclosure: false)])
            dataSource.apply(s, animatingDifferences: false)
            return
        }
        if let problem = cloud.problem {
            s.appendSections(["status"])
            s.appendItems([Row(id: problem.billing ? "status:billing" : "status", title: problem.title, subtitle: problem.detail, image: CloudUI.symbol("exclamationmark.triangle"), tint: Palette.warning, accessory: problem.billing ? "Add credits" : nil, disclosure: problem.billing)])
        }
        s.appendSections(["GitHub"])
        s.appendItems([Row(
            id: "github",
            title: cloud.githubAccount.map { "@\($0)" } ?? "GitHub",
            subtitle: cloud.githubConnected ? "Connected" : "Connect so Cloud agents can clone and push",
            image: CloudUI.githubMark,
            accessory: cloud.githubConnected ? nil : "Connect"
        )])
        if cloud.githubConnected {
            s.appendSections(["Repositories"])
            let repos = (cloud.repos ?? []).sorted { $0.pushedAtMs > $1.pushedAtMs }
            var rows = repos.prefix(8).map { repo in
                Row(id: "repo:\(repo.fullName)", title: repo.fullName, subtitle: repo.private ? "Private" : nil, image: CloudUI.symbol(repo.private ? "lock" : "book.closed", 15), tint: Palette.secondary, accessory: nil, disclosure: false)
            }
            if repos.count > 8 {
                rows.append(Row(id: "repos:more", title: "\(repos.count - 8) more", image: CloudUI.symbol("ellipsis", 15), tint: Palette.secondary, accessory: nil, disclosure: false))
            }
            let summary = switch cloud.repos?.count {
            case nil: "Checking which repositories Cloud can open…"
            case 0?: "None yet: choose the repositories Cloud sessions can open."
            case 1?: "Cloud sessions can open 1 repository."
            case let n?: "Cloud sessions can open \(n) repositories."
            }
            rows.append(Row(id: "repos:choose", title: "Choose on GitHub", subtitle: summary, image: CloudUI.symbol("arrow.up.right.square"), tint: Palette.text))
            s.appendItems(rows)
        }
        dataSource.apply(s, animatingDifferences: false)
    }

    func collectionView(_ cv: UICollectionView, didSelectItemAt path: IndexPath) {
        cv.deselectItem(at: path, animated: true)
        guard let row = dataSource.itemIdentifier(for: path) else { return }
        switch row.id {
        case "github":
            if cloud.githubConnected {
                githubActions(from: cv.cellForItem(at: path))
            } else {
                present(GitHubConnectViewController.sheet(app: app), animated: true)
            }
        case "status:billing":
            navigationController?.pushViewController(BillingViewController(app: app), animated: true)
        case "repos:choose":
            if let url = cloud.vault?.githubInstallUrl.flatMap(URL.init(string:)) {
                present(SFSafariViewController(url: url), animated: true)
            }
        default:
            break
        }
    }

    private func githubActions(from source: UIView?) {
        let sheet = UIAlertController(title: cloud.githubAccount.map { "@\($0)" }, message: "Cloud reaches the repositories the Zeron GitHub App is installed on.", preferredStyle: .actionSheet)
        if let url = cloud.vault?.githubInstallUrl.flatMap(URL.init(string:)) {
            sheet.addAction(UIAlertAction(title: "Choose Repositories on GitHub", style: .default) { [weak self] _ in
                self?.present(SFSafariViewController(url: url), animated: true)
            })
        }
        sheet.addAction(UIAlertAction(title: "Reconnect", style: .default) { [weak self] _ in
            guard let self else { return }
            self.present(GitHubConnectViewController.sheet(app: self.app), animated: true)
        })
        sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
        sheet.popoverPresentationController?.sourceView = source
        present(sheet, animated: true)
    }
}
