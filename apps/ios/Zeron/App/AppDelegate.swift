import UIKit
import UserNotifications

@main
final class AppDelegate: UIResponder, UIApplicationDelegate {
    func application(
        _ application: UIApplication,
        didFinishLaunchingWithOptions launchOptions: [UIApplication.LaunchOptionsKey: Any]?
    ) -> Bool {
        // Before launch finishes, so a tap that launched the app is delivered.
        UNUserNotificationCenter.current().delegate = PushNotifications.shared
        // Before any scene connects.
        LegacySceneRepair.install()
        return true
    }

    func application(_ application: UIApplication, didRegisterForRemoteNotificationsWithDeviceToken deviceToken: Data) {
        PushNotifications.shared.didRegister(deviceToken: deviceToken)
    }

    func application(_ application: UIApplication, didFailToRegisterForRemoteNotificationsWithError error: Error) {
        PushNotifications.shared.didFailToRegister(error)
    }

    func application(
        _ application: UIApplication,
        configurationForConnecting connectingSceneSession: UISceneSession,
        options: UIScene.ConnectionOptions
    ) -> UISceneConfiguration {
        UISceneConfiguration(name: "Default", sessionRole: connectingSceneSession.role)
    }
}

/// Takes over an app scene restored from the legacy SwiftUI app's session.
///
/// Phones updated in place from the legacy Comet app (same bundle id, SwiftUI
/// `App` lifecycle) keep its persisted scene session, whose configuration
/// names SwiftUI's own scene delegate. UIKit restores that session without
/// asking `application(_:configurationForConnecting:options:)`, and system
/// frameworks load SwiftUI anyway, so the stored class resolves: `SceneDelegate`
/// never runs (a blank window) and SwiftUI's delegate traps with "Missing
/// AppGraph" once the scene deactivates. iPhone can't destroy the session (it
/// supports one scene) and hands out copies of its configuration, so swap the
/// delegate as the scene connects. UIKit has already sent this connection's
/// `willConnect`, including any cold-launch link, to the restored delegate.
enum LegacySceneRepair {
    static func install() {
        NotificationCenter.default.addObserver(forName: UIScene.willConnectNotification, object: nil, queue: nil) { note in
            MainActor.assumeIsolated {
                guard let scene = note.object as? UIWindowScene,
                      scene.session.role == .windowApplication,
                      let restored = scene.delegate, !(restored is SceneDelegate) else { return }
                NSLog("Crew: replacing restored scene delegate %@ with SceneDelegate", String(describing: type(of: restored)))
                let delegate = SceneDelegate()
                scene.delegate = delegate
                delegate.connect(scene, urlContexts: [])
            }
        }
    }
}
