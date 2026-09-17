import XCTest

@MainActor
final class ProjectlessSessionUITests: XCTestCase {
    private var app: XCUIApplication!

    override func setUp() {
        continueAfterFailure = false
        app = XCUIApplication()
        XCUIDevice.shared.orientation = .portrait
    }

    private func launch(_ arguments: [String] = [], filter: String = "") {
        app.launchArguments = ["-demo", "-sethomefilter", filter] + arguments
        app.launch()
    }

    private func openHostPicker() {
        let newSession = app.buttons["new-session"]
        XCTAssertTrue(newSession.waitForExistence(timeout: 5))
        newSession.tap()
        app.buttons["new-projectless-session"].tap()
        XCTAssertTrue(app.navigationBars["Select a device"].waitForExistence(timeout: 5))
    }

    private var composer: XCUIElement {
        app.descendants(matching: .any)["composer-input"].firstMatch
    }

    private func expectContext(_ label: String) {
        let context = app.staticTexts["new-session-context"]
        let matches = XCTNSPredicateExpectation(predicate: NSPredicate(format: "label == %@", label),
                                                object: context)
        XCTAssertEqual(XCTWaiter.wait(for: [matches], timeout: 5), .completed)
    }

    private func sendAndReopenProjectlessSession() {
        openHostPicker()
        // The offline Linux host is valid even with no project on it.
        let offlineHost = app.buttons["session-host-dev-vps"]
        XCTAssertTrue(offlineHost.label.contains("Offline"))
        offlineHost.tap()
        expectContext("No project · hetzner-01")
        XCTAssertFalse(app.buttons["session-checkout"].exists)
        XCTAssertFalse(app.buttons["session-ref"].exists)

        let prompt = "Inspect my home folder without a project."
        composer.tap()
        composer.typeText(prompt)
        // Switching hosts preserves the draft and refreshes the context.
        app.buttons["session-host"].tap()
        let mac = app.buttons["session-host-dev-mac"]
        XCTAssertTrue(mac.waitForExistence(timeout: 5))
        mac.tap()
        expectContext("No project · MacBook Pro")
        app.buttons["composer-send"].tap()
        let transcript = app.otherElements["transcript"]
        XCTAssertTrue(transcript.waitForExistence(timeout: 5))
        XCTAssertTrue(transcript.staticTexts[prompt].waitForExistence(timeout: 5))

        app.navigationBars.buttons.firstMatch.tap()
        let row = app.buttons.matching(NSPredicate(format: "label CONTAINS %@", "No project @ MacBook Pro")).firstMatch
        XCTAssertTrue(row.waitForExistence(timeout: 5), "the new session must be visible in Home")
        row.tap()
        XCTAssertTrue(transcript.waitForExistence(timeout: 5))
        XCTAssertTrue(transcript.staticTexts[prompt].waitForExistence(timeout: 5))
    }

    func testCreateSendAndReopenWithoutAnyProjects() {
        launch(["-no-projects"])
        sendAndReopenProjectlessSession()
    }

    func testProjectlessOptionWhileHomeIsFilteredToAProject() {
        launch(filter: "space-zeron")
        sendAndReopenProjectlessSession()
    }

    func testAnIOSOnlyAccountCannotSelectThePhoneAsHost() {
        launch(["-no-projects", "-ios-only"])
        openHostPicker()
        XCTAssertTrue(app.staticTexts["Connect a desktop device to start a session. No project is required."].exists)
        XCTAssertFalse(app.buttons["session-host-ios-demo"].exists)
        app.buttons["Cancel"].tap()
        XCTAssertTrue(app.buttons["new-session"].waitForExistence(timeout: 5))
    }

    func testProjectSessionStillUsesItsCheckoutAndCanSend() {
        launch(filter: "space-zeron")
        let newSession = app.buttons["new-session"]
        XCTAssertTrue(newSession.waitForExistence(timeout: 5))
        newSession.tap()
        app.buttons["New session in zeron"].tap()
        expectContext("zeron · MacBook Pro")
        XCTAssertTrue(app.buttons["session-checkout"].exists)
        XCTAssertTrue(app.buttons["session-ref"].exists)
        XCTAssertFalse(app.buttons["session-host"].exists)
        composer.tap()
        composer.typeText("Inspect this project.")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.otherElements["transcript"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["zeron @ MacBook Pro"].exists)
    }
}
