import UIKit

extension Notification.Name {
    static let crewOpenSessionLink = Notification.Name("crew.openSessionLink")
}

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let app = AppModel()
    private var linkObserver: AnyObject?
    private var pastedLinkObserver: NSObjectProtocol?
    private var deferredLink: String?
    private var resolvingLink = false
    private var requestedLink: String?
    private var attemptedHosts: Set<String> = []
    private var pendingLink: String? {
        get { UserDefaults.standard.string(forKey: "crew.pendingSessionLink") }
        set { UserDefaults.standard.set(newValue, forKey: "crew.pendingSessionLink") }
    }

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let scene = scene as? UIWindowScene else { return }
        connect(scene, urlContexts: connectionOptions.urlContexts)
    }

    /// Builds the window and shell once; later calls only deliver links.
    /// UIKit reaches it through `willConnect`; a scene restored from the
    /// legacy app's session reaches it through `LegacySceneRepair`.
    func connect(_ scene: UIWindowScene, urlContexts: Set<UIOpenURLContext>) {
        guard window == nil else {
            for context in urlContexts { acceptURL(context.url) }
            return
        }
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
        linkObserver = app.observe { [weak self] in self?.drainSessionLink() }
        pastedLinkObserver = NotificationCenter.default.addObserver(forName: .crewOpenSessionLink, object: nil, queue: .main) { [weak self] note in
            guard let text = note.object as? String, let url = URL(string: text) else { return }
            self?.acceptURL(url)
        }
        for context in urlContexts { acceptURL(context.url) }
        if !args.contains("-lab") { shellShown() }

        #if DEBUG
        if args.contains("-voice-preview") {
            app.voice.preview()
            if args.contains("-voice-stage") {
                DispatchQueue.main.async { [app] in window.rootViewController?.presentVoiceStage(app: app, source: nil) }
            }
        }
        TerminalProbe.runIfAsked(app: app, router: window.rootViewController as? AppRouter)
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
    /// registration and open a session a notification was tapped for.
    private func shellShown() {
        guard app.isSignedIn else { return }
        if !app.isDemo { PushNotifications.shared.signedIn() }
        PushNotifications.shared.shellReady()
        drainSessionLink()
    }

    private func showRoot(animated: Bool) {
        guard let window else { return }
        // The split shell adapts live between compact phones and regular
        // iPad or unfolded layouts while preserving the active session.
        let root: UIViewController = !app.isSignedIn
            ? SignInViewController(app: app)
            : SplitRootController(app: app)
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

    func scene(_ scene: UIScene, openURLContexts contexts: Set<UIOpenURLContext>) {
        for context in contexts { acceptURL(context.url) }
    }

    private func acceptURL(_ url: URL) {
        if url.scheme == Endpoints.callbackScheme, url.host == "voice" {
            if app.voice.live { window?.rootViewController?.presentVoiceStage(app: app, source: nil) }
        } else if crewIsSessionLink(url: url.absoluteString) {
            pendingLink = url.absoluteString
            requestedLink = nil
            deferredLink = nil
            attemptedHosts.removeAll()
            drainSessionLink()
        }
    }

    private func drainSessionLink() {
        guard !resolvingLink, let url = pendingLink, deferredLink != url, let client = app.client,
              let deployment = Endpoints.deploymentId,
              let router = window?.rootViewController as? AppRouter else { return }
        do {
            if let id = try client.crewCachedLink(url: url, deployment: deployment) {
                pendingLink = nil
                requestedLink = nil
                router.openSession(id)
                return
            }
        } catch {
            showLinkError(error.localizedDescription)
            return
        }
        guard requestedLink != url,
              let host = app.hostOptions.first(where: { $0.online && !attemptedHosts.contains($0.id) }) else { return }
        attemptedHosts.insert(host.id)
        resolvingLink = true
        Task { @MainActor in
            defer { resolvingLink = false }
            do {
                _ = try await client.crewResolveLink(deviceId: host.id, url: url, deployment: deployment)
                guard pendingLink == url else { return }
                requestedLink = url
            } catch {
                // Other online devices may be sandboxes; only a local controller may resolve.
                if !app.hostOptions.contains(where: { $0.online && !attemptedHosts.contains($0.id) }) {
                    showLinkError("Connect a desktop Crew controller to discover this session. \(error.localizedDescription)")
                }
            }
            DispatchQueue.main.async { [weak self] in self?.drainSessionLink() }
        }
    }

    private func showLinkError(_ message: String) {
        guard let root = window?.rootViewController, root.presentedViewController == nil else { return }
        let alert = UIAlertController(title: "Crew session", message: message, preferredStyle: .alert)
        alert.addAction(UIAlertAction(title: "Keep for later", style: .cancel) { [weak self] _ in
            self?.deferredLink = self?.pendingLink
        })
        alert.addAction(UIAlertAction(title: "Dismiss link", style: .default) { [weak self] _ in self?.pendingLink = nil })
        root.present(alert, animated: true)
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        app.didEnterBackground()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        app.willEnterForeground()
        requestedLink = nil
        deferredLink = nil
        attemptedHosts.removeAll()
        shellShown()
    }
}
