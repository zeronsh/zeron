import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?
    private let app = AppModel()

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
    }

    private func showRoot(animated: Bool) {
        guard let window else { return }
        // iPad gets the split shell (it collapses to the tab shell
        // at compact widths); iPhone the tab shell directly.
        let root: UIViewController = !app.isSignedIn
            ? SignInViewController(app: app)
            : UIDevice.current.userInterfaceIdiom == .pad ? SplitRootController(app: app) : MainTabController(app: app)
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

    func sceneDidEnterBackground(_ scene: UIScene) {
        app.didEnterBackground()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        app.willEnterForeground()
    }
}
