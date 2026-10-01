import XCTest
@testable import Zeron

@MainActor
final class EmbeddedCodexTests: XCTestCase {
    func testRealCodexTurnCallsMobileShellAndResumesThread() async throws {
        guard let endpoint = ProcessInfo.processInfo.environment["NATIVE_CODEX_FIXTURE_URL"] else {
            throw XCTSkip("Run scripts/ios/native-agent/model-fixture.py and set TEST_RUNNER_NATIVE_CODEX_FIXTURE_URL")
        }
        let directory = FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString)
        let codex = EmbeddedCodex()
        defer { codex.close() }
        let shell = MobileShellRuntime(checkpointURL: directory.appendingPathComponent("workspace.json"))
        var done = expectation(description: "Actual Codex completed the model/tool/model loop")
        var message = ""
        var toolCalls = 0
        codex.onEvent = { event in
            let params = event["params"] as? [String: Any] ?? [:]
            switch event["method"] as? String {
            case "item/tool/call":
                toolCalls += 1
                Task {
                    let result = await MobileCodexTools.dispatch(params, shell: shell)
                    try codex.respond(id: event["id"]!, result: result)
                }
            case "item/agentMessage/delta": message += params["delta"] as? String ?? ""
            case "turn/completed":
                let turn = params["turn"] as? [String: Any] ?? [:]
                XCTAssertEqual(turn["status"] as? String, "completed", "\(turn)")
                done.fulfill()
            case "mobile/error": XCTFail("\(params)"); done.fulfill()
            default: break
            }
        }
        try await codex.start(home: directory.appendingPathComponent("codex"), fixtureBaseURL: endpoint)
        let thread = try await codex.request("thread/start")
        XCTAssertEqual((thread["sandbox"] as? [String: Any])?["type"] as? String, "workspaceWrite")
        let id = try XCTUnwrap((thread["thread"] as? [String: Any])?["id"] as? String)
        _ = try await codex.request("turn/start", ["threadId": id, "input": [["type": "text", "text": "VERIFY_NATIVE_SETTINGS Write hello.txt using the mobile shell."]], "effort": "high", "serviceTier": "flex"])
        await fulfillment(of: [done], timeout: 45)
        XCTAssertEqual(toolCalls, 1)
        XCTAssertEqual(message, "Edited hello.txt on this iPhone.")
        let content = try await shell.readFile("/workspace/hello.txt")
        XCTAssertEqual(content, "Hello from native Codex\n")
        codex.close()
        let relocated = directory.appendingPathComponent("relocated-codex")
        try FileManager.default.moveItem(at: directory.appendingPathComponent("codex"), to: relocated)
        try await codex.start(home: relocated, fixtureBaseURL: endpoint)
        let resumed = try await codex.request("thread/resume", ["threadId": id])
        XCTAssertEqual((resumed["thread"] as? [String: Any])?["id"] as? String, id)
        XCTAssertEqual((resumed["sandbox"] as? [String: Any])?["type"] as? String, "workspaceWrite")
        done = expectation(description: "Restored thread still exposes writable mobile tools")
        _ = try await codex.request("turn/start", ["threadId": id, "input": [["type": "text", "text": "Check the workspace again."]], "effort": "high", "serviceTier": "flex"])
        await fulfillment(of: [done], timeout: 45)
        XCTAssertEqual(toolCalls, 1, "Resumed model context includes the original tool result")
        XCTAssertEqual(message, "Edited hello.txt on this iPhone.Edited hello.txt on this iPhone.")
        await shell.cancel()
    }

    func testInterruptPendingToolAndIgnoreLateReply() async throws {
        guard let endpoint = ProcessInfo.processInfo.environment["NATIVE_CODEX_FIXTURE_URL"] else { throw XCTSkip("Requires the local model fixture") }
        let codex = EmbeddedCodex()
        defer { codex.close() }
        let tool = expectation(description: "Pending mobile tool")
        let stopped = expectation(description: "Interrupted turn")
        var requestId: Any?
        codex.onEvent = { event in
            let params = event["params"] as? [String: Any] ?? [:]
            if event["method"] as? String == "item/tool/call" { requestId = event["id"]; tool.fulfill() }
            if event["method"] as? String == "turn/completed" {
                XCTAssertEqual((params["turn"] as? [String: Any])?["status"] as? String, "interrupted")
                stopped.fulfill()
            }
        }
        try await codex.start(home: FileManager.default.temporaryDirectory.appendingPathComponent(UUID().uuidString), fixtureBaseURL: endpoint)
        let thread = try await codex.request("thread/start")
        let threadId = try XCTUnwrap((thread["thread"] as? [String: Any])?["id"] as? String)
        let response = try await codex.request("turn/start", ["threadId": threadId, "input": [["type": "text", "text": "Write hello.txt"]]])
        let turnId = try XCTUnwrap((response["turn"] as? [String: Any])?["id"] as? String)
        await fulfillment(of: [tool], timeout: 15)
        _ = try await codex.request("turn/interrupt", ["threadId": threadId, "turnId": turnId])
        await fulfillment(of: [stopped], timeout: 15)
        try codex.respond(id: XCTUnwrap(requestId), result: ["success": false, "contentItems": [["type": "inputText", "text": "Cancelled"]]])
        _ = try await codex.request("account/read", ["refreshToken": false])
        XCTAssertTrue(codex.isReady)
    }
}
