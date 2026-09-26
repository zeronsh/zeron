import XCTest

@MainActor
final class HarnessUpdatesUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
        app.launchArguments = ["-demo", "-route", "updates:dev-mac"]
        XCUIDevice.shared.orientation = .portrait
        app.launch()
    }

    func testOfflineFixtureCanCancelRetryAndCompleteUpdates() {
        XCTAssertTrue(app.navigationBars["Agent updates"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Codex"].isHittable)
        XCTAssertTrue(app.staticTexts["Claude Code"].exists)
        XCTAssertTrue(app.staticTexts["OpenCode"].exists)
        XCTAssertTrue(app.staticTexts["cursor-agent update"].exists)

        // The queued Claude fixture stays cancellable until explicitly acted on.
        // A fresh Codex download can finish while XCTest waits for sheet resizing.
        let cancel = app.buttons["agent-update-CancelHarnessUpdate-claude-code"]
        XCTAssertTrue(cancel.waitForExistence(timeout: 3))
        cancel.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.buttons["agent-update-ApplyHarnessUpdate-claude-code"].waitForExistence(timeout: 3))

        let update = app.buttons["agent-update-ApplyHarnessUpdate-codex"]
        XCTAssertTrue(update.waitForExistence(timeout: 3))

        update.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.staticTexts["Updated"].waitForExistence(timeout: 10))

        let retry = app.buttons["agent-update-CheckHarnessUpdates-opencode"]
        XCTAssertTrue(retry.waitForExistence(timeout: 3))
        retry.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
        XCTAssertTrue(app.staticTexts["Version 1.0.187 available"].waitForExistence(timeout: 3))
    }

    func testDevicesPaneFitsContentAndResizesForUpdates() {
        app.terminate()
        app.launchArguments = ["-demo", "-sheet", "devices"]
        app.launch()
        let devices = app.navigationBars["Devices"]
        XCTAssertTrue(devices.waitForExistence(timeout: 5))
        // Two devices should leave the Home screen visible above the pane.
        let compactTop = devices.frame.minY
        XCTAssertGreaterThan(compactTop, app.frame.height * 0.45)
        app.buttons["device-updates-dev-mac"].tap()
        let updates = app.navigationBars["Agent updates"]
        XCTAssertTrue(updates.waitForExistence(timeout: 5))
        XCTAssertLessThan(updates.frame.minY, compactTop)
        XCTAssertTrue(app.staticTexts["cursor-agent update"].isHittable)
        updates.buttons["Devices"].tap()
        XCTAssertTrue(devices.waitForExistence(timeout: 5))
        XCTAssertGreaterThan(devices.frame.minY, app.frame.height * 0.45)
        app.buttons["device-updates-dev-vps"].tap()
        XCTAssertTrue(app.staticTexts["Device offline. Updates can be started when it reconnects."].waitForExistence(timeout: 5))
    }
}
