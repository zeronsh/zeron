import Foundation
import WebKit
import UIKit
import ImageIO

/// Disposable shell interpreter. NativeWorkspaceStore owns all persistent files.
/// No device paths, credentials, or network capabilities enter the worker.
@MainActor
final class MobileShellRuntime: NSObject, WKNavigationDelegate {
    struct Result: Decodable {
        let stdout: String
        let stderr: String
        let exitCode: Int
        var changedPaths: [String] = []

        private enum CodingKeys: String, CodingKey { case stdout, stderr, exitCode }
    }

    enum Failure: LocalizedError {
        case message(String)
        var errorDescription: String? { if case .message(let text) = self { return text }; return nil }
    }

    private let webView: WKWebView
    var generatedImagesDirectory: URL?
    private var renderer: NativeWorkspaceRenderer?
    private var previewServer: NativeWorkspaceServer?
    private(set) var previewEntry = "/workspace/index.html"
    private(set) var previewURL: URL?
    let store: NativeWorkspaceStore
    private var generation: String?
    private var commandID: String?
    private var navigation: CheckedContinuation<Void, Error>?
    private var loaded = false
    private var initialized = false
    private var busy = false

    init(checkpointURL: URL) {
        store = NativeWorkspaceStore(checkpointURL: checkpointURL)
        let configuration = WKWebViewConfiguration()
        configuration.websiteDataStore = .nonPersistent()
        webView = WKWebView(frame: .zero, configuration: configuration)
        super.init()
        webView.navigationDelegate = self
        configuration.userContentController.addScriptMessageHandler(WorkspaceBridge(owner: self), contentWorld: .page, name: "workspace")
    }

    deinit { previewServer?.stop() }

    func execute(_ command: String) async throws -> Result {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true
        let id = UUID().uuidString
        commandID = id
        defer { busy = false; commandID = nil }
        try await store.beginCommand(id)
        do {
            try await prepare(commandID: id)
            guard commandID == id else { throw Failure.message("Cancelled") }
            let response = try await request(["method": "exec", "command": command])
            var value = try JSONDecoder().decode(Result.self, from: response)
            value.changedPaths = await store.changedPaths()
            await store.endCommand(id)
            return value
        } catch {
            await cancel()
            let paths = await store.changedPaths()
            let suffix = paths.isEmpty ? "" : " Saved changes remain in: " + paths.joined(separator: ", ")
            throw Failure.message(error.localizedDescription + suffix)
        }
    }

    func snapshot() async throws -> [NativeWorkspaceEntry] {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        return try await store.snapshot()
    }

    func entries() async throws -> [NativeWorkspaceEntry] {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        return try await store.entries()
    }
    func fileData(_ path: String) async throws -> Data {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        return try await store.data(path)
    }
    func exportSelection(path: String?, to staging: URL) async throws -> URL {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        return try await store.exportSelection(path: path, to: staging)
    }

    func startPreview(entry: String? = nil) async throws -> URL {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        return try await publishPreview(entry: entry ?? previewEntry)
    }
    private func publishPreview(entry: String, expectedCommand: String? = nil) async throws -> URL {
        let relative = try NativeWorkspaceFiles.relativePath(entry)
        guard ["html", "htm"].contains((entry as NSString).pathExtension.lowercased()) else { throw Failure.message("Serve an HTML entry file") }
        _ = try await store.data(entry)
        let stage = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        do {
            let root = try await store.exportSelection(path: nil, to: stage)
            if let expectedCommand, commandID != expectedCommand { throw CancellationError() }
            let server = try NativeWorkspaceServer(root: root)
            let url = try await server.start(entry: relative)
            if let expectedCommand, commandID != expectedCommand { server.stop(); throw CancellationError() }
            previewServer?.stop(); previewServer = server; previewURL = url; previewEntry = entry
            return url
        } catch { try? FileManager.default.removeItem(at: stage); throw error }
    }
    func stopPreview() { previewServer?.stop(); previewServer = nil; previewURL = nil }

    func importURLs(_ urls: [URL]) async throws -> Int {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        return try await store.importURLs(urls)
    }

    func importEntries(_ entries: [NativeWorkspaceEntry]) async throws {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        try await store.importEntries(entries)
    }

    func readFile(_ path: String) async throws -> String {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        return try await store.readFile(path)
    }

    func writeFile(_ path: String, content: String) async throws {
        guard !busy else { throw Failure.message("A mobile tool is already running") }
        busy = true; defer { busy = false }
        try await store.writeFile(path, content: content)
    }

    /// Stop JS, reject further callbacks, and drain any already accepted native write.
    /// Acknowledged mutations survive cancellation; interrupted scripts are never replayed.
    func cancel() async {
        renderer?.cancel(); renderer = nil
        let id = commandID
        if let id { id.withCString { zeron_git_cancel($0) } }
        commandID = nil
        generation = nil
        initialized = false
        if let id { await store.endCommand(id) }
        if loaded { _ = try? await webView.evaluateJavaScript("globalThis.mobileShell?.stop('Cancelled')") }
    }

    fileprivate func filesystem(_ body: Any) async throws -> [String: Any] {
        guard let message = body as? [String: Any], let token = message["generation"] as? String,
              token == generation, let commandID, let request = message["request"] as? [String: Any] else {
            throw Failure.message("Shell filesystem request has expired")
        }
        if request["method"] as? String == "serve", let args = request["args"] as? [String] {
            guard args.count <= 1 else { throw Failure.message("Usage: serve [/workspace/index.html] or serve stop") }
            if args.first == "stop" { stopPreview(); return ["value": "Preview stopped"] }
            let url = try await publishPreview(entry: args.first ?? previewEntry, expectedCommand: commandID)
            return ["value": "Serving a snapshot at \(url.absoluteString). Open Preview website in the chat menu. Relative HTML/CSS/JS/assets and JSON fetch are supported. Run serve again or tap Refresh to publish edits. Foreground only; no Node/server-side code."]
        }
        if request["method"] as? String == "pdf", let args = request["args"] as? [String] {
            let usage = "Usage: pdf html /workspace/input.html /workspace/output.pdf [a4|letter] [margin_points]; or pdf images /workspace/output.pdf /workspace/image.png [...]. Image pages use A4 and 36-point margins."
            let data: Data, output: String
            if args.first == "html", (3...5).contains(args.count) {
                output = args[2]
                let paper = args.count > 3 ? args[3] : "a4"
                guard paper == "a4" || paper == "letter", let margin = Double(args.count > 4 ? args[4] : "36"), margin.isFinite, (0...144).contains(margin) else { throw Failure.message(usage) }
                let document = NativeWorkspaceRenderer(); renderer = document; defer { renderer = nil }
                data = try await document.render(html: store.readFile(args[1]), width: paper == "a4" ? 595 : 612, height: paper == "a4" ? 842 : 792, format: "pdf", margin: margin)
            } else if args.first == "images", (3...26).contains(args.count) {
                output = args[1]
                var images: [UIImage] = []
                var pixels = 0
                for path in args.dropFirst(2) {
                    let bytes = try await store.data(path)
                    guard let source = CGImageSourceCreateWithData(bytes as CFData, nil), let properties = CGImageSourceCopyPropertiesAtIndex(source, 0, nil) as? [CFString: Any],
                          let width = properties[kCGImagePropertyPixelWidth] as? Int, let height = properties[kCGImagePropertyPixelHeight] as? Int,
                          width > 0, height > 0, width <= 8192, height <= 8192 else { throw Failure.message("Invalid or oversized image") }
                    pixels += width * height
                    guard pixels <= 16_000_000, let image = UIImage(data: bytes) else { throw Failure.message("Images exceed the 16-million-pixel PDF limit") }
                    images.append(image)
                }
                data = UIGraphicsPDFRenderer(bounds: CGRect(x: 0, y: 0, width: 595, height: 842)).pdfData { context in
                    for image in images {
                        context.beginPage()
                        let scale = min(523 / image.size.width, 770 / image.size.height)
                        let size = CGSize(width: image.size.width * scale, height: image.size.height * scale)
                        image.draw(in: CGRect(x: (595 - size.width) / 2, y: (842 - size.height) / 2, width: size.width, height: size.height))
                    }
                }
            } else { throw Failure.message(usage) }
            guard (output as NSString).pathExtension.lowercased() == "pdf" else { throw Failure.message("Output must be a /workspace/*.pdf path") }
            try await store.saveArtifact(output, data: data, commandID: commandID)
            let index = try await store.handle(["method": "index"], commandID: commandID)
            return ["value": "Saved \(output) (\(data.count) bytes). Open Workspace files to preview or Save to Files.", "paths": index["value"] ?? []]
        }
        if request["method"] as? String == "git", let args = request["args"] as? [String] {
            let cwd = request["cwd"] as? String ?? "/workspace"
            return try await store.git(["-C", cwd] + args, id: commandID)
        }
        if let method = request["method"] as? String, method == "render" || method == "importImage" {
            guard let args = request["args"] as? [String] else { throw Failure.message("Invalid artifact arguments") }
            let path: String
            let data: Data
            if method == "importImage" {
                guard (1...2).contains(args.count), let directory = generatedImagesDirectory else { throw Failure.message("Usage: import_image GENERATED_FILENAME.png [/workspace/output.png]") }
                let name = try NativeWorkspaceArtifacts.name(args[0])
                path = args.count == 2 ? args[1] : "/workspace/generated/" + name
                data = try await Task.detached { try NativeWorkspaceArtifacts.read(args[0], directory: directory) }.value
            } else {
                guard args.count == 4, let width = Int(args[2]), let height = Int(args[3]) else { throw Failure.message("Usage: render /workspace/input.html /workspace/output.pdf|png WIDTH HEIGHT. Self-contained HTML, inline JS/canvas/SVG and data images only. Optional window.zeronReady Promise.") }
                path = args[1]
                _ = try NativeWorkspaceFiles.relativePath(path)
                let html = try await store.readFile(args[0])
                let document = NativeWorkspaceRenderer(); renderer = document
                defer { renderer = nil }
                data = try await document.render(html: html, width: width, height: height, format: (path as NSString).pathExtension.lowercased())
            }
            try await store.saveArtifact(path, data: data, commandID: commandID)
            let index = try await store.handle(["method": "index"], commandID: commandID)
            return ["value": "Saved \(path) (\(data.count) bytes). Open Workspace files to preview or Save to Files.", "paths": index["value"] ?? []]
        }
        return try await store.handle(request, commandID: commandID)
    }

    private func prepare(commandID id: String) async throws {
        guard commandID == id else { throw Failure.message("Cancelled") }
        guard !initialized else { return }
        guard let workerURL = Bundle.main.url(forResource: "NativeShellWorker", withExtension: "js") else {
            throw Failure.message("Build the mobile shell resource: cd scripts/ios/native-agent && npm ci && npm run build")
        }
        let source = try String(contentsOf: workerURL, encoding: .utf8)
        if !loaded {
            try await withCheckedThrowingContinuation { continuation in
                navigation = continuation
                webView.loadHTMLString("<html><head><meta http-equiv='Content-Security-Policy' content=\"default-src 'none'; script-src 'unsafe-eval'; worker-src blob:; connect-src 'none'\"></head><body></body></html>", baseURL: nil)
            }
            loaded = true
        }
        guard commandID == id else { throw Failure.message("Cancelled") }
        let token = UUID().uuidString
        generation = token
        _ = try await webView.callAsyncJavaScript(Self.bootstrap, arguments: ["source": source, "generation": token], in: nil, contentWorld: .page)
        _ = try await request(["method": "initialize"])
        guard commandID == id else { throw Failure.message("Cancelled") }
        initialized = true
    }

    private func request(_ message: [String: Any]) async throws -> Data {
        let value = try await webView.callAsyncJavaScript(
            "return JSON.stringify(await globalThis.mobileShell.request(message));",
            arguments: ["message": message], in: nil, contentWorld: .page
        )
        guard let json = value as? String, let data = json.data(using: .utf8) else {
            throw Failure.message("Invalid mobile tool response")
        }
        return data
    }

    func webView(_ webView: WKWebView, didFinish navigation: WKNavigation!) {
        self.navigation?.resume(); self.navigation = nil
    }

    func webView(_ webView: WKWebView, didFail navigation: WKNavigation!, withError error: Error) {
        self.navigation?.resume(throwing: error); self.navigation = nil
    }

    func webView(_ webView: WKWebView, didFailProvisionalNavigation navigation: WKNavigation!, withError error: Error) {
        self.navigation?.resume(throwing: error); self.navigation = nil
    }

    func webViewWebContentProcessDidTerminate(_ webView: WKWebView) {
        loaded = false
        generation = nil
        initialized = false
        navigation?.resume(throwing: Failure.message("Mobile tool web process terminated"))
        navigation = nil
    }

    private static let bootstrap = """
    globalThis.mobileShell?.stop('Restarted');
    const url = URL.createObjectURL(new Blob([source], {type: 'text/javascript'}));
    const worker = new Worker(url);
    URL.revokeObjectURL(url);
    const pending = new Map();
    let sequence = 0;
    let stopped = false;
    function stop(reason) {
      stopped = true;
      worker.terminate();
      for (const {reject, timer} of pending.values()) {
        clearTimeout(timer);
        reject(new Error(reason));
      }
      pending.clear();
    }
    worker.onmessage = ({data}) => {
      if (data.fsId) {
        if (stopped) return;
        const extended = data.request?.method === 'git';
        if (extended) for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.timer = setTimeout(() => stop('Git exceeded its time limit'), 45000); }
        function resumeDeadline() {
          if (extended) for (const waiter of pending.values()) { clearTimeout(waiter.timer); waiter.timer = setTimeout(() => stop('Mobile tool exceeded its time limit'), 7000); }
        }
        globalThis.webkit.messageHandlers.workspace.postMessage({generation, request: data.request}).then(
          result => { resumeDeadline(); if (!stopped) worker.postMessage({fsId: data.fsId, result}); },
          error => { resumeDeadline(); if (!stopped) worker.postMessage({fsId: data.fsId, error: String(error)}); }
        );
        return;
      }
      const waiter = pending.get(data.id);
      if (!waiter) return;
      clearTimeout(waiter.timer);
      pending.delete(data.id);
      if (data.error) waiter.reject(new Error(data.error));
      else waiter.resolve(data.result);
    };
    worker.onerror = event => stop(event.message || 'Worker failed');
    globalThis.mobileShell = {
      stop,
      request(message) {
        return new Promise((resolve, reject) => {
          if (stopped) { reject(new Error('Worker stopped')); return; }
          const id = ++sequence;
          const timer = setTimeout(() => stop('Mobile tool exceeded its time limit'), 7000);
          pending.set(id, {resolve, reject, timer});
          worker.postMessage({...message, id});
        });
      }
    };
    return true;
    """
}

@MainActor
private final class WorkspaceBridge: NSObject, WKScriptMessageHandlerWithReply {
    weak var owner: MobileShellRuntime?
    init(owner: MobileShellRuntime) { self.owner = owner }
    func userContentController(_ userContentController: WKUserContentController, didReceive message: WKScriptMessage, replyHandler: @escaping (Any?, String?) -> Void) {
        Task {
            do {
                guard let owner else { throw MobileShellRuntime.Failure.message("Shell was closed") }
                replyHandler(try await owner.filesystem(message.body), nil)
            } catch { replyHandler(nil, error.localizedDescription) }
        }
    }
}
