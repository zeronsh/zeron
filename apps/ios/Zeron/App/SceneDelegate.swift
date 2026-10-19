import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let app = AppModel()
    /// a widget or control route that arrived before the signed-in shell
    /// could take it (signed out, or still launching).
    private var pendingRoute: URL?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        window.overrideUserInterfaceStyle = UIUserInterfaceStyle(rawValue: UserDefaults.standard.integer(forKey: "appearance")) ?? .unspecified
        window.tintColor = Palette.accent
        self.window = window
        let push = PushNotifications.shared
        push.install(app: app)
        push.openChat = { [weak self] chatId in
            guard let router = self?.window?.rootViewController as? AppRouter else { return false }
            router.openSession(chatId)
            return true
        }
        app.onSignedIn = { [weak self] in
            self?.showRoot(animated: true)
            self?.shellShown()
        }
        app.onSignOut = { [weak self] in
            guard let self else { return }
            self.pendingRoute = nil
            push.signingOut(client: self.app.client)
            self.app.signOutLocally()
            self.showRoot(animated: true)
        }
        let args = ProcessInfo.processInfo.arguments
        // `-wallpaper <path> [-wallpaper-effect ascii]`: set the wallpaper at
        // launch (screenshots, tests); `-wallpaper none` clears it.
        if let i = args.firstIndex(of: "-wallpaper"), i + 1 < args.count {
            if args[i + 1] == "none" {
                WallpaperStore.remove()
            } else if let image = UIImage(contentsOfFile: args[i + 1]) {
                WallpaperStore.set(image, name: (args[i + 1] as NSString).lastPathComponent)
            }
        }
        if let i = args.firstIndex(of: "-wallpaper-effect"), i + 1 < args.count,
           let effect = WallpaperStore.allEffects.first(where: { WallpaperStore.key($0) == args[i + 1] }) {
            WallpaperStore.effect = effect
        }
        if args.contains("-lab") {
            window.rootViewController = MainTabController.nav(TranscriptLabViewController())
        } else {
            showRoot(animated: false)
        }
        window.makeKeyAndVisible()
        if !args.contains("-lab") { shellShown() }
        if let url = connectionOptions.urlContexts.first?.url {
            DispatchQueue.main.async { self.open(url) }
        }
        DispatchQueue.main.async { [weak self] in
            LinkRouter.handler = { url in self?.open(url) }
        }

        #if DEBUG
        if args.contains("-voice-preview") {
            app.voice.preview()
            if args.contains("-voice-stage") {
                DispatchQueue.main.async { [app] in window.rootViewController?.presentVoiceStage(app: app, source: nil) }
            }
        }
        #endif
        if let router = window.rootViewController as? AppRouter, let i = args.firstIndex(of: "-route"), i + 1 < args.count {
            let route = args[i + 1]
            DispatchQueue.main.async {
                if route.hasPrefix("chat:") { router.openSession(String(route.dropFirst(5))) }
                if route == "new" { router.presentNewSession(prompt: nil) }
                if route == "more" { router.showSettings() }
                if route == "search" { router.showSearch() }
            }
        }
    }

    /// The signed-in shell is up: refresh this phone's notification
    /// registration and open a session a notification (or widget) was
    /// tapped for.
    private func shellShown() {
        guard app.isSignedIn else { return }
        if !app.isDemo { PushNotifications.shared.signedIn() }
        PushNotifications.shared.shellReady()
        if let url = pendingRoute {
            pendingRoute = nil
            DispatchQueue.main.async { self.open(url) }
        }
    }

    private func showRoot(animated: Bool) {
        guard let window else { return }
        // The split shell everywhere: it shows the sidebar when the window
        // is regular in both directions (iPad, an unfolded iPhone Duo) and
        // collapses to the tab shell otherwise, live as the window changes —
        // folding or unfolding carries the open session across.
        let root: UIViewController = !app.isSignedIn ? SignInViewController(app: app) : SplitRootController(app: app)
        // Sheets (new session, Settings) belong to the old root: they'd
        // stay on top of the new one.
        if let old = window.rootViewController, old.presentedViewController != nil {
            old.dismiss(animated: false)
        }
        guard animated, window.rootViewController != nil else {
            window.rootViewController = root
            return
        }
        UIView.transition(with: window, duration: 0.35, options: [.transitionCrossDissolve, .allowAnimatedContent]) {
            window.rootViewController = root
        }
    }

    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        URLContexts.forEach { open($0.url) }
    }

    /// `zeron://voice` (the call's Live Activity) returns to the stage;
    /// `zeron://chat/<id>`, `zeron://new`, `zeron://call` and
    /// `zeron://sessions` come from the widgets and controls.
    private func open(_ url: URL) {
        guard url.scheme == "zeron", let root = window?.rootViewController else { return }
        if url.host == "voice" {
            guard app.voice.live else { return }
            return root.presentVoiceStage(app: app, source: nil)
        }
        guard app.isSignedIn, let router = root as? AppRouter else {
            pendingRoute = url
            return
        }
        switch url.host {
        case "call":
            openVoiceOnceHostsKnown(deadline: Date().addingTimeInterval(5))
        case "chat":
            guard let chatId = url.pathComponents.dropFirst().first else { return }
            router.openSession(chatId)
        case "new":
            router.presentNewSession(prompt: nil)
        case "sessions":
            router.showSessions()
        default:
            break
        }
    }

    /// a cold launch from the call control lands before the registry: until
    /// it syncs (or a host shows up), the host picker would claim there's no
    /// online device.
    private func openVoiceOnceHostsKnown(deadline: Date) {
        guard app.isSignedIn, let root = window?.rootViewController as? UIViewController & AppRouter else { return }
        let voice = app.voice
        guard voice.live || !voice.hosts.isEmpty || app.synced || Date() >= deadline else {
            DispatchQueue.main.asyncAfter(deadline: .now() + 0.25) { [weak self] in
                self?.openVoiceOnceHostsKnown(deadline: deadline)
            }
            return
        }
        root.openVoice(app: app, source: nil)
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        app.didEnterBackground()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        app.willEnterForeground()
    }
}
