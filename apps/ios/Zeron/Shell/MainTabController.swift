import UIKit

/// Root on iPhone: the Sessions list, with a Liquid Glass bottom bar of
/// native toolbar items — Settings, Search (the system's toolbar-integrated
/// field), voice, and New session — instead of tabs. A plain container for
/// the Sessions navigation stack: a hidden tab bar came back on device
/// whenever a screen that hides the bottom bar was popped.
final class MainTabController: UIViewController, AppRouter {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    private lazy var sessionsList = SessionsViewController(app: app)
    private lazy var nav = sessionsNav()
    private var voiceToken: AnyObject?
    private var appToken: AnyObject?

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        addChild(nav)
        nav.view.frame = view.bounds
        nav.view.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(nav.view)
        nav.didMove(toParent: self)
        voiceToken = app.voice.observe { [weak self] in self?.updateBottomBar() }
        // Hosts coming and going decide whether voice is offered.
        appToken = app.observe { [weak self] in self?.updateBottomBar() }
    }

    override var childForStatusBarStyle: UIViewController? { nav }

    private func sessionsNav() -> UINavigationController {
        sessionsList.enableToolbarSearch()
        updateBottomBar()
        let nav = Self.nav(sessionsList)
        nav.isToolbarHidden = false
        nav.toolbar.tintColor = Palette.text
        return nav
    }

    private lazy var settingsItem: UIBarButtonItem = {
        let item = UIBarButtonItem(image: UIImage(systemName: "gearshape"), primaryAction: UIAction { [weak self] _ in self?.showSettings() })
        item.accessibilityLabel = "Settings"
        item.accessibilityIdentifier = "bottom-settings"
        return item
    }()

    /// Voice: a tap calls (or opens the live call); a long press picks the
    /// host and style.
    private lazy var voiceItem: UIBarButtonItem = {
        let item = UIBarButtonItem(title: nil, image: UIImage(systemName: "waveform"), primaryAction: UIAction { [weak self] _ in
            guard let self else { return }
            self.openVoice(app: self.app, source: nil)
        }, menu: voiceMenu(app: app))
        return item
    }()

    private lazy var composeItem: UIBarButtonItem = {
        let item = UIBarButtonItem(image: UIImage(systemName: "square.and.pencil"), primaryAction: UIAction { [weak self] _ in self?.presentNewSession() })
        item.style = .prominent
        item.tintColor = Palette.text
        item.accessibilityLabel = "New session"
        item.accessibilityIdentifier = "new-session"
        return item
    }()

    /// The bar: Settings · Search · voice (when a host can run it, or a call
    /// is live) · New session. Rebuilt only when what it shows changes (the
    /// voice observer also fires for audio levels mid-call).
    private var shownBar: (live: Bool, voice: Bool)?
    private func updateBottomBar() {
        let voice = app.voice
        let state = (live: voice.live, voice: voice.live || voice.available)
        if let shownBar, shownBar == state { return }
        shownBar = state
        voiceItem.image = UIImage(systemName: state.live ? "waveform.circle.fill" : "waveform")
        voiceItem.tintColor = state.live ? Palette.accent : nil
        voiceItem.accessibilityLabel = state.live ? "Voice call" : "Talk to Codex"
        voiceItem.accessibilityIdentifier = state.live ? "voice-live" : "bottom-voice"
        // Holding the live call must not offer to change its host.
        voiceItem.menu = state.live ? nil : voiceMenu(app: app)
        // Fixed spaces split the glass: each control its own shape.
        var items: [UIBarButtonItem] = [settingsItem, .fixedSpace(0), sessionsList.navigationItem.searchBarPlacementBarButtonItem, .fixedSpace(0)]
        if state.voice { items += [voiceItem, .fixedSpace(0)] }
        items.append(composeItem)
        // `viewIfLoaded`: loading the list outside its nav controller lays it
        // out without the large title, and it opens collapsed.
        sessionsList.setToolbarItems(items, animated: sessionsList.viewIfLoaded?.window != nil)
    }

    /// The bottom bar's frame when it's on screen (toast placement).
    func bottomBarFrame(in window: UIWindow) -> CGRect? {
        guard nav.topViewController === sessionsList, !nav.isToolbarHidden, nav.toolbar.window != nil
        else { return nil }
        return nav.toolbar.convert(nav.toolbar.bounds, to: window)
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

    /// Settings over the list (the bottom bar steps aside).
    func showSettings() {
        if presentedViewController != nil { dismiss(animated: false) }
        if nav.topViewController is MoreViewController { return }
        nav.popToRootViewController(animated: false)
        let more = MoreViewController(app: app)
        more.hidesBottomBarWhenPushed = true
        nav.pushViewController(more, animated: true)
    }

    func showSearch() {
        popToFrontPage()
        sessionsList.activateSearch()
    }

    /// From the new-session sheet: the chat goes in under the sheet at once,
    /// the sheet leaves without its slide, and the handoff animation carries
    /// the draft (page, composer, message) into the chat.
    func openSession(_ chatId: String, handoff: DraftHandoff?) {
        guard let handoff, let window = view.window else { return openSession(chatId) }
        guard let session = SessionViewController(app: app, chatId: chatId) else {
            return showSessionOpenError()
        }
        UIView.performWithoutAnimation {
            nav.popToRootViewController(animated: false)
            nav.pushViewController(session, animated: false)
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
        nav.popToRootViewController(animated: false)
    }

    /// The split's open session, handed over as it is when the window
    /// narrows (iPhone Duo folding): same page, scroll position and draft.
    func adoptSession(_ session: SessionViewController) {
        nav.popToRootViewController(animated: false)
        nav.pushViewController(session, animated: false)
    }

    /// The pushed session, taken off the stack for the split's main column
    /// when the window widens; nil when none is open. It comes back marked
    /// `relocating`, for the split to clear once it's placed.
    func releaseSession() -> SessionViewController? {
        // The session itself, not a side chat pushed above it.
        guard let session = nav.viewControllers.first(where: { ($0 as? SessionViewController)?.isCompanion == false }) as? SessionViewController else {
            return nil
        }
        session.relocating = true
        nav.popToRootViewController(animated: false)
        return session
    }

    func showSessionOpenError() {
        if presentedViewController != nil { dismiss(animated: false) }
        view.endEditing(true)
        popToFrontPage()
        view.layoutIfNeeded()
        Toast.show("Couldn't open session. Try again.", in: view.window)
    }

    /// Push a session on the Sessions list (from new-session, deep links, search).
    func openSession(_ chatId: String) {
        guard let session = SessionViewController(app: app, chatId: chatId) else {
            return showSessionOpenError()
        }
        if presentedViewController != nil { dismiss(animated: true) }
        nav.popToRootViewController(animated: false)
        nav.pushViewController(session, animated: true)
    }
}

extension UIViewController {
    /// The iPhone shell this screen sits in, if any (not beside the iPad
    /// sidebar, not a standalone sheet).
    var mainShell: MainTabController? {
        var ancestor = parent
        while let current = ancestor {
            if let shell = current as? MainTabController { return shell }
            ancestor = current.parent
        }
        return nil
    }
}
