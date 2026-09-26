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
        let root = UIViewController()
        root.view.backgroundColor = .systemBackground
        let label = UILabel()
        label.text = "Zeron core \(coreVersion())"
        label.translatesAutoresizingMaskIntoConstraints = false
        root.view.addSubview(label)
        NSLayoutConstraint.activate([
            label.centerXAnchor.constraint(equalTo: root.view.centerXAnchor),
            label.centerYAnchor.constraint(equalTo: root.view.centerYAnchor),
        ])
        window.rootViewController = root
        window.makeKeyAndVisible()
        self.window = window
    }
}
