import UIKit
import UserNotifications

/// Session notifications, like the desktop's: a run finished, a session is
/// waiting on you, a run failed. The edge decides and sends them (APNs); this
/// asks for permission, hands the edge this phone's token and choices, stays
/// quiet while the app is open (the desktop's "only in the background"), and
/// opens the session a notification is about.
@MainActor
final class PushNotifications: NSObject, UNUserNotificationCenterDelegate {
    static let shared = PushNotifications()

    enum Kind: String, CaseIterable {
        case done, input, failed

        var title: String {
            switch self {
            case .done: "Run finished"
            case .input: "Waiting on your input"
            case .failed: "Run failed"
            }
        }
    }

    private weak var app: AppModel?
    private var token: String?
    /// A tapped notification's chat, until the app can show it.
    private var pendingChat: String?
    /// Opens a chat once the shell is up (set by the scene).
    var openChat: ((String) -> Bool)?

    private let defaults = UserDefaults.standard
    private static let enabledKey = "notifications.enabled"
    private static let askedKey = "notifications.asked"

    // MARK: Choices

    /// The master switch (on unless turned off here; the system permission
    /// is separate).
    var enabled: Bool {
        get { defaults.object(forKey: Self.enabledKey) as? Bool ?? true }
        set {
            defaults.set(newValue, forKey: Self.enabledKey)
            Task { await self.sync() }
        }
    }

    func isOn(_ kind: Kind) -> Bool { defaults.object(forKey: "notifications.\(kind.rawValue)") as? Bool ?? true }

    func set(_ kind: Kind, _ on: Bool) {
        defaults.set(on, forKey: "notifications.\(kind.rawValue)")
        Task { await self.sync() }
    }

    private var prefs: PushPrefs { PushPrefs(done: isOn(.done), input: isOn(.input), failed: isOn(.failed)) }

    // MARK: Lifecycle

    /// At launch: route taps; once signed in, keep the edge's registration fresh.
    func install(app: AppModel) {
        self.app = app
        UNUserNotificationCenter.current().delegate = self
    }

    /// Signed in (or launched signed in): re-register if already allowed.
    /// Tokens can change between launches; the edge keeps the latest.
    func signedIn() {
        Task {
            let settings = await UNUserNotificationCenter.current().notificationSettings()
            if Self.allowed(settings.authorizationStatus) {
                UIApplication.shared.registerForRemoteNotifications()
            }
        }
    }

    /// The first time a session is started from this phone: the moment the
    /// ask makes sense ("tell me when it's done").
    func askAfterFirstSession() {
        guard app?.isDemo == false, enabled, !defaults.bool(forKey: Self.askedKey) else { return }
        defaults.set(true, forKey: Self.askedKey)
        Task { _ = await self.requestPermission() }
    }

    /// Ask the system (once; later it's the Settings app's call).
    @discardableResult
    func requestPermission() async -> Bool {
        let center = UNUserNotificationCenter.current()
        let status = await center.notificationSettings().authorizationStatus
        let granted: Bool
        if status == .notDetermined {
            granted = (try? await center.requestAuthorization(options: [.alert, .sound, .badge])) ?? false
        } else {
            granted = Self.allowed(status)
        }
        if granted { UIApplication.shared.registerForRemoteNotifications() }
        return granted
    }

    func authorizationStatus() async -> UNAuthorizationStatus {
        await UNUserNotificationCenter.current().notificationSettings().authorizationStatus
    }

    private static func allowed(_ status: UNAuthorizationStatus) -> Bool {
        status == .authorized || status == .provisional || status == .ephemeral
    }

    /// Signing out: this phone stops getting the account's notifications.
    func signingOut(client: CoreClient?) {
        token = nil
        if let client { Task.detached { try? await client.unregisterPushTarget() } }
        UIApplication.shared.unregisterForRemoteNotifications()
    }

    // MARK: Token

    func didRegister(deviceToken: Data) {
        token = deviceToken.map { String(format: "%02x", $0) }.joined()
        Task { await sync() }
    }

    func didFailToRegister(_ error: Error) {
        NSLog("push registration failed: \(error)")
    }

    /// Tell the edge what this phone wants now (or that it wants nothing).
    private func sync() async {
        guard let client = app?.client, app?.isDemo == false else { return }
        do {
            if enabled, let token {
                try await client.registerPushTarget(token: token, environment: Self.apnsEnvironment, prefs: prefs)
            } else if !enabled {
                try await client.unregisterPushTarget()
            }
        } catch {
            NSLog("push target sync failed: \(error)")
        }
    }

    /// Which APNs this build's tokens belong to: development-signed builds
    /// (Xcode) say so in their provisioning profile; App Store / TestFlight
    /// builds carry none and are production.
    static let apnsEnvironment: String = {
        #if targetEnvironment(simulator)
        return "sandbox"
        #else
        guard let url = Bundle.main.url(forResource: "embedded", withExtension: "mobileprovision"),
              let data = try? Data(contentsOf: url),
              let text = String(data: data, encoding: .isoLatin1)
        else { return "production" }
        return text.range(of: "<key>aps-environment</key>\\s*<string>development</string>", options: .regularExpression) != nil
            ? "sandbox" : "production"
        #endif
    }()

    // MARK: Delivery

    // Completion-handler forms, on the main queue: the async variants
    // complete off the main thread, which UIKit asserts on (crash on tap).

    /// In the app: no banner (the list and the session already show it).
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        willPresent notification: UNNotification,
        withCompletionHandler completionHandler: @escaping (UNNotificationPresentationOptions) -> Void
    ) {
        completionHandler([])
    }

    /// Tapped: open that session.
    nonisolated func userNotificationCenter(
        _ center: UNUserNotificationCenter,
        didReceive response: UNNotificationResponse,
        withCompletionHandler completionHandler: @escaping () -> Void
    ) {
        let chatId = response.notification.request.content.userInfo["chatId"] as? String
        DispatchQueue.main.async {
            MainActor.assumeIsolated {
                if let chatId { self.open(chatId) }
            }
            completionHandler()
        }
    }

    private func open(_ chatId: String) {
        if openChat?(chatId) != true { pendingChat = chatId }
    }

    /// The shell is up: open a chat tapped before it was.
    func shellReady() {
        guard let chat = pendingChat else { return }
        pendingChat = nil
        _ = openChat?(chat)
    }
}
