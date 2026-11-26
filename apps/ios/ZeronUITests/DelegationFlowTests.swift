import XCTest

/// Background delegation over the demo: "Release check" delegated to four
/// subagent chats, one of them (on another device) asking to run a command,
/// another asking which branch to cut.
final class DelegationFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    func testCompanionsListDelegatedSubagentsAndAllowTheirApproval() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-route", "chat:chat-release", "-companions"]
        app.launch()
        let list = app.collectionViews["companions"]
        XCTAssertTrue(list.waitForExistence(timeout: 10))
        XCTAssertTrue(list.cells["companion-delegated-chat-release-audit"].waitForExistence(timeout: 10))
        // All four, as subagents: none under Side chats.
        for chat in ["chat-release-notes", "chat-release-bench", "chat-release-branch"] {
            XCTAssertTrue(delegatedRow(list, chat).exists, chat)
        }
        XCTAssertEqual(list.cells.matching(identifier: "companion-side-chat").count, 0)
        // Back up to the first (a swipe at the top would pull the sheet).
        let audit = list.cells["companion-delegated-chat-release-audit"]
        for _ in 0..<4 where !(audit.exists && audit.isHittable) { list.swipeDown(velocity: .slow) }
        XCTAssertTrue(audit.waitForExistence(timeout: 5))

        // The approval is on its row: the command, Deny and Allow once.
        XCTAssertTrue(audit.staticTexts["cargo fuzz run parse -- -max_total_time=60"].exists)
        XCTAssertTrue(audit.buttons["delegated-deny"].exists)
        let allow = audit.buttons["delegated-allow"]
        XCTAssertTrue(allow.exists)
        allow.tap()

        // Answered: the ask clears and the row stays (the demo's host runs
        // the command and finishes).
        XCTAssertTrue(allow.waitForNonExistence(timeout: 10))
        XCTAssertEqual(app.state, .runningForeground)
        XCTAssertTrue(audit.exists)
        XCTAssertFalse(audit.buttons["delegated-deny"].exists)
        // Still up a moment later (the list reloads after the answer).
        Thread.sleep(forTimeInterval: 1.5)
        XCTAssertEqual(app.state, .runningForeground)
        XCTAssertTrue(audit.exists)
    }

    func testQuestionOptionAnswersIt() {
        let app = launch()
        let list = app.collectionViews["companions"]
        let branch = delegatedRow(list, "chat-release-branch")
        XCTAssertTrue(branch.exists)
        XCTAssertTrue(branch.staticTexts["Which branch should 0.3.21 be cut from?"].exists)
        for option in ["main", "release/0.3", "next"] {
            XCTAssertTrue(branch.buttons[option].exists, option)
        }
        XCTAssertTrue(branch.buttons["delegated-open"].exists)
        branch.buttons["release/0.3"].tap()
        XCTAssertTrue(branch.buttons["release/0.3"].waitForNonExistence(timeout: 10))
        XCTAssertEqual(app.state, .runningForeground)
        XCTAssertTrue(branch.exists)
    }

    func testAnswerInItsChatOpensTheChild() {
        let app = launch()
        let list = app.collectionViews["companions"]
        let branch = delegatedRow(list, "chat-release-branch")
        XCTAssertTrue(branch.exists)
        branch.buttons["delegated-open"].tap()
        // The child as a full chat, headed Subagent.
        XCTAssertTrue(app.navigationBars.staticTexts["Subagent"].waitForExistence(timeout: 10))
        XCTAssertFalse(list.exists)
    }

    private func launch() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-route", "chat:chat-release", "-companions"]
        app.launch()
        XCTAssertTrue(app.collectionViews["companions"].waitForExistence(timeout: 10))
        return app
    }

    /// A delegated row, scrolled to if the sheet hides it.
    private func delegatedRow(_ list: XCUIElement, _ chat: String) -> XCUIElement {
        let row = list.cells["companion-delegated-\(chat)"]
        for _ in 0..<4 where !(row.exists && row.isHittable) { list.swipeUp(velocity: .slow) }
        return row
    }
}
