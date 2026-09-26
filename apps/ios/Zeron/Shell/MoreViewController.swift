import UIKit

/// Settings & utilities: account, devices, appearance, archive, diagnostics.
final class MoreViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<String, Row>!

    struct Row: Hashable {
        let id: String
        let title: String
        var subtitle: String?
        var symbol: String
        var tint: UIColor = Palette.secondary
        var accessory: Accessory = .disclosure
        var destructive = false

        enum Accessory: Hashable {
            case disclosure
            case none
            case check(Bool)
            case dot(Bool)
        }
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "More"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .always
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        config.headerMode = .supplementary
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { cell, _, row in
            var c = UIListContentConfiguration.subtitleCell()
            c.text = row.title
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.textProperties.color = row.destructive ? Palette.danger : Palette.text
            c.secondaryText = row.subtitle
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.secondaryTextProperties.color = Palette.secondary
            c.image = UIImage(systemName: row.symbol)
            c.imageProperties.tintColor = row.destructive ? Palette.danger : row.tint
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
            switch row.accessory {
            case .disclosure: cell.accessories = [.disclosureIndicator()]
            case .none: cell.accessories = []
            case let .check(on): cell.accessories = on ? [.checkmark(options: .init(tintColor: Palette.accent))] : []
            case let .dot(online):
                let dot = UIView(frame: CGRect(x: 0, y: 0, width: 8, height: 8))
                dot.backgroundColor = online ? Palette.success : Palette.tertiary
                dot.layer.cornerRadius = 4
                cell.accessories = [.customView(configuration: .init(customView: dot, placement: .trailing()))]
            }
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, path in
            var c = UIListContentConfiguration.groupedHeader()
            c.text = self?.dataSource.snapshot().sectionIdentifiers[path.section]
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, row in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: row)
        }
        dataSource.supplementaryViewProvider = { cv, _, path in
            cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
        }
        reload()
    }

    private func reload() {
        var s = NSDiffableDataSourceSnapshot<String, Row>()
        s.appendSections(["Account"])
        s.appendItems([
            Row(id: "account", title: app.accountName, subtitle: app.accountDetail, symbol: "person.crop.circle", accessory: .none),
            Row(id: "org", title: "Switch Organization", symbol: "building.2"),
        ])
        s.appendSections(["Devices"])
        s.appendItems(app.hostOptions.map { Row(id: "device:\($0.id)", title: $0.name, subtitle: $0.online ? "Online" : "Offline", symbol: "desktopcomputer", accessory: .dot($0.online)) })
        s.appendSections(["Appearance"])
        let style = UserDefaults.standard.integer(forKey: "appearance")
        s.appendItems([
            Row(id: "appearance:0", title: "System", symbol: "circle.lefthalf.filled", accessory: .check(style == 0)),
            Row(id: "appearance:1", title: "Light", symbol: "sun.max", accessory: .check(style == 1)),
            Row(id: "appearance:2", title: "Dark", symbol: "moon", accessory: .check(style == 2)),
        ])
        s.appendSections(["Sessions"])
        s.appendItems([Row(id: "archived", title: "Archived Sessions", symbol: "archivebox")])
        s.appendSections(["Diagnostics"])
        s.appendItems([
            Row(id: "lab", title: "Transcript Lab", subtitle: "Layout engine + painter over fixtures", symbol: "text.viewfinder"),
            Row(id: "version", title: "Zeron core \(coreVersion())", subtitle: "Rust mobile core linked via UniFFI", symbol: "cpu", accessory: .none),
        ])
        s.appendSections([" "])
        s.appendItems([Row(id: "signout", title: "Sign Out", symbol: "rectangle.portrait.and.arrow.right", accessory: .none, destructive: true)])
        dataSource.apply(s, animatingDifferences: false)
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let row = dataSource.itemIdentifier(for: path) else { return }
        switch row.id {
        case let id where id.hasPrefix("appearance:"):
            let style = Int(id.dropFirst(11)) ?? 0
            UserDefaults.standard.set(style, forKey: "appearance")
            view.window?.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: style) ?? .unspecified
            reload()
        case "archived":
            navigationController?.pushViewController(FolderViewController(app: app, folder: FolderRowVM(id: "archived", name: "Archived", count: 0, symbol: "archivebox")), animated: true)
        case "lab":
            navigationController?.pushViewController(TranscriptLabViewController(), animated: true)
        case "signout":
            let alert = UIAlertController(title: "Sign out?", message: "Local drafts stay on this device.", preferredStyle: .actionSheet)
            alert.addAction(UIAlertAction(title: "Sign Out", style: .destructive) { [weak self] _ in self?.app.signOut() })
            alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            present(alert, animated: true)
        default:
            break
        }
    }
}
