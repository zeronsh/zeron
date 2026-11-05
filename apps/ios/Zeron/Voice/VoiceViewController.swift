import UIKit

/// Voice settings, in the Settings list style: which registered device runs
/// Codex for a call and which voice it speaks with. Calls start from the bar.
final class VoiceViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Section, Row>!
    private var voiceToken: AnyObject?
    private var appToken: AnyObject?
    private var signature = ""

    enum Section: Int, Hashable { case hero, hosts, voice, call }

    struct Row: Hashable {
        let id: String
        let title: String
        var subtitle: String?
        var symbol: String?
        var selected = false
        var enabled = true
        var status: UIColor?
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Voice"
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.largeTitleDisplayMode = .never
        var config = UICollectionLayoutListConfiguration(appearance: .insetGrouped)
        config.backgroundColor = .clear
        config.headerMode = .supplementary
        config.footerMode = .supplementary
        let layout = UICollectionViewCompositionalLayout { index, environment in
            var section = config
            if Section(rawValue: index) == .hero {
                section.headerMode = .none
                section.footerMode = .none
            }
            return .list(using: section, layoutEnvironment: environment)
        }
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: layout)
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)

        let hero = UICollectionView.CellRegistration<VoiceHeroCell, Row> { cell, _, _ in
            cell.backgroundConfiguration = .clear()
        }
        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { [weak self] cell, _, row in
            var c = row.id == "style" ? UIListContentConfiguration.valueCell() : UIListContentConfiguration.subtitleCell()
            c.text = row.title
            c.textProperties.font = Fonts.ui(.sansMedium, 16)
            c.textProperties.color = row.enabled ? Palette.text : Palette.tertiary
            c.secondaryText = row.subtitle
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.secondaryTextProperties.color = Palette.secondary
            if let symbol = row.symbol {
                c.image = UIImage(systemName: symbol)
                c.imageProperties.tintColor = row.enabled ? Palette.secondary : Palette.tertiary
            }
            cell.contentConfiguration = c
            var bg = UIBackgroundConfiguration.listGroupedCell()
            bg.backgroundColor = Palette.elevated
            cell.backgroundConfiguration = bg
            var accessories: [UICellAccessory] = []
            if let status = row.status {
                let dot = UIView(frame: CGRect(x: 0, y: 0, width: 8, height: 8))
                dot.backgroundColor = status
                dot.layer.cornerRadius = 4
                accessories.append(.customView(configuration: .init(customView: dot, placement: .trailing())))
            }
            if row.selected { accessories.append(.checkmark(options: .init(tintColor: Palette.accent))) }
            if row.id == "style", let menu = self?.styleMenu() {
                accessories.append(.popUpMenu(menu, options: .init(tintColor: Palette.secondary)))
            }
            cell.accessories = accessories
            cell.accessibilityIdentifier = "voice-\(row.id)"
            cell.accessibilityTraits = row.selected ? [.button, .selected] : row.enabled ? .button : .notEnabled
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { view, _, path in
            var c = UIListContentConfiguration.groupedHeader()
            c.text = Self.header(Section(rawValue: path.section))
            view.contentConfiguration = c
        }
        let footer = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionFooter) { [weak self] view, _, path in
            var c = UIListContentConfiguration.groupedFooter()
            c.text = self?.footer(Section(rawValue: path.section))
            c.textProperties.font = Fonts.ui(.sans, 13)
            c.textProperties.color = Palette.secondary
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, row in
            if Section(rawValue: path.section) == .hero {
                return cv.dequeueConfiguredReusableCell(using: hero, for: path, item: row)
            }
            return cv.dequeueConfiguredReusableCell(using: cell, for: path, item: row)
        }
        dataSource.supplementaryViewProvider = { cv, kind, path in
            kind == UICollectionView.elementKindSectionHeader
                ? cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
                : cv.dequeueConfiguredReusableSupplementary(using: footer, for: path)
        }
        voiceToken = app.voice.observe { [weak self] in self?.reload() }
        appToken = app.observe { [weak self] in self?.reload() }
        reload()
    }

    private static func header(_ section: Section?) -> String? {
        switch section {
        case .hosts: "Codex runs on"
        case .voice: "Voice"
        case .call: "During a call"
        default: nil
        }
    }

    private func footer(_ section: Section?) -> String? {
        switch section {
        case .hosts: app.voice.live
            ? "End the call to change devices."
            : "Audio stays on this iPhone. Codex, its sign-in and its tools run on the device you choose."
        case .voice: "Applies to your next call."
        default: nil
        }
    }

    /// Phones can't host Codex; only machines that can are listed.
    private var devices: [DeviceView] { (app.client?.devices() ?? []).filter(\.isExecutionHost) }

    private func reload() {
        let voice = app.voice
        let devices = self.devices
        // Level frames during a call must not rebuild the list.
        let signature = "\(voice.live)|\(voice.selectedHost ?? "")|\(voice.selectedStyle ?? "")|\(voice.styles)|"
            + devices.map { "\($0.id):\($0.online):\($0.capabilities)" }.joined()
        guard signature != self.signature else { return }
        self.signature = signature

        var s = NSDiffableDataSourceSnapshot<Section, Row>()
        s.appendSections([.hero, .hosts, .voice, .call])
        s.appendItems([Row(id: "hero", title: "")], toSection: .hero)
        let chosen = voice.startHost?.id
        if devices.isEmpty {
            s.appendItems([Row(id: "no-hosts", title: "No Mac or server yet", subtitle: "Open Zeron on your Mac or server with remote voice enabled.", symbol: "desktopcomputer", enabled: false)], toSection: .hosts)
        } else {
            s.appendItems(devices.map { device in
                let compatible = RemoteVoiceController.compatible(device)
                let ready = device.online && compatible
                return Row(
                    id: "host-\(device.id)",
                    title: device.name,
                    subtitle: !device.online ? "Offline" : !compatible ? "Update Zeron and enable remote voice" : "Ready",
                    symbol: Self.symbol(for: device.platform),
                    selected: device.id == chosen,
                    enabled: ready && !voice.live,
                    status: ready ? Palette.success : Palette.tertiary
                )
            }, toSection: .hosts)
        }
        s.appendItems([Row(id: "style", title: "Voice", symbol: "person.wave.2")], toSection: .voice)
        s.appendItems([
            Row(id: "tip-ear", title: "Hold it to your ear", subtitle: "The screen turns off and Codex plays through the earpiece.", symbol: "ear", enabled: true),
            Row(id: "tip-lock", title: "Lock your iPhone", subtitle: "The call keeps going. An incoming call ends it.", symbol: "lock", enabled: true),
            Row(id: "tip-bar", title: "Start from the bottom bar", subtitle: "Tap the waveform; hold it to switch device or voice.", symbol: "waveform", enabled: true),
        ], toSection: .call)
        dataSource.apply(s, animatingDifferences: view.window != nil)
    }

    private static func symbol(for platform: String) -> String {
        switch platform.lowercased() {
        case let p where p.contains("mac") || p.contains("darwin"): "laptopcomputer"
        case let p where p.contains("win"): "pc"
        default: "server.rack"
        }
    }

    func collectionView(_ collectionView: UICollectionView, shouldHighlightItemAt path: IndexPath) -> Bool {
        guard let row = dataSource.itemIdentifier(for: path) else { return false }
        return (row.id.hasPrefix("host-") && row.enabled) || row.id == "style"
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt path: IndexPath) {
        collectionView.deselectItem(at: path, animated: true)
        guard let row = dataSource.itemIdentifier(for: path) else { return }
        let voice = app.voice
        guard row.id.hasPrefix("host-"), row.enabled else { return }
        voice.selectedHost = String(row.id.dropFirst("host-".count))
        UISelectionFeedbackGenerator().selectionChanged()
    }

    /// Every voice the host offers, the current one checked.
    private func styleMenu() -> UIMenu {
        let voice = app.voice
        let styles: [String?] = [nil] + voice.styles.map(Optional.some)
        return UIMenu(options: .singleSelection, children: styles.map { style in
            UIAction(title: style?.capitalized ?? "Codex default", state: style == voice.selectedStyle ? .on : .off) { _ in
                voice.selectedStyle = style
                UISelectionFeedbackGenerator().selectionChanged()
            }
        })
    }
}

/// The settings hero: the desktop orb at rest over a one-line explanation.
private final class VoiceHeroCell: UICollectionViewCell {
    private let orb = OrbView(preset: .large, orb: .idle)
    private let title = UILabel()
    private let detail = UILabel()

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.text = "Talk with Codex"
        title.font = Fonts.ui(.sansSemibold, 20)
        title.textColor = Palette.text
        title.textAlignment = .center
        detail.text = "A voice orchestrator that starts sessions, checks on them and reports back."
        detail.font = Fonts.ui(.sans, 14)
        detail.textColor = Palette.secondary
        detail.textAlignment = .center
        detail.numberOfLines = 0
        let stack = UIStackView(arrangedSubviews: [orb, title, detail])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 6
        stack.setCustomSpacing(10, after: orb)
        stack.translatesAutoresizingMaskIntoConstraints = false
        contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            orb.widthAnchor.constraint(equalToConstant: 112),
            orb.heightAnchor.constraint(equalToConstant: 112),
            stack.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 4),
            stack.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -8),
            stack.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -24),
            detail.widthAnchor.constraint(lessThanOrEqualToConstant: 320),
        ])
        isAccessibilityElement = true
        accessibilityLabel = "Talk with Codex. " + (detail.text ?? "")
    }

    required init?(coder: NSCoder) { fatalError() }
}
