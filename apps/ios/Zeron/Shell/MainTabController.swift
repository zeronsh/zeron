import UIKit

/// Root: native tab bar (Liquid Glass comes from the system, so tab switches,
/// minimize-on-scroll and the search morph are render-server animations), with
/// the "Ask anything" composer as the tab bar's bottom accessory.
final class MainTabController: UITabBarController, UITabBarControllerDelegate, AppRouter {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        // The selected tab reads through the glass pill + label weight; the
        // violet accent made it shout.
        tabBar.tintColor = Palette.text
        tabBarMinimizeBehavior = .onScrollDown

        let sessions = UITab(title: "Sessions", image: UIImage(named: "tab-chat"), identifier: "sessions") { [app] _ in
            Self.nav(SessionsViewController(app: app))
        }
        let more = UITab(title: "Settings", image: UIImage(named: "tab-settings"), identifier: "more") { [app] _ in
            Self.nav(MoreViewController(app: app))
        }
        let search = UISearchTab { [app] _ in
            Self.nav(SearchViewController(app: app))
        }
        search.automaticallyActivatesSearch = true
        tabs = [sessions, more, search]
        selectedTab = sessions

        bottomAccessory = accessory
        delegate = self
        accessoryContent.update(app.live)
        liveToken = app.observe { [weak self] in
            guard let self else { return }
            self.accessoryContent.update(self.app.live)
        }
    }

    /// The accessory stays attached across tab switches, search included —
    /// the system carries it through the search morph (above the field, then
    /// under the keyboard). Taking it off for search and adding it back on
    /// the way out made the tab bar reflow mid-morph (pill and icons out of
    /// step for several frames). Leaving search, the shrinking field crosses
    /// the accessory for a couple of frames, so its content fades back in
    /// just after.
    func tabBarController(_ tabBarController: UITabBarController, didSelectTab selectedTab: UITab, previousTab: UITab?) {
        syncAccessory()
        guard previousTab is UISearchTab, !(selectedTab is UISearchTab), bottomAccessory != nil,
              !UIAccessibility.isReduceMotionEnabled else { return }
        accessoryContent.alpha = 0
        UIView.animate(withDuration: 0.25, delay: 0.12, options: [.curveEaseOut, .allowUserInteraction]) {
            self.accessoryContent.alpha = 1
        }
    }

    private lazy var accessoryContent = AskAnythingAccessory { [weak self] in self?.presentNewSession() }
    private lazy var accessory = UITabAccessory(contentView: accessoryContent)
    private var liveToken: AnyObject?

    /// Accessory state from what's on screen: hidden over a session (it has
    /// its own composer).
    func syncAccessory(animated: Bool = false) {
        let top = (selectedTab?.viewController as? UINavigationController)?.topViewController
        setAccessoryVisible(!(top is SessionViewController), animated: animated)
    }

    /// Pushed sessions carry their own composer; the accessory steps aside.
    /// Swipe-back / pop: put the accessory in place *before* the transition
    /// animation (inside it, its first layout animates from a zero frame — it
    /// flew in from the top of the screen), content transparent so it can
    /// fade in with the gesture.
    func prepareAccessoryForReveal() {
        guard bottomAccessory == nil else { return }
        UIView.performWithoutAnimation {
            setBottomAccessory(accessory, animated: false)
            view.layoutIfNeeded()
            accessoryContent.alpha = 0
        }
    }

    /// The accessory's frame when it's on screen (toast placement).
    func accessoryFrame(in window: UIWindow) -> CGRect? {
        guard bottomAccessory != nil, accessoryContent.window != nil, !accessoryContent.isHidden else { return nil }
        // The content view sits inside the glass capsule; its bounds are the capsule's.
        return accessoryContent.convert(accessoryContent.bounds, to: window)
    }

    func setAccessoryContentAlpha(_ alpha: CGFloat) {
        accessoryContent.alpha = alpha
    }

    func setAccessoryVisible(_ visible: Bool, animated: Bool) {
        let target = visible ? accessory : nil
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
        let vc = NewSessionViewController(app: app, prompt: prompt) { [weak self] chatId, handoff in
            guard let self else { return }
            self.openSession(chatId, handoff: handoff)
        }
        let nav = UINavigationController(rootViewController: vc)
        nav.modalPresentationStyle = .pageSheet
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.large()]
            sheet.prefersGrabberVisible = true
        }
        present(nav, animated: true)
    }

    func showSettings() {
        selectedTab = tabs.first { $0.identifier == "more" }
    }

    func showSearch() {
        selectedTab = tabs.first { $0 is UISearchTab }
    }

    /// From the new-session sheet: the chat goes in under the sheet at once,
    /// the sheet leaves without its slide, and the handoff animation carries
    /// the draft (page, composer, message) into the chat.
    func openSession(_ chatId: String, handoff: DraftHandoff?) {
        guard let handoff, let window = view.window,
              let tab = tabs.first(where: { $0.identifier == "sessions" })
        else { return openSession(chatId) }
        selectedTab = tab
        guard let nav = tab.viewController as? UINavigationController else { return openSession(chatId) }
        let session = SessionViewController(app: app, chatId: chatId)
        UIView.performWithoutAnimation {
            nav.popToRootViewController(animated: false)
            nav.pushViewController(session, animated: false)
            setAccessoryVisible(false, animated: false)
            view.layoutIfNeeded()
            session.prepareArrival()
            // Sending puts the keyboard away; it slides down with the handoff.
            presentedViewController?.view.endEditing(true)
            // Hide the sheet now; tear it down only once the motion is done.
            // Dismissal (plus the keyboard re-hosting with it) is a heavy
            // compositor frame — mid-animation it swallowed the motion.
            if let sheet = presentedViewController?.presentationController as? UISheetPresentationController {
                // Drop the sheet's background dimming with it (it would sit over
                // the chat until the deferred dismissal, then snap off).
                sheet.largestUndimmedDetentIdentifier = .large
            }
            presentedViewController?.presentationController?.containerView?.alpha = 0
        }
        DraftHandoffAnimator.run(handoff, into: session, window: window) { [weak self] in
            if self?.presentedViewController != nil { self?.dismiss(animated: false) }
        }
    }

    /// Back to the Sessions list, releasing any pushed session (the iPad
    /// split takes the open session back into its own column).
    func popToFrontPage() {
        guard let nav = tabs.first(where: { $0.identifier == "sessions" })?.viewController as? UINavigationController else { return }
        nav.popToRootViewController(animated: false)
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

/// The capsule above the tab bar: a plus, "New session", and a live
/// summary of what's running ("2 working · 1 needs you"). Tapping it opens the
/// new-session composer.
final class AskAnythingAccessory: UIControl {
    private let onTap: () -> Void
    private let label = UILabel()
    private let summary = UILabel()
    private let cells = StatusGlyph()
    private let mark = UIImageView(image: UIImage(systemName: "plus", withConfiguration: UIImage.SymbolConfiguration(pointSize: 15, weight: .semibold)))

    init(onTap: @escaping () -> Void) {
        self.onTap = onTap
        super.init(frame: .zero)
        accessibilityIdentifier = "new-session"
        accessibilityTraits = .button

        mark.tintColor = Palette.accent
        mark.contentMode = .center
        let plate = UIView()
        plate.backgroundColor = Palette.accentSoft
        // A circle, concentric with the accessory capsule (7pt inset).
        plate.layer.cornerRadius = 17
        plate.layer.cornerCurve = .continuous
        plate.isUserInteractionEnabled = false
        plate.addSubview(mark)
        label.text = "New session"
        label.font = Fonts.ui(.sansMedium, 16)
        label.textColor = Palette.text
        summary.font = Fonts.ui(.sansMedium, 13)
        summary.textColor = Palette.secondary
        summary.textAlignment = .right
        cells.isHidden = true
        for v in [plate, label, mark, summary, cells] as [UIView] { v.translatesAutoresizingMaskIntoConstraints = false }
        for v in [plate, label, summary, cells] as [UIView] { addSubview(v) }
        summary.setContentCompressionResistancePriority(.required, for: .horizontal)
        NSLayoutConstraint.activate([
            plate.leadingAnchor.constraint(equalTo: leadingAnchor, constant: 7),
            plate.centerYAnchor.constraint(equalTo: centerYAnchor),
            plate.widthAnchor.constraint(equalToConstant: 34),
            plate.heightAnchor.constraint(equalToConstant: 34),
            mark.centerXAnchor.constraint(equalTo: plate.centerXAnchor),
            mark.centerYAnchor.constraint(equalTo: plate.centerYAnchor),
            label.leadingAnchor.constraint(equalTo: plate.trailingAnchor, constant: 11),
            label.centerYAnchor.constraint(equalTo: centerYAnchor),
            label.trailingAnchor.constraint(lessThanOrEqualTo: cells.leadingAnchor, constant: -10),
            summary.trailingAnchor.constraint(equalTo: trailingAnchor, constant: -16),
            summary.centerYAnchor.constraint(equalTo: centerYAnchor),
            cells.trailingAnchor.constraint(equalTo: summary.leadingAnchor, constant: -7),
            cells.centerYAnchor.constraint(equalTo: centerYAnchor),
            cells.widthAnchor.constraint(equalToConstant: 12),
            cells.heightAnchor.constraint(equalToConstant: 12),
        ])
        addAction(UIAction { [weak self] _ in self?.onTap() }, for: .touchUpInside)
        registerForTraitChanges([UITraitTabAccessoryEnvironment.self]) { (self: AskAnythingAccessory, _) in
            // Inline (minimized tab bar): the plus and the live summary only.
            let inline = self.traitCollection.tabAccessoryEnvironment == .inline
            self.label.alpha = inline ? 0 : 1
        }
        update(AppModel.LiveCounts())
    }

    required init?(coder: NSCoder) { fatalError() }

    func update(_ live: AppModel.LiveCounts) {
        var parts: [String] = []
        if live.working > 0 { parts.append("\(live.working) working") }
        if live.awaiting > 0 { parts.append("\(live.awaiting) need\(live.awaiting == 1 ? "s" : "") you") }
        summary.text = parts.joined(separator: " · ")
        cells.isHidden = parts.isEmpty
        cells.kind = live.working > 0 ? .spinner : .dot(StatusTone.input)
        accessibilityLabel = parts.isEmpty ? "New session" : "New session, " + parts.joined(separator: ", ")
    }

    override var isHighlighted: Bool {
        didSet { UIView.animate(withDuration: 0.15) { self.alpha = self.isHighlighted ? 0.6 : 1 } }
    }
}
