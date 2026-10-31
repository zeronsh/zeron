import UIKit

/// Ways into Cloud's pages from anywhere in the app (a toast's button, a
/// chat's banner), and the Cloud errors those places show.
@MainActor
enum CloudRoutes {
    /// Short enough for a toast beside its button.
    static let outOfCreditsMessage = "Out of Cloud credits"

    /// Settings → Billing over whatever is showing.
    static func openBilling(app: AppModel, from host: UIViewController) {
        let billing = BillingViewController(app: app)
        billing.navigationItem.rightBarButtonItem = UIBarButtonItem(systemItem: .done, primaryAction: UIAction { [weak billing] _ in
            billing?.dismiss(animated: true)
        })
        let nav = UINavigationController(rootViewController: billing)
        nav.modalPresentationStyle = .pageSheet
        (host.presentedViewController ?? host).present(nav, animated: true)
    }

    /// The desktop's out-of-credits toast, with its "Add credits" button.
    static func showOutOfCredits(app: AppModel, from host: UIViewController, above anchor: UIView? = nil) {
        UINotificationFeedbackGenerator().notificationOccurred(.warning)
        Toast.show(outOfCreditsMessage, action: "Add credits", in: host.view.window, above: anchor) { [weak host] in
            guard let host else { return }
            openBilling(app: app, from: host)
        }
    }

    /// A Cloud error worth a button: out of credits gets "Add credits".
    static func isOutOfCredits(_ error: Error) -> Bool {
        CloudStore.message(error).localizedCaseInsensitiveContains("out of Cloud credits")
    }
}
