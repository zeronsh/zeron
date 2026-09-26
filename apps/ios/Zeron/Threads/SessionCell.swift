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

/// Two-line session row, laid out by hand: fixed height, no Auto Layout
/// solving per cell, no text measurement beyond single-line labels.
final class SessionCell: UICollectionViewListCell {
    static let height: CGFloat = 64

    private let dot = UIView()
    private let title = UILabel()
    private let meta = UILabel()
    private let time = UILabel()
    private let harness = UIImageView()
    private let prIcon = UIImageView()
    private let status = DotGridView(style: .idle)
    private let pin = UIImageView(image: UIImage(systemName: "pin.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 9, weight: .semibold)))
    private var vm: SessionRowVM?
    var indent: CGFloat = 0 { didSet { setNeedsLayout() } }

    override init(frame: CGRect) {
        super.init(frame: frame)
        dot.layer.cornerRadius = 4.5
        title.font = Fonts.ui(.sansMedium, 17)
        title.textColor = Palette.text
        meta.font = Fonts.ui(.sans, 14)
        meta.textColor = Palette.secondary
        time.font = Fonts.ui(.sans, 14)
        time.textColor = Palette.secondary
        time.textAlignment = .right
        harness.contentMode = .scaleAspectFit
        harness.tintColor = Palette.secondary
        prIcon.contentMode = .scaleAspectFit
        pin.tintColor = Palette.tertiary
        for v in [dot, title, meta, time, harness, prIcon, status, pin] { contentView.addSubview(v) }
        var bg = UIBackgroundConfiguration.listCell()
        bg.backgroundColor = .clear
        backgroundConfiguration = bg
    }

    required init?(coder: NSCoder) { fatalError() }

    override func updateConfiguration(using state: UICellConfigurationState) {
        var bg = UIBackgroundConfiguration.listCell().updated(for: state)
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.chip.withAlphaComponent(0.7) : .clear
        bg.cornerRadius = 14
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }

    func configure(_ vm: SessionRowVM) {
        self.vm = vm
        dot.backgroundColor = Palette.projectDots[vm.colorIndex % Palette.projectDots.count]
        title.text = vm.title
        title.font = Fonts.ui(vm.unseen ? .sansSemibold : .sansMedium, 17)
        title.textColor = vm.unseen || vm.status != .idle ? Palette.text : Palette.text.withAlphaComponent(0.86)
        var parts = [vm.projectName]
        if let n = vm.prNumber { parts.append(String(n)) } else if let b = vm.branch, !b.isEmpty { parts.append(b) }
        meta.text = parts.joined(separator: "  ")
        harness.image = BrandMarks.image(for: vm.harness)
        if let pr = vm.pr {
            prIcon.isHidden = false
            prIcon.image = UIImage(systemName: pr == .merged ? "arrow.triangle.merge" : "arrow.triangle.pull", withConfiguration: UIImage.SymbolConfiguration(pointSize: 11, weight: .semibold))
            prIcon.tintColor = switch pr {
            case .open: Palette.success
            case .draft: Palette.secondary
            case .merged: UIColor(hex: 0x8250DF)
            case .closed: Palette.danger
            }
        } else {
            prIcon.isHidden = true
        }
        pin.isHidden = !vm.pinned
        switch vm.status {
        case .working, .awaiting, .errored:
            status.isHidden = false
            time.isHidden = true
            status.style = vm.status == .working ? .working : vm.status == .awaiting ? .awaiting : .errored
        case .idle, .completed:
            status.isHidden = true
            time.isHidden = false
            time.text = vm.sendFailed ? "Failed" : vm.timeLabel
            time.textColor = vm.sendFailed ? Palette.danger : Palette.secondary
        }
        accessibilityLabel = "\(vm.title), \(vm.projectName)"
        accessibilityIdentifier = "session-\(vm.id)"
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let b = contentView.bounds
        let left: CGFloat = 20 + indent
        let right: CGFloat = 20
        dot.frame = CGRect(x: left, y: 17, width: 9, height: 9)
        let textX = left + 9 + 16
        let trailing: CGFloat
        if !status.isHidden {
            status.frame = CGRect(x: b.width - right - 16, y: 13, width: 16, height: 16)
            trailing = status.frame.minX - 10
        } else {
            let tw = ceil(time.sizeThatFits(CGSize(width: 80, height: 20)).width)
            time.frame = CGRect(x: b.width - right - tw, y: 11, width: tw, height: 20)
            trailing = time.frame.minX - 10
        }
        title.frame = CGRect(x: textX, y: 9, width: max(0, trailing - textX), height: 23)
        var x = textX
        if harness.image != nil {
            harness.isHidden = false
            harness.frame = CGRect(x: x, y: 38, width: 15, height: 15)
            x += 21
        } else {
            harness.isHidden = true
        }
        if !pin.isHidden {
            pin.frame = CGRect(x: x, y: 39, width: 11, height: 13)
            x += 16
        }
        let metaW = min(ceil(meta.sizeThatFits(CGSize(width: b.width, height: 18)).width), b.width - right - x - 22)
        meta.frame = CGRect(x: x, y: 36, width: max(0, metaW), height: 19)
        prIcon.frame = CGRect(x: meta.frame.maxX + 6, y: 38, width: 14, height: 15)
    }
}

/// Folder row: icon, name, count, chevron.
final class FolderCell: UICollectionViewListCell {
    static let height: CGFloat = 50

    func configure(_ vm: FolderRowVM) {
        var c = UIListContentConfiguration.cell()
        c.image = UIImage(systemName: vm.symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: 16, weight: .regular))
        c.imageProperties.tintColor = Palette.secondary
        c.imageToTextPadding = 18
        let text = NSMutableAttributedString(string: vm.name, attributes: [.font: Fonts.ui(.sansMedium, 17), .foregroundColor: Palette.secondary])
        text.append(NSAttributedString(string: "  \(vm.count)", attributes: [.font: Fonts.ui(.sans, 17), .foregroundColor: Palette.tertiary]))
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
        bg.backgroundColor = state.isHighlighted || state.isSelected ? Palette.chip.withAlphaComponent(0.7) : .clear
        bg.cornerRadius = 14
        bg.backgroundInsets = NSDirectionalEdgeInsets(top: 1, leading: 8, bottom: 1, trailing: 8)
        backgroundConfiguration = bg
    }
}
