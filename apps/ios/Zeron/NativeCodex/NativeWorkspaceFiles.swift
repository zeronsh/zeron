import Foundation

struct NativeWorkspaceEntry: Codable, Sendable {
    var path: String
    var type: String
    var mode: Int
    var content: String?
    var size: Int? = nil
}

/// Project import/export works on copies. The Files provider's original is never edited.
enum NativeWorkspaceFiles {
    static let maxBytes = 128 * 1024 * 1024
    static let maxEntries = 20_000
    static let maxFileBytes = 16 * 1024 * 1024
    static let excludedDirectories: Set<String> = [".git", "node_modules", ".build", "DerivedData"]

    static func relativePath(_ path: String) throws -> String {
        guard path.hasPrefix("/workspace/"), !path.contains("\0") else { throw failure("Invalid workspace path") }
        let relative = String(path.dropFirst(11))
        guard !relative.isEmpty, relative.split(separator: "/", omittingEmptySubsequences: false).allSatisfy({ !$0.isEmpty && $0 != "." && $0 != ".." && $0.lowercased() != ".git" }) else { throw failure("Invalid workspace path") }
        return relative
    }

    static func merge(existing: [NativeWorkspaceEntry], incoming: [NativeWorkspaceEntry]) throws -> [NativeWorkspaceEntry] {
        var entries: [String: NativeWorkspaceEntry] = [:]
        for entry in existing + incoming {
            _ = try relativePath(entry.path)
            guard entry.type == "file" || entry.type == "directory" else { throw failure("Unsupported workspace entry") }
            if let old = entries[entry.path], old.type != "directory" || entry.type != "directory" { throw failure("\(entry.path) already exists. Import into an empty workspace or rename the file.") }
            entries[entry.path] = entry
        }
        guard entries.count <= maxEntries else { throw failure("The workspace supports up to 20,000 entries") }
        var size = 0
        for entry in entries.values where entry.type == "file" {
            guard let content = entry.content, let data = Data(base64Encoded: content) else { throw failure("Invalid file data") }
            guard data.count <= maxFileBytes else { throw failure("A file exceeds the 16 MB limit") }; size += data.count
        }
        guard size <= maxBytes else { throw failure("The workspace supports up to 128 MB. Import a smaller project or selected source files.") }
        return entries.values.sorted { $0.path < $1.path }
    }

    struct ImportItem: Sendable { let source: URL; let path: String; let directory: Bool; let bytes: Int }
    /// Validate and inventory imports without reading their contents into memory.
    static func importPlan(_ urls: [URL]) throws -> [ImportItem] {
        var items: [ImportItem] = [], total = 0
        func add(_ source: URL, relative: String) throws {
            let values = try source.resourceValues(forKeys: [.isDirectoryKey, .isRegularFileKey, .isSymbolicLinkKey, .fileSizeKey])
            guard values.isSymbolicLink != true, values.isDirectory == true || values.isRegularFile == true else { throw failure("Unsupported imported file: \(relative)") }
            _ = try relativePath("/workspace/" + relative)
            let bytes = values.isDirectory == true ? 0 : values.fileSize ?? 0
            total += bytes
            guard bytes <= maxFileBytes, total <= maxBytes, items.count < maxEntries else { throw failure("Import exceeds workspace limits: 128 MB, 20,000 entries, 16 MB per file") }
            items.append(.init(source: source, path: "/workspace/" + relative, directory: values.isDirectory == true, bytes: bytes))
        }
        for url in urls {
            let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            guard values.isSymbolicLink != true else { throw failure("Symbolic links are not supported") }
            if values.isDirectory == true {
                let root = url.resolvingSymlinksInPath().standardizedFileURL
                var problem: Error?
                guard let scan = FileManager.default.enumerator(at: root, includingPropertiesForKeys: [.isDirectoryKey], errorHandler: { _, error in problem = error; return false }) else { throw failure("Could not read imported folder") }
                while let child = scan.nextObject() as? URL {
                    if excludedDirectories.contains(child.lastPathComponent), try child.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true { scan.skipDescendants(); continue }
                    let components = child.standardizedFileURL.pathComponents
                    guard components.starts(with: root.pathComponents), components.count > root.pathComponents.count else { throw failure("Invalid import path") }
                    try add(child, relative: components.dropFirst(root.pathComponents.count).joined(separator: "/"))
                }
                if let problem { throw problem }
            } else { try add(url, relative: url.lastPathComponent) }
        }
        var seen: [String: Bool] = [:]
        for item in items {
            if let previous = seen[item.path], !previous || !item.directory { throw failure("Duplicate imported path: \(item.path)") }
            seen[item.path] = item.directory
        }
        return items
    }

    static func collect(_ urls: [URL]) throws -> [NativeWorkspaceEntry] {
        var entries: [NativeWorkspaceEntry] = []
        var bytes = 0
        func add(_ url: URL, relative: String) throws {
            let values = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey, .fileSizeKey, .isRegularFileKey])
            guard values.isSymbolicLink != true else { throw failure("Symbolic links are not supported: \(relative)") }
            let path = "/workspace/" + relative
            _ = try relativePath(path)
            if values.isDirectory == true {
                entries.append(.init(path: path, type: "directory", mode: 493))
            } else {
                guard values.isRegularFile == true else { throw failure("Unsupported file: \(relative)") }
                guard (values.fileSize ?? 0) <= min(maxFileBytes, maxBytes - bytes) else { throw failure("This project exceeds the 128 MB workspace limit") }
                let data = try Data(contentsOf: url)
                bytes += data.count
                guard bytes <= maxBytes else { throw failure("This project exceeds the 128 MB workspace limit") }
                entries.append(.init(path: path, type: "file", mode: 420, content: data.base64EncodedString()))
            }
            guard entries.count <= maxEntries else { throw failure("This project exceeds the 20,000-entry workspace limit") }
        }
        for url in urls {
            let rootValues = try url.resourceValues(forKeys: [.isDirectoryKey, .isSymbolicLinkKey])
            guard rootValues.isSymbolicLink != true else { throw failure("Symbolic links are not supported") }
            if rootValues.isDirectory == true {
                var enumerationError: Error?
                let folder = url.resolvingSymlinksInPath().standardizedFileURL
                let rootComponents = folder.pathComponents
                guard let enumerator = FileManager.default.enumerator(at: folder, includingPropertiesForKeys: [.isDirectoryKey, .isSymbolicLinkKey], options: [], errorHandler: { _, error in enumerationError = error; return false }) else { throw failure("Could not read the project folder") }
                while let child = enumerator.nextObject() as? URL {
                    if excludedDirectories.contains(child.lastPathComponent), try child.resourceValues(forKeys: [.isDirectoryKey]).isDirectory == true {
                        enumerator.skipDescendants(); continue
                    }
                    let components = child.standardizedFileURL.pathComponents
                    guard components.starts(with: rootComponents), components.count > rootComponents.count else { throw failure("Invalid imported path") }
                    let relative = components.dropFirst(rootComponents.count).joined(separator: "/")
                    try add(child, relative: relative)
                }
                if let enumerationError { throw enumerationError }
            } else { try add(url, relative: url.lastPathComponent) }
        }
        return try merge(existing: [], incoming: entries)
    }

    static func export(_ entries: [NativeWorkspaceEntry], to directory: URL) throws {
        let validated = try merge(existing: [], incoming: entries)
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        for entry in validated {
            let target = directory.appendingPathComponent(try relativePath(entry.path))
            if entry.type == "directory" { try FileManager.default.createDirectory(at: target, withIntermediateDirectories: true) }
            else {
                try FileManager.default.createDirectory(at: target.deletingLastPathComponent(), withIntermediateDirectories: true)
                try Data(base64Encoded: entry.content ?? "")!.write(to: target, options: .atomic)
            }
        }
    }
    /// Export a fresh selection without including siblings or exposing app storage.
    static func exportSelection(_ entries: [NativeWorkspaceEntry], path: String?, to staging: URL) throws -> URL {
        guard let path else {
            let folder = staging.appendingPathComponent("Workspace")
            try export(entries, to: folder)
            return folder
        }
        _ = try relativePath(path)
        guard let entry = entries.first(where: { $0.path == path }) else { throw failure("This item no longer exists.") }
        let destination = staging.appendingPathComponent((path as NSString).lastPathComponent)
        if entry.type == "directory" {
            let prefix = path + "/"
            let children = entries.filter { $0.path.hasPrefix(prefix) }.map { child -> NativeWorkspaceEntry in
                var child = child
                child.path = "/workspace/" + child.path.dropFirst(prefix.count)
                return child
            }
            try export(children, to: destination)
        } else {
            guard let content = entry.content, let data = Data(base64Encoded: content) else { throw failure("Invalid file data") }
            try FileManager.default.createDirectory(at: staging, withIntermediateDirectories: true)
            try data.write(to: destination, options: .atomic)
        }
        return destination
    }
    static func failure(_ text: String) -> NSError { NSError(domain: "NativeWorkspace", code: 1, userInfo: [NSLocalizedDescriptionKey: text]) }
}
