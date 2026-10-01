import Foundation
import ImageIO

/// Access only images belonging to the active thread, never arbitrary host paths.
enum NativeWorkspaceArtifacts {
    static func name(_ source: String) throws -> String {
        let name = (source as NSString).lastPathComponent
        guard name.hasSuffix(".png"), name.count <= 200,
              name.dropLast(4).allSatisfy({ $0.isASCII && ($0.isLetter || $0.isNumber || $0 == "_" || $0 == "-") }), name.count > 4 else {
            throw NativeWorkspaceFiles.failure("Expected the generated image's .png filename")
        }
        return name
    }
    static func read(_ source: String, directory: URL) throws -> Data {
        let name = try name(source)
        // Rebase old container paths by filename; callers cannot choose another thread.
        let root = directory.resolvingSymlinksInPath().standardizedFileURL
        let file = directory.appendingPathComponent(name)
        guard try file.resourceValues(forKeys: [.isSymbolicLinkKey]).isSymbolicLink != true,
              file.resolvingSymlinksInPath().deletingLastPathComponent() == root else { throw NativeWorkspaceFiles.failure("Invalid generated image path") }
        let values = try file.resourceValues(forKeys: [.fileSizeKey, .isRegularFileKey])
        guard values.isRegularFile == true, (values.fileSize ?? Int.max) <= NativeWorkspaceFiles.maxBytes else { throw NativeWorkspaceFiles.failure("Generated image exceeds the workspace limit") }
        let data = try Data(contentsOf: file)
        guard data.starts(with: [137,80,78,71,13,10,26,10]), CGImageSourceCreateWithData(data as CFData, nil) != nil else { throw NativeWorkspaceFiles.failure("Generated image is not a valid PNG") }
        return data
    }
}
