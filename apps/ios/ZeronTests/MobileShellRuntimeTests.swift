import XCTest
@testable import Zeron

@MainActor
final class MobileShellRuntimeTests: XCTestCase {
    func testProjectImportIsAtomicAndVisibleToShell() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let runtime = MobileShellRuntime(checkpointURL: directory.appendingPathComponent("workspace.json"))
        let entry = NativeWorkspaceEntry(path: "/workspace/src/main.txt", type: "file", mode: 420, content: Data("project".utf8).base64EncodedString())
        try await runtime.importEntries([entry])
        let result = try await runtime.execute("cat src/main.txt")
        XCTAssertEqual(result.stdout, "project")
        do { try await runtime.importEntries([entry]); XCTFail("Must not overwrite an existing file") } catch {}
        let content = try await runtime.readFile(entry.path)
        XCTAssertEqual(content, "project")
        await runtime.cancel()
        let restored = MobileShellRuntime(checkpointURL: directory.appendingPathComponent("workspace.json"))
        let saved = try await restored.readFile(entry.path)
        XCTAssertEqual(saved, "project")
        await restored.cancel()
    }

    func testWorkerSharesFileEditsAndRestoresCheckpointInNewRuntime() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let url = directory.appendingPathComponent("workspace.json")
        let runtime = MobileShellRuntime(checkpointURL: url)
        try await runtime.writeFile("/workspace/hello.txt", content: "hello\nworld\n")
        let result = try await runtime.execute("cat hello.txt | grep hello; sed -i 's/world/mobile/' hello.txt")
        XCTAssertEqual(result.stdout, "hello\n")
        XCTAssertEqual(result.stderr, "")
        XCTAssertEqual(result.exitCode, 0)
        let edited = try await runtime.readFile("/workspace/hello.txt")
        XCTAssertEqual(edited, "hello\nmobile\n")
        await runtime.cancel()
        let restored = MobileShellRuntime(checkpointURL: url)
        let text = try await restored.readFile("/workspace/hello.txt")
        XCTAssertEqual(text, edited)
        await restored.cancel()
    }

    func testCancellationPreservesAcknowledgedNativeEditsAndAllowsNextCommand() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let runtime = MobileShellRuntime(checkpointURL: directory.appendingPathComponent("workspace.json"))
        try await runtime.writeFile("/workspace/hello.txt", content: "saved\n")
        let task = Task { try await runtime.execute("echo partial > hello.txt; sleep 4") }
        for _ in 0..<100 {
            if try await runtime.store.readFile("/workspace/hello.txt") == "partial\n" { break }
            try await Task.sleep(for: .milliseconds(50))
        }
        let beforeCancel = try await runtime.store.readFile("/workspace/hello.txt")
        XCTAssertEqual(beforeCancel, "partial\n")
        await runtime.cancel()
        do {
            _ = try await task.value
            XCTFail("Cancelled command should fail")
        } catch { /* The worker's pending promise must reject. */ }
        let text = try await runtime.readFile("/workspace/hello.txt")
        XCTAssertEqual(text, "partial\n")
        let next = try await runtime.execute("echo recovered")
        XCTAssertEqual(next.stdout, "recovered\n")
        await runtime.cancel()
    }
    func testNativeFilesGlobsBinaryMovesAndFailedCommandChanges() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        let runtime = MobileShellRuntime(checkpointURL: directory.appendingPathComponent("workspace.json"))
        try await runtime.writeFile("/workspace/native.txt", content: "native")
        // Native reads and writes do not need to start WebKit.
        let disk = try String(contentsOf: directory.appendingPathComponent("workspace.files/native.txt"), encoding: .utf8)
        XCTAssertEqual(disk, "native")
        try await runtime.importEntries([.init(path: "/workspace/input.bin", type: "file", mode: 420, content: Data([0, 255]).base64EncodedString())])
        let result = try await runtime.execute("mkdir src; cp native.txt src/copy.txt; mv src/copy.txt src/moved.txt; sed -i 's/native/edited/' src/moved.txt; cat src/*.txt; cp input.bin bytes; cat bytes > roundtrip.bin; false")
        XCTAssertEqual(result.stdout, "edited")
        XCTAssertNotEqual(result.exitCode, 0)
        XCTAssertTrue(result.changedPaths.contains("/workspace/src/moved.txt"))
        XCTAssertTrue(result.changedPaths.contains("/workspace/bytes"))
        let edited = try await runtime.readFile("/workspace/src/moved.txt")
        XCTAssertEqual(edited, "edited")
        XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("workspace.files/bytes")), Data([0, 255]))
        XCTAssertEqual(try Data(contentsOf: directory.appendingPathComponent("workspace.files/roundtrip.bin")), Data([0, 255]))
        await runtime.cancel()
        let next = try await runtime.execute("cat src/*.txt; rm -r src; ls src")
        XCTAssertEqual(next.stdout, "edited")
        XCTAssertNotEqual(next.exitCode, 0)
        XCTAssertTrue(next.changedPaths.contains("/workspace/src/moved.txt"))
        await runtime.cancel()
    }

    func testLegacyCheckpointMigratesOnceWithoutOverwritingNativeEdits() async throws {
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: directory) }
        try FileManager.default.createDirectory(at: directory, withIntermediateDirectories: true)
        let url = directory.appendingPathComponent("workspace.json")
        let legacy = [NativeWorkspaceEntry(path: "/workspace/old.txt", type: "file", mode: 420, content: Data("legacy".utf8).base64EncodedString())]
        try JSONEncoder().encode(legacy).write(to: url)
        let runtime = MobileShellRuntime(checkpointURL: url)
        let old = try await runtime.readFile("/workspace/old.txt")
        XCTAssertEqual(old, "legacy")
        try await runtime.writeFile("/workspace/old.txt", content: "native")
        let restored = MobileShellRuntime(checkpointURL: url)
        let current = try await restored.readFile("/workspace/old.txt")
        XCTAssertEqual(current, "native")
        XCTAssertTrue(FileManager.default.fileExists(atPath: url.path), "Keep the migration backup")
    }

}
