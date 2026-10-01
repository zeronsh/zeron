import UIKit
import WebKit

/// Disposable document context: no filesystem bridge, credentials, or network.
@MainActor
final class NativeWorkspaceRenderer: NSObject, WKNavigationDelegate {
    private var webView: WKWebView?
    private var completion: CheckedContinuation<Data, Error>?
    private var watchdog: Task<Void, Never>?
    private var size = CGSize.zero
    private var format = "pdf"
    private var margin: CGFloat?

    func render(html: String, width: Int, height: Int, format: String, margin: CGFloat? = nil) async throws -> Data {
        guard (1...2048).contains(width), (1...2048).contains(height), width * height <= 4_000_000,
              ["pdf", "png"].contains(format) else { throw NativeWorkspaceFiles.failure("Render needs PNG or PDF and dimensions 1–2048 (at most 4 million pixels).") }
        guard completion == nil else { throw NativeWorkspaceFiles.failure("A render is already running") }
        self.size = CGSize(width: width, height: height); self.format = format; self.margin = margin
        let config = WKWebViewConfiguration()
        config.websiteDataStore = .nonPersistent()
        // The earliest CSP cannot be relaxed by a later meta tag in generated HTML.
        let policy = "default-src 'none'; script-src 'unsafe-inline'; style-src 'unsafe-inline'; img-src data: blob:; font-src data:; connect-src 'none'; frame-src 'none'; worker-src 'none'; object-src 'none'; base-uri 'none'; form-action 'none'"
        let document = "<!doctype html><meta http-equiv='Content-Security-Policy' content=\"\(policy)\"><meta name='viewport' content='width=device-width,initial-scale=1'>" + html
        let web = WKWebView(frame: CGRect(origin: .zero, size: size), configuration: config)
        web.navigationDelegate = self
        web.isInspectable = false
        webView = web
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                completion = continuation
                watchdog = Task { [weak self] in
                    do { try await Task.sleep(for: .seconds(4)) } catch { return }
                    self?.finish(.failure(NativeWorkspaceFiles.failure("Rendering timed out. Simplify the document or use fewer iterations.")))
                }
                web.loadHTMLString(document, baseURL: nil)
            }
        } onCancel: { Task { @MainActor [weak self] in self?.cancel() } }
    }
    func cancel() { finish(.failure(CancellationError())) }
    private func finish(_ result: Result<Data, Error>) {
        guard let completion else { return }
        self.completion = nil; watchdog?.cancel(); watchdog = nil
        webView?.stopLoading(); webView?.navigationDelegate = nil; webView = nil
        completion.resume(with: result)
    }
    func webView(_ webView: WKWebView, decidePolicyFor navigationAction: WKNavigationAction, decisionHandler: @escaping (WKNavigationActionPolicy) -> Void) {
        decisionHandler(navigationAction.request.url?.absoluteString == "about:blank" ? .allow : .cancel)
    }
    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        // Weak callback captures let a timed-out document be discarded even when
        // page JavaScript never settles; it must not retain its WebView indefinitely.
        webView.callAsyncJavaScript("await document.fonts.ready; await Promise.all(Array.from(document.images).map(i => i.decode())); if (window.zeronReady) await window.zeronReady; await new Promise(r => setTimeout(r, 50)); return true;", arguments: [:], in: nil, in: .page) { [weak self, weak webView] result in
            guard let self, let webView, self.completion != nil else { return }
            if case .failure(let error) = result { self.finish(.failure(error)); return }
            if let margin = self.margin {
                let printer = WorkspacePrintRenderer(size: self.size, margin: margin)
                printer.addPrintFormatter(webView.viewPrintFormatter(), startingAtPageAt: 0)
                printer.prepare(forDrawingPages: NSRange(location: 0, length: printer.numberOfPages))
                let count = printer.numberOfPages
                guard count > 0, count <= 100 else { self.finish(.failure(NativeWorkspaceFiles.failure("PDF must contain 1–100 pages"))); return }
                let pdf = UIGraphicsPDFRenderer(bounds: CGRect(origin: .zero, size: self.size)).pdfData { context in
                    for page in 0..<count { context.beginPage(); printer.drawPage(at: page, in: CGRect(origin: .zero, size: self.size)) }
                }
                self.finish(.success(pdf)); return
            }
            let options = WKPDFConfiguration(); options.rect = CGRect(origin: .zero, size: self.size)
            webView.createPDF(configuration: options) { [weak self] result in
                guard let self, self.completion != nil else { return }
                do {
                    let pdf = try result.get()
                    if self.format == "pdf" { self.finish(.success(pdf)); return }
                    guard let provider = CGDataProvider(data: pdf as CFData), let document = CGPDFDocument(provider), let page = document.page(at: 1) else { throw NativeWorkspaceFiles.failure("Could not rasterize rendered document") }
                    let rendererFormat = UIGraphicsImageRendererFormat(); rendererFormat.scale = 1; rendererFormat.opaque = true
                    let png = UIGraphicsImageRenderer(size: self.size, format: rendererFormat).pngData { context in
                        UIColor.white.setFill(); context.fill(CGRect(origin: .zero, size: self.size))
                        let cg = context.cgContext
                        cg.translateBy(x: 0, y: self.size.height); cg.scaleBy(x: 1, y: -1)
                        cg.concatenate(page.getDrawingTransform(.mediaBox, rect: CGRect(origin: .zero, size: self.size), rotate: 0, preserveAspectRatio: true))
                        cg.drawPDFPage(page)
                    }
                    self.finish(.success(png))
                } catch { self.finish(.failure(error)) }
            }
        }
    }
    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) { finish(.failure(error)) }
    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) { finish(.failure(error)) }
    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) { finish(.failure(NativeWorkspaceFiles.failure("Document renderer stopped"))) }
}

@MainActor
private final class WorkspacePrintRenderer: UIPrintPageRenderer {
    private let bounds: CGRect
    private let printable: CGRect
    init(size: CGSize, margin: CGFloat) { bounds = CGRect(origin: .zero, size: size); printable = bounds.insetBy(dx: margin, dy: margin); super.init() }
    override var paperRect: CGRect { bounds }
    override var printableRect: CGRect { printable }
}
