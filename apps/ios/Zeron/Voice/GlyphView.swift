import UIKit

/// The desktop's voice glyph (`zeron-orb`'s glyph engine): the core decides
/// which dots light, how much and in what colour, exactly as the desktop
/// stage does, and this view only fills them as circles on a transparent
/// canvas.
///
/// `.disc` is a control's glyph; `.stage` is a call's, its dots running to
/// its edge and its rim easing into what holds it; `.beam` is a dictation's
/// track, a strip rising from its bottom as the voice does.
///
/// Redraws at 30 fps while on screen and moving; hidden, off-window or
/// backgrounded it costs nothing and its clock stops. Reduce Motion holds it
/// still.
final class GlyphView: UIView {
    var orb: VoiceOrb {
        didSet {
            guard orb != oldValue else { return }
            renderer.setOrb(orb: orb)
            syncLink()
        }
    }

    /// Each dot's diameter, in points.
    var dot: CGFloat = 6 { didSet { setNeedsDisplay() } }
    /// The radius of the box's corners, which a beam keeps clear of.
    var corner: CGFloat = 0 { didSet { setNeedsDisplay() } }

    private let renderer: GlyphRenderer
    private var frameData: [Float] = []
    private var link: CADisplayLink?
    private var observers: [NSObjectProtocol] = []
    private static let stride = 7
    /// The accent's glyph rows (`StatusGlyph`), light to deep.
    private static let accent: [(UInt32, UInt32)] = [(0x7965EC, 0xABA1F9), (0x5B43E8, 0x8B7CF6), (0x4332AC, 0x7266CA)]

    init(shape: GlyphShape, orb: VoiceOrb = .idle, dot: CGFloat = 6) {
        self.orb = orb
        self.dot = dot
        renderer = GlyphRenderer(shape: shape, orb: orb)
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
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: GlyphView, _) in self.redraw(animating: false) }
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit {
        link?.invalidate()
        observers.forEach(NotificationCenter.default.removeObserver)
    }

    /// Normalized 0…1 peaks of the call's two voices.
    func setLevels(microphone: Float, speaker: Float) {
        renderer.setAudioLevels(microphone: microphone, speaker: speaker)
        syncLink()
    }

    override var isHidden: Bool { didSet { syncLink() } }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        syncLink()
    }

    override func layoutSubviews() {
        super.layoutSubviews()
        redraw(animating: false)
    }

    private var shouldAnimate: Bool {
        window != nil && !isHidden && !UIAccessibility.isReduceMotionEnabled
            && UIApplication.shared.applicationState != .background && renderer.moving()
    }

    private func syncLink() {
        if shouldAnimate {
            guard link == nil else { return }
            let link = CADisplayLink(target: GlyphLinkProxy(self), selector: #selector(GlyphLinkProxy.tick))
            link.preferredFrameRateRange = CAFrameRateRange(minimum: 30, maximum: 30, preferred: 30)
            link.add(to: .main, forMode: .common)
            self.link = link
        } else {
            link?.invalidate()
            link = nil
            if window != nil { redraw(animating: false) }
        }
    }

    fileprivate func tick() {
        redraw(animating: true)
        if !renderer.moving() { syncLink() }
    }

    private func redraw(animating: Bool) {
        guard bounds.width > 0, bounds.height > 0 else { return }
        let dark = traitCollection.userInterfaceStyle == .dark
        let accent = Self.accent.flatMap { light, deep -> [Float] in
            let hex = dark ? deep : light
            return [Float((hex >> 16) & 0xFF) / 255, Float((hex >> 8) & 0xFF) / 255, Float(hex & 0xFF) / 255]
        }
        frameData = renderer.nextFrame(
            width: Float(bounds.width), height: Float(bounds.height), dot: Float(dot), corner: Float(corner),
            rooms: [], accent: accent, animating: animating
        )
        setNeedsDisplay()
    }

    override func draw(_ rect: CGRect) {
        guard let ctx = UIGraphicsGetCurrentContext(), !frameData.isEmpty else { return }
        // One filled circle a dot: light is told by colour, never by glow.
        let data = frameData
        let stride = Self.stride
        var i = 0
        while i + stride <= data.count {
            let size = CGFloat(data[i + 2])
            ctx.setFillColor(red: CGFloat(data[i + 3]), green: CGFloat(data[i + 4]), blue: CGFloat(data[i + 5]), alpha: CGFloat(min(1, max(0, data[i + 6]))))
            ctx.fillEllipse(in: CGRect(x: CGFloat(data[i]) - size / 2, y: CGFloat(data[i + 1]) - size / 2, width: size, height: size))
            i += stride
        }
    }
}


/// CADisplayLink retains its target; the proxy keeps the glyph releasable.
private final class GlyphLinkProxy: NSObject {
    weak var view: GlyphView?
    init(_ view: GlyphView) { self.view = view }
    @objc func tick() { MainActor.assumeIsolated { view?.tick() } }
}
