import XCTest
import PDFKit
import UIKit
import WebKit
@testable import Zeron

@MainActor
final class NativeWorkspaceToolsTests: XCTestCase {
    func testGitSharesShellFilesAndSurvivesRestart() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let checkpoint = root.appendingPathComponent("workspace.json")
        let shell = MobileShellRuntime(checkpointURL: checkpoint)
        let result = try await shell.execute("mkdir project; cd project; git init && git config user.name Test && git config user.email test@example.invalid && echo first > file.txt && git add . && git commit -m first && echo second >> file.txt && git diff")
        XCTAssertEqual(result.exitCode, 0, result.stderr)
        XCTAssertTrue(result.stdout.contains("+second"), result.stdout)
        let entries = try await shell.entries()
        XCTAssertFalse(entries.contains { $0.path.contains(".git") })
        let hidden = try await shell.execute("cat /workspace/project/.git/config")
        XCTAssertNotEqual(hidden.exitCode, 0)
        let escaped = try await shell.execute("git -C /workspace/../ init")
        XCTAssertNotEqual(escaped.exitCode, 0)
        await shell.cancel()
        let restored = MobileShellRuntime(checkpointURL: checkpoint)
        let log = try await restored.execute("git -C /workspace/project log")
        XCTAssertEqual(log.exitCode, 0, log.stderr); XCTAssertTrue(log.stdout.contains("first"))
        let stage = root.appendingPathComponent("export")
        let export = try await restored.exportSelection(path: nil, to: stage)
        XCTAssertFalse(FileManager.default.fileExists(atPath: export.appendingPathComponent("project/.git").path))
        XCTAssertEqual(try String(contentsOf: export.appendingPathComponent("project/file.txt"), encoding: .utf8), "first\nsecond\n")
        // Import must preserve hidden repository metadata without loading existing files.
        try await restored.importEntries([.init(path: "/workspace/new.txt", type: "file", mode: 420, content: Data("new".utf8).base64EncodedString())])
        let afterImport = try await restored.execute("git -C /workspace/project log")
        XCTAssertEqual(afterImport.exitCode, 0, afterImport.stderr)
    }

    func testLargerImportUsesMetadataAndRejectsConflictsAtomically() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let input = root.appendingPathComponent("input")
        try FileManager.default.createDirectory(at: input, withIntermediateDirectories: true)
        let bytes = Data(repeating: 65, count: 10 * 1024 * 1024)
        try bytes.write(to: input.appendingPathComponent("large.txt"))
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        let count = try await shell.importURLs([input])
        XCTAssertEqual(count, 1)
        let entries = try await shell.entries()
        XCTAssertEqual(entries.first?.size, bytes.count)
        XCTAssertNil(entries.first?.content)
        try Data("new".utf8).write(to: input.appendingPathComponent("new.txt"))
        do { _ = try await shell.importURLs([input]); XCTFail("Conflicting import accepted") } catch {}
        let unchanged = try await shell.entries()
        XCTAssertEqual(unchanged.count, 1)
        let selected = try await shell.fileData("/workspace/large.txt")
        XCTAssertEqual(selected, bytes)
    }

    func testPublicHTTPSClone() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        let clone = try await shell.execute("git clone https://github.com/octocat/Hello-World.git hello && cat hello/README")
        XCTAssertEqual(clone.exitCode, 0, clone.stderr)
        XCTAssertTrue(clone.stdout.contains("Hello World!"), clone.stdout)
        let log = try await shell.execute("git -C /workspace/hello log")
        XCTAssertEqual(log.exitCode, 0, log.stderr)
        let credentials = try await shell.execute("git clone https://name:secret@example.invalid/repo denied")
        XCTAssertNotEqual(credentials.exitCode, 0)
        XCTAssertFalse(credentials.stderr.contains("secret"))
    }

    func testPagedPDFAndImagePDF() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        try await shell.writeFile("/workspace/report.html", content: "<h1>First page</h1><div style='page-break-before:always'><h1>Second page</h1></div>")
        let result = try await shell.execute("pdf html /workspace/report.html /workspace/report.pdf letter 36")
        XCTAssertEqual(result.exitCode, 0, result.stderr)
        let data = try await shell.fileData("/workspace/report.pdf")
        let document = try XCTUnwrap(PDFDocument(data: data))
        XCTAssertEqual(document.pageCount, 2)
        XCTAssertEqual(document.page(at: 0)?.bounds(for: .mediaBox).size, CGSize(width: 612, height: 792))
        XCTAssertTrue(document.string?.contains("Second page") == true)
        let png = UIGraphicsImageRenderer(size: CGSize(width: 20, height: 10)).pngData { context in UIColor.red.setFill(); context.fill(CGRect(x: 0, y: 0, width: 20, height: 10)) }
        try await shell.store.saveArtifact("/workspace/image.png", data: png)
        let images = try await shell.execute("pdf images /workspace/images.pdf /workspace/image.png /workspace/image.png")
        XCTAssertEqual(images.exitCode, 0, images.stderr)
        let imagePDF = try await shell.fileData("/workspace/images.pdf")
        XCTAssertEqual(PDFDocument(data: imagePDF)?.pageCount, 2)
    }

    func testHTTPPreviewAssetsSnapshotAndContainment() async throws {
        let root = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        defer { try? FileManager.default.removeItem(at: root) }
        let shell = MobileShellRuntime(checkpointURL: root.appendingPathComponent("workspace.json"))
        defer { shell.stopPreview() }
        try await shell.writeFile("/workspace/index.html", content: "<script src='app.js'></script><link rel='stylesheet' href='style.css'><p>old</p>")
        try await shell.writeFile("/workspace/app.js", content: "document.documentElement.dataset.loaded='yes'")
        try await shell.writeFile("/workspace/style.css", content: "p{color:rgb(255,0,0)}")
        let command = try await shell.execute("serve /workspace/index.html")
        XCTAssertEqual(command.exitCode, 0, command.stderr)
        let url = try XCTUnwrap(shell.previewURL)
        let session = URLSession(configuration: .ephemeral)
        let (data, response) = try await session.data(from: url)
        XCTAssertEqual((response as? HTTPURLResponse)?.statusCode, 200)
        XCTAssertTrue(String(decoding: data, as: UTF8.self).contains("old"))
        let web = WKWebView()
        web.load(URLRequest(url: url))
        var loaded = false
        for _ in 0..<50 {
            if (try? await web.evaluateJavaScript("document.documentElement.dataset.loaded")) as? String == "yes" { loaded = true; break }
            try await Task.sleep(for: .milliseconds(100))
        }
        XCTAssertTrue(loaded, "Website executed its separate JavaScript asset")
        let color = try await web.evaluateJavaScript("getComputedStyle(document.querySelector('p')).color") as? String
        XCTAssertEqual(color, "rgb(255, 0, 0)")
        let bridge = try await web.evaluateJavaScript("typeof window.webkit?.messageHandlers?.workspace") as? String
        XCTAssertEqual(bridge, "undefined")
        let asset = url.deletingLastPathComponent().appendingPathComponent("app.js")
        let (_, assetResponse) = try await session.data(from: asset)
        XCTAssertEqual((assetResponse as? HTTPURLResponse)?.mimeType, "text/javascript")
        for path in [".git/config", "%2e%2e/private", "missing"] {
            let (_, bad) = try await session.data(from: URL(string: path, relativeTo: url)!.absoluteURL)
            XCTAssertEqual((bad as? HTTPURLResponse)?.statusCode, 404)
        }
        var post = URLRequest(url: url); post.httpMethod = "POST"
        let (_, rejected) = try await session.data(for: post)
        XCTAssertEqual((rejected as? HTTPURLResponse)?.statusCode, 405)
        try await shell.writeFile("/workspace/index.html", content: "new")
        let (old, _) = try await session.data(from: url)
        XCTAssertTrue(String(decoding: old, as: UTF8.self).contains("old"))
        let freshURL = try await shell.startPreview()
        let (fresh, _) = try await session.data(from: freshURL)
        XCTAssertEqual(String(decoding: fresh, as: UTF8.self), "new")
    }
}
