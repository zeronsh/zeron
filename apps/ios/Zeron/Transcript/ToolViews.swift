import UIKit

/// Desktop motion curves (`ui/src/motion.rs`).
enum Motion {
    /// `EASE_OUT` cubic-bezier(0, 0, 0.58, 1) — folds, chevrons (140 ms).
    static let fold: TimeInterval = 0.14
    /// `EASE_OUT_EXPO` (0.16, 1, 0.3, 1) — new tool row height reveal (360 ms).
    static let rowReveal: TimeInterval = 0.36
    static let expo = UICubicTimingParameters(controlPoint1: CGPoint(x: 0.16, y: 1), controlPoint2: CGPoint(x: 0.3, y: 1))
    /// Connector draw: 480 ms `EASE_OUT_QUINT` (0.22, 1, 0.36, 1).
    static let connector: Double = 0.48
    /// Row start stagger / first-row lead-in of a brand-new group.
    static let stagger: Double = 0.065
    static let leadIn: Double = 0.09

    /// cubic-bezier(0.22, 1, 0.36, 1) evaluated at time `t` (0…1).
    static func quintOut(_ t: Double) -> Double {
        bezier(t, 0.22, 1, 0.36, 1)
    }

    private static func bezier(_ t: Double, _ x1: Double, _ y1: Double, _ x2: Double, _ y2: Double) -> Double {
        // Solve x(s) = t by Newton, return y(s).
        func coord(_ s: Double, _ a: Double, _ b: Double) -> Double {
            3 * (1 - s) * (1 - s) * s * a + 3 * (1 - s) * s * s * b + s * s * s
        }
        var s = t
        for _ in 0..<8 {
            let x = coord(s, x1, x2) - t
            let dx = 3 * (1 - s) * (1 - s) * x1 + 6 * (1 - s) * s * (x2 - x1) + 3 * s * s * (1 - x2)
            if abs(dx) < 1e-6 { break }
            s = min(1, max(0, s - x / dx))
        }
        return coord(s, y1, y2)
    }
}

/// The tool group header chevron: `alt-arrow-down`, rotated −90° while
/// collapsed; flips with the 140 ms fold curve.
final class ToolChevronView: UIImageView {
    /// Last painted state per group, so a rebuilt view animates from it.
    private static var lastExpanded: [UInt64: Bool] = [:]

    init(key: UInt64, expanded: Bool) {
        super.init(image: UIImage(named: "tool-alt-arrow-down"))
        tintColor = Palette.color(.textSecondary)
        contentMode = .scaleAspectFit
        isUserInteractionEnabled = false
        let angle = { (open: Bool) in CGAffineTransform(rotationAngle: open ? 0 : -.pi / 2) }
        let previous = Self.lastExpanded[key]
        Self.lastExpanded[key] = expanded
        if let previous, previous != expanded, !UIAccessibility.isReduceMotionEnabled {
            transform = angle(previous)
            DispatchQueue.main.async {
                UIView.animate(withDuration: Motion.fold, delay: 0, options: [.curveEaseOut, .allowUserInteraction]) { self.transform = angle(expanded) }
            }
        } else {
            transform = angle(expanded)
        }
    }

    required init?(coder: NSCoder) { fatalError() }
}

/// The activity rail: a 1pt trunk under the chevron with a quadratic elbow
/// into each row. Rows that arrive while the group is live draw in with the
/// desktop's phased connector (previous trunk → incoming trunk → branch,
/// staggered 65 ms), and their icon + text fade in with the branch.
final class ToolRailView: UIView {
    /// Rows already revealed per group (never re-animate a row).
    private static var revealed: [UInt64: Int] = [:]
    /// Groups on screen when a transcript attaches never animate (desktop).
    static var quietUntil: CFTimeInterval = 0

    private let trunkX: CGFloat, bend: CGFloat, branchEnd: CGFloat, rowMid: CGFloat
    private let tops: [CGFloat], heights: [CGFloat]
    private let rowWidth: CGFloat
    private let color: UIColor

    init(key: UInt64, trunkX: Float, bend: Float, branchEnd: Float, rowMid: Float, tops: [Float], heights: [Float], rowWidth: CGFloat, live: Bool) {
        self.trunkX = CGFloat(trunkX)
        self.bend = CGFloat(bend)
        self.branchEnd = CGFloat(branchEnd)
        self.rowMid = CGFloat(rowMid)
        self.tops = tops.map { CGFloat($0) }
        self.heights = heights.map { CGFloat($0) }
        self.rowWidth = rowWidth
        self.color = Palette.color(.toolRail)
        super.init(frame: .zero)
        isUserInteractionEnabled = false
        clipsToBounds = false
        let count = tops.count
        let known = Self.revealed[key]
        Self.revealed[key] = max(known ?? 0, count)
        let quiet = CACurrentMediaTime() < Self.quietUntil || UIAccessibility.isReduceMotionEnabled
        let from: Int? = if quiet { nil } else if let known { known < count ? known : nil } else { live ? 0 : nil }
        build(animateFrom: from, brandNew: known == nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    private func segments(_ i: Int) -> (incoming: UIBezierPath, branch: UIBezierPath, continuation: UIBezierPath?) {
        let top = tops[i]
        let mid = top + rowMid
        let incoming = UIBezierPath()
        incoming.move(to: CGPoint(x: trunkX, y: top))
        incoming.addLine(to: CGPoint(x: trunkX, y: mid - bend))
        let branch = UIBezierPath()
        branch.move(to: CGPoint(x: trunkX, y: mid - bend))
        branch.addQuadCurve(to: CGPoint(x: trunkX + bend, y: mid), controlPoint: CGPoint(x: trunkX, y: mid))
        branch.addLine(to: CGPoint(x: branchEnd, y: mid))
        var continuation: UIBezierPath?
        if i + 1 < tops.count {
            let c = UIBezierPath()
            c.move(to: CGPoint(x: trunkX, y: mid - bend))
            c.addLine(to: CGPoint(x: trunkX, y: top + heights[i]))
            continuation = c
        }
        return (incoming, branch, continuation)
    }

    private var strokes: [CAShapeLayer] = []
    private var covers: [CALayer] = []

    /// Colors resolve against the traits this view actually lives in: at init
    /// it isn't in the window yet, and its traits are the *system's* — with the
    /// app set to Light on a Dark device that painted a white rail (invisible
    /// on light) and a black cover the rows seemed to fade in from.
    private func restyle() {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        let stroke = color.resolvedColor(with: traitCollection).cgColor
        let bg = Palette.background.resolvedColor(with: traitCollection).cgColor
        for l in strokes { l.strokeColor = stroke }
        for c in covers { c.backgroundColor = bg }
        CATransaction.commit()
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil { restyle() }
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        restyle()
    }

    private func shape(_ path: UIBezierPath) -> CAShapeLayer {
        let l = CAShapeLayer()
        strokes.append(l)
        l.path = path.cgPath
        l.strokeColor = color.resolvedColor(with: traitCollection).cgColor
        l.fillColor = nil
        l.lineWidth = 1
        l.lineCap = .butt
        layer.addSublayer(l)
        return l
    }

    private func build(animateFrom: Int?, brandNew: Bool) {
        let first = animateFrom ?? tops.count
        // Settled rows: one stroked path (joints never double their alpha).
        let settled = UIBezierPath()
        for i in 0..<first {
            let s = segments(i)
            settled.append(s.incoming)
            settled.append(s.branch)
            // The last settled row's trunk extends only once its successor draws.
            if let c = s.continuation, i + 1 < first { settled.append(c) }
        }
        if !settled.isEmpty { _ = shape(settled) }
        guard first < tops.count else { return }
        let now = CACurrentMediaTime()
        for i in first..<tops.count {
            let s = segments(i)
            let start = now + (brandNew ? Motion.leadIn : 0) + Double(i - first) * Motion.stagger
            let isFirst = i == 0
            // Phases on the eased 480 ms progress (desktop T:2619-2637).
            if !isFirst {
                let prev = segments(i - 1)
                if let c = prev.continuation { draw(shape(c), from: 0, to: 0.45, start: start) }
            }
            draw(shape(s.incoming), from: isFirst ? 0 : 0.45, to: isFirst ? 0.62 : 0.72, start: start)
            let branchFrom = isFirst ? 0.58 : 0.68
            draw(shape(s.branch), from: branchFrom, to: 1, start: start)
            // Content (icon + text) fades in with the branch: a background
            // cover over the row's content fades out.
            let cover = CALayer()
            covers.append(cover)
            cover.backgroundColor = Palette.background.resolvedColor(with: traitCollection).cgColor
            cover.frame = CGRect(x: branchEnd + 2, y: tops[i], width: max(0, rowWidth - branchEnd - 2), height: rowMid * 2)
            layer.addSublayer(cover)
            cover.opacity = 0
            let fade = keyframes(keyPath: "opacity", from: branchFrom, to: 1, start: start) { 1 - $0 }
            cover.add(fade, forKey: "reveal")
        }
        // The last settled row's continuation is drawn by its successor's
        // first phase above; with nothing new after it, it never extends.
    }

    /// Stroke a segment over the connector progress window [a, b].
    private func draw(_ layer: CAShapeLayer, from a: Double, to b: Double, start: CFTimeInterval) {
        layer.strokeEnd = 1
        layer.add(keyframes(keyPath: "strokeEnd", from: a, to: b, start: start) { $0 }, forKey: "draw")
    }

    /// Sampled keyframes of `value(local)` where local is the connector
    /// progress (quint-out over 480 ms) mapped into [a, b].
    private func keyframes(keyPath: String, from a: Double, to b: Double, start: CFTimeInterval, value: (Double) -> Double) -> CAKeyframeAnimation {
        let n = 24
        var values: [Double] = []
        var times: [NSNumber] = []
        for k in 0...n {
            let t = Double(k) / Double(n)
            let p = Motion.quintOut(t)
            let local = min(1, max(0, (p - a) / (b - a)))
            values.append(value(local))
            times.append(NSNumber(value: t))
        }
        let anim = CAKeyframeAnimation(keyPath: keyPath)
        anim.values = values
        anim.keyTimes = times
        anim.duration = Motion.connector
        anim.beginTime = start
        anim.fillMode = .both
        anim.isRemovedOnCompletion = true
        return anim
    }
}

/// Shimmer over the live group's title: the same runs re-drawn in `text`
/// through a moving triangle mask (3.4 s sweep, highlights every 3 title
/// widths, half-width 0.36 — desktop T:1996-2087).
final class ShimmerView: UIView {
    private let text = ShimmerText()
    private let sweepMask = CAGradientLayer()

    init(model: RowModel, rect: CGRect) {
        super.init(frame: rect)
        isUserInteractionEnabled = false
        text.model = model
        text.origin = rect.origin
        text.frame = CGRect(origin: .zero, size: rect.size)
        addSubview(text)
        let w = max(rect.width, 1)
        sweepMask.startPoint = CGPoint(x: 0, y: 0.5)
        sweepMask.endPoint = CGPoint(x: 1, y: 0.5)
        let half = 0.36 / 9
        var locations: [NSNumber] = []
        var colors: [CGColor] = []
        for k in 0..<3 {
            let c = (1.5 + 3 * Double(k)) / 9
            for (dx, a) in [(-half, 0.0), (0, 1.0), (half, 0.0)] {
                locations.append(NSNumber(value: c + dx))
                colors.append(UIColor.black.withAlphaComponent(a).cgColor)
            }
        }
        sweepMask.colors = colors
        sweepMask.locations = locations
        sweepMask.frame = CGRect(x: -6 * w, y: 0, width: 9 * w, height: rect.height)
        layer.mask = sweepMask
        guard !UIAccessibility.isReduceMotionEnabled else {
            isHidden = true
            return
        }
        let sweep = CABasicAnimation(keyPath: "position.x")
        sweep.fromValue = -6 * w + 4.5 * w
        sweep.toValue = 4.5 * w
        sweep.duration = 3.4
        sweep.repeatCount = .infinity
        sweep.timingFunction = CAMediaTimingFunction(name: .linear)
        sweep.isRemovedOnCompletion = false
        sweepMask.add(sweep, forKey: "sweep")
    }

    required init?(coder: NSCoder) { fatalError() }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        // Core Animation drops repeat animations when a view leaves a window.
        if window != nil, sweepMask.animation(forKey: "sweep") == nil, !UIAccessibility.isReduceMotionEnabled {
            let w = max(bounds.width, 1)
            let sweep = CABasicAnimation(keyPath: "position.x")
            sweep.fromValue = -6 * w + 4.5 * w
            sweep.toValue = 4.5 * w
            sweep.duration = 3.4
            sweep.repeatCount = .infinity
            sweep.isRemovedOnCompletion = false
            sweepMask.add(sweep, forKey: "sweep")
        }
    }
}

private final class ShimmerText: UIView {
    var model: RowModel?
    var origin: CGPoint = .zero

    override init(frame: CGRect) {
        super.init(frame: frame)
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
    }

    required init?(coder: NSCoder) { fatalError() }

    override func draw(_ rect: CGRect) {
        guard let model, let ctx = UIGraphicsGetCurrentContext() else { return }
        ctx.translateBy(x: -origin.x, y: -origin.y)
        model.drawRuns(in: CGRect(origin: origin, size: bounds.size), color: Palette.text, ctx: ctx, traits: traitCollection)
    }
}

/// Tap target for one tool row's inline detail.
final class ToolToggleControl: UIControl {
    override var isHighlighted: Bool {
        didSet { backgroundColor = isHighlighted ? Palette.controlFill : .clear }
    }
}
