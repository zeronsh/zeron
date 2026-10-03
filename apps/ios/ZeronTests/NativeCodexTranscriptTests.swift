import XCTest
@testable import Zeron

final class NativeCodexTranscriptTests: XCTestCase {
    func testOldToolEnvelopeUsesStructuredRowsButOrdinaryMarkdownDoesNot() throws {
        let legacy = NativeCodexMessage(id: "call", user: false, text: "**mobile_write_file**\n\n```\n/workspace/index.html\n```\n\n```\nFile saved.\n```\n")
        let entry = legacy.transcriptEntry(streaming: false, working: false)
        XCTAssertEqual(entry.tool?.name, "mobile_write_file")
        XCTAssertEqual(entry.tool?.argument, "/workspace/index.html")
        XCTAssertEqual(entry.tool?.output, "File saved.")
        XCTAssertTrue(entry.text.isEmpty)
        let prose = NativeCodexMessage(id: "text", user: false, text: "Use **mobile_write_file** to save files.")
        XCTAssertNil(prose.transcriptEntry(streaming: false, working: false).tool)
        let oldJSON = Data(#"{"id":"old","user":false,"text":"hello"}"#.utf8)
        XCTAssertNil(try JSONDecoder().decode(NativeCodexMessage.self, from: oldJSON).tool)
    }

    func testToolStateAndReadableShellResultPersist() throws {
        let output = #"{"stdout":"hello\n","stderr":"","exitCode":1,"changedPaths":["/workspace/file"]}"#
        let message = NativeCodexMessage(id: "call", user: false, text: "", tool: .init(name: "mobile_shell", argument: "cat file", output: output, resolved: true, isError: true))
        let restored = try JSONDecoder().decode(NativeCodexMessage.self, from: JSONEncoder().encode(message))
        let tool = try XCTUnwrap(restored.transcriptEntry(streaming: false, working: false).tool)
        XCTAssertTrue(tool.resolved)
        XCTAssertTrue(tool.isError)
        XCTAssertTrue(tool.output?.contains("Exit code: 1") == true)
        XCTAssertTrue(tool.output?.contains("Changed files:\n/workspace/file") == true)
        let pending = NativeCodexMessage(id: "pending", user: false, text: "", tool: .init(name: "mobile_read_file", argument: "/workspace/file"))
        XCTAssertEqual(pending.transcriptEntry(streaming: true, working: true).tool?.resolved, false)
        XCTAssertEqual(pending.transcriptEntry(streaming: false, working: false).tool?.isError, true)
    }
    func testCopyTranscriptIncludesToolFailuresAndSkipsEmptyAssistantEntries() {
        let tool = NativeCodexMessage(id: "call", user: false, text: "", tool: .init(name: "mobile_shell", argument: "awk -f graph.awk", output: #"{"stdout":"","stderr":"awk: invalid option -- f","exitCode":1}"#, resolved: true, isError: true))
        XCTAssertTrue(tool.copiedText?.contains("awk: invalid option -- f") == true)
        XCTAssertTrue(tool.copiedText?.contains("Failed") == true)
        XCTAssertNil(NativeCodexMessage(id: "empty", user: false, text: "").copiedText)
    }

}
