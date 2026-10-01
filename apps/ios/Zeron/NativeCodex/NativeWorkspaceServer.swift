import Foundation
import Network
import UniformTypeIdentifiers

/// Foreground loopback-only static HTTP. Serves an immutable native snapshot;
/// website JavaScript never receives the workspace bridge or private app paths.
final class NativeWorkspaceServer: @unchecked Sendable {
    private let queue = DispatchQueue(label: "sh.zeron.workspace-http")
    private let listener: NWListener
    private let root: URL
    private let token = UUID().uuidString
    private var connections: [UUID: NWConnection] = [:]
    private var started = false
    private var port: UInt16?

    init(root: URL) throws {
        self.root = root
        let parameters = NWParameters.tcp
        parameters.requiredLocalEndpoint = .hostPort(host: "127.0.0.1", port: .any)
        listener = try NWListener(using: parameters)
    }
    func start(entry: String) async throws -> URL {
        let port: UInt16 = try await withCheckedThrowingContinuation { continuation in
            queue.async {
                guard !self.started else { continuation.resume(throwing: NativeWorkspaceFiles.failure("Preview already started")); return }
                self.started = true
                var pending: CheckedContinuation<UInt16, Error>? = continuation
                self.listener.stateUpdateHandler = { [weak self] state in
                    guard let self else { return }
                    switch state {
                    case .ready:
                        if let port = self.listener.port?.rawValue { self.port = port; pending?.resume(returning: port); pending = nil }
                    case .failed(let error): pending?.resume(throwing: error); pending = nil
                    case .cancelled: pending?.resume(throwing: CancellationError()); pending = nil
                    default: break
                    }
                }
                self.listener.newConnectionHandler = { [weak self] connection in
                    guard let self else { connection.cancel(); return }
                    guard self.connections.count < 32 else { connection.cancel(); return }
                    let id = UUID(); self.connections[id] = connection
                    connection.start(queue: self.queue)
                    self.queue.asyncAfter(deadline: .now() + 10) { [weak self, weak connection] in connection?.cancel(); self?.connections.removeValue(forKey: id) }
                    self.receive(connection, id: id, buffer: Data())
                }
                self.listener.start(queue: self.queue)
            }
        }
        let encoded = entry.split(separator: "/").map { String($0).addingPercentEncoding(withAllowedCharacters: .urlPathAllowed.subtracting(CharacterSet(charactersIn: "?#%")))! }.joined(separator: "/")
        return URL(string: "http://127.0.0.1:\(port)/\(token)/\(encoded)")!
    }
    func stop() {
        queue.async {
            self.listener.cancel()
            self.connections.values.forEach { $0.cancel() }; self.connections.removeAll()
            try? FileManager.default.removeItem(at: self.root.deletingLastPathComponent())
        }
    }
    deinit { listener.cancel() }
    private func receive(_ connection: NWConnection, id: UUID, buffer: Data) {
        connection.receive(minimumIncompleteLength: 1, maximumLength: 16_384) { [weak self] data, _, complete, error in
            guard let self else { connection.cancel(); return }
            var buffer = buffer; if let data { buffer.append(data) }
            guard buffer.count <= 16_384 else { self.send(connection, id: id, code: 431, data: Data()); return }
            if let end = buffer.range(of: Data("\r\n\r\n".utf8)) {
                self.respond(connection, id: id, header: String(decoding: buffer[..<end.lowerBound], as: UTF8.self))
            } else if error != nil || complete { connection.cancel(); self.connections.removeValue(forKey: id) }
            else { self.receive(connection, id: id, buffer: buffer) }
        }
    }
    private func respond(_ connection: NWConnection, id: UUID, header: String) {
        let lines = header.components(separatedBy: "\r\n"), request = header.components(separatedBy: "\r\n")[0].split(separator: " ")
        guard request.count == 3, ["GET", "HEAD"].contains(request[0]) else { send(connection, id: id, code: 405, data: Data()); return }
        let host = lines.dropFirst().first { $0.lowercased().hasPrefix("host:") }?.dropFirst(5).trimmingCharacters(in: .whitespaces)
        guard host == "127.0.0.1:\(port ?? 0)" else { send(connection, id: id, code: 403, data: Data()); return }
        let raw = String(request[1]).components(separatedBy: "?")[0]
        guard let path = raw.removingPercentEncoding, path.hasPrefix("/\(token)/") else { send(connection, id: id, code: 404, data: Data()); return }
        var relative = String(path.dropFirst(token.count + 2))
        if relative.isEmpty || relative.hasSuffix("/") { relative += "index.html" }
        do {
            _ = try NativeWorkspaceFiles.relativePath("/workspace/" + relative)
            let file = root.appendingPathComponent(relative)
            let values = try file.resourceValues(forKeys: [.isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
            guard values.isRegularFile == true, values.isSymbolicLink != true, (values.fileSize ?? 0) <= NativeWorkspaceFiles.maxFileBytes else { throw NativeWorkspaceFiles.failure("Not a served file") }
            let data = try Data(contentsOf: file, options: .mappedIfSafe)
            let mime: String
            switch file.pathExtension.lowercased() {
            case "js", "mjs": mime = "text/javascript"
            case "css": mime = "text/css"
            case "html", "htm": mime = "text/html; charset=utf-8"
            case "json": mime = "application/json"
            case "svg": mime = "image/svg+xml"
            default: mime = UTType(filenameExtension: file.pathExtension)?.preferredMIMEType ?? "application/octet-stream"
            }
            send(connection, id: id, code: 200, data: data, mime: mime, head: request[0] == "HEAD")
        } catch { send(connection, id: id, code: 404, data: Data()) }
    }
    private func send(_ connection: NWConnection, id: UUID, code: Int, data: Data, mime: String = "text/plain", head: Bool = false) {
        // Local relative assets and fetch work; external sites and privileged APIs do not.
        let policy = "default-src 'self'; script-src 'self' 'unsafe-inline'; style-src 'self' 'unsafe-inline'; img-src 'self' data: blob:; font-src 'self' data:; connect-src 'self'; object-src 'none'; frame-src 'none'; worker-src 'none'; base-uri 'self'; form-action 'none'; frame-ancestors 'none'"
        let header = "HTTP/1.1 \(code) \(code == 200 ? "OK" : "Error")\r\nContent-Type: \(mime)\r\nContent-Length: \(data.count)\r\nCache-Control: no-store\r\nX-Content-Type-Options: nosniff\r\nContent-Security-Policy: \(policy)\r\nConnection: close\r\n\r\n"
        var response = Data(header.utf8); if !head { response.append(data) }
        connection.send(content: response, completion: .contentProcessed { [weak self] _ in connection.cancel(); self?.connections.removeValue(forKey: id) })
    }
}
