import XCTest

/// Agent visuals in the demo transcript: rendered by their Rust workers,
/// driven through the touch targets laid over their controls.
final class VisualFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    func testDashboardControlsDriveTheScript() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-route", "chat:chat-visuals"]
        app.launch()

        let toggle = app.buttons["Show table"]
        XCTAssertTrue(toggle.waitForExistence(timeout: 15))
        XCTAssertEqual(toggle.value as? String, "Off")
        snapshot(app, "dashboard")

        toggle.tap()
        // The script re-renders, the worker lays out again, the target updates.
        let on = NSPredicate(format: "value == 'On'")
        expectation(for: on, evaluatedWith: toggle)
        waitForExpectations(timeout: 5)
        snapshot(app, "dashboard-table")

        let slider = app.otherElements["Smoothing window"].firstMatch.exists
            ? app.otherElements["Smoothing window"].firstMatch
            : app.descendants(matching: .any)["Smoothing window"].firstMatch
        XCTAssertTrue(slider.exists)
        XCTAssertEqual(slider.value as? String, "3")
        // A touch on the track moves the thumb there (1…7 in steps of 1).
        slider.coordinate(withNormalizedOffset: CGVector(dx: 0.84, dy: 0.5)).tap()
        expectation(for: NSPredicate(format: "value == '6'"), evaluatedWith: slider)
        waitForExpectations(timeout: 5)

        let tab = app.buttons["30d"]
        XCTAssertTrue(tab.exists)
        tab.tap()
        expectation(for: NSPredicate(format: "selected == true"), evaluatedWith: tab)
        waitForExpectations(timeout: 5)
        snapshot(app, "dashboard-30d")
    }
}
