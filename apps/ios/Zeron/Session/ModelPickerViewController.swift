import UIKit

/// One provider in the model picker: its tab and its models, loaded when the
/// picker opens.
struct ModelPickerTab {
    let harness: String
    let label: String
    /// Over the list: what picking here does to the chat.
    var footer: String? = nil
    let load: () async -> [ModelPickerRow]
}

struct ModelPickerRow: Equatable {
    let id: String
    let label: String
    let detail: String?
}

/// The model picker: a glass sheet sized to its list. A left-aligned title
/// says where the chat runs; with more than one provider a switcher sits over
/// swipeable pages (the current provider first). Models sit right on the
/// glass — no cards, no rules — and the current one is held in a soft accent
/// tint.
final class ModelPickerViewController: UIViewController, UIScrollViewDelegate {
    private let tabs: [ModelPickerTab]
    private let host: (name: String, symbol: String)
    private let onPick: (_ harness: String, _ model: ModelPickerRow) -> Void

    private var selected: Int
    private var pages: [Page] = []
    private let header = UIView()
    private let pager = UIScrollView()
    private var switcher: ProviderSwitcher?
    private weak var sheet: UISheetPresentationController?
    private var picked = false

    /// The picker in its sheet, ready to present.
    static func sheet(tabs: [ModelPickerTab], harness: String, model: String?, host: (name: String, symbol: String), note: String? = nil, onPick: @escaping (String, ModelPickerRow) -> Void) -> UIViewController {
        let picker = ModelPickerViewController(tabs: tabs, harness: harness, model: model, host: host, note: note, onPick: onPick)
        picker.modalPresentationStyle = .pageSheet
        if let sheet = picker.sheetPresentationController {
            sheet.detents = [
                .custom(identifier: .init("fit")) { [weak picker] ctx in
                    picker?.fittedHeight(ctx.maximumDetentValue) ?? ctx.maximumDetentValue * 0.55
                },
                .large(),
            ]
            sheet.prefersGrabberVisible = true
            sheet.prefersScrollingExpandsWhenScrolledToEdge = true
            sheet.preferredCornerRadius = 34
            picker.sheet = sheet
        }
        return picker
    }

    private init(tabs: [ModelPickerTab], harness: String, model: String?, host: (name: String, symbol: String), note: String?, onPick: @escaping (String, ModelPickerRow) -> Void) {
        self.tabs = tabs
        self.host = host
        self.onPick = onPick
        self.selected = tabs.firstIndex { $0.harness == harness } ?? 0
        super.init(nibName: nil, bundle: nil)
        pages = tabs.enumerated().map { ix, tab in
            Page(tab: tab, current: (harness, model), notice: tab.footer ?? (ix == selected ? note : nil))
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        // Clear: the sheet's own glass is the surface.
        view.backgroundColor = .clear
        buildHeader()

        pager.isPagingEnabled = true
        pager.showsHorizontalScrollIndicator = false
        pager.alwaysBounceHorizontal = tabs.count > 1
        pager.contentInsetAdjustmentBehavior = .never
        pager.delegate = self
        pager.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(pager)
        for page in pages {
            page.onPick = { [weak self] harness, row in self?.pick(harness, row) }
            page.onLoaded = { [weak self] in self?.refit() }
            pager.addSubview(page)
        }

        var top = header.bottomAnchor
        if tabs.count > 1 {
            let switcher = ProviderSwitcher(items: tabs.map { ($0.harness, $0.label) })
            switcher.onSelect = { [weak self] ix in self?.select(ix, animated: true) }
            switcher.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(switcher)
            NSLayoutConstraint.activate([
                switcher.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 18),
                switcher.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
                switcher.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
                switcher.heightAnchor.constraint(equalToConstant: ProviderSwitcher.height),
            ])
            self.switcher = switcher
            top = switcher.bottomAnchor
        }
        NSLayoutConstraint.activate([
            pager.topAnchor.constraint(equalTo: top, constant: 10),
            pager.leadingAnchor.constraint(equalTo: view.leadingAnchor),
            pager.trailingAnchor.constraint(equalTo: view.trailingAnchor),
            pager.bottomAnchor.constraint(equalTo: view.bottomAnchor),
        ])
        switcher?.setPosition(CGFloat(selected))
        // The current provider first, then the rest, so a swipe lands on
        // a loaded list.
        pages[selected].load()
        for (ix, page) in pages.enumerated() where ix != selected { page.load() }
        setContentScrollView(pages[selected].list, for: .top)
    }

    override func viewDidLayoutSubviews() {
        super.viewDidLayoutSubviews()
        let size = pager.bounds.size
        guard size.width > 0 else { return }
        for (ix, page) in pages.enumerated() {
            page.frame = CGRect(x: CGFloat(ix) * size.width, y: 0, width: size.width, height: size.height)
        }
        pager.contentSize = CGSize(width: size.width * CGFloat(pages.count), height: size.height)
        if !pager.isDragging && !pager.isDecelerating {
            pager.contentOffset.x = CGFloat(selected) * size.width
        }
    }

    // MARK: Header

    /// "Model" over where it runs (with one provider, that provider first),
    /// and a glass close button.
    private func buildHeader() {
        let title = UILabel()
        title.text = "Model"
        title.font = Fonts.ui(.sansSemibold, 22)
        title.textColor = Palette.text
        title.accessibilityTraits = .header

        let sub = UILabel()
        sub.font = Fonts.ui(.sans, 13)
        sub.textColor = Palette.secondary
        let icon = UIImageView()
        icon.contentMode = .scaleAspectFit
        icon.tintColor = Palette.secondary
        if tabs.count == 1, let only = tabs.first {
            sub.text = "\(only.label) · \(host.name)"
            icon.image = BrandMarks.image(for: only.harness, side: 13)
        } else {
            sub.text = host.name
            icon.image = UIImage(systemName: host.symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .medium))
        }
        let line = UIStackView(arrangedSubviews: [icon, sub])
        line.spacing = 5
        line.alignment = .center
        let text = UIStackView(arrangedSubviews: [title, line])
        text.axis = .vertical
        text.alignment = .leading
        text.spacing = 3
        text.accessibilityIdentifier = "model-picker-title"

        var c = UIButton.Configuration.glass()
        c.image = UIImage(systemName: "xmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
        c.baseForegroundColor = Palette.secondary
        c.cornerStyle = .capsule
        let close = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        close.accessibilityLabel = "Close"
        close.accessibilityIdentifier = "model-picker-close"

        for v in [text, close] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            header.addSubview(v)
        }
        header.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(header)
        NSLayoutConstraint.activate([
            header.topAnchor.constraint(equalTo: view.topAnchor, constant: 26),
            header.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            header.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -18),
            text.topAnchor.constraint(equalTo: header.topAnchor),
            text.bottomAnchor.constraint(equalTo: header.bottomAnchor),
            text.leadingAnchor.constraint(equalTo: header.leadingAnchor),
            text.trailingAnchor.constraint(lessThanOrEqualTo: close.leadingAnchor, constant: -12),
            close.trailingAnchor.constraint(equalTo: header.trailingAnchor),
            close.centerYAnchor.constraint(equalTo: title.centerYAnchor, constant: 4),
            close.widthAnchor.constraint(equalToConstant: 36),
            close.heightAnchor.constraint(equalToConstant: 36),
        ])
    }

    // MARK: Paging

    private func select(_ ix: Int, animated: Bool) {
        guard ix != selected, pages.indices.contains(ix) else { return }
        settle(on: ix)
        let x = CGFloat(ix) * pager.bounds.width
        let move = {
            self.pager.contentOffset.x = x
            self.switcher?.setPosition(CGFloat(ix))
        }
        guard animated, !UIAccessibility.isReduceMotionEnabled else { return move() }
        UIView.animate(withDuration: 0.45, delay: 0, usingSpringWithDamping: 0.86, initialSpringVelocity: 0, options: [.allowUserInteraction, .beginFromCurrentState], animations: move)
    }

    private func settle(on ix: Int) {
        guard ix != selected else { return }
        selected = ix
        UISelectionFeedbackGenerator().selectionChanged()
        view.endEditing(true)
        setContentScrollView(pages[ix].list, for: .top)
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        guard scrollView === pager, pager.bounds.width > 0, pager.isTracking || pager.isDecelerating else { return }
        let position = min(max(pager.contentOffset.x / pager.bounds.width, 0), CGFloat(pages.count - 1))
        switcher?.setPosition(position)
        settle(on: Int(position.rounded()))
    }

    // MARK: Sizing

    /// Tall enough for the longest provider's list, up to most of the screen.
    private func fittedHeight(_ maximum: CGFloat) -> CGFloat {
        let estimate = maximum * 0.55
        guard isViewLoaded, view.window != nil, pager.bounds.width > 0 else { return estimate }
        let lists = pages.map(\.contentHeight).max() ?? 0
        guard lists > 0 else { return estimate }
        return min(maximum * 0.78, max(pager.frame.minY + lists, 280))
    }

    private func refit() {
        guard let sheet, view.window != nil, sheet.selectedDetentIdentifier != .large else { return }
        sheet.animateChanges { sheet.invalidateDetents() }
    }

    override func viewDidAppear(_ animated: Bool) {
        super.viewDidAppear(animated)
        refit()
    }

    // MARK: Picking

    private func pick(_ harness: String, _ row: ModelPickerRow) {
        guard !picked else { return }
        picked = true
        for page in pages { page.setCurrent((harness, row.id)) }
        UIImpactFeedbackGenerator(style: .light).impactOccurred()
        onPick(harness, row)
        // A beat to see the selection land.
        DispatchQueue.main.asyncAfter(deadline: .now() + 0.22) { [weak self] in
            self?.dismiss(animated: true)
        }
    }
}

// MARK: - A provider's page

/// One provider's models, straight on the glass; a notice over them when
/// there's something to know, and a search field once the list is long
/// (OpenCode can offer hundreds).
private final class Page: UIView, UICollectionViewDelegate, UISearchTextFieldDelegate {
    private enum Item: Hashable {
        case model(String)
        case placeholder(Int)
    }

    let tab: ModelPickerTab
    var onPick: ((String, ModelPickerRow) -> Void)?
    var onLoaded: (() -> Void)?
    let list: UICollectionView
    private var current: (harness: String, model: String?)
    private let notice: String?
    private var rows: [ModelPickerRow]?
    private var byId: [String: ModelPickerRow] = [:]
    private var query = ""
    private var data: UICollectionViewDiffableDataSource<Int, Item>!
    private var started = false
    private var listTop: NSLayoutConstraint!

    static let searchThreshold = 12

    init(tab: ModelPickerTab, current: (harness: String, model: String?), notice: String?) {
        self.tab = tab
        self.current = current
        self.notice = notice
        let layout = UICollectionViewCompositionalLayout { _, _ in
            let size = NSCollectionLayoutSize(widthDimension: .fractionalWidth(1), heightDimension: .estimated(62))
            let group = NSCollectionLayoutGroup.vertical(layoutSize: size, subitems: [NSCollectionLayoutItem(layoutSize: size)])
            let section = NSCollectionLayoutSection(group: group)
            section.interGroupSpacing = 2
            section.contentInsets = NSDirectionalEdgeInsets(top: 0, leading: 12, bottom: 20, trailing: 12)
            if notice != nil {
                let header = NSCollectionLayoutBoundarySupplementaryItem(
                    layoutSize: NSCollectionLayoutSize(widthDimension: .fractionalWidth(1), heightDimension: .estimated(48)),
                    elementKind: UICollectionView.elementKindSectionHeader, alignment: .top)
                section.boundarySupplementaryItems = [header]
            }
            return section
        }
        list = UICollectionView(frame: .zero, collectionViewLayout: layout)
        super.init(frame: .zero)
        list.backgroundColor = .clear
        list.keyboardDismissMode = .onDrag
        list.delegate = self
        list.accessibilityIdentifier = "model-picker-list-\(tab.harness)"
        list.translatesAutoresizingMaskIntoConstraints = false
        addSubview(list)
        listTop = list.topAnchor.constraint(equalTo: topAnchor)
        NSLayoutConstraint.activate([
            listTop,
            list.leadingAnchor.constraint(equalTo: leadingAnchor),
            list.trailingAnchor.constraint(equalTo: trailingAnchor),
            list.bottomAnchor.constraint(equalTo: bottomAnchor),
        ])
        configureData()
        apply(animated: false)
    }

    required init?(coder: NSCoder) { fatalError() }

    private func configureData() {
        let modelCell = UICollectionView.CellRegistration<ModelCell, String> { [weak self] cell, _, id in
            guard let self, let row = self.byId[id] else { return }
            cell.configure(row, current: self.tab.harness == self.current.harness && id == self.current.model)
        }
        let placeholderCell = UICollectionView.CellRegistration<SkeletonCell, Int> { cell, _, ix in
            cell.configure(seed: ix)
        }
        data = UICollectionViewDiffableDataSource(collectionView: list) { list, indexPath, item in
            switch item {
            case .model(let id): list.dequeueConfiguredReusableCell(using: modelCell, for: indexPath, item: id)
            case .placeholder(let ix): list.dequeueConfiguredReusableCell(using: placeholderCell, for: indexPath, item: ix)
            }
        }
        let noticeView = UICollectionView.SupplementaryRegistration<NoticeView>(elementKind: UICollectionView.elementKindSectionHeader) { [weak self] view, _, _ in
            view.text = self?.notice
        }
        data.supplementaryViewProvider = { list, _, indexPath in
            list.dequeueConfiguredReusableSupplementary(using: noticeView, for: indexPath)
        }
    }

    func load() {
        guard !started else { return }
        started = true
        Task { @MainActor [weak self] in
            guard let self else { return }
            var seen = Set<String>()
            let rows = await self.tab.load().filter { seen.insert($0.id).inserted }
            self.rows = rows
            self.byId = Dictionary(uniqueKeysWithValues: rows.map { ($0.id, $0) })
            if rows.count > Self.searchThreshold { self.addSearch() }
            self.apply(animated: true)
            self.onLoaded?()
        }
    }

    func setCurrent(_ current: (harness: String, model: String?)) {
        self.current = current
        for cell in list.visibleCells {
            guard let cell = cell as? ModelCell, let ip = list.indexPath(for: cell),
                  case .model(let id) = data.itemIdentifier(for: ip), let row = byId[id] else { continue }
            UIView.animate(withDuration: 0.2) {
                cell.configure(row, current: self.tab.harness == current.harness && id == current.model)
            }
        }
    }

    /// The list's natural height (search included), for the sheet's fit.
    var contentHeight: CGFloat {
        list.layoutIfNeeded()
        return list.collectionViewLayout.collectionViewContentSize.height + listTop.constant
    }

    private var visible: [ModelPickerRow] {
        let all = rows ?? []
        let q = query.trimmingCharacters(in: .whitespaces).lowercased()
        guard !q.isEmpty else { return all }
        return all.filter { $0.label.lowercased().contains(q) || $0.id.lowercased().contains(q) }
    }

    private func apply(animated: Bool) {
        var snap = NSDiffableDataSourceSnapshot<Int, Item>()
        snap.appendSections([0])
        if rows == nil {
            snap.appendItems((0..<4).map(Item.placeholder))
        } else {
            snap.appendItems(visible.map { .model($0.id) })
        }
        data.apply(snap, animatingDifferences: animated && !UIAccessibility.isReduceMotionEnabled)
        list.backgroundView = rows != nil && visible.isEmpty ? emptyLabel() : nil
    }

    private func emptyLabel() -> UIView {
        let label = UILabel()
        label.text = query.isEmpty ? "No models available" : "No matches"
        label.font = Fonts.ui(.sans, 15)
        label.textColor = Palette.tertiary
        label.textAlignment = .center
        let holder = UIView()
        holder.addSubview(label)
        label.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: holder.centerXAnchor),
            label.topAnchor.constraint(equalTo: holder.topAnchor, constant: 40),
        ])
        return holder
    }

    private func addSearch() {
        let field = UISearchTextField()
        field.placeholder = "Search \(tab.label) models"
        field.font = Fonts.ui(.sans, 15)
        field.returnKeyType = .done
        field.delegate = self
        field.accessibilityIdentifier = "model-picker-search"
        field.addAction(UIAction { [weak self, weak field] _ in
            self?.query = field?.text ?? ""
            self?.apply(animated: false)
        }, for: .editingChanged)
        field.translatesAutoresizingMaskIntoConstraints = false
        addSubview(field)
        NSLayoutConstraint.activate([
            field.topAnchor.constraint(equalTo: topAnchor, constant: 2),
            field.leadingAnchor.constraint(equalTo: safeAreaLayoutGuide.leadingAnchor, constant: 20),
            field.trailingAnchor.constraint(equalTo: safeAreaLayoutGuide.trailingAnchor, constant: -20),
        ])
        listTop.constant = 48
    }

    func textFieldShouldReturn(_ textField: UITextField) -> Bool {
        textField.resignFirstResponder()
        return true
    }

    func collectionView(_ collectionView: UICollectionView, didSelectItemAt indexPath: IndexPath) {
        collectionView.deselectItem(at: indexPath, animated: false)
        guard case .model(let id) = data.itemIdentifier(for: indexPath), let row = byId[id] else { return }
        onPick?(tab.harness, row)
    }

    func collectionView(_ collectionView: UICollectionView, shouldHighlightItemAt indexPath: IndexPath) -> Bool {
        if case .model = data.itemIdentifier(for: indexPath) { return true }
        return false
    }
}

/// A model: its name over one or two lines of what it's for. The current
/// one sits in a soft accent tint with an accent check; a press dims in.
private final class ModelCell: UICollectionViewCell {
    private let plate = UIView()
    private let name = UILabel()
    private let detail = UILabel()
    private let check = UIImageView(image: UIImage(systemName: "checkmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .bold)))
    private var isCurrent = false

    override init(frame: CGRect) {
        super.init(frame: frame)
        plate.layer.cornerRadius = 18
        plate.layer.cornerCurve = .continuous
        name.font = Fonts.ui(.sansMedium, 16)
        name.textColor = Palette.text
        detail.font = Fonts.ui(.sans, 13)
        detail.textColor = Palette.secondary
        detail.numberOfLines = 2
        check.tintColor = Palette.accent
        check.contentMode = .center
        check.setContentHuggingPriority(.required, for: .horizontal)
        check.setContentCompressionResistancePriority(.required, for: .horizontal)
        let text = UIStackView(arrangedSubviews: [name, detail])
        text.axis = .vertical
        text.spacing = 3
        let row = UIStackView(arrangedSubviews: [text, check])
        row.alignment = .center
        row.spacing = 12
        for v in [plate, row] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(v)
        }
        NSLayoutConstraint.activate([
            plate.topAnchor.constraint(equalTo: contentView.topAnchor),
            plate.bottomAnchor.constraint(equalTo: contentView.bottomAnchor),
            plate.leadingAnchor.constraint(equalTo: contentView.leadingAnchor),
            plate.trailingAnchor.constraint(equalTo: contentView.trailingAnchor),
            row.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 12),
            row.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -12),
            row.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: contentView.trailingAnchor, constant: -16),
            check.widthAnchor.constraint(equalToConstant: 20),
        ])
        isAccessibilityElement = true
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ row: ModelPickerRow, current: Bool) {
        name.text = row.label
        detail.text = row.detail
        detail.isHidden = (row.detail ?? "").isEmpty
        isCurrent = current
        check.alpha = current ? 1 : 0
        updatePlate()
        accessibilityLabel = [row.label, row.detail].compactMap { $0 }.joined(separator: ", ")
        accessibilityValue = current ? "Current" : nil
        accessibilityIdentifier = "model-picker-row-\(row.id)"
    }

    override var accessibilityTraits: UIAccessibilityTraits {
        get { isCurrent ? [.button, .selected] : .button }
        set {}
    }

    override var isHighlighted: Bool {
        didSet { UIView.animate(withDuration: isHighlighted ? 0.06 : 0.3, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) { self.updatePlate() } }
    }

    private func updatePlate() {
        plate.backgroundColor = isCurrent ? Palette.accentSoft : isHighlighted ? Palette.controlFill : .clear
        plate.transform = isHighlighted ? CGAffineTransform(scaleX: 0.985, y: 0.985) : .identity
    }
}

/// What to know before picking ("Your next message continues this chat
/// with Codex"): a quiet tinted line over the list.
private final class NoticeView: UICollectionReusableView {
    private let label = UILabel()

    var text: String? {
        get { label.text }
        set { label.text = newValue }
    }

    override init(frame: CGRect) {
        super.init(frame: frame)
        let plate = UIView()
        plate.backgroundColor = Palette.controlFill
        plate.layer.cornerRadius = 14
        plate.layer.cornerCurve = .continuous
        let icon = UIImageView(image: UIImage(systemName: "info.circle", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .medium)))
        icon.tintColor = Palette.secondary
        icon.setContentHuggingPriority(.required, for: .horizontal)
        label.font = Fonts.ui(.sans, 13)
        label.textColor = Palette.secondary
        label.numberOfLines = 0
        let row = UIStackView(arrangedSubviews: [icon, label])
        row.spacing = 8
        row.alignment = .firstBaseline
        for v in [plate, row] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            plate.topAnchor.constraint(equalTo: topAnchor),
            plate.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
            plate.leadingAnchor.constraint(equalTo: leadingAnchor),
            plate.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.topAnchor.constraint(equalTo: plate.topAnchor, constant: 11),
            row.bottomAnchor.constraint(equalTo: plate.bottomAnchor, constant: -11),
            row.leadingAnchor.constraint(equalTo: plate.leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: plate.trailingAnchor, constant: -14),
        ])
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// A loading row: two soft bars that breathe until the models arrive.
private final class SkeletonCell: UICollectionViewCell {
    private let title = UIView()
    private let detail = UIView()
    private var widths: [NSLayoutConstraint] = []

    override init(frame: CGRect) {
        super.init(frame: frame)
        for bar in [title, detail] {
            bar.backgroundColor = Palette.controlFill
            bar.layer.cornerRadius = 5
            bar.layer.cornerCurve = .continuous
            bar.translatesAutoresizingMaskIntoConstraints = false
            contentView.addSubview(bar)
        }
        NSLayoutConstraint.activate([
            title.topAnchor.constraint(equalTo: contentView.topAnchor, constant: 16),
            title.leadingAnchor.constraint(equalTo: contentView.leadingAnchor, constant: 14),
            title.heightAnchor.constraint(equalToConstant: 13),
            detail.topAnchor.constraint(equalTo: title.bottomAnchor, constant: 9),
            detail.leadingAnchor.constraint(equalTo: title.leadingAnchor),
            detail.heightAnchor.constraint(equalToConstant: 10),
            detail.bottomAnchor.constraint(equalTo: contentView.bottomAnchor, constant: -16),
        ])
        isUserInteractionEnabled = false
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(seed: Int) {
        NSLayoutConstraint.deactivate(widths)
        let t: [CGFloat] = [0.3, 0.38, 0.26, 0.34]
        let d: [CGFloat] = [0.6, 0.48, 0.55, 0.42]
        widths = [
            title.widthAnchor.constraint(equalTo: contentView.widthAnchor, multiplier: t[seed % 4]),
            detail.widthAnchor.constraint(equalTo: contentView.widthAnchor, multiplier: d[seed % 4]),
        ]
        NSLayoutConstraint.activate(widths)
        guard !UIAccessibility.isReduceMotionEnabled else { return }
        for bar in [title, detail] {
            bar.layer.removeAllAnimations()
            let pulse = CABasicAnimation(keyPath: "opacity")
            pulse.fromValue = 1
            pulse.toValue = 0.4
            pulse.duration = 0.9
            pulse.beginTime = CACurrentMediaTime() + Double(seed) * 0.12
            pulse.autoreverses = true
            pulse.repeatCount = .infinity
            pulse.timingFunction = CAMediaTimingFunction(name: .easeInEaseOut)
            bar.layer.add(pulse, forKey: "pulse")
        }
    }
}

// MARK: - Provider switcher

/// A capsule track of provider segments (logo + name) with a thumb that
/// follows the pages under it, finger for finger. When the names don't all
/// fit, the others shrink to their logos and the chosen one keeps its name;
/// when even that doesn't fit (a computer offering every agent), the track
/// scrolls, keeps the chosen one in view, and fades out at the edges with
/// more to see.
private final class ProviderSwitcher: UIView, UIScrollViewDelegate {
    static let height: CGFloat = 40
    private static let inset: CGFloat = 3
    private static let fade: CGFloat = 34

    var onSelect: ((Int) -> Void)?
    private let clip = UIView()
    private let scroller = UIScrollView()
    private let edges = CAGradientLayer()
    private let thumb = UIView()
    private var segments: [Segment] = []
    private var position: CGFloat = 0

    init(items: [(harness: String, title: String)]) {
        super.init(frame: .zero)
        backgroundColor = Palette.controlFill
        layer.cornerRadius = Self.height / 2
        layer.cornerCurve = .continuous
        clip.layer.cornerRadius = Self.height / 2
        clip.layer.cornerCurve = .continuous
        clip.clipsToBounds = true
        addSubview(clip)
        scroller.showsHorizontalScrollIndicator = false
        scroller.alwaysBounceHorizontal = false
        scroller.contentInsetAdjustmentBehavior = .never
        scroller.delegate = self
        clip.addSubview(scroller)
        edges.startPoint = CGPoint(x: 0, y: 0.5)
        edges.endPoint = CGPoint(x: 1, y: 0.5)
        thumb.backgroundColor = Palette.dual(.white, UIColor(white: 1, alpha: 0.14))
        thumb.layer.cornerCurve = .continuous
        thumb.layer.shadowColor = UIColor.black.cgColor
        thumb.layer.shadowOffset = CGSize(width: 0, height: 1)
        thumb.layer.shadowRadius = 5
        scroller.addSubview(thumb)
        for (ix, item) in items.enumerated() {
            // The tab is short on room: "Claude", not "Claude Code".
            let segment = Segment(harness: item.harness, title: item.title == "Claude Code" ? "Claude" : item.title)
            segment.addAction(UIAction { [weak self] _ in self?.onSelect?(ix) }, for: .touchUpInside)
            segments.append(segment)
            scroller.addSubview(segment)
        }
        updateShadow()
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: ProviderSwitcher, _) in self.updateShadow() }
    }

    required init?(coder: NSCoder) { fatalError() }

    private func updateShadow() {
        thumb.layer.shadowOpacity = traitCollection.userInterfaceStyle == .dark ? 0 : 0.1
    }

    /// Fractional: 1.4 = 40% of the way from the second provider to the third.
    func setPosition(_ position: CGFloat) {
        self.position = position
        setNeedsLayout()
        layoutIfNeeded()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        guard !segments.isEmpty, bounds.width > 0 else { return }
        clip.frame = bounds
        scroller.frame = bounds
        let lower = max(0, min(segments.count - 1, Int(position.rounded(.down))))
        let upper = min(segments.count - 1, lower + 1)
        let t = min(max(position - CGFloat(lower), 0), 1)
        let a = frames(selected: lower), b = frames(selected: upper)
        let compact = isCompact
        for (k, segment) in segments.enumerated() {
            // Bounds + center, not frame: a pressed segment is scaled.
            let f = lerp(a[k], b[k], t)
            segment.bounds = CGRect(origin: .zero, size: f.size)
            segment.center = CGPoint(x: f.midX, y: f.midY)
            segment.nameVisibility = compact ? lerp(k == lower ? 1 : 0, k == upper ? 1 : 0, t) : 1
            segment.emphasis = max(0, 1 - abs(position - CGFloat(k)))
            segment.setNeedsLayout()
            segment.layoutIfNeeded()
        }
        thumb.frame = lerp(a[lower], b[upper], t)
        thumb.layer.cornerRadius = thumb.bounds.height / 2
        let content = max(bounds.width, (segments.last?.frame.maxX ?? 0) + Self.inset)
        scroller.contentSize = CGSize(width: content, height: bounds.height)
        // Keep the chosen provider in the middle of the view.
        let maxOffset = content - bounds.width
        if maxOffset > 0 {
            scroller.contentOffset.x = min(max(thumb.frame.midX - bounds.width / 2, 0), maxOffset)
        }
        updateEdges()
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) { updateEdges() }

    /// Fade the side(s) that have more providers past them.
    private func updateEdges() {
        let maxOffset = scroller.contentSize.width - bounds.width
        guard maxOffset > 0.5, bounds.width > 0 else {
            clip.layer.mask = nil
            return
        }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let x = scroller.contentOffset.x
        let f = Self.fade / bounds.width
        let start: UIColor = x > 0.5 ? .clear : .black
        let end: UIColor = x < maxOffset - 0.5 ? .clear : .black
        edges.frame = clip.bounds
        edges.colors = [start.cgColor, UIColor.black.cgColor, UIColor.black.cgColor, end.cgColor]
        edges.locations = [0, NSNumber(value: Double(f)), NSNumber(value: Double(1 - f)), 1]
        clip.layer.mask = edges
        CATransaction.commit()
    }

    private var available: CGFloat { bounds.width - Self.inset * 2 }

    private var isCompact: Bool { segments.map(\.naturalWidth).reduce(0, +) > available }

    private func frames(selected: Int) -> [CGRect] {
        let natural = segments.map(\.naturalWidth)
        let n = CGFloat(segments.count)
        var widths: [CGFloat]
        if (natural.max() ?? 0) * n <= available {
            widths = Array(repeating: available / n, count: segments.count)
        } else if natural.reduce(0, +) <= available {
            let extra = (available - natural.reduce(0, +)) / n
            widths = natural.map { $0 + extra }
        } else {
            // Past this the track scrolls rather than squeezing logos.
            widths = segments.indices.map { $0 == selected ? natural[$0] : Segment.logoWidth }
            let extra = max(0, (available - widths.reduce(0, +)) / n)
            widths = widths.map { $0 + extra }
        }
        var x = Self.inset
        return widths.map { w in
            defer { x += w }
            return CGRect(x: x, y: Self.inset, width: w, height: bounds.height - Self.inset * 2)
        }
    }

    private func lerp(_ a: CGFloat, _ b: CGFloat, _ t: CGFloat) -> CGFloat { a + (b - a) * t }

    private func lerp(_ a: CGRect, _ b: CGRect, _ t: CGFloat) -> CGRect {
        CGRect(x: lerp(a.minX, b.minX, t), y: lerp(a.minY, b.minY, t), width: lerp(a.width, b.width, t), height: lerp(a.height, b.height, t))
    }
}

/// One provider in the switcher. Two stacked names (muted, full) cross-fade
/// with `emphasis`, so the color follows the finger too.
private final class Segment: UIControl {
    static let logoWidth: CGFloat = 46
    private static let logo: CGFloat = 17
    private static let gap: CGFloat = 7

    var nameVisibility: CGFloat = 1
    var emphasis: CGFloat = 0
    private let icon = UIImageView()
    private let muted = UILabel()
    private let full = UILabel()
    private let nameWidth: CGFloat

    init(harness: String, title: String) {
        for label in [muted, full] {
            label.text = title
            label.font = Fonts.ui(.sansMedium, 14)
        }
        muted.textColor = Palette.secondary
        full.textColor = Palette.text
        nameWidth = ceil(full.intrinsicContentSize.width)
        super.init(frame: .zero)
        icon.image = BrandMarks.image(for: harness, side: Self.logo)
        icon.tintColor = Palette.text
        icon.contentMode = .scaleAspectFit
        for v in [icon, muted, full] as [UIView] {
            v.isUserInteractionEnabled = false
            addSubview(v)
        }
        clipsToBounds = true
        isAccessibilityElement = true
        accessibilityLabel = title
        accessibilityIdentifier = "model-picker-tab-\(harness)"
    }

    required init?(coder: NSCoder) { fatalError() }

    var naturalWidth: CGFloat { 14 + Self.logo + Self.gap + nameWidth + 16 }

    override var accessibilityTraits: UIAccessibilityTraits {
        get { emphasis > 0.5 ? [.button, .selected] : .button }
        set {}
    }

    override var isHighlighted: Bool {
        didSet {
            UIView.animate(withDuration: isHighlighted ? 0.08 : 0.25) {
                self.icon.alpha = self.isHighlighted ? 0.45 : self.iconAlpha
                self.transform = self.isHighlighted ? CGAffineTransform(scaleX: 0.96, y: 0.96) : .identity
            }
        }
    }

    private var iconAlpha: CGFloat { 0.55 + 0.45 * emphasis }

    override func layoutSubviews() {
        super.layoutSubviews()
        // Frames in bounds space: the press scale is a transform.
        let size = bounds.size
        let content = Self.logo + (Self.gap + nameWidth) * nameVisibility
        let x0 = ((size.width - content) / 2).rounded()
        icon.frame = CGRect(x: x0, y: ((size.height - Self.logo) / 2).rounded(), width: Self.logo, height: Self.logo)
        icon.alpha = isHighlighted ? 0.45 : iconAlpha
        let h = ceil(full.font.lineHeight)
        let name = CGRect(x: x0 + Self.logo + Self.gap, y: ((size.height - h) / 2).rounded(), width: nameWidth, height: h)
        muted.frame = name
        full.frame = name
        muted.alpha = nameVisibility * (1 - emphasis)
        full.alpha = nameVisibility * emphasis
    }
}
