import UIKit

/// Root: native tab bar (Liquid Glass comes from the system, so tab switches,
/// minimize-on-scroll and the search morph are render-server animations), with
/// the "Ask anything" composer as the tab bar's bottom accessory.
final class MainTabController: UITabBarController, UITabBarControllerDelegate {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        tabBar.tintColor = Palette.accent
        tabBarMinimizeBehavior = .onScrollDown

        let projects = UITab(title: "Projects", image: UIImage(systemName: "square.grid.2x2"), identifier: "projects") { [app] _ in
            Self.nav(ProjectsViewController(app: app))
        }
        let sessions = UITab(title: "Sessions", image: UIImage(systemName: "list.bullet"), identifier: "sessions") { [app] _ in
            Self.nav(SessionsViewController(app: app))
        }
        let prs = UITab(title: "PRs", image: UIImage(systemName: "arrow.triangle.pull"), identifier: "prs") { [app] _ in
            Self.nav(PullRequestsViewController(app: app))
        }
        let more = UITab(title: "More", image: UIImage(systemName: "ellipsis"), identifier: "more") { [app] _ in
            Self.nav(MoreViewController(app: app))
        }
        let search = UISearchTab { [app] _ in
            Self.nav(SearchViewController(app: app))
        }
        search.automaticallyActivatesSearch = true
        tabs = [projects, sessions, prs, more, search]
        selectedTab = sessions

        bottomAccessory = accessory
        delegate = self
    }

    /// Search has its own bottom field; the composer accessory steps aside.
    func tabBarController(_ tabBarController: UITabBarController, didSelectTab selectedTab: UITab, previousTab: UITab?) {
        setAccessoryVisible(!(selectedTab is UISearchTab), animated: true)
    }

    private lazy var accessory = UITabAccessory(contentView: AskAnythingAccessory { [weak self] in self?.presentNewSession() })

    /// Pushed sessions carry their own composer; the accessory steps aside.
    func setAccessoryVisible(_ visible: Bool, animated: Bool) {
        let target = visible && !(selectedTab is UISearchTab) ? accessory : nil
        guard bottomAccessory !== target else { return }
        setBottomAccessory(target, animated: animated)
    }

    static func nav(_ root: UIViewController) -> UINavigationController {
        let nav = UINavigationController(rootViewController: root)
        nav.navigationBar.prefersLargeTitles = true
        // Setting an appearance object drops iOS 26's scroll-edge effect;
        // style titles through the bar's own attributes instead.
        nav.navigationBar.largeTitleTextAttributes = [.font: Fonts.ui(.sansSemibold, 30), .foregroundColor: Palette.text]
        nav.navigationBar.titleTextAttributes = [.font: Fonts.ui(.sansSemibold, 17), .foregroundColor: Palette.text]
        nav.navigationBar.tintColor = Palette.text
        return nav
    }

    func presentNewSession(prompt: String? = nil) {
        let vc = NewSessionViewController(app: app, prompt: prompt) { [weak self] chatId in
            guard let self else { return }
            self.openSession(chatId)
        }
        let nav = UINavigationController(rootViewController: vc)
        nav.modalPresentationStyle = .pageSheet
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
        present(nav, animated: true)
    }

    /// Push a session on the Sessions tab (from new-session, deep links, search).
    func openSession(_ chatId: String) {
        if presentedViewController != nil { dismiss(animated: true) }
        guard let tab = tabs.first(where: { $0.identifier == "sessions" }) else { return }
        selectedTab = tab
        guard let nav = tab.viewController as? UINavigationController else { return }
        nav.popToRootViewController(animated: false)
        nav.pushViewController(SessionViewController(app: app, chatId: chatId), animated: true)
    }
}

/// The capsule above the tab bar. Tapping it opens the new-session composer.
final class AskAnythingAccessory: UIControl {
    private let onTap: () -> Void

    init(onTap: @escaping () -> Void) {
        self.onTap = onTap
        super.init(frame: .zero)
        accessibilityLabel = "New session"
        accessibilityIdentifier = "new-session"
        accessibilityTraits = .button

        let plus = UIImageView(image: UIImage(systemName: "plus", withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .medium)))
        plus.tintColor = Palette.text
        plus.contentMode = .center
        let plate = UIView()
        plate.backgroundColor = Palette.chip.withAlphaComponent(0.8)
        plate.layer.cornerRadius = 16
        plate.isUserInteractionEnabled = false
        plate.addSubview(plus)
        let label = UILabel()
        label.text = "Ask anything"
        label.font = Fonts.ui(.sans, 17)
        label.textColor = Palette.secondary
        for v in [plate, label, plus] { v.translatesAutoresizingMaskIntoConstraints = false }
        addSubview(plate)
        addSubview(label)
        NSLayoutConstraint.activate([
            plate.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 8),
            plate.centerYAnchor.constraint(equalTo: centerYAnchor),
            plate.widthAnchor.constraint(equalToConstant: 32),
            plate.heightAnchor.constraint(equalToConstant: 32),
            plus.centerXAnchor.constraint(equalTo: plate.centerXAnchor),
            plus.centerYAnchor.constraint(equalTo: plate.centerYAnchor),
            label.leadingAnchor.constraint(equalTo: plate.trailingAnchor, constant: 10),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.trailingAnchor.constraint(lessThanOrEqualTo: trailingAnchor, constant: -12),
        ])
        addAction(UIAction { [weak self] _ in self?.onTap() }, for: .touchUpInside)
        registerForTraitChanges([UITraitTabAccessoryEnvironment.self]) { (self: AskAnythingAccessory, _) in
            // Inline (minimized tab bar): drop the label, keep the plus.
            let inline = self.traitCollection.tabAccessoryEnvironment == .inline
            label.alpha = inline ? 0 : 1
        }
    }

    required init?(coder: NSCoder) { fatalError() }

    override var isHighlighted: Bool {
        didSet { UIView.animate(withDuration: 0.15) { self.alpha = self.isHighlighted ? 0.6 : 1 } }
    }
}
