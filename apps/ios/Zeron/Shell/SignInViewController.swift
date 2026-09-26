import AuthenticationServices
import UIKit

enum Endpoints {
    static let edgeURL = URL(string: "https://edge.zeron.sh")!
    static let workosClientId = "client_01KWD0EAKZKD50YCQJNYSRE4BY"
    static let callbackScheme = "zeron"

    static func authorizeURL(state: String) -> URL {
        var c = URLComponents(string: "https://api.workos.com/user_management/authorize")!
        c.queryItems = [
            URLQueryItem(name: "response_type", value: "code"),
            URLQueryItem(name: "client_id", value: workosClientId),
            URLQueryItem(name: "redirect_uri", value: "\(callbackScheme)://callback"),
            URLQueryItem(name: "provider", value: "authkit"),
            URLQueryItem(name: "state", value: state),
        ]
        return c.url!
    }
}

/// Sign in with WorkOS (system web auth sheet), or explore the offline demo.
final class SignInViewController: UIViewController, ASWebAuthenticationPresentationContextProviding {
    private let app: AppModel
    private let status = UILabel()
    private let signIn = UIButton(type: .system)
    private var session: ASWebAuthenticationSession?

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        let mark = UIImageView(image: UIImage(systemName: "sparkle", withConfiguration: UIImage.SymbolConfiguration(pointSize: 44, weight: .light)))
        mark.tintColor = Palette.text
        let title = UILabel()
        title.text = "Zeron"
        title.font = Fonts.ui(.sansSemibold, 34)
        title.textColor = Palette.text
        let tagline = UILabel()
        tagline.text = "Your coding agents, from anywhere."
        tagline.font = Fonts.ui(.sans, 17)
        tagline.textColor = Palette.secondary

        var primary = UIButton.Configuration.filled()
        primary.title = "Sign In"
        primary.baseBackgroundColor = Palette.text
        primary.baseForegroundColor = Palette.background
        primary.cornerStyle = .capsule
        primary.contentInsets = NSDirectionalEdgeInsets(top: 15, leading: 20, bottom: 15, trailing: 20)
        primary.titleTextAttributesTransformer = UIConfigurationTextAttributesTransformer { a in
            var a = a
            a.font = Fonts.ui(.sansSemibold, 17)
            a.foregroundColor = Palette.background
            return a
        }
        signIn.configuration = primary
        signIn.accessibilityIdentifier = "sign-in"
        signIn.addAction(UIAction { [weak self] _ in self?.startWorkOS() }, for: .touchUpInside)

        var secondary = UIButton.Configuration.glass()
        secondary.title = "Explore the Demo"
        secondary.baseForegroundColor = Palette.text
        secondary.cornerStyle = .capsule
        secondary.contentInsets = NSDirectionalEdgeInsets(top: 15, leading: 20, bottom: 15, trailing: 20)
        let demo = UIButton(configuration: secondary, primaryAction: UIAction { [weak self] _ in self?.app.enterDemo() })
        demo.accessibilityIdentifier = "demo"

        status.font = Fonts.ui(.sans, 14)
        status.textColor = Palette.danger
        status.numberOfLines = 0
        status.textAlignment = .center

        let top = UIStackView(arrangedSubviews: [mark, title, tagline])
        top.axis = .vertical
        top.alignment = .center
        top.spacing = 12
        let buttons = UIStackView(arrangedSubviews: [status, signIn, demo])
        buttons.axis = .vertical
        buttons.spacing = 12
        for v in [top, buttons] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        NSLayoutConstraint.activate([
            top.centerXAnchor.constraint(equalTo: view.centerXAnchor),
            top.centerYAnchor.constraint(equalTo: view.centerYAnchor, constant: -80),
            buttons.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            buttons.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            buttons.bottomAnchor.constraint(equalTo: view.safeAreaLayoutGuide.bottomAnchor, constant: -24),
        ])
    }

    private func startWorkOS() {
        let state = UUID().uuidString
        let s = ASWebAuthenticationSession(url: Endpoints.authorizeURL(state: state), callback: .customScheme(Endpoints.callbackScheme)) { [weak self] url, error in
            guard let self else { return }
            if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin { return }
            guard let url, let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
                  let code = items.first(where: { $0.name == "code" })?.value,
                  items.first(where: { $0.name == "state" })?.value == state
            else {
                self.status.text = "Sign-in didn't complete. Try again."
                return
            }
            self.signIn.configuration?.showsActivityIndicator = true
            Task { @MainActor in
                do {
                    try await self.app.signIn(code: code) { orgs in await self.pickOrg(orgs) }
                } catch {
                    self.status.text = error.localizedDescription
                }
                self.signIn.configuration?.showsActivityIndicator = false
            }
        }
        s.presentationContextProvider = self
        s.prefersEphemeralWebBrowserSession = false
        session = s
        s.start()
    }

    /// Several organizations: let the user pick (nil = cancelled).
    @MainActor
    private func pickOrg(_ orgs: [AuthOrg]) async -> AuthOrg? {
        await withCheckedContinuation { cont in
            let sheet = UIAlertController(title: "Choose an organization", message: nil, preferredStyle: .actionSheet)
            for org in orgs {
                sheet.addAction(UIAlertAction(title: org.name, style: .default) { _ in cont.resume(returning: org) })
            }
            sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in cont.resume(returning: nil) })
            sheet.popoverPresentationController?.sourceView = signIn
            present(sheet, animated: true)
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        view.window ?? ASPresentationAnchor()
    }
}
