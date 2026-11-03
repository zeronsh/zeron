import UIKit

/// The desktop voice orb (`zeron-orb`): the core runs the same geometry,
/// clock, audio response and 300 ms state crossfades as the GPUI widget, and
/// this view only paints the frames — lines, then disks back to front, in
/// monochrome ink on a transparent canvas.
///
/// Redraws at 30 fps (the desktop rate) while on screen; hidden, off-window or
/// backgrounded it costs nothing and its clock stops, so motion resumes where
/// it left off. Reduce Motion shows the desktop's static frame.
final class OrbView: UIView {
    var orb: VoiceOrb {
        didSet {
            guard orb != oldValue else { return }
            renderer.setOrb(orb: orb)
            if link == nil { redraw(animating: false) }
        }
    }

    private let renderer: OrbRenderer
    private var frameData: OrbFrame?
    private var link: CADisplayLink?
    private var observers: [NSObjectProtocol] = []

    init(preset: OrbPreset, orb: VoiceOrb = .idle) {
        self.orb = orb
        renderer = OrbRenderer(preset: preset, orb: orb)
        super.init(frame: .zero)
        isOpaque = false
        backgroundColor = .clear
        contentMode = .redraw
        isUserInteractionEnabled = false
        isAccessibilityElement = false
        // Resume on didBecomeActive: at willEnterForeground the app still
        // reports .background, so the display link would stay stopped.
        for name in [UIApplication.didEnterBackgroundNotification, UIApplication.didBecomeActiveNotification, UIAccessibility.reduceMotionStatusDidChangeNotification] {
            observers.append(NotificationCenter.default.addObserver(forName: name, object: nil, queue: .main) { [weak self] _ in
                self?.syncLink()
            })
        }
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: OrbView, _) in self.setNeedsDisplay() }
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        link?.invalidate()
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    /// Normalized 0…1 peaks. They quicken the motion, like the desktop orb.
    func setLevels(microphone: Float, speaker: Float) {
        renderer.setAudioLevels(microphone: microphone, speaker: speaker)
    }

    override var isHidden: Bool { didSet { syncLink() } }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        syncLink()
    }

    private var shouldAnimate: Bool {
        window != nil && !isHidden && !UIAccessibility.isReduceMotionEnabled
            && UIApplication.shared.applicationState != .background
    }

    private func syncLink() {
        if shouldAnimate {
            guard link == nil else { return }
            let link = CADisplayLink(target: DisplayLinkProxy(self), selector: #selector(DisplayLinkProxy.tick))
            link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 30, preferred: 30)
            link.add(to: .main, forMode: .common)
            self.link = link
            redraw(animating: true)
        } else {
            link?.invalidate()
            link = nil
            // Stop the clock now, so the hidden time never plays back as a jump.
            if window != nil { redraw(animating: false) }
        }
    }

    fileprivate func tick() { redraw(animating: true) }

    private func redraw(animating: Bool) {
        frameData = renderer.nextFrame(animating: animating, reducedMotion: UIAccessibility.isReduceMotionEnabled)
        setNeedsDisplay()
    }

    override func draw(_ rect: CGRect) {
        guard let frame = frameData, frame.size > 0, let ctx = UIGraphicsGetCurrentContext() else { return }
        let dark = traitCollection.userInterfaceStyle == .dark
        // The preset's artwork, scaled uniformly to fit and centered.
        let scale = min(bounds.width, bounds.height) / CGFloat(frame.size)
        ctx.translateBy(x: (bounds.width - CGFloat(frame.size) * scale) / 2, y: (bounds.height - CGFloat(frame.size) * scale) / 2)
        ctx.scaleBy(x: scale, y: scale)
        ctx.setLineCap(.butt)
        let lines = frame.lines
        var i = 0
        while i + 6 < lines.count {
            ctx.setStrokeColor(gray: ink(lines[i + 5], dark), alpha: CGFloat(min(1, max(0, lines[i + 6]))))
            ctx.setLineWidth(CGFloat(lines[i + 4]))
            ctx.move(to: CGPoint(x: CGFloat(lines[i]), y: CGFloat(lines[i + 1])))
            ctx.addLine(to: CGPoint(x: CGFloat(lines[i + 2]), y: CGFloat(lines[i + 3])))
            ctx.strokePath()
            i += 7
        }
        let dots = frame.dots
        i = 0
        while i + 4 < dots.count {
            let r = CGFloat(dots[i + 2])
            ctx.setFillColor(gray: ink(dots[i + 3], dark), alpha: CGFloat(min(1, max(0, dots[i + 4]))))
            ctx.fillEllipse(in: CGRect(x: CGFloat(dots[i]) - r, y: CGFloat(dots[i + 1]) - r, width: r * 2, height: r * 2))
            i += 5
        }
    }

    /// Ink `white` on paper; dark appearances mirror it (desktop `ink_color`).
    private func ink(_ white: Float, _ dark: Bool) -> CGFloat {
        let w = CGFloat(min(1, max(0, white)))
        return dark ? 1 - w : w
    }
}

/// CADisplayLink retains its target; the proxy keeps the orb releasable.
private final class DisplayLinkProxy: NSObject {
    weak var view: OrbView?
    init(_ view: OrbView) { self.view = view }
    @objc func tick() { MainActor.assumeIsolated { view?.tick() } }
}
