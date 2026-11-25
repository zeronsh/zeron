import XCTest

/// Background delegation over the demo: "Release check" delegated to three
/// subagent chats, one of them (on another device) asking to run a command.
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
        let audit = list.cells["companion-delegated-chat-release-audit"]
        XCTAssertTrue(audit.waitForExistence(timeout: 10))
        XCTAssertTrue(list.cells["companion-delegated-chat-release-notes"].exists)
        XCTAssertTrue(list.cells["companion-delegated-chat-release-bench"].exists)
        // Subagents, not side chats.
        XCTAssertEqual(list.cells.matching(identifier: "companion-side-chat").count, 0)

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
}
