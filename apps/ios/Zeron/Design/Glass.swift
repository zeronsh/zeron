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
