import UIKit

/// Status colors, as the desktop sidebar tints them (`status_dot_color`).
enum StatusTone {
    static let working = Palette.dynamic(0x5B43E8, 0x8B7CF6, alpha: 0.55)
    static let input = Palette.dynamic(0x5B43E8, 0x8B7CF6, alpha: 0.6)
    static let failed = Palette.dynamic(0xDC2626, 0xF87171, alpha: 0.65)
    static let done = Palette.dynamic(0x15803D, 0x34D399, alpha: 0.9)
    static let idle = Palette.dynamic(0x000000, 0xFFFFFF, alpha: 0.14)
    /// Time-ago text on idle rows (`text_muted` @ 0.5).
    static let time = Palette.dynamic(0x62626A, 0xA9A9AE, alpha: 0.5)
}

/// The desktop's status glyphs:
/// - `.spinner`: the sidebar's mini glyph spinner — a 2×3 grid of violet
///   circles whose brightness chases around the ring (750 ms).
/// - `.trailer`: the transcript's 3×3 gradient spinner (pastel rows).
/// - `.dot`: a static 6 pt circle (every non-working state).
/// - `.check`: completed-and-unseen.
/// Animations are Core Animation keyframes — render-server driven, no
/// main-thread work per frame.
final class StatusGlyph: UIView {
    enum Kind: Equatable {
        case none
        case spinner
        case trailer
        case dot(UIColor)
        case check(UIColor)
    }

    var kind: Kind { didSet { if kind != oldValue { rebuild() } } }
    private var cells: [CALayer] = []
    private let checkLayer = CAShapeLayer()

    init(_ kind: Kind = .none) {
        self.kind = kind
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        checkLayer.fillColor = nil
        checkLayer.lineWidth = 1.6
        checkLayer.lineCap = .round
        checkLayer.lineJoin = .round
        layer.addSublayer(checkLayer)
        rebuild()
    }

    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: CGSize { CGSize(width: 12, height: 12) }

    // Desktop geometry scaled for touch: mini spinner 2 px cells / 1 px gap
    // → 3.5 / 1.5; trailer 2.5 / 1.25 → 3.5 / 1.75; dot 6 → 7.
    private static let ring = [[0, 1], [5, 2], [4, 3]]
    private static let glyphTints = [
        Palette.dynamic(0x7965EC, 0xABA1F9),
        Palette.dynamic(0x5B43E8, 0x8B7CF6),
        Palette.dynamic(0x4332AC, 0x7266CA),
    ]
    private static let trailerTints = [UIColor(hex: 0xB6D3EF), UIColor(hex: 0xEDB185), UIColor(hex: 0xF888A0)]

    /// `gspin_opacity`: hold bright, fall over 45 %, rest dim, snap back over the last 8 %.
    static func opacity(_ t: Double, dim: Double = 0.1) -> Double {
        let t = t - floor(t)
        if t < 0.45 { return 1 + (dim - 1) * t / 0.45 }
        if t < 0.92 { return dim }
        return dim + (1 - dim) * (t - 0.92) / 0.08
    }

    private func rebuild() {
        cells.forEach { $0.removeFromSuperlayer() }
        cells.removeAll()
        checkLayer.isHidden = true
        switch kind {
        case .none:
            break
        case .spinner:
            for _ in 0..<6 { cells.append(CALayer()) }
        case .trailer:
            for _ in 0..<9 { cells.append(CALayer()) }
        case .dot:
            cells.append(CALayer())
        case .check:
            checkLayer.isHidden = false
        }
        cells.forEach { layer.addSublayer($0) }
        restyle()
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let c = CGPoint(x: bounds.midX, y: bounds.midY)
        switch kind {
        case .spinner:
            let d: CGFloat = 3.5, gap: CGFloat = 1.5
            let ox = c.x - (d * 2 + gap) / 2, oy = c.y - (d * 3 + gap * 2) / 2
            for (i, cell) in cells.enumerated() {
                cell.frame = CGRect(x: ox + CGFloat(i % 2) * (d + gap), y: oy + CGFloat(i / 2) * (d + gap), width: d, height: d)
                cell.cornerRadius = d / 2
            }
        case .trailer:
            let d: CGFloat = 3.5, gap: CGFloat = 1.75
            let side = d * 3 + gap * 2
            for (i, cell) in cells.enumerated() {
                cell.frame = CGRect(x: c.x - side / 2 + CGFloat(i % 3) * (d + gap), y: c.y - side / 2 + CGFloat(i / 3) * (d + gap), width: d, height: d)
                cell.cornerRadius = d / 2
            }
        case .dot:
            cells.first?.frame = CGRect(x: c.x - 3.5, y: c.y - 3.5, width: 7, height: 7)
            cells.first?.cornerRadius = 3.5
        case .check:
            // check.svg: M3.5 8.5 l3 3 6-7 on a 16 grid, drawn at 12 pt.
            let s = min(bounds.width, bounds.height) / 16 * 1.05
            let o = CGPoint(x: c.x - 8 * s, y: c.y - 8 * s)
            let p = UIBezierPath()
            p.move(to: CGPoint(x: o.x + 3.5 * s, y: o.y + 8.5 * s))
            p.addLine(to: CGPoint(x: o.x + 6.5 * s, y: o.y + 11.5 * s))
            p.addLine(to: CGPoint(x: o.x + 12.5 * s, y: o.y + 4.5 * s))
            checkLayer.frame = bounds
            checkLayer.path = p.cgPath
        case .none:
            break
        }
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        restyle()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil { restyle() }
    }

    private func restyle() {
        let reduce = UIAccessibility.isReduceMotionEnabled
        switch kind {
        case .none:
            break
        case let .dot(color):
            cells.first?.backgroundColor = color.resolvedColor(with: traitCollection).cgColor
        case let .check(color):
            checkLayer.strokeColor = color.resolvedColor(with: traitCollection).cgColor
        case .spinner, .trailer:
            let spinner = kind == .spinner
            for (i, cell) in cells.enumerated() {
                let row = spinner ? i / 2 : i / 3
                let col = spinner ? i % 2 : i % 3
                let tint = spinner ? Self.glyphTints[row] : Self.trailerTints[row]
                cell.backgroundColor = tint.resolvedColor(with: traitCollection).cgColor
                let phase = spinner ? Double(Self.ring[row][col]) / 6 : Double(2 - row + abs(col - 1)) / 4
                cell.removeAllAnimations()
                cell.opacity = Float(Self.opacity(phase))
                guard !reduce else { continue }
                let a = CAKeyframeAnimation(keyPath: "opacity")
                a.values = [1, 0.1, 0.1, 1]
                a.keyTimes = [0, 0.45, 0.92, 1]
                a.duration = 0.75
                a.repeatCount = .infinity
                a.timeOffset = phase * 0.75
                a.isRemovedOnCompletion = false
                cell.add(a, forKey: "spin")
            }
        }
    }
}

/// The desktop's pull-request badge: the PR icon and the bare number in
/// mono, tinted by state (tone @ 0.08 fill, tone @ 0.85 ink).
final class PRBadgeView: UIView {
    private let icon = UIImageView()
    private let label = UILabel()
    private var fontSize: CGFloat = 11

    override init(frame: CGRect) {
        super.init(frame: frame)
        layer.cornerCurve = .continuous
        icon.contentMode = .center
        addSubview(icon)
        addSubview(label)
        isUserInteractionEnabled = false
    }

    required init?(coder: NSCoder) { fatalError() }

    static func tone(_ state: SessionRowVM.PR) -> UIColor {
        switch state {
        case .open: Palette.success
        case .merged: Palette.accent
        case .closed: Palette.danger
        case .draft: Palette.secondary
        }
    }

    func configure(number: UInt64, state: SessionRowVM.PR, size: CGFloat = 11) {
        fontSize = size
        let tone = Self.tone(state)
        backgroundColor = tone.withAlphaComponent(0.08)
        icon.image = PRIcon.image(side: size)
        icon.tintColor = tone.withAlphaComponent(0.85)
        label.font = Fonts.ui(.monoMedium, size)
        label.textColor = tone.withAlphaComponent(0.85)
        label.text = "\(number)"
        let word = switch state { case .open: "open"; case .merged: "merged"; case .closed: "closed"; case .draft: "draft" }
        accessibilityLabel = "Pull request \(number), \(word)"
        invalidateIntrinsicContentSize()
        setNeedsLayout()
    }

    override var intrinsicContentSize: CGSize {
        let pad = (fontSize * 0.45).rounded()
        let lw = ceil(label.sizeThatFits(CGSize(width: 200, height: 40)).width)
        return CGSize(width: pad * 2 + fontSize + 3 + lw, height: (fontSize * 1.65).rounded())
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        let pad = (fontSize * 0.45).rounded()
        layer.cornerRadius = (fontSize * 0.45).rounded()
        icon.frame = CGRect(x: pad, y: 0, width: fontSize, height: bounds.height)
        label.frame = CGRect(x: icon.frame.maxX + 3, y: 0, width: bounds.width - icon.frame.maxX - 3 - pad + 1, height: bounds.height)
    }
}

/// `pull-request.svg` from the desktop (24 grid, 1.5 stroke), as a template image.
enum PRIcon {
    private static var cache: [CGFloat: UIImage] = [:]

    static func image(side: CGFloat) -> UIImage {
        if let hit = cache[side] { return hit }
        let s = side / 24
        let img = UIGraphicsImageRenderer(size: CGSize(width: side, height: side)).image { ctx in
            let p = UIBezierPath()
            for c in [CGPoint(x: 6, y: 5), CGPoint(x: 6, y: 19), CGPoint(x: 18, y: 19)] {
                p.append(UIBezierPath(arcCenter: CGPoint(x: c.x * s, y: c.y * s), radius: 2.25 * s, startAngle: 0, endAngle: .pi * 2, clockwise: true))
            }
            p.move(to: CGPoint(x: 6 * s, y: 7.25 * s))
            p.addLine(to: CGPoint(x: 6 * s, y: 16.75 * s))
            p.move(to: CGPoint(x: 15 * s, y: 5 * s))
            p.addLine(to: CGPoint(x: 15.75 * s, y: 5 * s))
            p.addArc(withCenter: CGPoint(x: 15.75 * s, y: 7.25 * s), radius: 2.25 * s, startAngle: -.pi / 2, endAngle: 0, clockwise: true)
            p.addLine(to: CGPoint(x: 18 * s, y: 16.75 * s))
            p.move(to: CGPoint(x: 12.75 * s, y: 7.75 * s))
            p.addLine(to: CGPoint(x: 15.25 * s, y: 5 * s))
            p.addLine(to: CGPoint(x: 12.75 * s, y: 2.25 * s))
            p.lineWidth = max(1, 1.5 * s * 1.15)
            p.lineCapStyle = .round
            p.lineJoinStyle = .round
            UIColor.black.setStroke()
            p.stroke()
            _ = ctx
        }.withRenderingMode(.alwaysTemplate)
        cache[side] = img
        return img
    }
}
