import SafariServices
import UIKit

/// Connect GitHub for Cloud: GitHub's device flow. The code is shown big
/// (and copied), GitHub opens in a browser sheet, and the page follows the
/// approval until the account is connected.
final class GitHubConnectViewController: UIViewController {
    private let app: AppModel
    /// Called once connected (onboarding moves on).
    var onConnected: ((String?) -> Void)?

    private let codeLabel = UILabel()
    private let status = UILabel()
    private let spinner = UIActivityIndicatorView(style: .medium)
    private var openButton: UIButton!
    private var flow: GithubDeviceFlowInfo?
    private var poll: Task<Void, Never>?

    static func sheet(app: AppModel, onConnected: ((String?) -> Void)? = nil) -> UIViewController {
        let vc = GitHubConnectViewController(app: app)
        vc.onConnected = onConnected
        let nav = UINavigationController(rootViewController: vc)
        if let sheet = nav.sheetPresentationController {
            sheet.detents = [.medium(), .large()]
            sheet.prefersGrabberVisible = true
        }
        return nav
    }

    init(app: AppModel) {
        self.app = app
        super.init(nibName: nil, bundle: nil)
    }

    required init?(coder: NSCoder) { fatalError() }

    deinit { poll?.cancel() }

    override func viewDidLoad() {
        super.viewDidLoad()
        view.backgroundColor = Palette.background
        navigationItem.leftBarButtonItem = UIBarButtonItem(systemItem: .close, primaryAction: UIAction { [weak self] _ in
            self?.dismiss(animated: true)
        })
        let tile = CloudUI.tile(image: CloudUI.githubMark, side: 56)
        let header = CloudUI.header("Connect GitHub", "Enter this code on GitHub to let Cloud agents clone your repositories and push their work.", alignment: .center)

        codeLabel.font = UIFont.monospacedSystemFont(ofSize: 34, weight: .semibold)
        codeLabel.textColor = Palette.text
        codeLabel.textAlignment = .center
        codeLabel.text = "· · · ·"
        codeLabel.accessibilityIdentifier = "github-code"
        let codeCard = UIView()
        codeCard.backgroundColor = Palette.elevated
        codeCard.layer.cornerRadius = 18
        codeCard.layer.cornerCurve = .continuous
        codeLabel.translatesAutoresizingMaskIntoConstraints = false
        codeCard.addSubview(codeLabel)
        NSLayoutConstraint.activate([
            codeLabel.topAnchor.constraint(equalTo: codeCard.topAnchor, constant: 20),
            codeLabel.bottomAnchor.constraint(equalTo: codeCard.bottomAnchor, constant: -20),
            codeLabel.leadingAnchor.constraint(equalTo: codeCard.leadingAnchor, constant: 16),
            codeLabel.trailingAnchor.constraint(equalTo: codeCard.trailingAnchor, constant: -16),
        ])

        status.font = Fonts.ui(.sans, 14)
        status.textColor = Palette.secondary
        status.textAlignment = .center
        status.numberOfLines = 0
        let statusRow = UIStackView(arrangedSubviews: [spinner, status])
        statusRow.spacing = 8
        statusRow.alignment = .center

        openButton = CloudUI.primaryButton("Copy Code & Open GitHub", symbol: "arrow.up.right") { [weak self] in self?.openGitHub() }
        openButton.isEnabled = false
        openButton.accessibilityIdentifier = "github-open"

        let stack = UIStackView(arrangedSubviews: [tile, header, codeCard, statusRow, openButton])
        stack.axis = .vertical
        stack.alignment = .center
        stack.spacing = 22
        stack.setCustomSpacing(16, after: tile)
        stack.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(stack)
        NSLayoutConstraint.activate([
            stack.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor, constant: 8),
            stack.leadingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.leadingAnchor, constant: 24),
            stack.trailingAnchor.constraint(equalTo: view.safeAreaLayoutGuide.trailingAnchor, constant: -24),
            header.widthAnchor.constraint(equalTo: stack.widthAnchor),
            codeCard.widthAnchor.constraint(equalTo: stack.widthAnchor),
            openButton.widthAnchor.constraint(equalTo: stack.widthAnchor),
        ])
        start()
    }

    private func setStatus(_ text: String, busy: Bool, color: UIColor = Palette.secondary) {
        status.text = text
        status.textColor = color
        busy ? spinner.startAnimating() : spinner.stopAnimating()
    }

    private func start() {
        guard let client = app.client else { return }
        setStatus("Getting a code…", busy: true)
        Task { @MainActor [weak self] in
            do {
                let flow = try await client.githubConnectStart()
                guard let self else { return }
                self.flow = flow
                self.codeLabel.text = flow.userCode
                self.openButton.isEnabled = true
                self.setStatus("Waiting for GitHub…", busy: true)
                self.follow(flow, client: client)
            } catch {
                self?.setStatus(CloudStore.message(error), busy: false, color: Palette.danger)
            }
        }
    }

    private func openGitHub() {
        guard let flow, let url = URL(string: flow.verificationUri) else { return }
        UIPasteboard.general.string = flow.userCode
        present(SFSafariViewController(url: url), animated: true)
    }

    private func follow(_ flow: GithubDeviceFlowInfo, client: CoreClient) {
        poll?.cancel()
        poll = Task { @MainActor [weak self] in
            let interval = UInt64(max(flow.intervalSecs, 2)) * 1_000_000_000
            while !Task.isCancelled {
                try? await Task.sleep(nanoseconds: interval)
                guard let self, !Task.isCancelled else { return }
                guard let progress = try? await client.githubConnectPoll(flowId: flow.flowId) else { continue }
                switch progress.state {
                case "connected":
                    await self.connected(progress.account)
                    return
                case "failed":
                    self.setStatus(progress.error ?? "GitHub didn't connect. Try again.", busy: false, color: Palette.danger)
                    return
                default:
                    continue
                }
            }
        }
    }

    @MainActor private func connected(_ account: String?) async {
        UINotificationFeedbackGenerator().notificationOccurred(.success)
        codeLabel.text = "Connected"
        codeLabel.font = Fonts.ui(.sansSemibold, 26)
        codeLabel.textColor = Palette.success
        setStatus(account.map { "Signed in to GitHub as @\($0)." } ?? "GitHub is connected.", busy: false)
        openButton.isHidden = true
        if presentedViewController != nil { dismiss(animated: true) }
        await app.cloud.refreshGitHub()
        let done = onConnected
        try? await Task.sleep(nanoseconds: 900_000_000)
        dismiss(animated: true) { done?(account) }
    }
}
