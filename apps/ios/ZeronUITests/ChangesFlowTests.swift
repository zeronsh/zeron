import XCTest

/// Synthetic checkout through the production host RPC, parser and UI.
final class ChangesFlowTests: XCTestCase {
    override func setUp() { continueAfterFailure = false }

    func testReadFoldCommentCancelEditAndSendReview() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-route", "chat:chat-deploy"]
        app.launch()
        let layers = app.buttons["session-companions"]
        XCTAssertTrue(layers.waitForExistence(timeout: 15))
        layers.tap()
        let changes = app.cells["companion-changes"]
        XCTAssertTrue(changes.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS '+4' AND label CONTAINS '2 files'")).firstMatch.waitForExistence(timeout: 10))
        changes.tap()
        XCTAssertTrue(app.tables["changes-diff"].waitForExistence(timeout: 5))
        let added = app.cells.matching(NSPredicate(format: "label CONTAINS 'Added 2:'")).firstMatch
        XCTAssertTrue(added.waitForExistence(timeout: 5))
        let file = app.buttons["change-file-Sources/Welcome.swift"]
        file.tap()
        XCTAssertFalse(added.exists)
        file.tap()
        XCTAssertTrue(added.waitForExistence(timeout: 3))
        let screenshot = XCTAttachment(screenshot: app.screenshot())
        screenshot.name = "ios-changes-unified-diff"
        screenshot.lifetime = .keepAlways
        add(screenshot)

        let commentButton = app.buttons["changes-comment"]
        let input = app.textViews["change-comment-input"]
        let close = app.buttons["change-comment-close"]
        XCTAssertFalse(commentButton.exists)
        // A plain tap comments directly. Closing it never starts selection.
        added.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 3))
        close.tap()
        XCTAssertFalse(commentButton.exists)
        // A hold starts the persistent range mode without opening a sheet.
        added.press(forDuration: 0.7)
        XCTAssertTrue(commentButton.waitForExistence(timeout: 3))
        XCTAssertFalse(input.exists)
        let fourth = app.cells.matching(NSPredicate(format: "label CONTAINS 'Added 4:'")).firstMatch
        let third = app.cells.matching(NSPredicate(format: "label CONTAINS 'Added 3:'")).firstMatch
        fourth.tap()
        XCTAssertTrue(commentButton.label.contains("3 selected lines"))
        third.tap()
        XCTAssertTrue(commentButton.label.contains("2 selected lines"))
        app.buttons["changes-clear-selection"].tap()
        XCTAssertFalse(commentButton.exists)
        added.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 3))
        input.tap()
        input.typeText("Discard this draft")
        close.tap()
        XCTAssertFalse(app.buttons["changes-review"].exists)
        added.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 3))
        XCTAssertEqual(input.value as? String, "")
        input.tap()
        input.typeText("Please add a test for this title.")
        app.buttons["change-comment-save"].tap()
        app.buttons["changes-review"].tap()
        XCTAssertTrue(app.tables["change-review-list"].waitForExistence(timeout: 3))
        XCTAssertTrue(app.staticTexts["Sources/Welcome.swift:2 (R)"].exists)
        app.tables["change-review-list"].cells.firstMatch.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 3))
        input.tap()
        input.typeText(" Include empty names.")
        app.buttons["change-comment-save"].tap()
        XCTAssertTrue(app.staticTexts["Please add a test for this title. Include empty names."].waitForExistence(timeout: 3))
        app.buttons["change-review-send"].tap()
        XCTAssertTrue(app.tables["changes-diff"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["changes-review"].exists)
        // A second note proves the previous review cleared. Remove it.
        added.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 3))
        input.tap()
        input.typeText("Remove this note")
        app.buttons["change-comment-save"].tap()
        app.buttons["changes-review"].tap()
        let note = app.tables["change-review-list"].cells.firstMatch
        note.swipeLeft()
        app.buttons["Delete"].tap()
        XCTAssertFalse(app.buttons["change-review-send"].isEnabled)
    }
}
