import XCTest

/// Drives every glass transition slowly (for screen recordings / visual
/// review): push + pop, tab switches, the new-session sheet, and
/// scroll-driven tab bar minimize.
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
        // Tabs.
        for tab in ["Projects", "PRs", "More", "Sessions"] {
            app.tabBars.buttons[tab].tap()
            sleep(1)
        }
        // Scroll to minimize the tab bar, then back.
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
    /// Just push + pop (fast iteration on the tab bar / accessory handoff).
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
