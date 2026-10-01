import Foundation

/// Owns an actual in-process Codex app-server, with no CLI or network transport
/// between Swift and Rust. Model requests still use OpenAI over the network.
@MainActor
final class EmbeddedCodex {
    enum Failure: LocalizedError {
        case message(String)
        var errorDescription: String? { switch self { case .message(let text): return text } }
    }
    var onEvent: (([String: Any]) -> Void)?
    private var handle: UInt64 = 0
    private var sequence = 0
    private var pending: [Int: CheckedContinuation<[String: Any], Error>] = [:]
    private var ready: CheckedContinuation<Void, Error>?
    private var poller: Task<Void, Never>?
    private(set) var isReady = false

    func start(home: URL, fixtureBaseURL: String? = nil) async throws {
        guard handle == 0 else { throw Failure.message("Codex is already starting or running") }
        try FileManager.default.createDirectory(at: home, withIntermediateDirectories: true,
            attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
        var protected = home
        var values = URLResourceValues()
        values.isExcludedFromBackup = true
        try protected.setResourceValues(values)
        var options: [String: Any] = ["home": home.path]
        #if DEBUG
        if let fixtureBaseURL { options["fixtureBaseUrl"] = fixtureBaseURL }
        #endif
        let json = String(decoding: try JSONSerialization.data(withJSONObject: options), as: UTF8.self)
        handle = json.withCString { zeron_codex_open($0) }
        guard handle != 0 else { throw Failure.message("Could not initialize embedded Codex") }
        try await withCheckedThrowingContinuation { continuation in
            ready = continuation
            poller = Task { [weak self] in
                while !Task.isCancelled {
                    self?.drain()
                    try? await Task.sleep(for: .milliseconds(20))
                }
            }
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(60))
                if self?.ready != nil { self?.close(error: Failure.message("Codex startup timed out")) }
            }
        }
    }

    func request(_ method: String, _ params: [String: Any] = [:]) async throws -> [String: Any] {
        guard isReady else { throw Failure.message("Codex is not ready") }
        sequence += 1
        let id = sequence
        return try await withCheckedThrowingContinuation { continuation in
            pending[id] = continuation
            do { try send(["id": id, "method": method, "params": params]) }
            catch { pending.removeValue(forKey: id)?.resume(throwing: error) }
            Task { [weak self] in
                try? await Task.sleep(for: .seconds(60))
                self?.pending.removeValue(forKey: id)?.resume(throwing: Failure.message("\(method) timed out"))
            }
        }
    }

    func respond(id: Any, result: [String: Any]) throws { try send(["id": id, "result": result]) }
    func reject(id: Any, message: String) throws {
        try send(["id": id, "error": ["code": -32601, "message": message]])
    }

    func close(error: Error = CancellationError()) {
        poller?.cancel(); poller = nil
        if handle != 0 { zeron_codex_close(handle); handle = 0 }
        isReady = false
        ready?.resume(throwing: error); ready = nil
        for continuation in pending.values { continuation.resume(throwing: error) }
        pending.removeAll()
    }

    private func send(_ message: [String: Any]) throws {
        let data = try JSONSerialization.data(withJSONObject: message)
        let status = String(decoding: data, as: UTF8.self).withCString { zeron_codex_send(handle, $0) }
        guard status == 0 else { throw Failure.message("Codex command queue is unavailable") }
    }

    private func drain() {
        guard handle != 0, let pointer = zeron_codex_poll(handle) else { return }
        defer { zeron_codex_free(pointer) }
        let data = Data(String(cString: pointer).utf8)
        guard let messages = try? JSONSerialization.jsonObject(with: data) as? [[String: Any]] else {
            close(error: Failure.message("Invalid Codex event stream")); return
        }
        for message in messages {
            if let method = message["method"] as? String {
                if method == "mobile/ready" {
                    isReady = true; ready?.resume(); ready = nil
                } else if method == "mobile/error" {
                    let text = (message["params"] as? [String: Any])?["message"] as? String ?? "Codex stopped"
                    close(error: Failure.message(text))
                    onEvent?(message)
                } else { onEvent?(message) }
            } else if let id = message["id"] as? Int, let continuation = pending.removeValue(forKey: id) {
                if let error = message["error"] as? [String: Any] {
                    continuation.resume(throwing: Failure.message(error["message"] as? String ?? "Codex request failed"))
                } else { continuation.resume(returning: message["result"] as? [String: Any] ?? [:]) }
            }
        }
    }

    deinit {
        poller?.cancel()
        if handle != 0 { zeron_codex_close(handle) }
    }
}
