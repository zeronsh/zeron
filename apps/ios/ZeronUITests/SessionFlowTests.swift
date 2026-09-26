import XCTest

/// End-to-end flows over the Rust core's demo workspace (real registry,
/// session docs, command ledger and simulated host — no network).
final class SessionFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(_ args: [String] = [], fast: Bool = true) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"] + (fast ? ["-fast"] : []) + args
        app.launch()
        return app
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    func testFrontPageShowsFoldersAndRecents() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.cells["folder-pinned"].exists)
        snapshot(app, "front-page")
        app.cells["folder-pinned"].tap()
        XCTAssertTrue(app.navigationBars["Pinned"].waitForExistence(timeout: 5))
        snapshot(app, "pinned-folder")
    }

    func testSendEchoesAndStreamsReply() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Summarize the launch post in three bullets.")
        app.buttons["composer-send"].tap()
        // Optimistic echo appears immediately, then the host streams a reply.
        let echo = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'You: Summarize the launch post'")).firstMatch
        XCTAssertTrue(echo.waitForExistence(timeout: 3))
        snapshot(app, "streaming")
        // The host's reply lands after the echo and the turn settles.
        let reply = app.staticTexts.matching(identifier: "row-markdown").element(boundBy: 0)
        XCTAssertTrue(reply.waitForExistence(timeout: 15))
        XCTAssertTrue(app.buttons["Send message"].waitForExistence(timeout: 30))
        snapshot(app, "reply-complete")
    }

    func testQuestionPanelAnswersAndResumes() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Pick a tone for the post ?ask")
        app.buttons["composer-send"].tap()
        let option = app.buttons["question-option-0"]
        XCTAssertTrue(option.waitForExistence(timeout: 20))
        snapshot(app, "question")
        option.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 10), "composer returns after answering")
    }

    func testQueueWhileWorking() {
        let app = launch(["-route", "chat:chat-home", "-longreply"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Write a long reply please.")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.buttons["Stop response"].waitForExistence(timeout: 5))
        input.typeText("Then add a TL;DR.")
        XCTAssertTrue(app.buttons["Queue message"].waitForExistence(timeout: 2))
        app.buttons["Queue message"].tap()
        XCTAssertTrue(app.buttons["More queue actions"].waitForExistence(timeout: 5))
        snapshot(app, "queued")
    }

    func testJumpToLatestAfterScrollingUp() {
        let app = launch(["-route", "chat:chat-veil", "-big"])
        let transcript = app.scrollViews["transcript"]
        XCTAssertTrue(transcript.waitForExistence(timeout: 10))
        transcript.swipeDown(velocity: .fast)
        transcript.swipeDown(velocity: .fast)
        let jump = app.buttons["jump-to-latest"]
        XCTAssertTrue(jump.waitForExistence(timeout: 5))
        snapshot(app, "scrolled-up")
        jump.tap()
    }

    func testTabsAndSearch() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        app.tabBars.buttons["Projects"].tap()
        XCTAssertTrue(app.navigationBars["Projects"].waitForExistence(timeout: 5))
        snapshot(app, "projects")
        app.tabBars.buttons["PRs"].tap()
        XCTAssertTrue(app.navigationBars["Pull Requests"].waitForExistence(timeout: 5))
        snapshot(app, "pull-requests")
    }

    func testNewSessionFromAccessory() {
        let app = launch()
        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 10))
        accessory.tap()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        snapshot(app, "new-session")
        input.typeText("Add a dark mode toggle to settings")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10), "pushes the new session")
        snapshot(app, "new-session-created")
    }

    func testFileMentionSuggestions() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Look at @lay")
        let first = app.buttons["mention-0"]
        XCTAssertTrue(first.waitForExistence(timeout: 5))
        snapshot(app, "mentions")
        first.tap()
        XCTAssertTrue((input.value as? String ?? "").contains("@mod.rs"), "token inserted: \(input.value ?? "")")
    }

    func testArchiveWithUndo() {
        let app = launch()
        let cell = app.cells["session-chat-deploy"]
        XCTAssertTrue(cell.waitForExistence(timeout: 10))
        cell.swipeLeft()
        app.buttons["Archive"].tap()
        let undo = app.buttons["toast-action"]
        XCTAssertTrue(undo.waitForExistence(timeout: 3))
        snapshot(app, "archive-undo")
        undo.tap()
        XCTAssertTrue(app.cells["session-chat-deploy"].waitForExistence(timeout: 5), "unarchived session returns")
    }
}
