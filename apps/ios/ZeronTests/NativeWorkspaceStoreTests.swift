import XCTest
@testable import Zeron

final class NativeWorkspaceStoreTests: XCTestCase {
    func testContainmentLimitsAndExpiredCallbacks() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let store = NativeWorkspaceStore(checkpointURL: root.appendingPathComponent("workspace.json"))
        try await store.writeFile("/workspace/saved.txt", content: "keep")
        do { try await store.writeFile("/workspace/../escape", content: "bad"); XCTFail("Traversal accepted") } catch {}
        try FileManager.default.createSymbolicLink(atPath: root.appendingPathComponent("workspace.files/link").path, withDestinationPath: root.path)
        do { try await store.writeFile("/workspace/link/escape", content: "bad"); XCTFail("Symlink accepted") } catch {}
        XCTAssertFalse(FileManager.default.fileExists(atPath: root.appendingPathComponent("escape").path))
        try FileManager.default.removeItem(at: root.appendingPathComponent("workspace.files/link"))
        do { try await store.writeFile("/workspace/saved.txt", content: String(repeating: "x", count: NativeWorkspaceFiles.maxFileBytes + 1)); XCTFail("Quota accepted") } catch {}
        let saved = try await store.readFile("/workspace/saved.txt")
        XCTAssertEqual(saved, "keep")
        try await store.beginCommand("old")
        await store.endCommand("old")
        try await store.beginCommand("new")
        do {
            _ = try await store.handle(["method": "writeFile", "path": "/saved.txt", "content": Data("stale".utf8).base64EncodedString()], commandID: "old")
            XCTFail("Expired shell callback accepted")
        } catch {}
        let unchanged = try await store.readFile("/workspace/saved.txt")
        XCTAssertEqual(unchanged, "keep")
    }
}

extension NativeWorkspaceStoreTests {
    @MainActor
    func testAliasedAppContainerSeesConsecutiveWritesImmediately() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let real = root.appendingPathComponent("real")
        let alias = root.appendingPathComponent("alias")
        try FileManager.default.createDirectory(at: real, withIntermediateDirectories: true)
        try FileManager.default.createSymbolicLink(at: alias, withDestinationURL: real)
        let runtime = MobileShellRuntime(checkpointURL: alias.appendingPathComponent("chat/workspace.json"))
        try await runtime.writeFile("/workspace/index.html", content: "hello")
        let entries = try await runtime.snapshot()
        XCTAssertEqual(entries.map(\.path), ["/workspace/index.html"])
        try await runtime.writeFile("/workspace/styles.css", content: "body {}")
        let result = try await runtime.execute("ls /workspace; cat /workspace/index.html /workspace/styles.css")
        XCTAssertEqual(result.exitCode, 0)
        XCTAssertTrue(result.stdout.contains("hellobody {}"))
        await runtime.cancel()
    }
}
