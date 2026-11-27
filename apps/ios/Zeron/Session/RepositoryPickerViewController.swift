import UIKit

/// Where a new session in a project folder holding several repositories
/// works: the folder as it is, or a new worktree of the whole project — one
/// folder laid out like it, a worktree of each repository chosen, all on one
/// branch. The sheet chooses which repositories; every change is the
/// draft's at once (`onChange`), so closing it is all there is to do.
///
///   Checkout                                   (x)
///   Acme · MacBook Pro
///   [ Project folder | New worktree ]
///   REPOSITORIES                    2 of 3   All
///   (folder) api            ⑂ main            ✓
///   (folder) packages/ui    ⑂ main
///   (folder) scratch        No commits yet        (dimmed)
///   (folder) web            ⑂ develop         ✓
///   Each gets a worktree on one new branch, in a folder laid out like the
///   project.
final class RepositoryPickerViewController: UIViewController {
    private let project: String
    private let host: String
    private let repositories: [ProjectRepository]
    private var worktree: Bool
    private var chosen: [String]
    private let onChange: (_ worktree: Bool, _ chosen: [String]) -> Void

    private let scroll = UIScrollView()
    private let stack = UIStackView()
    private let mode = UISegmentedControl(items: ["Project folder", "New worktree"])
    private let count = UILabel()
    private let toggleAll = UIButton(type: .system)
    private let hint = UILabel()
    private var rows: [RepositoryRow] = []
    private weak var sheet: UISheetPresentationController?

    static func sheet(project: String, host: String, repositories: [ProjectRepository], worktree: Bool, chosen: [String], onChange: @escaping (Bool, [String]) -> Void) -> UIViewController {
        let picker = RepositoryPickerViewController(project: project, host: host, repositories: repositories, worktree: worktree, chosen: chosen, onChange: onChange)
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

    private init(project: String, host: String, repositories: [ProjectRepository], worktree: Bool, chosen: [String], onChange: @escaping (Bool, [String]) -> Void) {
        self.project = project
        self.host = host
        self.repositories = repositories
        self.worktree = worktree
        self.chosen = chosen
        self.onChange = onChange
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        // Clear: the sheet's own glass is the surface.
        view.backgroundColor = .clear
        let header = buildHeader()

        mode.selectedSegmentIndex = worktree ? 1 : 0
        mode.accessibilityIdentifier = "repositories-mode"
        mode.setTitleTextAttributes([.font: Fonts.ui(.sansMedium, 14)], for: .normal)
        mode.addAction(UIAction { [weak self] _ in
            guard let self else { return }
            self.worktree = self.mode.selectedSegmentIndex == 1
            UISelectionFeedbackGenerator().selectionChanged()
            self.changed()
        }, for: .valueChanged)

        let heading = UILabel()
        heading.text = "REPOSITORIES"
        heading.font = Fonts.ui(.sansMedium, 12)
        heading.textColor = Palette.tertiary
        count.font = Fonts.ui(.sans, 13)
        count.textColor = Palette.tertiary
        count.accessibilityIdentifier = "repositories-count"
        var all = UIButton.Configuration.plain()
        all.contentInsets = NSDirectionalEdgeInsets(top: 4, leading: 8, bottom: 4, trailing: 8)
        all.baseForegroundColor = Palette.accent
        toggleAll.configuration = all
        toggleAll.accessibilityIdentifier = "repositories-all"
        toggleAll.addAction(UIAction { [weak self] _ in self?.flipAll() }, for: .touchUpInside)
        let spacer = UIView()
        spacer.setContentHuggingPriority(.defaultLow, for: .horizontal)
        let headingRow = UIStackView(arrangedSubviews: [heading, spacer, count, toggleAll])
        headingRow.alignment = .center
        headingRow.spacing = 8
        headingRow.isLayoutMarginsRelativeArrangement = true
        headingRow.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 0, leading: 14, bottom: 0, trailing: 2)

        stack.axis = .vertical
        stack.spacing = 2
        stack.addArrangedSubview(headingRow)
        stack.setCustomSpacing(6, after: headingRow)
        for r in repositories {
            let row = RepositoryRow(repository: r)
            row.addAction(UIAction { [weak self] _ in self?.toggle(r.path) }, for: .touchUpInside)
            rows.append(row)
            stack.addArrangedSubview(row)
        }
        hint.font = Fonts.ui(.sans, 13)
        hint.textColor = Palette.secondary
        hint.numberOfLines = 0
        let hintBox = UIStackView(arrangedSubviews: [hint])
        hintBox.isLayoutMarginsRelativeArrangement = true
        hintBox.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 10, leading: 14, bottom: 0, trailing: 14)
        stack.addArrangedSubview(hintBox)

        scroll.alwaysBounceVertical = false
        scroll.showsVerticalScrollIndicator = false
        for v in [mode, scroll] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        stack.translatesAutoresizingMaskIntoConstraints = false
        scroll.addSubview(stack)
        NSLayoutConstraint.activate([
            mode.topAnchor.constraint(equalTo: header.bottomAnchor, constant: 18),
            mode.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 20),
            mode.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -20),
            mode.heightAnchor.constraint(equalToConstant: 34),
            scroll.topAnchor.constraint(equalTo: mode.bottomAnchor, constant: 18),
            scroll.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 10),
            scroll.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -10),
            scroll.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            stack.topAnchor.constraint(equalTo: scroll.contentLayoutGuide.topAnchor),
            stack.bottomAnchor.constraint(equalTo: scroll.contentLayoutGuide.bottomAnchor, constant: -24),
            stack.leadingAnchor.constraint(equalTo: scroll.frameLayoutGuide.leadingAnchor),
            stack.trailingAnchor.constraint(equalTo: scroll.frameLayoutGuide.trailingAnchor),
        ])
        setContentScrollView(scroll, for: .top)
        render(animated: false)
    }

    // MARK: Header

    /// "Checkout" over the project and where it runs, and a glass close
    /// button, as the model picker's.
    private func buildHeader() -> UIView {
        let header = UIView()
        let title = UILabel()
        title.text = "Checkout"
        title.font = Fonts.ui(.sansSemibold, 22)
        title.textColor = Palette.text
        title.accessibilityTraits = .header
        let sub = UILabel()
        sub.text = "\(project) · \(host)"
        sub.font = Fonts.ui(.sans, 13)
        sub.textColor = Palette.secondary
        let icon = UIImageView(image: UIImage(systemName: "folder", withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .medium)))
        icon.tintColor = Palette.secondary
        let line = UIStackView(arrangedSubviews: [icon, sub])
        line.spacing = 5
        line.alignment = .center
        let text = UIStackView(arrangedSubviews: [title, line])
        text.axis = .vertical
        text.alignment = .leading
        text.spacing = 3
        text.accessibilityIdentifier = "repositories-title"

        var c = UIButton.Configuration.glass()
        c.image = UIImage(systemName: "xmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13, weight: .semibold))
        c.baseForegroundColor = Palette.secondary
        c.cornerStyle = .capsule
        let close = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in self?.dismiss(animated: true) })
        close.accessibilityLabel = "Close"
        close.accessibilityIdentifier = "repositories-close"

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
        return header
    }

    // MARK: Choosing

    private var choosable: [String] { repositories.filter(\.hasCommits).map(\.path) }

    private func toggle(_ path: String) {
        guard worktree, choosable.contains(path) else { return }
        if chosen.contains(path) {
            chosen.removeAll { $0 == path }
        } else {
            chosen.append(path)
        }
        // (In the list's order, whichever order they were picked in.)
        chosen = choosable.filter(chosen.contains)
        UISelectionFeedbackGenerator().selectionChanged()
        changed()
    }

    private func flipAll() {
        chosen = chosen.count == choosable.count ? [] : choosable
        UISelectionFeedbackGenerator().selectionChanged()
        changed()
    }

    private func changed() {
        onChange(worktree, chosen)
        render(animated: true)
    }

    private func render(animated: Bool) {
        let update = {
            for row in self.rows {
                row.set(chosen: self.chosen.contains(row.repository.path), enabled: self.worktree)
            }
            self.count.text = "\(self.chosen.count) of \(self.choosable.count)"
            self.count.alpha = self.worktree ? 1 : 0.4
            self.toggleAll.isHidden = !self.worktree || self.choosable.count < 2
            var c = self.toggleAll.configuration
            c?.attributedTitle = AttributedString(self.chosen.count == self.choosable.count ? "None" : "All", attributes: AttributeContainer([.font: Fonts.ui(.sansMedium, 13)]))
            self.toggleAll.configuration = c
            if !self.worktree {
                self.hint.text = "Works in the project's own folder, as it is."
                self.hint.textColor = Palette.secondary
            } else if self.chosen.isEmpty {
                self.hint.text = "Choose at least one repository for the worktree."
                self.hint.textColor = Palette.warning
            } else {
                self.hint.text = "Each gets a worktree on one new branch, in a folder laid out like the project."
                self.hint.textColor = Palette.secondary
            }
        }
        if animated {
            UIView.animate(withDuration: 0.2, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction], animations: update)
        } else {
            update()
        }
    }

    // MARK: Sizing

    /// Its header, the mode, the heading, a row a repository and the hint,
    /// up to most of the screen (worked out, not laid out: a detent is
    /// resolved during layout).
    private func fittedHeight(_ maximum: CGFloat) -> CGFloat {
        let header: CGFloat = 26 + 52 + 18 + 34 + 18
        let rows = CGFloat(repositories.count) * 58
        let rest: CGFloat = 30 + 50 + 16
        return min(maximum * 0.78, max(header + rows + rest + view.safeAreaInsets.bottom, 280))
    }
}

/// A repository in the sheet: its folder glyph, its path, its branch (or why
/// it can't be chosen), and a check while chosen. The whole row takes the
/// tap.
private final class RepositoryRow: UIControl {
    let repository: ProjectRepository
    private let plate = UIView()
    private let check = UIImageView(image: UIImage(systemName: "checkmark", withConfiguration: UIImage.SymbolConfiguration(pointSize: 14, weight: .bold)))
    private var isChosen = false
    private var isLive = true

    init(repository: ProjectRepository) {
        self.repository = repository
        super.init(frame: .zero)
        plate.layer.cornerRadius = 18
        plate.layer.cornerCurve = .continuous
        plate.isUserInteractionEnabled = false
        let can = repository.hasCommits
        let glyph = UIImageView(image: UIImage(systemName: "folder", withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .regular)))
        glyph.tintColor = can ? Palette.secondary : Palette.tertiary
        glyph.contentMode = .center
        let name = UILabel()
        name.text = repository.path
        name.font = Fonts.ui(.sansMedium, 16)
        name.textColor = can ? Palette.text : Palette.tertiary
        name.lineBreakMode = .byTruncatingMiddle
        let branch = UILabel()
        branch.text = can ? (repository.branch ?? "Detached") : "No commits yet"
        branch.font = Fonts.ui(.sans, 13)
        branch.textColor = can ? Palette.secondary : Palette.tertiary
        let branchIcon = UIImageView(image: BranchIcon.sized(12))
        branchIcon.tintColor = Palette.secondary
        branchIcon.isHidden = !can
        let branchLine = UIStackView(arrangedSubviews: [branchIcon, branch])
        branchLine.spacing = 4
        branchLine.alignment = .center
        let text = UIStackView(arrangedSubviews: [name, branchLine])
        text.axis = .vertical
        text.alignment = .leading
        text.spacing = 2
        check.tintColor = Palette.accent
        check.contentMode = .center
        let row = UIStackView(arrangedSubviews: [glyph, text, check])
        row.alignment = .center
        row.spacing = 12
        row.isUserInteractionEnabled = false
        for v in [plate, row] as [UIView] {
            v.translatesAutoresizingMaskIntoConstraints = false
            addSubview(v)
        }
        NSLayoutConstraint.activate([
            plate.topAnchor.constraint(equalTo: topAnchor),
            plate.bottomAnchor.constraint(equalTo: bottomAnchor),
            plate.leadingAnchor.constraint(equalTo: leadingAnchor),
            plate.trailingAnchor.constraint(equalTo: trailingAnchor),
            row.topAnchor.constraint(equalTo: topAnchor, constant: 10),
            row.bottomAnchor.constraint(equalTo: bottomAnchor, constant: -10),
            row.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 14),
            row.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            glyph.widthAnchor.constraint(equalToConstant: 22),
            check.widthAnchor.constraint(equalToConstant: 20),
        ])
        isAccessibilityElement = true
        accessibilityLabel = [repository.path, can ? repository.branch : "No commits yet"].compactMap { $0 }.joined(separator: ", ")
        accessibilityIdentifier = "repository-row-\(repository.path)"
    }

    required init?(coder: NSCoder) { fatalError() }

    func set(chosen: Bool, enabled: Bool) {
        isChosen = chosen
        isLive = enabled
        isEnabled = enabled && repository.hasCommits
        check.alpha = chosen ? (enabled ? 1 : 0.35) : 0
        alpha = enabled ? 1 : 0.5
        updatePlate()
    }

    override var accessibilityTraits: UIAccessibilityTraits {
        get {
            var t: UIAccessibilityTraits = .button
            if isChosen { t.insert(.selected) }
            if !isEnabled { t.insert(.notEnabled) }
            return t
        }
        set {}
    }

    override var isHighlighted: Bool {
        didSet { UIView.animate(withDuration: isHighlighted ? 0.06 : 0.3, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction]) { self.updatePlate() } }
    }

    private func updatePlate() {
        plate.backgroundColor = isChosen && isLive ? Palette.accentSoft : isHighlighted ? Palette.controlFill : .clear
        plate.transform = isHighlighted ? CGAffineTransform(scaleX: 0.985, y: 0.985) : .identity
    }
}
