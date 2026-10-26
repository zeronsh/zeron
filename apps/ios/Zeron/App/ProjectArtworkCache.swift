import CryptoKit
import Foundation

/// Small PNG previews stored with the account's local project data.
final class ProjectArtworkCache {
    private let directory: URL

    init(directory: URL) {
        self.directory = directory
    }

    private func filename(_ key: String) -> String {
        SHA256.hash(data: Data(key.utf8)).map { String(format: "%02x", $0) }.joined() + ".png"
    }

    func load(_ key: String) -> Data? {
        try? Data(contentsOf: directory.appendingPathComponent(filename(key)))
    }

    func store(_ png: Data, for key: String) {
        do {
            try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true, attributes: [.protectionKey: FileProtectionType.completeUntilFirstUserAuthentication])
            var location = directory
            var values = URLResourceValues()
            values.isExcludedFromBackup = true
            try location.setResourceValues(values)
            try png.write(to: directory.appendingPathComponent(filename(key)), options: [.atomic, .completeFileProtectionUntilFirstUserAuthentication])
        } catch {
            // Artwork is disposable; a full disk must not prevent project edits.
        }
    }

    func retain(_ keys: Set<String>) {
        let names = Set(keys.map(filename))
        for file in (try? FileManager.default.contentsOfDirectory(at: directory, includingPropertiesForKeys: nil)) ?? [] where !names.contains(file.lastPathComponent) {
            try? FileManager.default.removeItem(at: file)
        }
    }

    func clear() {
        try? FileManager.default.removeItem(at: directory)
    }
}
