import AuthenticationServices
import UIKit

enum Endpoints {
    static let edgeURL = URL(string: "https://edge.zeron.sh")!
    static var workosClientId: String {
        #if DEBUG
        if let id = debugOverride("-workos", key: "debugWorkosClientId") { return id }
        #endif
        return "client_01KWD0EAKZKD50YCQJNYSRE4BY"
    }
    static let callbackScheme = "zeron"

    #if DEBUG
    /// A launch argument that sticks: `-edge <url>` / `-workos <client>` point
    /// the app at another stack (staging) and are remembered, so reopening it
    /// from the home screen stays there. `-edge prod` goes back.
    static func debugOverride(_ flag: String, key: String) -> String? {
        let args = ProcessInfo.processInfo.arguments
        if let i = args.firstIndex(of: flag), i + 1 < args.count {
            let value = args[i + 1]
            if value == "prod" {
                UserDefaults.standard.removeObject(forKey: key)
                return nil
            }
            UserDefaults.standard.set(value, forKey: key)
            return value
        }
        return UserDefaults.standard.string(forKey: key)
    }
    #endif

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

/// Signed out: the welcome page, as on the desktop — the mark, a line, and
/// "Get started" into the first steps (a look, then logging in).
final class SignInViewController: UIViewController {
    private let app: AppModel

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        let wallpaper = WallpaperView()
        wallpaper.frame = view.bounds
        wallpaper.autoresizingMask = [.flexibleWidth, .flexibleHeight]
        view.addSubview(wallpaper)
        let mark = UIImageView(image: CloudUI.zeronMark)
        mark.tintColor = Palette.text
        mark.contentMode = .scaleAspectFit
        mark.translatesAutoresizingMaskIntoConstraints = false
        NSLayoutConstraint.activate([
            mark.widthAnchor.constraint(equalToConstant: 46),
            mark.heightAnchor.constraint(equalToConstant: 53),
        ])
        let title = UILabel()
        title.text = "Welcome to Zeron"
        title.font = Fonts.ui(.sansSemibold, 30)
        title.textColor = Palette.text
        title.textAlignment = .center
        title.numberOfLines = 0
        let tagline = UILabel()
        tagline.text = "Your coding agents, from anywhere."
        tagline.font = Fonts.ui(.sans, 17)
        tagline.textColor = Palette.secondary
        tagline.textAlignment = .center
        tagline.numberOfLines = 0
        let start = CloudUI.primaryButton("Get started") { [weak self] in self?.getStarted() }
        start.accessibilityIdentifier = "sign-in"

        let top = UIStackView(arrangedSubviews: [mark, title, tagline])
        top.axis = .vertical
        top.alignment = .center
        top.spacing = 12
        top.setCustomSpacing(26, after: mark)
        for v in [top, start] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            top.centerXAnchor.constraint(equalTo: guide.centerXAnchor),
            top.centerYAnchor.constraint(equalTo: guide.centerYAnchor, constant: -40),
            top.leadingAnchor.constraint(greaterThanOrEqualTo: guide.leadingAnchor, constant: 32),
            start.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 24),
            start.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -24),
            start.bottomAnchor.constraint(equalTo: guide.bottomAnchor, constant: -16),
        ])
    }

    private func getStarted() {
        // The look is this phone's: picked once, then straight to logging in.
        let seen = UserDefaults.standard.bool(forKey: OnboardingViewController.appearanceSeenKey)
        let onboarding = OnboardingViewController(app: app, step: seen ? .sync : .appearance)
        onboarding.modalTransitionStyle = .crossDissolve
        present(onboarding, animated: true)
    }
}

/// Sign in with WorkOS (system web auth sheet), then pick an organization
/// when there are several.
@MainActor
final class WorkOSSignIn: NSObject, ASWebAuthenticationPresentationContextProviding {
    private var session: ASWebAuthenticationSession?
    private weak var host: UIViewController?

    /// Signs in; returns why it didn't (nil on success or cancel).
    func run(app: AppModel, from host: UIViewController) async -> String? {
        self.host = host
        let state = UUID().uuidString
        let url: URL? = await withCheckedContinuation { cont in
            let s = ASWebAuthenticationSession(url: Endpoints.authorizeURL(state: state), callback: .customScheme(Endpoints.callbackScheme)) { url, error in
                if let error = error as? ASWebAuthenticationSessionError, error.code == .canceledLogin {
                    return cont.resume(returning: nil)
                }
                cont.resume(returning: url ?? URL(string: "zeron://callback?failed=1"))
            }
            s.presentationContextProvider = self
            s.prefersEphemeralWebBrowserSession = false
            session = s
            if !s.start() { cont.resume(returning: nil) }
        }
        session = nil
        guard let url else { return nil }
        guard let items = URLComponents(url: url, resolvingAgainstBaseURL: false)?.queryItems,
              let code = items.first(where: { $0.name == "code" })?.value,
              items.first(where: { $0.name == "state" })?.value == state
        else { return "Sign-in didn't complete. Try again." }
        do {
            try await app.signIn(code: code) { [weak self] orgs in await self?.pickOrg(orgs) }
            return nil
        } catch {
            return error.localizedDescription
        }
    }

    /// Several organizations: let the user pick (nil = cancelled).
    private func pickOrg(_ orgs: [AuthOrg]) async -> AuthOrg? {
        guard let host else { return nil }
        return await withCheckedContinuation { cont in
            let sheet = UIAlertController(title: "Choose an organization", message: nil, preferredStyle: .actionSheet)
            for org in orgs {
                sheet.addAction(UIAlertAction(title: org.name, style: .default) { _ in cont.resume(returning: org) })
            }
            sheet.addAction(UIAlertAction(title: "Cancel", style: .cancel) { _ in cont.resume(returning: nil) })
            sheet.popoverPresentationController?.sourceView = host.view
            host.present(sheet, animated: true)
        }
    }

    func presentationAnchor(for session: ASWebAuthenticationSession) -> ASPresentationAnchor {
        host?.view.window ?? ASPresentationAnchor()
    }
}
