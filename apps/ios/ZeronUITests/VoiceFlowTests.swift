import XCTest

/// Voice controls around a scripted call (`-voice-preview`: no audio, host or relay).
final class VoiceFlowTests: XCTestCase {
    override func setUpWithError() throws {
        continueAfterFailure = false
        try XCTSkipUnless(UIDevice.current.userInterfaceIdiom == .phone, "iPhone tab shell")
    }

    private func launch(_ args: [String]) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-voice-preview"] + args
        app.launch()
        return app
    }

    /// Stage → transcript → back: the live strip in the bottom bar reopens the stage.
    func testLiveStripReopensTheStageAfterTheTranscript() {
        let app = launch(["-voice-stage"])
        let stage = app.otherElements["voice-stage"]
        XCTAssertTrue(stage.waitForExistence(timeout: 10))
        app.buttons["voice-transcript"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        XCTAssertFalse(stage.exists)
        app.navigationBars.buttons.element(boundBy: 0).tap()
        let strip = app.descendants(matching: .any)["voice-live"].firstMatch
        XCTAssertTrue(strip.waitForExistence(timeout: 5))
        strip.tap()
        XCTAssertTrue(stage.waitForExistence(timeout: 5), "the live strip opens the call stage")
    }

    /// Minimizing the stage and tapping the strip brings it back.
    func testLiveStripReopensAMinimizedStage() {
        let app = launch(["-voice-stage"])
        let stage = app.otherElements["voice-stage"]
        XCTAssertTrue(stage.waitForExistence(timeout: 10))
        app.buttons["voice-minimize"].tap()
        let strip = app.descendants(matching: .any)["voice-live"].firstMatch
        XCTAssertTrue(strip.waitForExistence(timeout: 5))
        sleep(1)
        strip.tap()
        XCTAssertTrue(stage.waitForExistence(timeout: 5))
    }

    /// Settings → Voice lists hosts and call tips; the voice is a pop-up menu.
    func testVoiceSettings() {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-remote-voice", "-route", "more"]
        app.launch()
        let row = app.cells["settings-voice"]
        XCTAssertTrue(row.waitForExistence(timeout: 10))
        row.tap()
        let style = app.cells["voice-style"]
        XCTAssertTrue(style.waitForExistence(timeout: 5))
        snapshot(app, "voice-settings")
        style.tap()
        let maple = app.buttons["Maple"]
        XCTAssertTrue(maple.waitForExistence(timeout: 5))
        snapshot(app, "voice-settings-menu")
        maple.tap()
        XCTAssertTrue(style.staticTexts["Maple"].waitForExistence(timeout: 5), "the chosen voice shows in the row")
        app.swipeUp()
        snapshot(app, "voice-settings-scrolled")
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }
}
