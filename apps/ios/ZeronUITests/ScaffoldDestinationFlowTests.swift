import XCTest

/// The desktop's Scaffold destinations under New Session's "Run on" menu.
/// The demo has no desktop controller, so this covers the picker: the rows,
/// what selecting one does to the chips, the empty managed-model catalog,
/// and going back to a device. (`-scaffold-destinations` is DEBUG-only.)
final class ScaffoldDestinationFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    private func menuItem(_ app: XCUIApplication, _ title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", title)).firstMatch
    }

    func testScaffoldDestinationsSitUnderTheDevices() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-scaffold-destinations", "-route", "new"]
        app.launch()
        let host = app.buttons["composer-chip-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 10))
        let device = host.label
        XCTAssertTrue(app.buttons["composer-chip-project"].exists)

        host.tap()
        XCTAssertTrue(app.staticTexts["New Scaffold sandbox"].waitForExistence(timeout: 5), "a sandbox section under the devices")
        for title in ["Local DB", "Staging DB", "Production DB"] {
            XCTAssertTrue(menuItem(app, title).exists, "\(title) is listed")
        }
        // The chip and its menu row both start with the device's name.
        XCTAssertGreaterThanOrEqual(app.buttons.matching(NSPredicate(format: "label BEGINSWITH %@", device)).count, 2, "the devices stay listed")
        snapshot(app, "run-on-menu")

        menuItem(app, "Staging DB").tap()
        XCTAssertTrue(app.buttons["composer-chip-project"].waitForNonExistence(timeout: 5), "a sandbox has no project to pick")
        XCTAssertEqual(host.label, "Scaffold (Staging DB)")
        XCTAssertFalse(app.buttons["composer-chip-branch"].exists)
        snapshot(app, "scaffold-selected")

        // No controller answers in the demo: say so rather than open an empty picker.
        app.buttons["composer-chip-model"].tap()
        XCTAssertTrue(app.alerts["No models yet"].waitForExistence(timeout: 5))
        snapshot(app, "no-managed-models")
        app.alerts.buttons["OK"].tap()

        // A device again: its project comes back.
        host.tap()
        let item = menuItem(app, device)
        XCTAssertTrue(item.waitForExistence(timeout: 5))
        item.tap()
        XCTAssertTrue(app.buttons["composer-chip-project"].waitForExistence(timeout: 5))
        XCTAssertEqual(host.label, device)
    }

    func testTheOfflineDemoOffersNoSandboxes() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-route", "new"]
        app.launch()
        let host = app.buttons["composer-chip-host"]
        XCTAssertTrue(host.waitForExistence(timeout: 10))
        host.tap()
        XCTAssertTrue(app.staticTexts["Run on"].waitForExistence(timeout: 5), "the menu is open")
        XCTAssertFalse(app.staticTexts["New Scaffold sandbox"].exists)
        XCTAssertFalse(menuItem(app, "Staging DB").exists)
    }
}
