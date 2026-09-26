import UIKit

/// Liquid Glass helpers. Everything uses the system `UIGlassEffect` so the
/// render server does the refraction and morphing — no snapshots, no custom
/// blur stacks, nothing that can stutter on the main thread.
enum Glass {
    static func effect(interactive: Bool = false, tint: UIColor? = nil) -> UIGlassEffect {
        let effect = UIGlassEffect(style: .regular)
        effect.isInteractive = interactive
        effect.tintColor = tint
        return effect
    }

    /// A capsule/rounded glass surface. Add content to `.contentView`.
    static func surface(interactive: Bool = false, radius: CGFloat? = nil) -> UIVisualEffectView {
        let v = UIVisualEffectView(effect: effect(interactive: interactive))
        if let radius {
            v.cornerConfiguration = .uniformCorners(radius: .fixed(radius))
        } else {
            v.cornerConfiguration = .capsule()
        }
        return v
    }

    /// Materialize / dematerialize a glass surface the way the system does:
    /// animating `effect` (nil ↔ glass) lets the render server grow the
    /// refraction in, instead of cross-fading a finished blur (no flashes).
    static func setVisible(_ view: UIVisualEffectView, _ visible: Bool, content: UIView? = nil, animated: Bool = true) {
        let target: UIVisualEffect? = visible ? effect() : nil
        let apply = {
            view.effect = target
            (content ?? view.contentView).alpha = visible ? 1 : 0
        }
        guard animated, !UIAccessibility.isReduceMotionEnabled else { return apply() }
        UIView.animate(withDuration: visible ? 0.35 : 0.25, delay: 0, options: [.beginFromCurrentState, .allowUserInteraction], animations: apply)
    }

    /// A round glass icon button (toolbar "…", search, attach).
    static func circleButton(symbol: String, size: CGFloat = 44, pointSize: CGFloat = 17, action: UIAction? = nil) -> UIButton {
        var config = UIButton.Configuration.glass()
        config.image = UIImage(systemName: symbol, withConfiguration: UIImage.SymbolConfiguration(pointSize: pointSize, weight: .medium))
        config.baseForegroundColor = Palette.text
        config.cornerStyle = .capsule
        let b = UIButton(configuration: config, primaryAction: action)
        b.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            b.widthAnchor.constraint(equalToConstant: size),
            b.heightAnchor.constraint(equalToConstant: size),
        ])
        return b
    }
}
