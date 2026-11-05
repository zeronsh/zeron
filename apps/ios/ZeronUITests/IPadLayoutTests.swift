import XCTest

/// The iPad split shell: the phone UI with a sidebar beside the session column.
final class IPadLayoutTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .pad, "iPad only")
    }

    private func launch(_ args: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast"] + args
        app.launch()
        return app
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    func testSidebarOpensSessionsBesideIt() {
        XCUIDevice.shared.orientation = .landscapeLeft
        let app = launch()
        // Launch lands on the new-session page beside the sidebar.
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        snapshot(app, "ipad-draft-landscape")
        let row = app.cells["session-chat-veil"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 5))
        XCTAssertTrue(row.exists, "sidebar stays beside the session")
        sleep(1)
        snapshot(app, "ipad-session-landscape")
        XCUIDevice.shared.orientation = .portrait
        sleep(1)
        snapshot(app, "ipad-session-portrait")
        // Another session replaces it in the main column.
        app.cells["session-chat-deploy"].tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Wrangler deploy hygiene'")).firstMatch.waitForExistence(timeout: 5))
    }

    func testNewSessionFromSidebarAndSearch() {
        XCUIDevice.shared.orientation = .landscapeLeft
        let app = launch(["-route", "chat:chat-deploy"])
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        app.buttons["new-session"].tap()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        input.typeText("Add a dark mode toggle to settings")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10), "the new session opens in the main column")
        let search = app.descendants(matching: .any)["sidebar-search"].firstMatch
        search.tap()
        search.typeText("wrangler")
        XCTAssertTrue(app.cells["session-chat-deploy"].waitForExistence(timeout: 5))
        snapshot(app, "ipad-search")
        search.typeText("\n")
        app.buttons["sidebar-settings"].tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 5))
        snapshot(app, "ipad-settings")
        // Regression: action sheets on iPad need an anchor (popover) or UIKit throws.
        let signOut = app.staticTexts["Sign Out"].firstMatch
        for _ in 0..<5 where !signOut.isHittable { app.swipeUp() }
        signOut.tap()
        XCTAssertTrue(app.staticTexts["Sign out?"].waitForExistence(timeout: 5), "sign-out sheet shows as a popover")
        XCTAssertEqual(app.state, .runningForeground)
    }
}
