import XCTest

/// Activity on the demo workspace: the view from the "…" menu, its list and
/// folded shelves (Working, Asleep, Archived), Sleep (a time, or the date
/// picker), Wake, Stop Waiting, and Settings → Activity. The demo has one
/// chat asleep (chat-blog), one woken (chat-ios-scroll) and one waiting on a
/// process (chat-ios-keyboard).
final class ActivityFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(activity: Bool) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo", "-fast", "-activity", activity ? "on" : "off"]
        app.launch()
        return app
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    private func section(_ app: XCUIApplication, _ id: String) -> XCUIElement {
        app.cells["section-\(id)"]
    }

    private func open(_ app: XCUIApplication, shelf id: String) {
        let header = section(app, id)
        XCTAssertTrue(header.waitForExistence(timeout: 10), "\(id) shelf")
        if !header.isHittable { app.collectionViews.firstMatch.swipeUp() }
        if header.value as? String == "Collapsed" { header.tap() }
        XCTAssertEqual(header.value as? String, "Expanded")
    }

    private func menu(_ app: XCUIApplication) {
        let options = app.buttons["sessions-options"].firstMatch
        XCTAssertTrue(options.waitForExistence(timeout: 10))
        options.tap()
    }

    /// Off, the Sessions page is as it always was: sections and Recent, no
    /// shelves; the waiting chat reads Working, the woken one doesn't ring.
    func testOffIsTheSessionsPageAsBefore() {
        let app = launch(activity: false)
        XCTAssertTrue(app.cells["session-chat-deploy"].waitForExistence(timeout: 10))
        XCTAssertFalse(section(app, "activity").exists)
        XCTAssertTrue(section(app, "recent").exists || app.staticTexts["Recent"].exists)
        let waiting = app.cells["session-chat-ios-keyboard"]
        XCTAssertTrue(waiting.label.contains("Working"), waiting.label)
        XCTAssertFalse(app.cells["session-chat-ios-scroll"].label.contains("Woke"))
        snapshot(app, "activity-off")
    }

    /// The "…" menu turns it on: one list, the shelves folded at its foot.
    func testMenuTurnsActivityOn() {
        let app = launch(activity: false)
        menu(app)
        app.buttons["Activity"].tap()
        XCTAssertTrue(section(app, "activity").waitForExistence(timeout: 5))
        for shelf in ["activity.working", "activity.asleep", "activity.archived"] {
            let header = section(app, shelf)
            if !header.exists { app.collectionViews.firstMatch.swipeUp() }
            XCTAssertTrue(header.waitForExistence(timeout: 5), shelf)
            XCTAssertEqual(header.value as? String, "Collapsed", "\(shelf) starts folded")
        }
        app.collectionViews.firstMatch.swipeDown()
        // Woke: at the top of the list, with the alarm.
        let woke = app.cells["session-chat-ios-scroll"]
        XCTAssertTrue(woke.waitForExistence(timeout: 5))
        XCTAssertTrue(woke.label.contains("Woke"), woke.label)
        // What Activity stands aside from is out of reach.
        menu(app)
        XCTAssertFalse(app.buttons["Group by Project"].isEnabled)
        XCTAssertFalse(app.buttons["New Section…"].isEnabled)
        snapshot(app, "activity-menu")
        app.buttons["Activity"].tap()
        XCTAssertTrue(app.cells["session-chat-deploy"].waitForExistence(timeout: 5))
        XCTAssertFalse(section(app, "activity").exists, "off again")
    }

    func testShelvesListWorkingAndAsleep() {
        let app = launch(activity: true)
        XCTAssertTrue(section(app, "activity").waitForExistence(timeout: 10))
        snapshot(app, "activity-on")
        open(app, shelf: "activity.working")
        XCTAssertTrue(app.cells["session-chat-ios-keyboard"].waitForExistence(timeout: 5), "waiting folds into Working")
        open(app, shelf: "activity.asleep")
        let blog = app.cells["session-chat-blog"]
        XCTAssertTrue(blog.waitForExistence(timeout: 5))
        app.collectionViews.firstMatch.swipeUp()
        snapshot(app, "activity-asleep")
    }

    /// Sleep from the long-press menu: a preset, then Custom… with the
    /// system date picker; then Wake from the swipe.
    func testSleepCustomSleepAndWake() {
        let app = launch(activity: true)
        let deploy = app.cells["session-chat-deploy"]
        XCTAssertTrue(deploy.waitForExistence(timeout: 10))
        deploy.press(forDuration: 1.0)
        app.buttons["Sleep"].tap()
        XCTAssertTrue(app.buttons["Tomorrow"].waitForExistence(timeout: 5))
        snapshot(app, "activity-sleep-menu")
        app.buttons["In 1 Hour"].tap()
        XCTAssertTrue(deploy.waitForNonExistence(timeout: 5), "asleep: off the list, onto the folded shelf")

        let home = app.cells["session-chat-home"]
        XCTAssertTrue(home.waitForExistence(timeout: 5))
        home.press(forDuration: 1.0)
        app.buttons["Sleep"].tap()
        app.buttons["Custom…"].tap()
        let confirm = app.buttons["sleep-confirm"]
        XCTAssertTrue(confirm.waitForExistence(timeout: 5))
        XCTAssertTrue(app.datePickers["sleep-picker"].exists)
        snapshot(app, "activity-custom-sleep")
        confirm.tap()
        XCTAssertTrue(home.waitForNonExistence(timeout: 5))

        // The demo's sleeper, and these two.
        open(app, shelf: "activity.asleep")
        XCTAssertEqual(section(app, "activity.asleep").label, "Asleep, 3")
        let asleep = app.cells["session-chat-deploy"]
        XCTAssertTrue(asleep.waitForExistence(timeout: 5))
        if !asleep.isHittable { app.collectionViews.firstMatch.swipeUp() }
        snapshot(app, "activity-asleep-shelf")
        asleep.swipeLeft()
        app.buttons["Wake"].tap()
        // Awake: back on the list; the shelf keeps the others.
        let shelf = section(app, "activity.asleep")
        expectation(for: NSPredicate(format: "label == 'Asleep, 2'"), evaluatedWith: shelf)
        waitForExpectations(timeout: 5)
        XCTAssertTrue(app.cells["session-chat-deploy"].exists)
    }

    func testStopWaiting() {
        let app = launch(activity: true)
        open(app, shelf: "activity.working")
        let waiting = app.cells["session-chat-ios-keyboard"]
        XCTAssertTrue(waiting.waitForExistence(timeout: 5))
        waiting.press(forDuration: 1.0)
        let stop = app.buttons["Stop Waiting"]
        XCTAssertTrue(stop.waitForExistence(timeout: 5))
        snapshot(app, "activity-stop-waiting")
        stop.tap()
        // Its turn reads as finished: it leaves Working (its only chat, so
        // the shelf goes) for the list.
        XCTAssertTrue(section(app, "activity.working").waitForNonExistence(timeout: 10))
        let back = app.cells["session-chat-ios-keyboard"]
        XCTAssertTrue(back.waitForExistence(timeout: 5))
        XCTAssertFalse(back.label.contains("Working"), back.label)
    }

    /// Settings → Activity, from the menu: switches and the archive picker,
    /// dimmed while it's off.
    func testSettings() {
        let app = launch(activity: true)
        menu(app)
        app.buttons["Activity Settings…"].tap()
        let view = app.switches["toggle-activity-view"]
        XCTAssertTrue(view.waitForExistence(timeout: 5))
        let working = app.switches["toggle-activity-working"]
        XCTAssertTrue(working.isEnabled)
        snapshot(app, "activity-settings-on")
        app.buttons["activity-archive-after"].tap()
        XCTAssertTrue(app.buttons["After 7 Days"].waitForExistence(timeout: 5))
        snapshot(app, "activity-settings-archive")
        app.buttons["After 7 Days"].tap()
        view.tap()
        XCTAssertFalse(working.isEnabled, "dimmed while off")
        XCTAssertFalse(app.buttons["activity-archive-after"].isEnabled)
        snapshot(app, "activity-settings-off")
    }
}
