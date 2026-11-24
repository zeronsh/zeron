import UIKit

/// Shared gutter sizing for the diff and its comment-sheet code preview.
enum ChangeDiffMetrics {
    static let lineNumberFont = UIFont.monospacedDigitSystemFont(ofSize: 12, weight: .regular)
    static let minimumGutterWidth: CGFloat = 20

    static func gutterWidth(maxLine: UInt32) -> CGFloat {
        let digits = String(repeating: "8", count: String(maxLine).count) as NSString
        return max(minimumGutterWidth, ceil(digits.size(withAttributes: [.font: lineNumberFont]).width) + 8)
    }
}

final class ChangeFileHeader: UITableViewHeaderFooterView {
    private let button = UIButton(type: .system)
    private let path = UILabel()
    private let chevron = UIImageView()
    private let border = UIView()
    private var toggle: (() -> Void)?

    override init(reuseIdentifier: String?) {
        super.init(reuseIdentifier: reuseIdentifier)
        contentView.backgroundColor = Palette.elevated
        path.font = Fonts.ui(.mono, 12)
        path.textColor = Palette.secondary
        path.numberOfLines = 1
        path.lineBreakMode = .byTruncatingMiddle
        chevron.tintColor = Palette.secondary
        chevron.contentMode = .center
        for v in [path, chevron, button, border] { contentView.addSubview(v) }
        button.addAction(UIAction { [weak self] _ in self?.toggle?() }, for: .touchUpInside)
        border.backgroundColor = Palette.hairline
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        let w = contentView.bounds.width, h = contentView.bounds.height
        chevron.frame = CGRect(x: 8, y: (h-24)/2, width: 24, height: 24)
        path.frame = CGRect(x: 40, y: 0, width: max(0,w-56), height: h)
        border.frame = CGRect(x: 0, y: h-0.5, width: w, height: 0.5)
        button.frame = contentView.bounds
    }

    func configure(_ file: ChangedFile, collapsed: Bool, toggle: @escaping () -> Void) {
        self.toggle = toggle
        path.text = file.path
        chevron.image = UIImage(systemName: collapsed ? "chevron.right" : "chevron.down", withConfiguration: UIImage.SymbolConfiguration(pointSize: 12, weight: .semibold))
        button.accessibilityIdentifier = "change-file-\(file.path)"
        button.accessibilityLabel = file.path
        button.accessibilityValue = collapsed ? "Collapsed" : "Expanded"
        path.isAccessibilityElement = false
    }
}

final class ChangeCodeCell: UITableViewCell, UIScrollViewDelegate {
    private let lineNumber = UILabel()
    private let marker = UILabel()
    private let scroller = UIScrollView()
    private let code = UILabel()
    private let comment = UIImageView()
    private let selectionIndicator = UIView()
    private var syncing = false
    private var codeWidth: CGFloat = 0
    private var planeWidth: CGFloat = 0
    private var gutterWidth = ChangeDiffMetrics.minimumGutterWidth
    var onScroll: ((CGFloat) -> Void)?
    var onTap: (() -> Void)?
    var onHold: (() -> Void)?
    var lineSelected = false { didSet {
        selectionIndicator.isHidden = !lineSelected
        if accessibilityTraits.contains(.button) {
            accessibilityTraits = lineSelected ? [.button, .selected] : .button
        }
    } }
    var hasComment = false { didSet { updateComment() } }

    override init(style: UITableViewCell.CellStyle, reuseIdentifier: String?) {
        super.init(style: style, reuseIdentifier: reuseIdentifier)
        selectionStyle = .none
        for n in [lineNumber, marker] {
            n.font = ChangeDiffMetrics.lineNumberFont
            n.textColor = Palette.tertiary
            n.textAlignment = .right
        }
        code.font = Fonts.ui(.mono, 12)
        code.textColor = Palette.text
        scroller.showsHorizontalScrollIndicator = false
        scroller.showsVerticalScrollIndicator = false
        scroller.alwaysBounceVertical = false
        scroller.isDirectionalLockEnabled = true
        scroller.delegate = self
        scroller.addSubview(code)
        selectionIndicator.backgroundColor = Palette.accent
        selectionIndicator.isUserInteractionEnabled = false
        selectionIndicator.isHidden = true
        contentView.addSubview(selectionIndicator)
        // The hold cancels the tap and table selection, so releasing a
        // successful hold never opens the single-line comment sheet.
        let hold = UILongPressGestureRecognizer(target: self, action: #selector(holdCode(_:)))
        hold.minimumPressDuration = 0.45
        hold.allowableMovement = 12
        hold.cancelsTouchesInView = true
        contentView.addGestureRecognizer(hold)
        let tap = UITapGestureRecognizer(target: self, action: #selector(tapCode))
        tap.require(toFail: hold)
        scroller.addGestureRecognizer(tap)
        for v in [lineNumber, marker, scroller, comment] { contentView.addSubview(v) }
        comment.contentMode = .center
        isAccessibilityElement = true
        accessibilityTraits = .button
    }

    required init?(coder: NSCoder) { fatalError() }
    @objc private func tapCode() { onTap?() }
    @objc private func holdCode(_ gesture: UILongPressGestureRecognizer) {
        if gesture.state == .began { onHold?() }
    }
    @objc private func accessibleSelection() -> Bool { onHold?(); return true }
    override func accessibilityActivate() -> Bool { onTap?(); return true }

    override func prepareForReuse() {
        super.prepareForReuse()
        onScroll = nil
        onTap = nil
        onHold = nil
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let w = contentView.bounds.width, h = contentView.bounds.height
        selectionIndicator.frame = CGRect(x: 0, y: 0, width: 3, height: h)
        lineNumber.frame = CGRect(x: 4, y: 0, width: gutterWidth-8, height: h)
        marker.frame = CGRect(x: gutterWidth+1, y: 0, width: 12, height: h)
        let codeStart = gutterWidth+16
        scroller.frame = CGRect(x: codeStart, y: 0, width: max(0,w-codeStart-(hasComment ? 36 : 12)), height: h)
        // Keep one common horizontal plane for all visible lines, including
        // short ones, so a long line never springs back when it is reused.
        scroller.contentSize = CGSize(width: max(max(codeWidth + 20, planeWidth), scroller.bounds.width), height: h)
        code.frame = CGRect(x: 0, y: 0, width: codeWidth + 8, height: h)
        comment.frame = CGRect(x: w-36, y: 0, width: 36, height: h)
    }

    func configure(_ line: ChangeLine, offset: CGFloat, planeWidth: CGFloat, gutterWidth: CGFloat, hasComment: Bool) {
        self.planeWidth = planeWidth
        self.gutterWidth = gutterWidth
        self.hasComment = hasComment
        let numbered = line.oldLine != nil || line.newLine != nil
        comment.isHidden = !hasComment
        // One compact gutter: deletions cite the original file; additions
        // and context cite the changed file. Comment anchors keep both sides.
        let sourceLine = line.kind == .deletion ? line.oldLine : (line.newLine ?? line.oldLine)
        lineNumber.text = sourceLine.map(String.init) ?? ""
        marker.text = line.kind == .addition ? "+" : line.kind == .deletion ? "−" : ""
        let wash: UIColor = switch line.kind {
        case .addition: Palette.dynamic(0xDAFBE1, 0x10291C)
        case .deletion: Palette.dynamic(0xFFEBE9, 0x351719)
        case .hunk: Palette.dynamic(0xDDF4FF, 0x0C1D32)
        default: Palette.codeBackground
        }
        contentView.backgroundColor = wash
        marker.textColor = line.kind == .addition ? Palette.success : Palette.danger
        let attributed = NSMutableAttributedString(string: line.text.replacingOccurrences(of: "\t", with: "    "), attributes: [.font: Fonts.ui(.mono, 12), .foregroundColor: line.kind == .hunk ? Palette.dynamic(0x0969DA, 0x79B8FF) : Palette.text])
        // Highlight ranges refer to the original UTF-16 text. Expand tabs
        // only after translating each range to the displayed string.
        let source = line.text as NSString
        for span in line.highlights {
            let range = NSRange(location: Int(span.startUtf16), length: Int(span.lenUtf16))
            guard NSMaxRange(range) <= source.length else { continue }
            let prefix = source.substring(to: range.location).replacingOccurrences(of: "\t", with: "    ") as NSString
            let token = source.substring(with: range).replacingOccurrences(of: "\t", with: "    ") as NSString
            attributed.addAttribute(.foregroundColor, value: Palette.color(span.color), range: NSRange(location: prefix.length, length: token.length))
        }
        code.attributedText = attributed
        codeWidth = ceil(attributed.size().width)
        accessibilityIdentifier = "change-line"
        accessibilityLabel = "\(line.kind == .addition ? "Added" : line.kind == .deletion ? "Deleted" : line.kind == .hunk ? "Hunk" : "Context") \(line.newLine ?? line.oldLine ?? 0): \(line.text)"
        accessibilityHint = numbered ? "Double tap to comment or adjust the active range. Hold to select multiple lines." : nil
        accessibilityCustomActions = numbered ? [UIAccessibilityCustomAction(name: "Select Multiple Lines", target: self, selector: #selector(accessibleSelection))] : nil
        accessibilityValue = hasComment ? "Has review comment" : nil
        accessibilityTraits = numbered ? (lineSelected ? [.button, .selected] : .button) : .staticText
        setNeedsLayout()
        layoutIfNeeded()
        setCodeOffset(offset)
    }

    private func updateComment() {
        comment.image = UIImage(systemName: "text.bubble.fill", withConfiguration: UIImage.SymbolConfiguration(pointSize: 13))
        comment.tintColor = Palette.accent
        comment.isHidden = !hasComment
        setNeedsLayout()
    }

    func setCodeOffset(_ offset: CGFloat) {
        syncing = true
        scroller.contentOffset = CGPoint(x: offset, y: 0)
        syncing = false
    }

    func scrollViewDidScroll(_ scrollView: UIScrollView) {
        guard !syncing, scrollView.isDragging || scrollView.isDecelerating else { return }
        onScroll?(max(0,scrollView.contentOffset.x))
    }
}
