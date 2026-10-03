import XCTest
@testable import Zeron

final class NativeWorkspaceFilesTests: XCTestCase {
    func testFolderRoundTripPreservesNestedBinaryAndEmptyDirectories() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let project = root.appendingPathComponent("project")
        try FileManager.default.createDirectory(at: project.appendingPathComponent("src/empty"), withIntermediateDirectories: true)
        try FileManager.default.createDirectory(at: project.appendingPathComponent("node_modules"), withIntermediateDirectories: true)
        let binary = Data([0, 255, 13, 10])
        try binary.write(to: project.appendingPathComponent("src/image.bin"))
        try Data("skip".utf8).write(to: project.appendingPathComponent("node_modules/dependency"))
        let entries = try NativeWorkspaceFiles.collect([project])
        XCTAssertTrue(entries.contains { $0.path == "/workspace/src/empty" && $0.type == "directory" })
        XCTAssertFalse(entries.contains { $0.path.contains("node_modules") })
        let output = root.appendingPathComponent("export")
        try NativeWorkspaceFiles.export(entries, to: output)
        XCTAssertEqual(try Data(contentsOf: output.appendingPathComponent("src/image.bin")), binary)
    }

    func testConflictsTraversalAndSymlinksAreRejected() throws {
        let file = NativeWorkspaceEntry(path: "/workspace/main.swift", type: "file", mode: 420, content: "")
        XCTAssertThrowsError(try NativeWorkspaceFiles.merge(existing: [file], incoming: [file]))
        XCTAssertThrowsError(try NativeWorkspaceFiles.relativePath("/workspace/../private"))
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        try FileManager.default.createDirectory(at: root, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(atPath: root.appendingPathComponent("link").path, withDestinationPath: "/tmp")
        XCTAssertThrowsError(try NativeWorkspaceFiles.collect([root]))
    }
    func testFolderImportThroughAliasedContainerPreservesRelativePaths() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let actual = root.appendingPathComponent("real")
        let alias = root.appendingPathComponent("much-longer-alias")
        try FileManager.default.createDirectory(at: actual.appendingPathComponent("project/src"), withIntermediateDirectories: true)
        try Data("content".utf8).write(to: actual.appendingPathComponent("project/src/index.html"))
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: actual)
        let entries = try NativeWorkspaceFiles.collect([alias.appendingPathComponent("project")])
        XCTAssertEqual(entries.map(\.path), ["/workspace/src", "/workspace/src/index.html"])
    }

    func testSaveSelectionPreservesBytesAndExcludesSiblings() throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let bytes = Data([0, 255, 13, 10])
        let entries: [NativeWorkspaceEntry] = [
            .init(path: "/workspace/assets", type: "directory", mode: 493),
            .init(path: "/workspace/assets/empty", type: "directory", mode: 493),
            .init(path: "/workspace/assets/photo.bin", type: "file", mode: 420, content: bytes.base64EncodedString()),
            .init(path: "/workspace/private.txt", type: "file", mode: 420, content: Data("sibling".utf8).base64EncodedString())
        ]
        let file = try NativeWorkspaceFiles.exportSelection(entries, path: "/workspace/assets/photo.bin", to: root.appendingPathComponent("single"))
        XCTAssertEqual(file.lastPathComponent, "photo.bin")
        XCTAssertEqual(try Data(contentsOf: file), bytes)
        XCTAssertEqual(try FileManager.default.contentsOfDirectory(atPath: file.deletingLastPathComponent().path), ["photo.bin"])
        let folder = try NativeWorkspaceFiles.exportSelection(entries, path: "/workspace/assets", to: root.appendingPathComponent("folder"))
        XCTAssertEqual(folder.lastPathComponent, "assets")
        XCTAssertEqual(try Data(contentsOf: folder.appendingPathComponent("photo.bin")), bytes)
        XCTAssertTrue(FileManager.default.fileExists(atPath: folder.appendingPathComponent("empty").path))
        XCTAssertFalse(FileManager.default.fileExists(atPath: folder.appendingPathComponent("private.txt").path))
        XCTAssertThrowsError(try NativeWorkspaceFiles.exportSelection(entries, path: "/workspace/missing.txt", to: root.appendingPathComponent("missing")))
    }

}
