import AuthenticationServices
import UIKit

enum Endpoints {
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

    /// WorkOS sign-in through `server`'s own client.
    static func authorizeURL(server: SavedServer, state: String) -> URL? {
        URL(string: workosAuthorizeUrl(workosClientId: server.workosClientId, state: state))
    }
}

/// Signed out: the welcome page, as on the desktop — the mark, a line, and
/// "Get started" into the first steps (a look, then logging in). Another
/// server is under More in the corner; once one is chosen, it heads the page.
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
            mark.widthAnchor.constraint(equalToConstant: 53),
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
        let more = WelcomeMoreButton(app: app, host: self)
        // Over the mark, outside its stack: the hero sits where it always
        // does, with or without a server above it.
        let identity = ServerIdentityView(app: app, host: self, centered: true)

        let top = UIStackView(arrangedSubviews: [mark, title, tagline])
        top.axis = .vertical
        top.alignment = .center
        top.spacing = 12
        top.setCustomSpacing(26, after: mark)
        for v in [top, start, identity, more] {
            v.translatesAutoresizingMaskIntoConstraints = false
            view.addSubview(v)
        }
        let guide = view.safeAreaLayoutGuide
        NSLayoutConstraint.activate([
            top.centerXAnchor.constraint(equalTo: guide.centerXAnchor),
            top.centerYAnchor.constraint(equalTo: guide.centerYAnchor, constant: -40),
            top.leadingAnchor.constraint(greaterThanOrEqualTo: guide.leadingAnchor, constant: 32),
            identity.bottomAnchor.constraint(equalTo: top.topAnchor, constant: -26),
            identity.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 32),
            identity.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -32),
            // Where the first steps put it (level with their progress), so it
            // stays put through the cross-fade into them.
            more.centerYAnchor.constraint(equalTo: guide.topAnchor, constant: OnboardingViewController.topRowCenter),
            more.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: WelcomeMoreButton.glyphInset - 24),
            start.leadingAnchor.constraint(equalTo: guide.leadingAnchor, constant: 24),
            start.trailingAnchor.constraint(equalTo: guide.trailingAnchor, constant: -24),
            start.bottomAnchor.constraint(equalTo: guide.bottomAnchor, constant: -16),
        ])
        // The page and its way in first; More, the corner's extra, last.
        view.accessibilityElements = [identity, title, tagline, start, more]
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

    /// A sign-in is under way (the browser is up, or its code is being
    /// exchanged): an invite link waits for none of it, and is dropped.
    private(set) static var isRunning = false

    /// Signs in on the chosen server; returns why it didn't (nil on success
    /// or cancel).
    func run(app: AppModel, from host: UIViewController) async -> String? {
        Self.isRunning = true
        defer { Self.isRunning = false }
        self.host = host
        let state = UUID().uuidString
        let server = app.signInServer
        guard let authorize = Endpoints.authorizeURL(server: server.reached, state: state) else { return "Sign-in didn't complete. Try again." }
        let url: URL? = await withCheckedContinuation { cont in
            let s = ASWebAuthenticationSession(url: authorize, callback: .customScheme(Endpoints.callbackScheme)) { url, error in
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
            try await app.signIn(code: code, server: server) { [weak self] orgs in await self?.pickOrg(orgs) }
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
