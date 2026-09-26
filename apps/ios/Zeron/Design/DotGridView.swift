import UIKit

/// The 3×3 dot-grid status glyph. Animation is a Core Animation keyframe per
/// dot (a diagonal wave) — rendered by the render server, zero main-thread
/// work per frame, and it pauses itself off-screen.
final class DotGridView: UIView {
    enum Style: Equatable {
        case working
        case awaiting
        case errored
        case idle
    }

    var style: Style {
        didSet { if style != oldValue { restyle() } }
    }

    private var dots: [CALayer] = []

    init(style: Style) {
        self.style = style
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        for _ in 0..<9 {
            let dot = CALayer()
            layer.addSublayer(dot)
            dots.append(dot)
        }
        restyle()
    }

    required init?(coder: NSCoder) { fatalError() }

    override var intrinsicContentSize: CGSize { CGSize(width: 14, height: 14) }

    override func layoutSubviews() {
        super.layoutSubviews()
        let side = min(bounds.width, bounds.height)
        let d = max(1.6, side * 0.17)
        let step = (side - d) / 2
        let ox = (bounds.width - side) / 2
        let oy = (bounds.height - side) / 2
        for (i, dot) in dots.enumerated() {
            dot.frame = CGRect(x: ox + CGFloat(i % 3) * step, y: oy + CGFloat(i / 3) * step, width: d, height: d)
            dot.cornerRadius = d / 2
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
        let color: UIColor
        switch style {
        case .working: color = Palette.secondary
        case .awaiting: color = Palette.warning
        case .errored: color = Palette.danger
        case .idle: color = Palette.tertiary
        }
        let cg = color.resolvedColor(with: traitCollection).cgColor
        for (i, dot) in dots.enumerated() {
            dot.backgroundColor = cg
            dot.removeAllAnimations()
            guard style == .working || style == .awaiting, !UIAccessibility.isReduceMotionEnabled else {
                dot.opacity = style == .idle ? 0.55 : 1
                continue
            }
            let a = CAKeyframeAnimation(keyPath: "opacity")
            a.values = style == .working ? [0.25, 1, 0.25] : [0.45, 1, 0.45]
            a.keyTimes = [0, 0.35, 1]
            a.duration = style == .working ? 1.1 : 1.6
            a.repeatCount = .infinity
            // Diagonal wave: dots on the same anti-diagonal pulse together.
            a.beginTime = CACurrentMediaTime() + Double(i % 3 + i / 3) * (style == .working ? 0.12 : 0.18)
            a.isRemovedOnCompletion = false
            dot.add(a, forKey: "wave")
        }
    }
}
