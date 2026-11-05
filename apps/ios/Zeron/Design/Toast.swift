import UIKit

/// A glass capsule that floats above the bottom chrome for a few seconds,
/// with an optional button ("Archived · Undo"). One at a time.
final class Toast: UIView {
    private static weak var current: Toast?
    private let glass = Glass.surface()
    private var dismissWork: DispatchWorkItem?

    /// `above`: a view of the screen's own (the composer) to float over
    /// instead of the tab chrome.
    static func show(_ text: String, action: String? = nil, in window: UIWindow?, above anchor: UIView? = nil, handler: (() -> Void)? = nil) {
        guard let window else { return }
        current?.dismiss(animated: false)
        let toast = Toast(text: text, action: action, handler: handler)
        toast.translatesAutoresizingMaskIntoConstraints = false
        window.addSubview(toast)
        // Sit just above the bottom chrome actually on screen (the "New
        // session" accessory or the tab bar — expanded or minimized), not at a
        // fixed height that floats over list rows.
        let gap: CGFloat = 12
        var top = chromeTop(in: window)
        if let anchor, anchor.window === window {
            top = min(top, anchor.convert(anchor.bounds, to: window).minY)
        }
        NSLayoutConstraint.activate([
            toast.centerXAnchor.constraint(equalTo: window.centerXAnchor),
            toast.bottomAnchor.constraint(equalTo: window.topAnchor, constant: top - gap),
            toast.widthAnchor.constraint(lessThanOrEqualTo: window.widthAnchor, constant: -48),
        ])
        current = toast
        toast.present()
    }

    /// Top edge of the bottom chrome in window coordinates.
    private static func chromeTop(in window: UIWindow) -> CGFloat {
        var top = window.bounds.height - window.safeAreaInsets.bottom
        var shell = window.rootViewController as? MainTabController
        if shell == nil, let split = window.rootViewController as? UISplitViewController, split.isCollapsed {
            shell = split.viewController(for: .compact) as? MainTabController
        }
        if let bar = shell?.bottomBarFrame(in: window) {
            top = min(top, bar.minY)
        }
        // A pushed screen's own bottom bar (e.g. the composer) isn't tab
        // chrome; keep clear of the keyboard / home indicator either way.
        return top
    }

    private init(text: String, action: String?, handler: (() -> Void)?) {
        super.init(frame: .zero)
        glass.translatesAutoresizingMaskIntoConstraints = false
        addSubview(glass)
        let label = UILabel()
        label.text = text
        label.font = Fonts.ui(.sansMedium, 15)
        label.textColor = Palette.text
        label.numberOfLines = 2
        let stack = UIStackView(arrangedSubviews: [label])
        stack.spacing = 14
        stack.alignment = .center
        if let action {
            // A small filled capsule in the text color, like the desktop's.
            var c = UIButton.Configuration.filled()
            c.title = action
            c.baseBackgroundColor = Palette.text
            c.baseForegroundColor = Palette.background
            c.cornerStyle = .capsule
            c.contentInsets = NSDirectionalEdgeInsets(top: 6, leading: 12, bottom: 6, trailing: 12)
            c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
                var a = a
                a.font = Fonts.ui(.sansSemibold, 14)
                return a
            }
            let b = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in
                handler?()
                self?.dismiss(animated: true)
            })
            b.accessibilityIdentifier = "toast-action"
            b.setContentHuggingPriority(.required, for: .horizontal)
            b.setContentCompressionResistancePriority(.required, for: .horizontal)
            stack.addArrangedSubview(b)
        }
        stack.translatesAutoresizingMaskIntoConstraints = false
        glass.contentView.addSubview(stack)
        NSLayoutConstraint.activate([
            glass.topAnchor.constraint(equalTo: topAnchor),
            glass.bottomAnchor.constraint(equalTo: bottomAnchor),
            glass.leadingAnchor.constraint(equalTo: leadingAnchor),
            glass.trailingAnchor.constraint(equalTo: trailingAnchor),
            stack.topAnchor.constraint(equalTo: glass.contentView.topAnchor, constant: 12),
            stack.bottomAnchor.constraint(equalTo: glass.contentView.bottomAnchor, constant: -12),
            stack.leadingAnchor.constraint(equalTo: glass.contentView.leadingAnchor, constant: 20),
            stack.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: action == nil ? -20 : -10),
        ])
        accessibilityIdentifier = "toast"
    }

    required init?(coder: NSCoder) { fatalError() }

    private func present() {
        Glass.setVisible(glass, false, animated: false)
        transform = CGAffineTransform(translationX: 0, y: 16)
        UIView.animate(withDuration: 0.4, delay: 0, usingSpringWithDamping: 0.85, initialSpringVelocity: 0) {
            self.transform = .identity
        }
        Glass.setVisible(glass, true)
        UIAccessibility.post(notification: .announcement, argument: (glass.contentView.subviews.first?.subviews.first as? UILabel)?.text)
        let work = DispatchWorkItem { [weak self] in self?.dismiss(animated: true) }
        dismissWork = work
        DispatchQueue.main.asyncAfter(deadline: .now() + 4, execute: work)
    }

    func dismiss(animated: Bool) {
        dismissWork?.cancel()
        guard animated else { return removeFromSuperview() }
        Glass.setVisible(glass, false)
        UIView.animate(withDuration: 0.25, animations: {
            self.transform = CGAffineTransform(translationX: 0, y: 12)
        }, completion: { _ in self.removeFromSuperview() })
    }
}
