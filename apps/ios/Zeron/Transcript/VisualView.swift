import CoreText
import UIKit

/// An agent visual in the transcript. Its Rust worker (`VisualSession`) runs
/// the script, lays it out, draws its canvases and charts and renders its
/// scenes and shaders; this view only shows what arrives and forwards
/// touches. Paint is rasterized on the worker's thread, so the main thread
/// just swaps layer contents.
final class VisualView: UIView {
    let session: VisualSession
    var openURL: ((URL) -> Void)?

    private let sink: VisualSink
    private let base = CALayer()
    private var surfaces: [UInt64: VisualSurfaceView] = [:]
    private var controls: [UInt64: VisualControlView] = [:]
    private(set) var display: VisualDisplay?
    private var attached = false
    private var foreground = UIApplication.shared.applicationState != .background
    private var observers: [NSObjectProtocol] = []

    init(session: VisualSession, scale: CGFloat) {
        self.session = session
        sink = VisualSink(scale: scale)
        super.init(frame: .zero)
        sink.view = self
        base.contentsGravity = .topLeft
        base.contentsScale = scale
        layer.addSublayer(base)
        clipsToBounds = false
        accessibilityLabel = session.title()
        accessibilityIdentifier = "transcript-visual"
        let center = NotificationCenter.default
        observers = [
            center.addObserver(forName: UIApplication.didEnterBackgroundNotification, object: nil, queue: .main) { [weak self] _ in
                self?.foreground = false
                self?.updateAttachment()
            },
            center.addObserver(forName: UIApplication.willEnterForegroundNotification, object: nil, queue: .main) { [weak self] _ in
                self?.foreground = true
                self?.updateAttachment()
            },
        ]
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    /// Whether the row showing this view is on screen (pooled rows stay in
    /// the window, hidden).
    var active = true {
        didSet { if active != oldValue { updateAttachment() } }
    }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        // Rows re-add their widgets on every configure: settle on the next
        // turn of the run loop, so a move between rows doesn't detach.
        DispatchQueue.main.async { [weak self] in self?.updateAttachment() }
    }

    /// Frames flow only while the visual can be seen.
    private func updateAttachment() {
        let wanted = window != nil && active && foreground
        guard wanted != attached else { return }
        attached = wanted
        if wanted {
            session.attach(listener: sink, dark: traitCollection.userInterfaceStyle == .dark, reduceMotion: UIAccessibility.isReduceMotionEnabled)
        } else {
            session.detach()
        }
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        if previous?.userInterfaceStyle != traitCollection.userInterfaceStyle {
            session.setAppearance(dark: traitCollection.userInterfaceStyle == .dark)
        }
    }

    /// A new layout: the static paint, surface frames and touch targets.
    fileprivate func apply(_ d: VisualDisplay, image: CGImage?) {
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        defer { CATransaction.commit() }
        display = d
        base.frame = CGRect(x: 0, y: 0, width: CGFloat(d.width), height: CGFloat(d.height))
        base.contents = image

        var liveSurfaces = Set<UInt64>()
        for s in d.surfaces {
            liveSurfaces.insert(s.id)
            let view = surfaces[s.id] ?? {
                let v = VisualSurfaceView(id: s.id, session: session)
                insertSubview(v, at: 0)
                surfaces[s.id] = v
                return v
            }()
            view.frame = CGRect(x: CGFloat(s.x), y: CGFloat(s.y), width: CGFloat(s.w), height: CGFloat(s.h))
            view.gesture = s.gesture
        }
        for (id, view) in surfaces where !liveSurfaces.contains(id) {
            view.removeFromSuperview()
            surfaces[id] = nil
        }

        var liveControls = Set<UInt64>()
        for c in d.controls {
            liveControls.insert(c.id)
            let view = controls[c.id] ?? {
                let v = VisualControlView(id: c.id, session: session)
                addSubview(v)
                controls[c.id] = v
                return v
            }()
            view.frame = CGRect(x: CGFloat(c.x), y: CGFloat(c.y), width: CGFloat(c.w), height: CGFloat(c.h))
            view.configure(c, openURL: { [weak self] url in self?.openURL?(url) })
        }
        for (id, view) in controls where !liveControls.contains(id) {
            view.removeFromSuperview()
            controls[id] = nil
        }
        // Later controls above earlier ones, as laid out (a tappable stack
        // under the controls inside it).
        for c in d.controls {
            if let view = controls[c.id] { bringSubviewToFront(view) }
        }
    }

    fileprivate func apply(frame id: UInt64, image: CGImage?, paint: CGImage?) {
        guard let view = surfaces[id] else { return }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        view.show(image: image, paint: paint)
        CATransaction.commit()
    }

    override var accessibilityElements: [Any]? {
        get { controls.values.sorted { ($0.frame.minY, $0.frame.minX) < ($1.frame.minY, $1.frame.minX) } }
        set {}
    }
}

/// Receives the worker's output on its thread, rasterizes it there, and
/// hands the main thread only the latest of each.
private final class VisualSink: VisualListener, @unchecked Sendable {
    weak var view: VisualView?
    private let scale: CGFloat
    private let fonts = StyleFonts()
    private let lock = NSLock()
    private var sizes: [UInt64: CGSize] = [:]
    private var pendingDisplay: (VisualDisplay, CGImage?)?
    private var pendingFrames: [UInt64: (CGImage?, CGImage?)] = [:]
    private var scheduled = false

    init(scale: CGFloat) {
        self.scale = scale
    }

    func display(display: VisualDisplay) {
        fonts.update(display.styles)
        let size = CGSize(width: CGFloat(display.width), height: CGFloat(display.height))
        let image = VisualPainter.render(display.paint, size: size, scale: scale, fonts: fonts)
        lock.lock()
        sizes = Dictionary(uniqueKeysWithValues: display.surfaces.map { ($0.id, CGSize(width: CGFloat($0.w), height: CGFloat($0.h))) })
        pendingDisplay = (display, image)
        schedule()
        lock.unlock()
    }

    func frame(frame: SurfaceFrame) {
        fonts.update(frame.styles)
        lock.lock()
        let size = sizes[frame.surface]
        lock.unlock()
        guard let size else { return }
        let image = frame.image.flatMap(VisualPainter.image)
        let paint = frame.paint.shapes.isEmpty && frame.paint.runs.isEmpty
            ? nil
            : VisualPainter.render(frame.paint, size: size, scale: scale, fonts: fonts)
        lock.lock()
        pendingFrames[frame.surface] = (image, paint)
        schedule()
        lock.unlock()
    }

    /// Called with the lock held.
    private func schedule() {
        guard !scheduled else { return }
        scheduled = true
        DispatchQueue.main.async { [self] in
            lock.lock()
            let display = pendingDisplay
            let frames = pendingFrames
            pendingDisplay = nil
            pendingFrames = [:]
            scheduled = false
            lock.unlock()
            guard let view else { return }
            if let (d, image) = display { view.apply(d, image: image) }
            for (id, (image, paint)) in frames { view.apply(frame: id, image: image, paint: paint) }
        }
    }
}

/// Draws Rust-placed shapes and runs. Pure CoreGraphics: safe off the main
/// thread.
enum VisualPainter {
    static func color(_ c: Rgba) -> CGColor {
        CGColor(srgbRed: CGFloat(c.r), green: CGFloat(c.g), blue: CGFloat(c.b), alpha: CGFloat(c.a))
    }

    private static let space = CGColorSpace(name: CGColorSpace.sRGB)!
    private static let bitmap = CGImageAlphaInfo.premultipliedFirst.rawValue | CGBitmapInfo.byteOrder32Little.rawValue

    /// A worker frame (premultiplied BGRA) as an image, without copying.
    static func image(_ frame: FrameImage) -> CGImage? {
        guard frame.width > 0, frame.height > 0,
              let provider = CGDataProvider(data: frame.bgra as CFData)
        else { return nil }
        return CGImage(
            width: Int(frame.width), height: Int(frame.height), bitsPerComponent: 8, bitsPerPixel: 32,
            bytesPerRow: Int(frame.width) * 4, space: space, bitmapInfo: CGBitmapInfo(rawValue: bitmap),
            provider: provider, decode: nil, shouldInterpolate: true, intent: .defaultIntent
        )
    }

    static func render(_ paint: VisualPaint, size: CGSize, scale: CGFloat, fonts: StyleFonts) -> CGImage? {
        let width = Int((size.width * scale).rounded(.up))
        let height = Int((size.height * scale).rounded(.up))
        guard width > 0, height > 0,
              let ctx = CGContext(data: nil, width: width, height: height, bitsPerComponent: 8, bytesPerRow: 0, space: space, bitmapInfo: bitmap)
        else { return nil }
        // Top-left origin in points, like the display list.
        ctx.translateBy(x: 0, y: CGFloat(height))
        ctx.scaleBy(x: scale, y: -scale)
        draw(paint, in: ctx, fonts: fonts)
        return ctx.makeImage()
    }

    static func draw(_ paint: VisualPaint, in ctx: CGContext, fonts: StyleFonts) {
        ctx.setLineJoin(.round)
        ctx.setLineCap(.round)
        for shape in paint.shapes {
            switch shape {
            case let .rect(x, y, w, h, radius, c):
                let rect = CGRect(x: CGFloat(x), y: CGFloat(y), width: CGFloat(w), height: CGFloat(h))
                let r = min(CGFloat(radius), rect.width / 2, rect.height / 2)
                ctx.setFillColor(color(c))
                if r > 0 {
                    ctx.addPath(CGPath(roundedRect: rect, cornerWidth: r, cornerHeight: r, transform: nil))
                    ctx.fillPath()
                } else {
                    ctx.fill(rect)
                }
            case let .path(points, closed, fill, stroke, lineWidth, dash):
                guard points.count >= 4 else { continue }
                let path = CGMutablePath()
                path.move(to: CGPoint(x: CGFloat(points[0]), y: CGFloat(points[1])))
                var i = 2
                while i + 1 < points.count {
                    path.addLine(to: CGPoint(x: CGFloat(points[i]), y: CGFloat(points[i + 1])))
                    i += 2
                }
                if closed { path.closeSubpath() }
                if let fill {
                    ctx.addPath(path)
                    ctx.setFillColor(color(fill))
                    ctx.fillPath()
                }
                if let stroke {
                    ctx.addPath(path)
                    ctx.setStrokeColor(color(stroke))
                    ctx.setLineWidth(CGFloat(lineWidth))
                    if let dash { ctx.setLineDash(phase: 0, lengths: [CGFloat(dash), CGFloat(dash)]) }
                    ctx.strokePath()
                    if dash != nil { ctx.setLineDash(phase: 0, lengths: []) }
                }
            }
        }
        let text = paint.text as NSString
        for run in paint.runs {
            let range = NSRange(location: Int(run.start), length: Int(run.len))
            guard range.location + range.length <= text.length, let font = fonts.font(run.style) else { continue }
            let s = NSAttributedString(string: text.substring(with: range), attributes: [
                .font: font.font,
                .ligature: font.ligatures ? 1 : 0,
                .foregroundColor: color(run.color),
            ])
            let line = CTLineCreateWithAttributedString(s)
            ctx.saveGState()
            ctx.textMatrix = .identity
            ctx.translateBy(x: CGFloat(run.x), y: CGFloat(run.baseline))
            ctx.scaleBy(x: 1, y: -1)
            ctx.textPosition = .zero
            CTLineDraw(line, ctx)
            ctx.restoreGState()
        }
    }
}

/// A canvas, chart, scene or shader: the worker's image under its vector
/// paint, taking touches the way the surface asks.
private final class VisualSurfaceView: UIView, UIGestureRecognizerDelegate {
    let id: UInt64
    private weak var session: VisualSession?
    private let imageLayer = CALayer()
    private let paintLayer = CALayer()
    private var recognizers: [UIGestureRecognizer] = []

    var gesture: SurfaceGesture = .none {
        didSet { if gesture != oldValue || recognizers.isEmpty { installGestures() } }
    }

    init(id: UInt64, session: VisualSession) {
        self.id = id
        self.session = session
        super.init(frame: .zero)
        for l in [imageLayer, paintLayer] {
            l.contentsGravity = .resize
            layer.addSublayer(l)
        }
        installGestures()
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        imageLayer.frame = bounds
        paintLayer.frame = bounds
        CATransaction.commit()
    }

    func show(image: CGImage?, paint: CGImage?) {
        imageLayer.contents = image
        paintLayer.contents = paint
    }

    private func installGestures() {
        recognizers.forEach(removeGestureRecognizer)
        recognizers = []
        isUserInteractionEnabled = gesture != .none
        switch gesture {
        case .none:
            break
        case .tap:
            let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
            recognizers = [tap]
        case .drag:
            // Tracks from touch-down, so drags here never scroll the transcript.
            let press = UILongPressGestureRecognizer(target: self, action: #selector(tracked(_:)))
            press.minimumPressDuration = 0
            press.allowableMovement = .greatestFiniteMagnitude
            recognizers = [press]
        case .horizontalDrag:
            let pan = UIPanGestureRecognizer(target: self, action: #selector(tracked(_:)))
            pan.delegate = self
            let tap = UITapGestureRecognizer(target: self, action: #selector(tapped(_:)))
            recognizers = [pan, tap]
        }
        recognizers.forEach(addGestureRecognizer)
    }

    /// Mostly-horizontal drags scrub; vertical ones scroll the transcript.
    /// (UIKit asks this view about ancestors' recognizers too: only judge
    /// our own.)
    override func gestureRecognizerShouldBegin(_ g: UIGestureRecognizer) -> Bool {
        guard let pan = g as? UIPanGestureRecognizer, pan.view === self, gesture == .horizontalDrag else {
            return super.gestureRecognizerShouldBegin(g)
        }
        let v = pan.velocity(in: self)
        return abs(v.x) > abs(v.y)
    }

    private func send(_ phase: PointerPhase, _ p: CGPoint) {
        session?.pointer(surface: id, phase: phase, x: Float(p.x), y: Float(p.y))
    }

    private var readoutTimer: Timer?

    @objc private func tapped(_ g: UITapGestureRecognizer) {
        let p = g.location(in: self)
        send(.down, p)
        guard gesture == .horizontalDrag else { return send(.up, p) }
        // A tap on a chart shows its readout for a moment.
        readoutTimer?.invalidate()
        readoutTimer = Timer.scheduledTimer(withTimeInterval: 2.5, repeats: false) { [weak self] _ in
            self?.send(.up, p)
        }
    }

    @objc private func tracked(_ g: UIGestureRecognizer) {
        let p = g.location(in: self)
        switch g.state {
        case .began: send(.down, p)
        case .changed: send(.move, p)
        case .ended: send(.up, p)
        case .cancelled, .failed: send(.cancel, p)
        default: break
        }
    }
}

/// A touch target over control art the worker painted: buttons, tabs,
/// toggles, sliders, links and tappable stacks.
private final class VisualControlView: UIControl {
    let id: UInt64
    private weak var session: VisualSession?
    private var kind: ControlKind = .tap
    private var openURL: ((URL) -> Void)?
    private let wash = CALayer()

    init(id: UInt64, session: VisualSession) {
        self.id = id
        self.session = session
        super.init(frame: .zero)
        wash.backgroundColor = Palette.controlFill.cgColor
        wash.opacity = 0
        wash.cornerCurve = .continuous
        layer.addSublayer(wash)
        isAccessibilityElement = true
        addAction(UIAction { [weak self] _ in self?.activated() }, for: .touchUpInside)
    }

    required init?(coder: NSCoder) { fatalError() }

    func configure(_ c: VisualControl, openURL: @escaping (URL) -> Void) {
        kind = c.kind
        self.openURL = openURL
        accessibilityLabel = c.label
        accessibilityValue = nil
        switch c.kind {
        case .button, .tap:
            accessibilityTraits = .button
        case let .tab(selected):
            accessibilityTraits = selected ? [.button, .selected] : .button
        case let .toggle(on):
            accessibilityTraits = .button
            accessibilityValue = on ? "On" : "Off"
        case let .slider(_, _, _, _, _, _, _, shown):
            accessibilityTraits = .adjustable
            accessibilityValue = shown
        case .link:
            accessibilityTraits = .link
        }
        let radius: CGFloat = switch c.kind {
        case .tap: 6
        case .button, .tab: 8
        default: 0
        }
        wash.cornerRadius = radius
        setNeedsLayout()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        wash.frame = bounds
    }

    override func traitCollectionDidChange(_ previous: UITraitCollection?) {
        super.traitCollectionDidChange(previous)
        wash.backgroundColor = Palette.controlFill.cgColor
    }

    override var isHighlighted: Bool {
        didSet {
            switch kind {
            case .slider, .toggle: return
            default: wash.opacity = isHighlighted ? 1 : 0
            }
        }
    }

    /// A finger-sized target around small art.
    override func point(inside point: CGPoint, with event: UIEvent?) -> Bool {
        let dy = max(0, (44 - bounds.height) / 2)
        return bounds.insetBy(dx: -4, dy: -dy).contains(point)
    }

    private func activated() {
        switch kind {
        case .button, .tap:
            session?.activate(control: id)
        case .tab, .toggle:
            UISelectionFeedbackGenerator().selectionChanged()
            session?.activate(control: id)
        case let .link(url):
            if let url = URL(string: url) { openURL?(url) }
        case .slider:
            break
        }
    }

    // Sliders track the finger along their track (in the visual's points).
    override func beginTracking(_ touch: UITouch, with event: UIEvent?) -> Bool {
        if case .slider = kind { slide(to: touch) }
        return true
    }

    override func continueTracking(_ touch: UITouch, with event: UIEvent?) -> Bool {
        if case .slider = kind { slide(to: touch) }
        return true
    }

    private func slide(to touch: UITouch) {
        guard case let .slider(_, _, _, _, _, trackX, trackW, _) = kind, let parent = superview, trackW > 0 else { return }
        let x = Float(touch.location(in: parent).x)
        session?.slide(control: id, fraction: max(0, min(1, (x - trackX) / trackW)))
    }

    override func accessibilityIncrement() { nudge(1) }
    override func accessibilityDecrement() { nudge(-1) }

    private func nudge(_ direction: Float) {
        guard case let .slider(_, min, max, step, fraction, _, _, _) = kind, max > min else { return }
        let by = step > 0 ? Float(step / (max - min)) : 0.1
        session?.slide(control: id, fraction: Swift.max(0, Swift.min(1, fraction + direction * by)))
    }
}
