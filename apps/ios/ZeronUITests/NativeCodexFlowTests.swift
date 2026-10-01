import XCTest

final class NativeCodexFlowTests: XCTestCase {
    func testNativeProviderCreatesNormalChatWithLoadingProgress() throws {
        guard let endpoint = ProcessInfo.processInfo.environment["NATIVE_CODEX_FIXTURE_URL"] else { throw XCTSkip("Requires model-fixture.py") }
        let app = XCUIApplication()
        app.launchArguments = ["-native-local", "-route", "new", "-native-codex-fixture", endpoint, "-native-codex-fixture-session", UUID().uuidString, "-native-codex-test-load-delay"]
        app.launch()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 15))
        input.tap()
        let prompt = "Create a file in this normal native chat"
        input.typeText(prompt)
        app.buttons["composer-chip-model"].tap()
        let provider = app.collectionViews.buttons["Native Codex"].firstMatch
        XCTAssertTrue(provider.waitForExistence(timeout: 10))
        provider.tap()
        XCTAssertEqual(input.value as? String, prompt)
        XCTAssertFalse(app.scrollViews["transcript"].exists, "Selecting a provider stays on the new-chat composer")
        XCTAssertTrue(app.progressIndicators["native-codex-loading"].waitForExistence(timeout: 2))
        app.buttons["composer-send"].tap()
        let reply = app.staticTexts.matching(NSPredicate(format: "identifier == 'row-markdown' AND label CONTAINS %@", "Edited hello.txt on this iPhone.")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 60))
        XCTAssertFalse(app.progressIndicators["native-codex-loading"].exists)
        app.navigationBars.buttons.element(boundBy: 0).tap()
        let row = app.cells.containing(NSPredicate(format: "label CONTAINS %@", prompt)).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 10), "Native chats appear in the regular Sessions list")
        row.tap()
        XCTAssertTrue(reply.waitForExistence(timeout: 15), "Native chats reopen from the regular list")
    }

    func testLocalConversationRunsAndRestores() throws {
        guard let endpoint = ProcessInfo.processInfo.environment["NATIVE_CODEX_FIXTURE_URL"] else {
            throw XCTSkip("Set TEST_RUNNER_NATIVE_CODEX_FIXTURE_URL and run model-fixture.py")
        }
        let app = XCUIApplication()
        app.launchArguments = ["-native-codex", "-native-codex-fixture", endpoint, "-native-codex-fixture-session", UUID().uuidString]
        app.launch()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 30))
        let model = app.buttons["composer-chip-model"]
        XCTAssertTrue(model.waitForExistence(timeout: 30))
        expectation(for: NSPredicate(format: "label CONTAINS %@", "gpt-5.1-codex"), evaluatedWith: model)
        waitForExpectations(timeout: 60)
        input.tap()
        app.buttons["composer-chip-effort"].tap()
        let high = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "High")).firstMatch
        XCTAssertTrue(high.waitForExistence(timeout: 5))
        high.tap()
        app.buttons["composer-chip-service-tier"].tap()
        let flex = app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", "Flex")).firstMatch
        XCTAssertTrue(flex.waitForExistence(timeout: 5))
        flex.tap()
        input.tap()
        input.typeText("VERIFY_NATIVE_SETTINGS Write hello.txt using the mobile shell.")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        let reply = app.staticTexts.matching(NSPredicate(format: "identifier == 'row-markdown' AND label CONTAINS %@", "Edited hello.txt on this iPhone.")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 60))
        XCTAssertTrue(app.staticTexts["row-tools"].firstMatch.waitForExistence(timeout: 5), "Mobile tool activity uses the standard tool group")
        let attachment = XCTAttachment(screenshot: app.screenshot())
        attachment.lifetime = .keepAlways
        add(attachment)
        app.terminate()
        app.launch()
        XCTAssertTrue(reply.waitForExistence(timeout: 30))
        XCTAssertTrue(app.staticTexts["row-tools"].firstMatch.waitForExistence(timeout: 5), "Tool rows survive relaunch")
        input.tap()
        XCTAssertEqual(app.buttons["composer-chip-effort"].label, "High")
        XCTAssertEqual(app.buttons["composer-chip-service-tier"].label, "Flex")
    }
    func testWorkspacePreviewAndSaveToFiles() throws {
        guard let endpoint = ProcessInfo.processInfo.environment["NATIVE_CODEX_FIXTURE_URL"] else { throw XCTSkip("Requires model fixture") }
        let app = XCUIApplication()
        app.launchArguments = ["-native-codex", "-native-codex-fixture", endpoint, "-native-codex-fixture-session", UUID().uuidString]
        app.launch()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 30))
        let model = app.buttons["composer-chip-model"]
        expectation(for: NSPredicate(format: "label CONTAINS %@", "gpt-5.1-codex"), evaluatedWith: model)
        waitForExpectations(timeout: 60)
        input.tap(); input.typeText("Write hello.txt"); app.buttons["composer-send"].tap()
        let reply = app.staticTexts.matching(NSPredicate(format: "identifier == 'row-markdown' AND label CONTAINS %@", "Edited hello.txt")).firstMatch
        XCTAssertTrue(reply.waitForExistence(timeout: 60))
        app.buttons["session-menu"].tap()
        app.buttons["Workspace files"].tap()
        XCTAssertTrue(app.tables["native-workspace-files"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["workspace-save-folder"].exists)
        let browser = XCTAttachment(screenshot: app.screenshot()); browser.lifetime = .keepAlways; add(browser)
        app.staticTexts["hello.txt"].tap()
        let reader = app.textViews["workspace-file-content"]
        XCTAssertTrue(reader.waitForExistence(timeout: 10))
        XCTAssertEqual(reader.value as? String, "Hello from native Codex\n")
        XCTAssertTrue(app.buttons["Edit"].exists)
        let preview = XCTAttachment(screenshot: app.screenshot()); preview.lifetime = .keepAlways; add(preview)
        app.buttons["workspace-save-file"].tap()
        XCTAssertTrue(app.buttons["Save"].waitForExistence(timeout: 15), "System Files export picker opens")
        let picker = XCTAttachment(screenshot: app.screenshot()); picker.lifetime = .keepAlways; add(picker)
        app.buttons["Save"].tap()
        let replace = app.buttons["Replace"]
        if replace.waitForExistence(timeout: 2) { replace.tap() }
        expectation(for: NSPredicate(format: "exists == false"), evaluatedWith: app.buttons["Save"])
        waitForExpectations(timeout: 15)
        XCTAssertTrue(reader.exists, "Returns to the file after saving a copy")
    }

}
