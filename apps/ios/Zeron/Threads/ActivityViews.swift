import UIKit

/// Sleep → Custom…: the system date picker (calendar and time), opening on
/// tomorrow at 9, never earlier than a minute from now. Cancel, or Sleep.
final class SleepPickerViewController: UIViewController {
    private let chatTitle: String?
    private let onSleep: (Date) -> Void
    private let picker = UIDatePicker()
    private let summary = UILabel()

    init(title chatTitle: String?, onSleep: @escaping (Date) -> Void) {
        self.chatTitle = chatTitle
        self.onSleep = onSleep
        super.init(nibName: nil, bundle: nil)
        title = "Sleep Until"
        modalPresentationStyle = .formSheet
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = .systemGroupedBackground
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .cancel, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        let sleep = UIBarButtonItem(title: "Sleep", primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            let until = max(self.picker.date, Date().addingTimeInterval(60))
            self.dismiss(animated: true)
            self.onSleep(until)
        })
        sleep.style = .done
        sleep.accessibilityIdentifier = "sleep-confirm"
        navigationItem.rightBarButtonItem = sleep

        let now = Date()
        picker.datePickerMode = .dateAndTime
        picker.preferredDatePickerStyle = .inline
        picker.minimumDate = now.addingTimeInterval(60)
        picker.date = ActivityList.customStart(now: now)
        picker.minuteInterval = 5
        picker.accessibilityIdentifier = "sleep-picker"
        picker.addAction(UIAction { [weak self] _ in self?.updateSummary() }, for: .valueChanged)

        summary.font = .preferredFont(forTextStyle: .footnote)
        summary.textColor = .secondaryLabel
        summary.numberOfLines = 0
        summary.adjustsFontForContentSizeCategory = true

        let card = UIView()
        card.backgroundColor = .secondarySystemGroupedBackground
        card.layer.cornerRadius = 26
        card.layer.cornerCurve = .continuous
        picker.translatesAutoresizingMaskIntoConstraints = false
        card.addSubview(picker)
        let stack = UIStackView(arrangedSubviews: [card, summary])
        stack.axis = .vertical
        stack.spacing = 12
        stack.translatesAutoresizingMaskIntoConstraints = false
        let scroll = UIScrollView()
        scroll.alwaysBounceVertical = true
        scroll.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        view.addSubview(scroll)
        NSLayoutConstraint.activate([
            scroll.topAnchor.constraint(equalTo: view.topAnchor),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            scroll.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            scroll.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor, constant: 16),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -24),
            stack.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor, constant: 16),
            stack.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor, constant: -16),
            picker.topAnchor.constraint(equalTo: card.topAnchor, constant: 8),
            picker.bottomAnchor.constraint(equalTo: card.bottomAnchor, constant: -8),
            picker.leadingAnchor.constraint(equalTo: card.leadingAnchor, constant: 8),
            picker.trailingAnchor.constraint(equalTo: card.trailingAnchor, constant: -8),
        ])
        updateSummary()
        if let sheet = sheetPresentationController ?? navigationController?.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
    }

    /// What it'll do, in a sentence under the picker.
    private func updateSummary() {
        let when = picker.date.formatted(.dateTime.weekday(.wide).month(.abbreviated).day().hour().minute())
        let name = chatTitle.map { "“\($0)”" } ?? "This chat"
        summary.text = "\(name) moves to Asleep until \(when), on every device. It wakes early if it asks you something, needs an approval or fails."
    }
}

/// Settings → Activity: the Activity sidebar and how it behaves. The view
/// itself (the Sessions "…" menu turns it on too), whether working chats
/// fold into Working, what Sleep does, and how long a chat may sit idle
/// before it's archived. Everything after the view's own switch applies
/// only while it's on, so it's dimmed and out of reach while it's off. All
/// of it is this phone's.
final class ActivitySettingsViewController: UIViewController, UICollectionViewDelegate {
    private let app: AppModel
    private var collectionView: UICollectionView!
    private var dataSource: UICollectionViewDiffableDataSource<Int, Row>!
    private var token: AnyObject?

    enum Row: Hashable {
        case view(Bool)
        case working(on: Bool, enabled: Bool)
        case sleep(enabled: Bool)
        case archiveAfter(days: Int, enabled: Bool)
    }

    private static let footers = [
        "One list of the chats that need you, the latest at the top. Working, asleep and archived chats fold away at its end. Also in the Sessions “…” menu.",
        "A chat moves into Working while its agent runs, or waits on work that will wake it, and comes back to the top when it’s done or needs you.\n\nSwipe a chat, or touch and hold it, and choose Sleep to put it in Asleep until a time you pick, on every device. It wakes early if it asks you something, needs an approval or fails, and comes back to the top.",
        "Chats with nothing new for this long move to Archived. Pinned, working and asleep chats stay, as do chats waiting on you.",
    ]
    private static let headers: [String?] = [nil, "Activity", "Archiving"]

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
        title = "Activity"
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
        collectionView = UICollectionView(frame: view.bounds, collectionViewLayout: UICollectionViewCompositionalLayout.list(using: config))
        collectionView.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        collectionView.backgroundColor = .clear
        collectionView.delegate = self
        view.addSubview(collectionView)

        let cell = UICollectionView.CellRegistration<UICollectionViewListCell, Row> { [weak self] cell, _, row in
            self?.configure(cell, row)
        }
        let header = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionHeader) { view, _, path in
            var c = UIListContentConfiguration.groupedHeader()
            c.text = Self.headers[path.section]
            view.contentConfiguration = c
        }
        let footer = UICollectionView.SupplementaryRegistration<UICollectionViewListCell>(elementKind: UICollectionView.elementKindSectionFooter) { view, _, path in
            var c = UIListContentConfiguration.groupedFooter()
            c.text = Self.footers[path.section]
            view.contentConfiguration = c
        }
        dataSource = UICollectionViewDiffableDataSource(collectionView: collectionView) { cv, path, row in
            cv.dequeueConfiguredReusableCell(using: cell, for: path, item: row)
        }
        dataSource.supplementaryViewProvider = { cv, kind, path in
            kind == UICollectionView.elementKindSectionHeader
                ? cv.dequeueConfiguredReusableSupplementary(using: header, for: path)
                : cv.dequeueConfiguredReusableSupplementary(using: footer, for: path)
        }
        reload()
        token = app.observe { [weak self] in self?.reload() }
    }

    private func reload() {
        let on = app.activityOn
        var s = NSDiffableDataSourceSnapshot<Int, Row>()
        s.appendSections([0, 1, 2])
        s.appendItems([.view(on)], toSection: 0)
        s.appendItems([.working(on: app.activityWorkingShelf, enabled: on), .sleep(enabled: on)], toSection: 1)
        s.appendItems([.archiveAfter(days: app.activityArchiveAfter, enabled: on)], toSection: 2)
        dataSource.apply(s, animatingDifferences: view.window != nil)
    }

    private func configure(_ cell: UICollectionViewListCell, _ row: Row) {
        var c = UIListContentConfiguration.valueCell()
        c.textProperties.font = Fonts.ui(.sansMedium, 16)
        c.textProperties.color = Palette.text
        c.imageProperties.tintColor = Palette.secondary
        var bg = UIBackgroundConfiguration.listGroupedCell()
        bg.backgroundColor = Palette.elevated
        cell.backgroundConfiguration = bg
        cell.accessories = []
        var enabled = true
        switch row {
        case let .view(on):
            c.text = "Activity Sidebar"
            c.image = UIImage(systemName: "tray.full")
            cell.accessories = [toggle("view", on: on, enabled: true) { [weak self] in self?.app.activityOn = $0 }]
        case let .working(on, isEnabled):
            enabled = isEnabled
            c.text = "Fold Working Chats into Working"
            c.textProperties.numberOfLines = 0
            c.image = UIImage(systemName: "circle.dashed")
            cell.accessories = [toggle("working", on: on, enabled: isEnabled) { [weak self] in self?.app.activityWorkingShelf = $0 }]
        case let .sleep(isEnabled):
            enabled = isEnabled
            c.text = "Sleep"
            c.secondaryText = "Swipe or touch and hold a chat"
            c.secondaryTextProperties.font = Fonts.ui(.sans, 13)
            c.image = UIImage(systemName: "moon")
        case let .archiveAfter(days, isEnabled):
            enabled = isEnabled
            c.text = "Archive Idle Chats"
            c.image = UIImage(systemName: "archivebox")
            cell.accessories = [.customView(configuration: .init(customView: archiveButton(days: days, enabled: isEnabled), placement: .trailing()))]
        }
        if !enabled {
            c.textProperties.color = Palette.tertiary
            c.imageProperties.tintColor = Palette.tertiary
            c.secondaryTextProperties.color = Palette.tertiary
        }
        cell.contentConfiguration = c
        cell.accessibilityIdentifier = "activity-setting-\(Self.id(row))"
    }

    private static func id(_ row: Row) -> String {
        switch row {
        case .view: "view"
        case .working: "working"
        case .sleep: "sleep"
        case .archiveAfter: "archive-after"
        }
    }

    private func toggle(_ id: String, on: Bool, enabled: Bool, _ changed: @escaping (Bool) -> Void) -> UICellAccessory {
        let toggle = UISwitch()
        toggle.isOn = on
        toggle.isEnabled = enabled
        toggle.onTintColor = Palette.accent
        toggle.accessibilityIdentifier = "toggle-activity-\(id)"
        toggle.addAction(UIAction { [weak toggle] _ in changed(toggle?.isOn ?? false) }, for: .valueChanged)
        return .customView(configuration: .init(customView: toggle, placement: .trailing()))
    }

    /// A pop-up button: Never, After 1 Day, … (the system's menu picker).
    private func archiveButton(days: Int, enabled: Bool) -> UIButton {
        var config = UIButton.Configuration.plain()
        config.indicator = .popup
        config.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 6, bottom: 6, trailing: 0)
        let button = UIButton(configuration: config)
        button.menu = UIMenu(children: ActivityList.archiveAfterChoices.map { choice in
            UIAction(title: ActivityList.archiveAfterLabel(choice), state: choice == days ? .on : .off) { [weak self] _ in
                self?.app.activityArchiveAfter = choice
            }
        })
        button.showsMenuAsPrimaryAction = true
        button.changesSelectionAsPrimaryAction = true
        button.isEnabled = enabled
        button.tintColor = Palette.secondary
        button.accessibilityIdentifier = "activity-archive-after"
        button.sizeToFit()
        return button
    }

    func collectionView(_ collectionView: UICollectionView, shouldHighlightItemAt indexPath: IndexPath) -> Bool { false }
}
