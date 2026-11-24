import XCTest

/// Choosing a server before signing in: nothing on the welcome names one
/// until it's chosen, More in the corner opens the sheet, and the core's
/// sentence shows when an address won't do (offline: an http:// address is
/// turned away before anything is looked up). An invite link fills the sheet
/// in and waits for a tap; the server it found then heads the welcome. The
/// invite test reads the fake server at http://localhost:8899 (name "Acme").
final class ServerFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    /// Signed out on Zeron's server, `prod` dropping any `-edge` / `-workos`
    /// a previous run left behind.
    private func launchSignedOut() -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-signedout", "-edge", "prod", "-workos", "prod"]
        app.launch()
        return app
    }

    private func identity(_ app: XCUIApplication) -> XCUIElement {
        app.descendants(matching: .any)["server-identity"]
    }

    private func waitFor(_ element: XCUIElement, label: String, timeout: TimeInterval) -> Bool {
        let labelled = expectation(for: NSPredicate(format: "label == %@", label), evaluatedWith: element)
        return XCTWaiter().wait(for: [labelled], timeout: timeout) == .completed
    }

    func testASelfHostedAddressIsCheckedBeforeSignIn() {
        let app = launchSignedOut()

        let more = app.buttons["welcome-more"]
        XCTAssertTrue(more.waitForExistence(timeout: 15))
        XCTAssertEqual(more.label, "More")
        // Zeron's server: no capsule, no server named anywhere.
        XCTAssertFalse(identity(app).exists, identity(app).debugDescription)
        let naming = NSPredicate(format: "label CONTAINS[c] 'server'")
        XCTAssertEqual(app.staticTexts.matching(naming).count, 0)
        XCTAssertEqual(app.buttons.matching(naming).count, 0)
        snapshot(app, "welcome")

        more.tap()
        let connectItem = app.buttons["connect-to-server"]
        XCTAssertTrue(connectItem.waitForExistence(timeout: 5))
        XCTAssertEqual(connectItem.label, "Connect to Server…")
        XCTAssertEqual(app.buttons["self-hosting-guide"].label, "Self-Hosting Guide")
        snapshot(app, "more-menu")

        connectItem.tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["server-option-zeron"].isSelected)
        XCTAssertFalse(app.textFields["server-address"].exists)
        snapshot(app, "sheet")

        app.buttons["server-option-self-hosted"].tap()
        let field = app.textFields["server-address"]
        XCTAssertTrue(field.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["Your account on a self-hosted server is separate from your Zeron account."].exists)
        let connect = app.buttons["server-connect"]
        XCTAssertEqual(connect.label, "Connect")
        XCTAssertFalse(connect.isEnabled)

        field.typeText("http://zeron.example.com")
        XCTAssertTrue(connect.isEnabled)
        connect.tap()
        let error = app.staticTexts["server-error"]
        XCTAssertTrue(error.waitForExistence(timeout: 15))
        XCTAssertEqual(error.label, "Use https://. Zeron connects over http:// only to servers on your own network.")
        snapshot(app, "insecure")

        // Editing the address clears the error.
        field.typeText("x")
        XCTAssertTrue(error.waitForNonExistence(timeout: 3))

        app.buttons["server-option-zeron"].tap()
        XCTAssertEqual(connect.label, "Use Zeron")
        connect.tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForNonExistence(timeout: 5))
        XCTAssertFalse(identity(app).exists, identity(app).debugDescription)
    }

    /// `zeron://server?address=…` opens the sheet on Self-hosted, looks the
    /// address up, and offers what it found; only the tap switches. The
    /// welcome then names the server, and tapping that opens the sheet on it.
    func testAnInviteLinkOffersItsServer() {
        let app = launchSignedOut()
        XCTAssertTrue(app.buttons["welcome-more"].waitForExistence(timeout: 15))

        app.open(URL(string: "zeron://server?address=http%3A%2F%2Flocalhost%3A8899")!)
        // Some systems ask before an app opens its own link.
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let allow = springboard.buttons["Open"]
        if allow.waitForExistence(timeout: 2) { allow.tap() }

        XCTAssertTrue(app.staticTexts["Choose a server"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.buttons["server-option-self-hosted"].isSelected)
        XCTAssertEqual(app.textFields["server-address"].value as? String, "http://localhost:8899")
        let connect = app.buttons["server-connect"]
        XCTAssertTrue(waitFor(connect, label: "Connect to Acme", timeout: 15), "found: \(connect.label)")
        XCTAssertTrue(connect.isEnabled)
        // Found, not switched: the welcome behind still names no server.
        XCTAssertFalse(identity(app).exists, identity(app).debugDescription)
        snapshot(app, "link-connect")

        connect.tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForNonExistence(timeout: 5))
        let capsule = identity(app)
        XCTAssertTrue(capsule.waitForExistence(timeout: 5))
        XCTAssertEqual(capsule.label, "Server, Acme, localhost:8899")
        XCTAssertTrue(app.staticTexts["Welcome to Zeron"].exists)
        snapshot(app, "welcome-acme")

        // The capsule is the way back to the sheet, on the server it names.
        capsule.tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["server-option-self-hosted"].isSelected)
        XCTAssertEqual(app.textFields["server-address"].value as? String, "http://localhost:8899")
        XCTAssertEqual(connect.label, "Connect")
        app.buttons["server-option-zeron"].tap()
        connect.tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForNonExistence(timeout: 5))
        XCTAssertTrue(capsule.waitForNonExistence(timeout: 5))
    }

    /// Signed in, Settings shows the server in use; picking another means
    /// signing out first, so the sheet's button says Continue.
    func testSettingsShowTheServerInUse() {
        let app = XCUIApplication()
        app.launchArguments = ["-signedout", "-dev", "alice", "org1", "-edge", "prod", "-workos", "prod", "-route", "more"]
        app.launch()

        let row = app.cells["settings-server"]
        XCTAssertTrue(row.waitForExistence(timeout: 15))
        XCTAssertTrue(row.staticTexts["Server"].exists)
        XCTAssertTrue(row.staticTexts["Zeron"].exists)
        snapshot(app, "settings")

        row.tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForExistence(timeout: 5))
        let connect = app.buttons["server-connect"]
        XCTAssertEqual(connect.label, "Continue")
        XCTAssertTrue(app.buttons["server-option-zeron"].isSelected)
        app.buttons["server-option-self-hosted"].tap()
        XCTAssertTrue(app.textFields["server-address"].waitForExistence(timeout: 5))
        XCTAssertEqual(connect.label, "Continue")
        snapshot(app, "settings-sheet")

        app.buttons["server-sheet-close"].tap()
        XCTAssertTrue(app.staticTexts["Choose a server"].waitForNonExistence(timeout: 5))
        XCTAssertTrue(row.exists)
    }
}
