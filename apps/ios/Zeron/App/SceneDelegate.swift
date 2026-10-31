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
            // Straight on from the sign-in steps: no sheet sliding up.
            self?.showOnboardingIfNeeded(animated: false)
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

        #if DEBUG
        if args.contains("-voice-preview") {
            app.voice.preview()
            if args.contains("-voice-stage") {
                DispatchQueue.main.async { [app] in window.rootViewController?.presentVoiceStage(app: app, source: nil) }
            }
        }
        #endif
        showOnboardingIfNeeded()
        #if DEBUG
        TerminalProbe.runIfAsked(app: app, router: window.rootViewController as? AppRouter)
        #endif
        if let router = window.rootViewController as? AppRouter, let i = args.firstIndex(of: "-route"), i + 1 < args.count {
            let route = args[i + 1]
            DispatchQueue.main.async {
                if route.hasPrefix("chat:") { router.openSession(String(route.dropFirst(5))) }
                if route == "new" { router.presentNewSession(prompt: nil) }
                if route == "more" { router.showSettings() }
                if route == "search" { router.showSearch() }
                #if DEBUG
                // Cloud screens straight away (screenshots, tests).
                if route.hasPrefix("cloud") || route.hasPrefix("providers") || route.hasPrefix("accounts") || route == "billing" || route == "welcome" { self.openCloudRoute(route) }
                #endif
            }
        }
    }

    /// First run after signing in (or `-onboarding <step>`: appearance, sync,
    /// github, agents, notifications, done): the steps after logging in, over
    /// the shell.
    @MainActor private func showOnboardingIfNeeded(animated: Bool = true) {
        let args = ProcessInfo.processInfo.arguments
        let forced = args.firstIndex(of: "-onboarding")
        guard app.isSignedIn, forced != nil || OnboardingViewController.shouldShow(for: app) else { return }
        var step = OnboardingViewController.Step.github
        if let i = forced, i + 1 < args.count {
            step = OnboardingViewController.Step(name: args[i + 1]) ?? .github
        }
        let onboarding = OnboardingViewController(app: app, step: step)
        onboarding.onFinish = { [weak self] startSession in
            guard startSession, let router = self?.window?.rootViewController as? AppRouter else { return }
            router.presentNewSession(prompt: nil)
        }
        DispatchQueue.main.async { [weak self] in
            self?.window?.rootViewController?.present(onboarding, animated: animated && !args.contains("-onboarding"))
        }
    }

    #if DEBUG
    /// `-route cloud` · `cloud:billing` · `cloud:agent:<harness>` · `cloud:repos`
    /// · `cloud:github` · `billing` · `providers[:cloud|:<device id>]` · `welcome`.
    @MainActor private func openCloudRoute(_ route: String) {
        guard let root = window?.rootViewController else { return }
        let parts = route.split(separator: ":").map(String.init)
        if parts.first == "providers" {
            return root.present(UINavigationController(rootViewController: ProvidersViewController(app: app, target: parts.dropFirst().first)), animated: false)
        }
        if parts.first == "accounts", let harness = parts.dropFirst().first, let host = app.hostOptions.first {
            // A computer's accounts for one agent (the first computer),
            // reached the way the app does: pushed from Providers.
            let nav = UINavigationController(rootViewController: ProvidersViewController(app: app, target: host.id))
            nav.pushViewController(
                AgentAccountsViewController(app: app, target: .device(id: host.id, name: host.name, harness: harness, label: harness)),
                animated: false
            )
            return root.present(nav, animated: false)
        }
        if parts.first == "welcome" {
            // The signed-out welcome page, without signing out.
            let welcome = SignInViewController(app: app)
            welcome.modalPresentationStyle = .fullScreen
            return root.present(welcome, animated: false)
        }
        if parts.first == "billing" {
            return root.present(UINavigationController(rootViewController: BillingViewController(app: app)), animated: false)
        }
        let settings = CloudSettingsViewController(app: app)
        let nav = UINavigationController(rootViewController: settings)
        switch parts.dropFirst().first {
        case "billing": nav.pushViewController(BillingViewController(app: app), animated: false)
        case "agent":
            if let h = parts.dropFirst(2).first, let agent = CloudStore.agents.first(where: { $0.harness == h }) {
                nav.pushViewController(AgentAccountsViewController(app: app, target: .cloud(agent)), animated: false)
            }
        case "repos":
            root.present(UINavigationController(rootViewController: CloudRepoPickerViewController(app: app)), animated: false)
            return
        case "github":
            root.present(GitHubConnectViewController.sheet(app: app), animated: false)
            return
        default: break
        }
        root.present(nav, animated: false)
    }
    #endif

    /// The signed-in shell is up: refresh this phone's notification
    /// registration and open a session a notification was tapped for.
    private func shellShown() {
        guard app.isSignedIn else { return }
        if !app.isDemo { PushNotifications.shared.signedIn() }
        PushNotifications.shared.shellReady()
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

    /// `zeron://voice` (the call's Live Activity) returns to the stage.
    func scene(_ scene: UIScene, openURLContexts URLContexts: Set<UIOpenURLContext>) {
        guard URLContexts.contains(where: { $0.url.scheme == "zeron" && $0.url.host == "voice" }),
              app.voice.live, let root = window?.rootViewController
        else { return }
        root.presentVoiceStage(app: app, source: nil)
    }

    func sceneDidEnterBackground(_ scene: UIScene) {
        app.didEnterBackground()
    }

    func sceneWillEnterForeground(_ scene: UIScene) {
        app.willEnterForeground()
    }
}
