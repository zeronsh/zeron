import XCTest

/// The session's bar on the unfolded iPhone Duo: the vertical strip at the
/// screen's edge. A short strip (the keyboard up) folds what doesn't fit
/// into a system ⋯; the session menu is that ⋯, so there's only ever one.
///
/// Runs on the iPhone Duo simulator, unfolded, in the orientation with the
/// vertical bar (the hinge is set outside the test: Device Hub, or a helper
/// posting its hinge event). Skipped anywhere else.
final class DuoBarTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
        let model = ProcessInfo.processInfo.environment["SIMULATOR_MODEL_IDENTIFIER"] ?? ""
        try XCTSkipUnless(model == "iPhone19,4", "iPhone Duo only")
    }

    private func launch() throws -> XCUIApplication {
        let app = XCUIApplication()
        // -remote-voice: voice is offered, so the bar holds everything it can.
        app.launchArguments = ["-demo", "-fast", "-route", "chat:chat-veil", "-remote-voice"]
        app.launch()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        let companions = app.buttons["session-companions"]
        XCTAssertTrue(companions.waitForExistence(timeout: 5))
        let width = app.windows.firstMatch.frame.width
        try XCTSkipUnless(width > 700 && companions.frame.minX > width - 100, "needs the unfolded Duo's vertical bar")
        return app
    }

    /// The bar's ⋯ buttons: those in the strip at the trailing edge (the
    /// keyboard has a "More" of its own).
    private func ellipses(_ app: XCUIApplication) -> [XCUIElement] {
        let width = app.windows.firstMatch.frame.width
        return app.buttons.matching(NSPredicate(format: "label == 'More'")).allElementsBoundByIndex
            .filter { $0.exists && $0.frame.minX > width - 100 }
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    func testOneEllipsisAndNewSessionReachable() throws {
        let app = try launch()
        XCTAssertEqual(ellipses(app).count, 1, "one ⋯ beside the sidebar")

        // The ⋯ is the session menu.
        ellipses(app)[0].tap()
        XCTAssertTrue(app.buttons["Copy Transcript"].waitForExistence(timeout: 5), "⋯ holds the session's actions")
        XCTAssertTrue(app.buttons["Archive"].exists)
        snapshot(app, "duo-session-menu")
        app.buttons["Copy Transcript"].tap()

        // Sidebar hidden, keyboard up: the strip is at its shortest.
        app.buttons["sidebar-hide"].tap()
        XCTAssertTrue(app.buttons["sidebar-show"].waitForExistence(timeout: 5))
        app.textViews["composer-input"].tap()
        sleep(2)
        XCTAssertEqual(ellipses(app).count, 1, "still one ⋯ with the keyboard up")
        snapshot(app, "duo-keyboard-sidebar-hidden")

        // New Session: its own button, or listed by name in the ⋯.
        let compose = app.buttons["detail-new-session"]
        if compose.exists && compose.isHittable {
            compose.tap()
        } else {
            ellipses(app)[0].tap()
            let item = app.buttons["New Session"]
            XCTAssertTrue(item.waitForExistence(timeout: 5), "New Session folds into ⋯ by name")
            item.tap()
        }
        XCTAssertTrue(app.textViews["composer-input"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.scrollViews["transcript"].exists, "the new-session page replaced the session")
    }
}
