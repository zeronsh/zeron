import XCTest

/// Drives every glass transition slowly (for screen recordings / visual
/// review): push + pop, Settings and back, the new-session sheet, and
/// scrolling under the bottom bar.
final class TransitionTourTests: XCTestCase {
    func testTour() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"]
        app.launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        sleep(1)
        // Push a session, then pop.
        app.cells.matching(identifier: "session-chat-cjk").firstMatch.tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 5))
        sleep(2)
        app.navigationBars.buttons.element(boundBy: 0).tap()
        sleep(2)
        // Settings and back.
        app.buttons["bottom-settings"].tap()
        sleep(1)
        app.navigationBars["Settings"].buttons.firstMatch.tap()
        sleep(1)
        // Scroll under the bottom bar, then back.
        let list = app.collectionViews.firstMatch
        list.swipeUp(velocity: .slow)
        sleep(1)
        list.swipeDown(velocity: .slow)
        sleep(1)
        // New-session sheet in and out.
        app.buttons["new-session"].tap()
        sleep(2)
        app.navigationBars["New Session"].buttons.firstMatch.tap()
        sleep(2)
    }
}

extension TransitionTourTests {
    /// Just push + pop (fast iteration on the bottom bar handoff).
    func testPushPop() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"]
        app.launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        sleep(1)
        app.cells.matching(identifier: "session-chat-cjk").firstMatch.tap()
        sleep(2)
        app.navigationBars.buttons.element(boundBy: 0).tap()
        sleep(2)
    }
}
