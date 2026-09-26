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
        window.makeKeyAndVisible()
        self.window = window
    }
}
