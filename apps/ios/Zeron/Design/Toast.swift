import UIKit

/// A glass capsule that floats above the bottom chrome for a few seconds,
/// with an optional action ("Archived · Undo"). One at a time.
final class Toast: UIView {
    private static weak var current: Toast?
    private let glass = Glass.surface()
    private var dismissWork: DispatchWorkItem?

    static func show(_ text: String, action: String? = nil, in window: UIWindow?, handler: (() -> Void)? = nil) {
        guard let window else { return }
        current?.dismiss(animated: false)
        let toast = Toast(text: text, action: action, handler: handler)
        toast.translatesAutoresizingMaskIntoConstraints = false
        window.addSubview(toast)
        // Sit just above the bottom chrome actually on screen (the "New
        // session" accessory or the tab bar — expanded or minimized), not at a
        // fixed height that floats over list rows.
        let gap: CGFloat = 12
        NSLayoutConstraint.activate([
            toast.centerXAnchor.constraint(equalTo: window.centerXAnchor),
            toast.bottomAnchor.constraint(equalTo: window.topAnchor, constant: chromeTop(in: window) - gap),
            toast.widthAnchor.constraint(lessThanOrEqualTo: window.widthAnchor, constant: -48),
        ])
        current = toast
        toast.present()
    }

    /// Top edge of the bottom chrome in window coordinates.
    private static func chromeTop(in window: UIWindow) -> CGFloat {
        var top = window.bounds.height - window.safeAreaInsets.bottom
        var tabs: UITabBarController? = window.rootViewController as? UITabBarController
        if tabs == nil, let split = window.rootViewController as? UISplitViewController, split.isCollapsed {
            tabs = split.viewController(for: .compact) as? UITabBarController
        }
        if let tabs, !tabs.tabBar.isHidden, tabs.tabBar.window != nil {
            top = min(top, tabs.tabBar.convert(tabs.tabBar.bounds, to: window).minY)
            if let accessory = (tabs as? MainTabController)?.accessoryFrame(in: window) {
                top = min(top, accessory.minY)
            }
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
        let stack = UIStackView(arrangedSubviews: [label])
        stack.spacing = 14
        stack.alignment = .center
        if let action {
            var c = UIButton.Configuration.plain()
            c.title = action
            c.baseForegroundColor = Palette.accent
            c.contentInsets = .zero
            c.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
                var a = a
                a.font = Fonts.ui(.sansSemibold, 15)
                return a
            }
            let b = UIButton(configuration: c, primaryAction: UIAction { [weak self] _ in
                handler?()
                self?.dismiss(animated: true)
            })
            b.accessibilityIdentifier = "toast-action"
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
            stack.trailingAnchor.constraint(equalTo: glass.contentView.trailingAnchor, constant: -20),
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
