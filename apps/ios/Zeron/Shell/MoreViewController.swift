import UIKit
import UserNotifications

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
            case toggle(Bool)
        }
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Settings"
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

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { [weak self] cell, _, row in
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
            case let .toggle(on):
                let toggle = UISwitch()
                toggle.isOn = on
                toggle.onTintColor = Palette.accent
                toggle.accessibilityIdentifier = "toggle-\(row.id)"
                toggle.addAction(UIAction { [weak self, weak toggle] _ in
                    self?.toggled(row.id, toggle?.isOn ?? false)
                }, for: .valueChanged)
                cell.accessories = [.customView(configuration: .init(customView: toggle, placement: .trailing()))]
            }
            cell.accessibilityIdentifier = "settings-\(row.id)"
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
        wallpaperObserver = NotificationCenter.default.addObserver(forName: WallpaperStore.didChange, object: nil, queue: .main) { [weak self] _ in self?.reload() }
        // Devices go on/offline and the org name backfills after opening.
        appToken = app.observe { [weak self] in self?.reload() }
        // Back from iOS Settings (notifications may have been allowed there).
        foregroundObserver = NotificationCenter.default.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { [weak self] _ in
            self?.refreshNotificationStatus()
        }
    }

    private var foregroundObserver: NSObjectProtocol?

    override func viewWillAppear(_ animated: Bool) {
        super.viewWillAppear(animated)
        if dataSource != nil { reload() }
        refreshNotificationStatus()
    }

    // MARK: Notifications

    private var notificationStatus: UNAuthorizationStatus = .notDetermined

    private func refreshNotificationStatus() {
        Task { @MainActor in
            let status = await PushNotifications.shared.authorizationStatus()
            guard status != self.notificationStatus else { return }
            self.notificationStatus = status
            self.reload()
        }
    }

    /// On when this phone wants them and iOS allows them.
    private var notificationsOn: Bool {
        PushNotifications.shared.enabled && (notificationStatus == .authorized || notificationStatus == .provisional || notificationStatus == .ephemeral)
    }

    private func notificationRows() -> [Row] {
        var rows = [Row(id: "notify:enabled", title: "Notifications", subtitle: notificationStatus == .denied ? "Turned off in iOS Settings" : "When a session finishes, needs you or fails", symbol: "bell", accessory: .toggle(notificationsOn))]
        if notificationsOn {
            rows += PushNotifications.Kind.allCases.map { kind in
                let symbol: String = switch kind {
                case .done: "checkmark.circle"
                case .input: "questionmark.bubble"
                case .failed: "exclamationmark.triangle"
                }
                return Row(id: "notify:\(kind.rawValue)", title: kind.title, symbol: symbol, accessory: .toggle(PushNotifications.shared.isOn(kind)))
            }
        }
        return rows
    }

    private func toggled(_ id: String, _ on: Bool) {
        let push = PushNotifications.shared
        if id == "notify:enabled" {
            guard on else {
                push.enabled = false
                return reload()
            }
            if notificationStatus == .denied {
                // Only the Settings app can turn them back on.
                let alert = UIAlertController(title: "Notifications are off", message: "Allow notifications for Zeron in iOS Settings.", preferredStyle: .alert)
                alert.addAction(UIAlertAction(title: "Not Now", style: .cancel) { [weak self] _ in self?.reload() })
                alert.addAction(UIAlertAction(title: "Open Settings", style: .default) { [weak self] _ in
                    if let url = URL(string: UIApplication.openSettingsURLString) { UIApplication.shared.open(url) }
                    self?.reload()
                })
                return present(alert, animated: true)
            }
            push.enabled = true
            Task { @MainActor in
                await push.requestPermission()
                self.notificationStatus = await push.authorizationStatus()
                self.reload()
            }
            return
        }
        if let kind = PushNotifications.Kind(rawValue: String(id.dropFirst("notify:".count))) {
            push.set(kind, on)
        }
    }

    private var wallpaperObserver: NSObjectProtocol?
    private var appToken: AnyObject?

    private func reload() {
        var s = NSDiffableDataSourceSnapshot<String, Row>()
        s.appendSections(["Account"])
        s.appendItems([
            Row(id: "account", title: app.accountName, subtitle: app.accountDetail, symbol: "person.crop.circle", accessory: .none),
        ])
        s.appendSections(["Devices"])
        s.appendItems(app.hostOptions.map { Row(id: "device:\($0.id)", title: $0.name, subtitle: $0.online ? "Online" : "Offline", symbol: "desktopcomputer", accessory: .dot($0.online)) })
        if app.voice.available {
            s.appendSections(["Voice"])
            s.appendItems([Row(id: "voice", title: "Voice", subtitle: app.voice.selectedStyle.map { "Codex voice · \($0.capitalized)" } ?? "Codex voice device and style", symbol: "waveform")])
        }
        s.appendSections(["Notifications"])
        s.appendItems(notificationRows())
        s.appendSections(["Appearance"])
        let style = UserDefaults.standard.integer(forKey: "appearance")
        s.appendItems([
            Row(id: "appearance:0", title: "System", symbol: "circle.lefthalf.filled", accessory: .check(style == 0)),
            Row(id: "appearance:1", title: "Light", symbol: "sun.max", accessory: .check(style == 1)),
            Row(id: "appearance:2", title: "Dark", symbol: "moon", accessory: .check(style == 2)),
        ])
        s.appendSections(["Wallpaper"])
        var wall = [Row(id: "wallpaper:choose", title: WallpaperStore.isSet ? "Change Wallpaper…" : "Choose Wallpaper…", subtitle: WallpaperStore.isSet ? WallpaperStore.name : "Shown behind new chats and the sessions list", symbol: "photo")]
        if WallpaperStore.isSet {
            let effect = WallpaperStore.effect
            wall.append(Row(id: "wallpaper:effect", title: "Effect", subtitle: "\(WallpaperStore.label(effect)) — \(WallpaperStore.detail(effect))", symbol: "wand.and.stars"))
            wall.append(Row(id: "wallpaper:remove", title: "Remove Wallpaper", symbol: "trash", accessory: .none, destructive: true))
        }
        s.appendItems(wall)
        s.appendSections(["Sessions"])
        s.appendItems([Row(id: "archived", title: "Archived Sessions", symbol: "archivebox")])
        s.appendSections([" "])
        s.appendItems([Row(id: "signout", title: "Sign Out", symbol: "rectangle.portrait.and.arrow.right", accessory: .none, destructive: true)])
        dataSource.apply(s, animatingDifferences: false)
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let row = dataSource.itemIdentifier(for: path) else { return }
        switch row.id {
        case "voice": navigationController?.pushViewController(VoiceViewController(app: app), animated: true)
        case let id where id.hasPrefix("appearance:"):
            let style = Int(id.dropFirst(11)) ?? 0
            UserDefaults.standard.set(style, forKey: "appearance")
            view.window?.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: style) ?? .unspecified
            reload()
        case "wallpaper:choose":
            WallpaperPicker.present(from: self)
        case "wallpaper:effect":
            let sheet = UIAlertController(title: "Wallpaper Effect", message: nil, preferredStyle: .actionSheet)
            for e in WallpaperStore.allEffects {
                let action = UIAlertAction(title: WallpaperStore.label(e), style: .default) { _ in WallpaperStore.effect = e }
                action.setValue(e == WallpaperStore.effect, forKey: "checked")
                sheet.addAction(action)
            }
            sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            sheet.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(sheet, animated: true)
        case "wallpaper:remove":
            WallpaperStore.remove()
        case "archived":
            navigationController?.pushViewController(FolderViewController(app: app, folder: FolderRowVM(id: "archived", name: "Archived", count: 0, symbol: "archivebox")), animated: true)
        case "signout":
            let alert = UIAlertController(title: "Sign out?", message: "Local drafts stay on this device.", preferredStyle: .actionSheet)
            alert.addAction(UIAlertAction(title: "Sign Out", style: .destructive) { [weak self] _ in self?.app.signOut() })
            alert.addAction(UIAlertAction(title: "Cancel", style: .cancel))
            alert.popoverPresentationController?.sourceView = collectionView.cellForItem(at: path)
            present(alert, animated: true)
        default:
            break
        }
    }
}
