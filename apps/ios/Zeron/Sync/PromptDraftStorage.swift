import Foundation
import CryptoKit

/// Profile-scoped immutable content cache and durable publication outbox.
@MainActor
final class PromptDraftStorage {
    private let config: AppConfig
    private let directory: URL
    private(set) var pending: [PromptDraftSave] = []
    init(config: AppConfig) {
        self.config = config
        let profile = SHA256.hash(data: Data("\(config.orgId)/\(config.userId)".utf8)).map { String(format: "%02x", $0) }.joined()
        directory = DocDisk.directory.appendingPathComponent("drafts-\(profile)", isDirectory: true)
        try? FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        if let data = try? Data(contentsOf: directory.appendingPathComponent("outbox.json")), let saved = try? JSONDecoder().decode([PromptDraftSave].self, from: data) { pending = saved }
        // A canvas left by an interrupted process is now an abandoned draft.
        if let files = try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil) {
            for file in files where file.pathExtension == "canvas" {
                if let data = try? Data(contentsOf: file), var save = try? JSONDecoder().decode(PromptDraftSave.self, from: data) {
                    save.deferred = false
                    if !pending.contains(where: { $0.publicationKey == save.publicationKey }) { pending.append(save) }
                    if (try? persist()) != nil { try? FileManager.default.removeItem(at: file) }
                }
            }
        }
    }
    private func url(_ id: String) throws -> URL {
        guard !id.isEmpty, id.count <= 128, id.utf8.allSatisfy({ (48...57).contains($0) || (65...90).contains($0) || (97...122).contains($0) || $0 == 45 }) else { throw CocoaError(.fileReadInvalidFileName) }
        return directory.appendingPathComponent(id)
    }
    private func persist() throws { try JSONEncoder().encode(pending).write(to: directory.appendingPathComponent("outbox.json"), options: .atomic) }
    func stage(_ save: PromptDraftSave, assets: [String: Data]) throws {
        let content = try JSONEncoder().encode(save.content)
        guard content.count <= 2 * 1024 * 1024, save.content.attachments.count <= 32 else { throw CocoaError(.fileWriteOutOfSpace) }
        for (id, data) in assets {
            guard data.count <= 32 * 1024 * 1024, SHA256.hash(data: data).map({ String(format: "%02x", $0) }).joined() == id else { throw CocoaError(.fileReadCorruptFile) }
            try data.write(to: url(id), options: .atomic)
        }
        let file = try url(save.revision)
        if let old = try? Data(contentsOf: file) {
            guard try JSONDecoder().decode(PromptDraftContent.self, from: old) == save.content else { throw CocoaError(.fileWriteFileExists) }
        } else { try content.write(to: file, options: .atomic) }
        if !pending.contains(where: { $0.publicationKey == save.publicationKey }) { pending.append(save) }
        try persist()
        if save.deferred == true {
            try JSONEncoder().encode(save).write(to: url(save.id).appendingPathExtension("canvas"), options: .atomic)
        } else { clearCanvas(save.id) }
    }
    func clearCanvas(_ id: String) { if let file = try? url(id) { try? FileManager.default.removeItem(at: file.appendingPathExtension("canvas")) } }
    func acknowledge(_ publicationKey: String) throws { pending.removeAll { $0.publicationKey == publicationKey }; try persist() }
    private func request(_ path: String, method: String = "GET", body: Data? = nil) async throws -> Data {
        guard let token = await config.currentToken() else { throw URLError(.userAuthenticationRequired) }
        var request = URLRequest(url: config.edgeURL.appending(path: path))
        request.httpMethod = method; request.httpBody = body; request.timeoutInterval = 30
        request.setValue("Bearer \(token)", forHTTPHeaderField: "Authorization")
        let (bytes, response) = try await URLSession.shared.data(for: request)
        guard let response = response as? HTTPURLResponse, (200..<300).contains(response.statusCode) else { throw URLError(.badServerResponse) }
        return bytes
    }
    func object(_ id: String) async throws -> Data {
        let file = try url(id)
        if let cached = try? Data(contentsOf: file) { return cached }
        let data = try await request("draft-content/\(config.orgId)/\(id)")
        guard data.count <= 32 * 1024 * 1024 else { throw CocoaError(.fileReadTooLarge) }
        if id.count == 64 && SHA256.hash(data: data).map({ String(format: "%02x", $0) }).joined() != id { throw CocoaError(.fileReadCorruptFile) }
        try data.write(to: file, options: .atomic)
        return data
    }
    func load(_ revision: String) async throws -> PromptDraftContent {
        let bytes = try await object(revision)
        return try JSONDecoder().decode(PromptDraftContent.self, from: bytes)
    }
    func upload(_ save: PromptDraftSave) async throws {
        for id in save.content.attachments.map(\.blob) + [save.revision] {
            let marker = directory.appendingPathComponent("uploaded-\(id)")
            if FileManager.default.fileExists(atPath: marker.path) { continue }
            let bytes = try await object(id)
            _ = try await request("draft-content/\(config.orgId)/\(id)", method: "PUT", body: bytes)
            try Data().write(to: marker, options: .atomic)
        }
    }
    func hasReservation(_ id: String) -> Bool {
        guard let file = try? url(id) else { return false }
        return FileManager.default.fileExists(atPath: file.appendingPathExtension("claim").path)
    }
    func claim(id: String, revision: String) async throws {
        try Data(revision.utf8).write(to: url(id).appendingPathExtension("claim"), options: .atomic)
        _ = try await request("registry/\(config.orgId)/draft-claim", method: "POST", body: JSONSerialization.data(withJSONObject: ["id": id, "revision": revision]))
    }
}
