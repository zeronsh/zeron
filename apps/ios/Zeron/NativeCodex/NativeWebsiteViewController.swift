import UIKit
import WebKit

/// Independent, unprivileged website preview with a stable snapshot per refresh.
@MainActor
final class NativeWebsiteViewController: UIViewController, WKNavigationDelegate {
    private let shell: MobileShellRuntime
    private let entry: String?
    private let web: WKWebView
    private let progress = UIProgressView(progressViewStyle: .bar)
    private var observation: NSKeyValueObservation?
    private var allowedURL: URL?
    init(shell: MobileShellRuntime, entry: String? = nil) {
        self.shell = shell; self.entry = entry
        let config = WKWebViewConfiguration(); config.websiteDataStore = .nonPersistent()
        web = WKWebView(frame: .zero, configuration: config)
        super.init(nibName: nil, bundle: nil)
        web.navigationDelegate = self
    }
    deinit { NotificationCenter.default.removeObserver(self) }
    required init?(coder: NSCoder) { fatalError() }
    override func viewDidLoad() {
        super.viewDidLoad(); title = "Website preview"; view.backgroundColor = .systemBackground
        web.translatesAutoresizingMaskIntoConstraints = false; progress.translatesAutoresizingMaskIntoConstraints = false
        view.addSubview(web); view.addSubview(progress)
        NSLayoutConstraint.activate([
            web.leadingAnchor.constraint(equalTo: view.leadingAnchor), web.trailingAnchor.constraint(equalTo: view.trailingAnchor), web.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor), web.bottomAnchor.constraint(equalTo: view.bottomAnchor),
            progress.leadingAnchor.constraint(equalTo: view.leadingAnchor), progress.trailingAnchor.constraint(equalTo: view.trailingAnchor), progress.topAnchor.constraint(equalTo: view.safeAreaLayoutGuide.topAnchor)
        ])
        observation = web.observe(\.estimatedProgress, options: [.new]) { [weak self] web, _ in
            Task { @MainActor in self?.progress.progress = Float(web.estimatedProgress); self?.progress.isHidden = web.estimatedProgress >= 1 }
        }
        navigationItem.rightBarButtonItems = [
            UIBarButtonItem(title: "Refresh", image: UIImage(systemName: "arrow.clockwise"), primaryAction: UIAction { [weak self] _ in self?.refresh() }),
            UIBarButtonItem(title: "Stop", image: UIImage(systemName: "stop"), primaryAction: UIAction { [weak self] _ in self?.web.stopLoading(); self?.shell.stopPreview(); self?.navigationController?.popViewController(animated: true) })
        ]
        NotificationCenter.default.addObserver(self, selector: #selector(background), name: UIApplication.didEnterBackgroundNotification, object: nil)
        NotificationCenter.default.addObserver(self, selector: #selector(foreground), name: UIApplication.willEnterForegroundNotification, object: nil)
        refresh()
    }
    @objc private func background() { shell.stopPreview() }
    @objc private func foreground() { if view.window != nil { refresh() } }
    private func refresh() {
        progress.isHidden = false; progress.progress = 0.1
        navigationItem.rightBarButtonItems?.forEach { $0.isEnabled = false }
        Task {
            defer { navigationItem.rightBarButtonItems?.forEach { $0.isEnabled = true } }
            do { let url = try await shell.startPreview(entry: entry); allowedURL = url; web.load(URLRequest(url: url)) }
            catch { progress.isHidden = true; workspaceError(error) }
        }
    }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { progress.isHidden = true; workspaceError(error) }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { progress.isHidden = true; workspaceError(error) }
    func webView(_ webView: WKWebView, decidePolicyFor action: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        guard let url = action.request.url, let allowedURL, url.scheme == "http", url.host == "127.0.0.1", url.port == allowedURL.port else { decisionHandler(.cancel); return }
        decisionHandler(.allow)
    }
}
