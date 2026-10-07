import UIKit

/// What a session row shows. Built from the core's front-page snapshot.
struct SessionRowVM: Hashable {
    enum Status: Hashable {
        case idle
        case completed
        case working
        case awaiting
        case errored
    }

    enum PR: Hashable {
        case open, draft, merged, closed
    }

    let id: String
    var title: String
    var projectName: String
    /// False for project-less sessions (the tile reads "H" for Home, like desktop).
    var hasProject: Bool
    var colorIndex: Int
    var harness: String?
    var branch: String?
    var pr: PR?
    var prNumber: UInt64?
    var status: Status
    var timeLabel: String
    var unseen: Bool
    var pinned: Bool
    var sendFailed: Bool
}

/// Folder row ("Pinned 2 ›", "P0 3 ›").
struct FolderRowVM: Hashable {
    let id: String
    var name: String
    var count: Int
    var symbol: String
}

struct SessionGroupVM: Equatable {
    let id: String
    let title: String
    let subtitle: String?
    let projectId: String?
    let sessions: [SessionRowVM]
}

/// Two-line session row, laid out by hand: fixed height, no Auto Layout
/// solving per cell, no text measurement beyond single-line labels. Status
/// and PR badge follow the desktop sidebar.
///
///   [mark]  Title of the session ··········· ⠿ Working
///           [P] project  branch ············· ⎇ 412
final class SessionCell: UICollectionViewListCell {
    static var height: CGFloat { (62 * TypeScale.factor).rounded() }

    private let harness = UIImageView()
    private let title = FadingLabel()
    private let meta = FadingLabel()
    private let time = UILabel()
    private let status = StatusGlyph()
    private let prBadge = PRBadgeView()
    private var vm: SessionRowVM?
    private var preferences = SessionViewPreferences()
    /// The session open beside the sidebar (iPad): a quiet fill.
    var isCurrent = false { didSet { if isCurrent != oldValue { setNeedsUpdateConfiguration() } } }

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.textColor = Palette.text
        time.textAlignment = .right
        harness.contentMode = .scaleAspectFit
        harness.tintColor = Palette.text
        for v in [harness, title, meta, time, status, prBadge] as [UIView] { contentView.addSubview(v) }
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
        // The project tile is a rasterized attachment: redraw it in the new tone.
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (cell: SessionCell, _) in
            if let vm = cell.vm { cell.configure(vm, preferences: cell.preferences) }
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : isCurrent ? Palette.rowActive : .clear
        bg.cornerRadius = 16
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }

    /// Fixed height: skip Auto Layout self-sizing entirely.
    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = vm.map { Self.height(for: $0, preferences: preferences) } ?? Self.height
        return attrs
    }

    static func height(for vm: SessionRowVM, preferences: SessionViewPreferences) -> CGFloat {
        let secondary = preferences.showProjectLabel || preferences.showProjectIcon
            || (preferences.showBranch && vm.branch?.isEmpty == false)
            || (preferences.showPullRequest && vm.pr != nil && vm.prNumber != nil)
        return ((secondary ? 62 : 42) * TypeScale.factor).rounded()
    }

    /// Desktop precedence: a failed send beats everything; then the live
    /// state; unseen-and-finished reads "Done"; otherwise the time.
    static func corner(_ vm: SessionRowVM) -> (word: String, color: UIColor, glyph: StatusGlyph.Kind)? {
        if vm.sendFailed { return ("Failed", Palette.danger, .dot(Palette.danger)) }
        switch vm.status {
        case .working: return ("Working", StatusTone.working, .spinner)
        case .awaiting: return ("Input", StatusTone.input, .dot(StatusTone.input))
        case .errored: return ("Failed", StatusTone.failed, .dot(StatusTone.failed))
        case .completed: return vm.unseen ? ("Done", StatusTone.done, .check(StatusTone.done)) : nil
        case .idle: return nil
        }
    }

    func configure(_ vm: SessionRowVM, preferences: SessionViewPreferences = SessionViewPreferences()) {
        self.vm = vm
        self.preferences = preferences
        harness.isHidden = !preferences.showHarness
        harness.image = BrandMarks.image(for: vm.harness ?? "claude-code", side: Self.markSide)
        title.text = vm.title
        title.font = Fonts.ui(vm.unseen ? .sansSemibold : .sansMedium, TypeScale.size(16.5))
        title.textColor = vm.unseen || vm.status != .idle ? Palette.text : Palette.text.withAlphaComponent(0.88)
        // The project tile rides the meta line as an attachment, so the text
        // system itself centers it on the name's x-height and starts it at
        // the title's leading edge (a separately framed tile drifted high).
        let metaFont = Fonts.ui(.sans, TypeScale.size(13.5))
        let tileSide = (14 * TypeScale.factor).rounded()
        let metaText = NSMutableAttributedString(string: "")
        if preferences.showProjectIcon {
            let tile = NSTextAttachment(image: ProjectTile.image(name: vm.hasProject ? vm.projectName : "Home", colorIndex: vm.colorIndex, side: tileSide))
            tile.bounds = CGRect(x: 0, y: (metaFont.xHeight - tileSide) / 2, width: tileSide, height: tileSide)
            metaText.append(NSAttributedString(attachment: tile))
        }
        if preferences.showProjectLabel {
            let gap = metaText.length > 0 ? "  " : ""
            metaText.append(NSAttributedString(string: gap + vm.projectName, attributes: [.font: metaFont, .foregroundColor: Palette.secondary]))
        }
        if preferences.showBranch, let b = vm.branch, !b.isEmpty, let icon = BranchIcon.image?.withTintColor(Palette.subline, renderingMode: .alwaysOriginal) {
            // Desktop line 3: git-branch icon + branch in the subline tone.
            let font = Fonts.ui(.sans, TypeScale.size(12.5))
            let side = (12 * TypeScale.factor).rounded()
            let attach = NSTextAttachment(image: icon)
            attach.bounds = CGRect(x: 0, y: (font.capHeight - side) / 2, width: side, height: side)
            if metaText.length > 0 { metaText.append(NSAttributedString(string: "   ")) }
            metaText.append(NSAttributedString(attachment: attach))
            metaText.append(NSAttributedString(string: " " + b, attributes: [.font: font, .foregroundColor: Palette.subline]))
        }
        meta.attributedText = metaText
        meta.isHidden = metaText.length == 0
        if preferences.showPullRequest, let pr = vm.pr, let n = vm.prNumber {
            prBadge.isHidden = false
            prBadge.configure(number: n, state: pr, size: TypeScale.size(11))
        } else {
            prBadge.isHidden = true
        }
        let corner = Self.corner(vm)
        if let corner {
            time.text = corner.word
            time.textColor = corner.color
            time.font = Fonts.ui(.sansMedium, TypeScale.size(13))
            status.kind = corner.glyph
            status.isHidden = false
        } else {
            time.text = vm.timeLabel
            time.textColor = StatusTone.time
            time.font = Fonts.ui(.sansMedium, TypeScale.size(13))
            status.kind = .none
            status.isHidden = true
        }
        accessibilityLabel = [vm.title, preferences.showProjectLabel ? vm.projectName : nil,
            preferences.showBranch ? vm.branch : nil, preferences.showHarness ? vm.harness : nil,
            corner?.word, prBadge.isHidden ? nil : prBadge.accessibilityLabel].compactMap { $0 }.joined(separator: ", ")
        accessibilityIdentifier = "session-\(vm.id)"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let left: CGFloat = 20
        let right: CGFloat = 20
        let k = TypeScale.factor
        let titleY = (10 * k).rounded()
        let titleH = (22 * k).rounded()
        let metaY = (35 * k).rounded()
        let metaH = (18 * k).rounded()
        // The mark centers on the title's x-height (font metrics, not the
        // line box): titles are mostly lowercase, and centering on the
        // capitals left the mark riding high — the project tile's rule too.
        let mark = Self.markSide
        let titleFont = title.font ?? Fonts.ui(.sansMedium, TypeScale.size(16.5))
        let baseline = titleY + (titleH - titleFont.lineHeight) / 2 + titleFont.ascender
        harness.frame = CGRect(x: left, y: (baseline - titleFont.xHeight / 2 - mark / 2).rounded(), width: mark, height: mark)
        let textX = preferences.showHarness ? left + mark + 14 : left
        let tw = ceil(time.sizeThatFits(CGSize(width: 140, height: 40)).width)
        time.frame = CGRect(x: b.width - right - tw, y: titleY, width: tw, height: titleH)
        var trailing = time.frame.minX - 10
        if !status.isHidden {
            status.frame = CGRect(x: time.frame.minX - 5 - 12, y: titleY + titleH / 2 - 6, width: 12, height: 12)
            trailing = status.frame.minX - 10
        }
        title.frame = CGRect(x: textX, y: titleY, width: max(0, trailing - textX), height: titleH)
        let x = textX
        var metaRight = b.width - right
        if !prBadge.isHidden {
            let size = prBadge.intrinsicContentSize
            prBadge.frame = CGRect(x: b.width - right - size.width, y: metaY + (metaH - size.height) / 2, width: size.width, height: size.height)
            metaRight = prBadge.frame.minX - 10
        }
        meta.frame = CGRect(x: x, y: metaY, width: max(0, metaRight - x), height: metaH)
    }

    /// Harness mark side.
    static let markSide: CGFloat = 20
}

/// Folder row: icon, name, count, chevron.
final class FolderCell: UICollectionViewListCell {
    static var height: CGFloat { (46 * TypeScale.factor).rounded() }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = Self.height
        return attrs
    }

    func configure(_ vm: FolderRowVM) {
        var c = UIListContentConfiguration.cell()
        c.image = UIImage(systemName: vm.symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 16, weight: .regular))
        c.imageProperties.tintColor = Palette.secondary
        c.imageToTextPadding = 18
        let text = NSMutableAttributedString(string: vm.name, attributes: [.font: Fonts.ui(.sansMedium, TypeScale.size(17)), .foregroundColor: Palette.secondary])
        text.append(NSAttributedString(string: "  \(vm.count)", attributes: [.font: Fonts.ui(.sans, TypeScale.size(17)), .foregroundColor: Palette.tertiary]))
        c.attributedText = text
        c.directionalLayoutMargins = NSDirectionalEdgeInsets(top: 0, leading: 20, bottom: 0, trailing: 20)
        contentConfiguration = c
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
        accessories = [.disclosureIndicator(options: .init(tintColor: Palette.tertiary))]
        accessibilityIdentifier = "folder-\(vm.id)"
    }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.controlFill : .clear
        bg.cornerRadius = 16
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }
}

/// Foldable section header (desktop sidebar style): name, count, a live
/// glyph when something inside is working, and a disclosure chevron.
final class SectionHeaderCell: UICollectionViewListCell {
    struct State: Equatable {
        let id: String
        var title: String
        var subtitle: String? = nil
        var count: Int
        var collapsed: Bool
        var live: StatusGlyph.Kind?
    }

    static var height: CGFloat { (40 * TypeScale.factor).rounded() }
    private let title = FadingLabel()
    private let subtitle = FadingLabel()
    private let count = UILabel()
    private let chevron = UIImageView(image: UIImage(systemName: "chevron.down", withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .semibold)))
    private let live = StatusGlyph()
    private var shownCollapsed: Bool?

    override init(frame: CGRect) {
        super.init(frame: frame)
        title.textColor = Palette.secondary
        count.textColor = Palette.tertiary
        chevron.tintColor = Palette.tertiary
        chevron.contentMode = .center
        subtitle.textColor = Palette.tertiary
        for v in [title, subtitle, count, chevron, live] as [UIView] { contentView.addSubview(v) }
        accessibilityTraits = .button
    }

    required init?(coder: NSCoder) { fatalError() }

    override func preferredLayoutAttributesFitting(_ attrs: UICollectionViewLayoutAttributes) -> UICollectionViewLayoutAttributes {
        attrs.size.height = subtitle.isHidden ? Self.height : (58 * TypeScale.factor).rounded()
        return attrs
    }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted ? Palette.controlFill : .clear
        bg.cornerRadius = 12
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 2, leading: 8, bottom: 2, trailing: 8)
        backgroundConfiguration = bg
    }

    func configure(_ s: State) {
        title.font = Fonts.ui(.sansSemibold, TypeScale.size(13.5))
        count.font = Fonts.ui(.sansMedium, TypeScale.size(13.5))
        title.text = s.title
        subtitle.font = Fonts.ui(.sans, TypeScale.size(11.5))
        subtitle.text = s.subtitle
        subtitle.isHidden = s.subtitle == nil
        count.text = "\(s.count)"
        live.isHidden = s.live == nil
        if let kind = s.live { live.kind = kind }
        let rotate = { self.chevron.transform = s.collapsed ? CGAffineTransform(rotationAngle: -.pi / 2) : .identity }
        if shownCollapsed != nil, shownCollapsed != s.collapsed, window != nil {
            UIView.animate(withDuration: 0.3, delay: 0, usingSpringWithDamping: 0.85, initialSpringVelocity: 0, animations: rotate)
        } else {
            rotate()
        }
        shownCollapsed = s.collapsed
        accessibilityIdentifier = "section-\(s.id)"
        accessibilityLabel = [s.title, s.subtitle, "\(s.count)"].compactMap { $0 }.joined(separator: ", ")
        accessibilityValue = s.collapsed ? "Collapsed" : "Expanded"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let h = (20 * TypeScale.factor).rounded()
        let y = b.height - h - (subtitle.isHidden ? 6 : 22) * TypeScale.factor
        let tw = ceil(title.sizeThatFits(b.size).width)
        title.frame = CGRect(x: 20, y: y, width: min(tw, b.width - 120), height: h)
        subtitle.frame = CGRect(x: 20, y: title.frame.maxY + 2 * TypeScale.factor,
            width: max(0, b.width - 40), height: 16 * TypeScale.factor)
        let cw = ceil(count.sizeThatFits(b.size).width)
        count.frame = CGRect(x: title.frame.maxX + 7, y: y, width: cw, height: h)
        live.frame = CGRect(x: count.frame.maxX + 7, y: y + h / 2 - 6, width: 12, height: 12)
        chevron.frame = CGRect(x: b.width - 20 - 16, y: y, width: 16, height: h)
    }
}
