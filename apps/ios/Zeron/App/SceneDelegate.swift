import UIKit

final class SceneDelegate: UIResponder, UIWindowSceneDelegate {
    var window: UIWindow?

    func scene(
        _ scene: UIScene,
        willConnectTo session: UISceneSession,
        options connectionOptions: UIScene.ConnectionOptions
    ) {
        guard let scene = scene as? UIWindowScene else { return }
        let window = UIWindow(windowScene: scene)
        let args = ProcessInfo.processInfo.arguments
        let root: UIViewController = args.contains("-lab")
            ? MainTabController.nav(TranscriptLabViewController())
            : MainTabController(app: AppModel())
        window.rootViewController = root
        if let tabs = root as? MainTabController, let i = args.firstIndex(of: "-route"), i + 1 < args.count {
            let route = args[i + 1]
            DispatchQueue.main.async {
                if route.hasPrefix("chat:") { tabs.openSession(String(route.dropFirst(5))) }
                if route == "new" { tabs.presentNewSession() }
            }
        }
        window.makeKeyAndVisible()
        self.window = window
    }
}
