import UIKit

/// The dictation's track: the voice glyph's beam rising from its bottom as
/// the voice does, dissolving into the recording clock at its end. It unrolls
/// from the action button across the composer's controls while a dictation
/// runs (the desktop composer's `render_voice_track`).
final class DictationTrack: UIView {
    static let height: CGFloat = 34
    /// The beam's dots: the desktop's.
    static let dot: CGFloat = 2.4
    /// Room the beam leaves for the clock, and the width over which it
    /// dissolves into it.
    static let clockRoom: CGFloat = 44
    static let beamFade: CGFloat = 40
    /// The clock warns this long before the limit.
    static let limitWarning: TimeInterval = 10

    let glyph = GlyphView(shape: .beam, orb: .connecting, dot: DictationTrack.dot)
    private let clock = UILabel()
    private let fade = CAGradientLayer()

    init() {
        super.init(frame: .zero)
        backgroundColor = Palette.controlFill
        layer.cornerRadius = Self.height / 2
        layer.cornerCurve = .circular
        clipsToBounds = true
        glyph.corner = Self.height / 2
        clock.font = Fonts.ui(.monoMedium, 11)
        clock.textColor = Palette.secondary
        clock.textAlignment = .right
        addSubview(glyph)
        addSubview(clock)
        accessibilityIdentifier = "dictation-track"
        isAccessibilityElement = true
        accessibilityTraits = .updatesFrequently
    }

    required init?(coder: NSCoder) { fatalError() }

    override func layoutSubviews() {
        super.layoutSubviews()
        let beam = CGRect(x: 0, y: 0, width: max(0, bounds.width - Self.clockRoom), height: bounds.height)
        if glyph.frame != beam { glyph.frame = beam }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        EdgeFade.mask(fade, bounds: glyph.bounds, leading: 0, trailing: Self.beamFade)
        if glyph.layer.mask !== fade { glyph.layer.mask = fade }
        CATransaction.commit()
        clock.frame = CGRect(x: bounds.width - Self.clockRoom, y: 0, width: Self.clockRoom - 12, height: bounds.height)
    }

    /// The clock: `0:07`, in the warning colour near the limit while live,
    /// faint once capture has closed.
    func show(elapsed: TimeInterval?, live: Bool, label: String) {
        if let elapsed {
            let seconds = Int(elapsed)
            clock.text = String(format: "%d:%02d", seconds / 60, seconds % 60)
            let nearLimit = live && elapsed + Self.limitWarning >= DictationDraft.maxSeconds
            clock.textColor = nearLimit ? Palette.warning : live ? Palette.secondary : Palette.tertiary
        } else {
            clock.text = nil
        }
        accessibilityLabel = label
    }
}
