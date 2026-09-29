import UIKit

/// Overflow fades. Content that doesn't fit fades out at the edge it runs
/// past instead of being cut or ellipsized — an alpha mask, so it works over
/// glass and any background.
enum EdgeFade {
    static let width: CGFloat = 24

    /// A horizontal alpha mask: `leading`/`trailing` fade widths (0 = hard edge).
    static func mask(_ mask: CAGradientLayer, bounds: CGRect, leading: CGFloat, trailing: CGFloat) {
        let w = max(bounds.width, 1)
        mask.startPoint = CGPoint(x: 0, y: 0.5)
        mask.endPoint = CGPoint(x: 1, y: 0.5)
        mask.colors = [UIColor.clear.cgColor, UIColor.black.cgColor, UIColor.black.cgColor, UIColor.clear.cgColor]
        let a = min(leading / w, 0.5)
        let b = max(1 - trailing / w, 0.5)
        mask.locations = [0, NSNumber(value: Double(a)), NSNumber(value: Double(b)), 1]
        mask.frame = bounds
    }
}

/// A single-line label that fades its tail instead of truncating with "…".
final class FadingLabel: UILabel {
    private let fade = CAGradientLayer()
    /// Alignment while the text fits; overflowing text starts at the leading
    /// edge so the fade lands on its tail.
    var fitsAlignment: NSTextAlignment = .natural { didSet { setNeedsLayout() } }

    override init(frame: CGRect) {
        super.init(frame: frame)
        lineBreakMode = .byClipping
        numberOfLines = 1
    }

    required init?(coder: NSCoder) { fatalError() }

    override var text: String? { didSet { setNeedsLayout() } }
    override var attributedText: NSAttributedString? { didSet { setNeedsLayout() } }
    override var font: UIFont! { didSet { setNeedsLayout() } }

    override func layoutSubviews() {
        super.layoutSubviews()
        let overflows = bounds.width > 0 && super.sizeThatFits(CGSize(width: CGFloat.greatestFiniteMagnitude, height: bounds.height)).width > bounds.width + 0.5
        let alignment: NSTextAlignment = overflows ? .natural : fitsAlignment
        if textAlignment != alignment { textAlignment = alignment }
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        if overflows {
            let rtl = effectiveUserInterfaceLayoutDirection == .rightToLeft
            EdgeFade.mask(fade, bounds: bounds, leading: rtl ? EdgeFade.width : 0, trailing: rtl ? 0 : EdgeFade.width)
            layer.mask = fade
        } else {
            layer.mask = nil
        }
        CATransaction.commit()
    }
}

/// A horizontal scroller whose edges fade while there's more content that
/// way (none at rest on the leading edge, none once scrolled to the end).
class FadingScrollView: UIScrollView {
    private let fade = CAGradientLayer()
    var fadeWidth: CGFloat = EdgeFade.width

    override func layoutSubviews() {
        super.layoutSubviews()
        updateFade()
    }

    func updateFade() {
        let maxX = contentSize.width + contentInset.left + contentInset.right - bounds.width
        guard maxX > 0.5 else {
            if layer.mask != nil { layer.mask = nil }
            return
        }
        let x = contentOffset.x + contentInset.left
        // Ramp the fade in over the first few points of travel so it never pops.
        let leading = fadeWidth * min(1, max(0, x / fadeWidth))
        let trailing = fadeWidth * min(1, max(0, (maxX - x) / fadeWidth))
        CATransaction.begin()
        CATransaction.setDisableActions(true)
        EdgeFade.mask(fade, bounds: bounds, leading: leading, trailing: trailing)
        if layer.mask !== fade { layer.mask = fade }
        CATransaction.commit()
    }
}

/// Content fading into the background at a screen edge (below the composer):
/// clear at the top of the view → background, eased so there's no visible
/// band. Pass-through for touches; a plain gradient layer, so it composites
/// for free while content scrolls underneath.
final class EdgeFadeOverlay: UIView {
    override class var layerClass: AnyClass { CAGradientLayer.self }
    private var gradient: CAGradientLayer { layer as! CAGradientLayer }

    override init(frame: CGRect) {
        super.init(frame: frame)
        isUserInteractionEnabled = false
        gradient.startPoint = CGPoint(x: 0.5, y: 0)
        gradient.endPoint = CGPoint(x: 0.5, y: 1)
        restyle()
        registerForTraitChanges([UITraitUserInterfaceStyle.self]) { (self: EdgeFadeOverlay, _) in self.restyle() }
    }

    required init?(coder: NSCoder) { fatalError() }

    override func didMoveToWindow() {
        super.didMoveToWindow()
        if window != nil { restyle() }
    }

    private func restyle() {
        let bg = Palette.background.resolvedColor(with: traitCollection)
        // Smoothstep-ish ramp over the first ~45%, then solid.
        let stops: [(CGFloat, CGFloat)] = [(0, 0), (0.12, 0.18), (0.24, 0.5), (0.36, 0.82), (0.46, 0.96), (0.55, 1), (1, 1)]
        gradient.colors = stops.map { bg.withAlphaComponent($0.1).cgColor }
        gradient.locations = stops.map { NSNumber(value: Double($0.0)) }
    }
}
