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
        app.launchArguments = ["-demo", "-fast", "-fresh-demo", "-activity", activity ? "on" : "off"]
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

    /// A row, scrolled to: a list's rows below the fold aren't made until
    /// they're near the screen.
    @discardableResult
    private func row(_ app: XCUIApplication, _ id: String, down: Bool = true) -> XCUIElement {
        let cell = app.cells["session-\(id)"]
        let list = app.collectionViews.firstMatch
        // That way first, then back the other way.
        for step in 0..<14 where !(cell.exists && cell.isHittable) {
            if (step < 5) == down { list.swipeUp(velocity: .slow) } else { list.swipeDown(velocity: .slow) }
        }
        XCTAssertTrue(cell.waitForExistence(timeout: 5), id)
        return cell
    }

    /// A menu item by its title (one with a subtitle reads "Title, subtitle").
    private func item(_ app: XCUIApplication, _ title: String) -> XCUIElement {
        app.buttons.matching(NSPredicate(format: "label == %@ OR label BEGINSWITH %@", title, title + ",")).firstMatch
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
        XCTAssertTrue(section(app, "pinned").waitForExistence(timeout: 10))
        XCTAssertFalse(section(app, "activity").exists)
        snapshot(app, "activity-off")
        let woken = row(app, "chat-ios-scroll")
        XCTAssertFalse(woken.label.contains("Woke"), woken.label)
        let waiting = row(app, "chat-ios-keyboard")
        XCTAssertTrue(waiting.label.contains("Working"), waiting.label)
        // (Recent, or project groups, as this phone groups them.)
        row(app, "chat-deploy")
        XCTAssertFalse(section(app, "activity.asleep").exists)
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
        // Woke: near the top of the list, with the alarm.
        let woke = row(app, "chat-ios-scroll", down: false)
        XCTAssertTrue(woke.label.contains("Woke"), woke.label)
        // What Activity stands aside from is out of reach.
        menu(app)
        XCTAssertFalse(app.buttons["Group by Project"].isEnabled)
        XCTAssertFalse(app.buttons["New Section…"].isEnabled)
        snapshot(app, "activity-menu")
        app.buttons["Activity"].tap()
        XCTAssertTrue(section(app, "activity").waitForNonExistence(timeout: 5), "off again")
        row(app, "chat-deploy")
    }

    func testShelvesListWorkingAndAsleep() {
        let app = launch(activity: true)
        XCTAssertTrue(section(app, "activity").waitForExistence(timeout: 10))
        snapshot(app, "activity-on")
        open(app, shelf: "activity.working")
        row(app, "chat-ios-keyboard")
        open(app, shelf: "activity.asleep")
        let blog = row(app, "chat-blog")
        // Its corner says when it wakes (the demo's: in about 3 hours).
        XCTAssertTrue(blog.label.contains("Draft the launch post"), blog.label)
        app.collectionViews.firstMatch.swipeUp()
        snapshot(app, "activity-asleep")
    }

    /// Sleep from the long-press menu: a preset, then Custom… with the
    /// system date picker; then Wake from the swipe.
    func testSleepCustomSleepAndWake() {
        let app = launch(activity: true)
        XCTAssertTrue(section(app, "activity").waitForExistence(timeout: 10))
        let deploy = row(app, "chat-deploy")
        deploy.press(forDuration: 1.0)
        app.buttons["Sleep"].tap()
        XCTAssertTrue(item(app, "Tomorrow").waitForExistence(timeout: 5))
        snapshot(app, "activity-sleep-menu")
        item(app, "In 1 Hour").tap()
        XCTAssertTrue(deploy.waitForNonExistence(timeout: 5), "asleep: off the list, onto the folded shelf")

        let home = row(app, "chat-home", down: false)
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
        let asleep = row(app, "chat-deploy")
        snapshot(app, "activity-asleep-shelf")
        // (Along its top edge: the list's foot sits under the bottom bar.)
        asleep.coordinate(withNormalizedOffset: CGVector(dx: 0.9, dy: 0.2))
            .press(forDuration: 0.05, thenDragTo: asleep.coordinate(withNormalizedOffset: CGVector(dx: 0.15, dy: 0.2)))
        let wake = app.buttons["Wake"]
        XCTAssertTrue(wake.waitForExistence(timeout: 5))
        snapshot(app, "activity-wake-swipe")
        wake.tap()
        // Awake: back on the list; the shelf keeps the others.
        let shelf = section(app, "activity.asleep")
        expectation(for: NSPredicate(format: "label == 'Asleep, 2'"), evaluatedWith: shelf)
        waitForExpectations(timeout: 5)
        XCTAssertTrue(app.cells["session-chat-deploy"].exists)
    }

    func testStopWaiting() {
        let app = launch(activity: true)
        open(app, shelf: "activity.working")
        let waiting = row(app, "chat-ios-keyboard")
        // Under way: Sleep in its swipe, no Archive (stop it first).
        waiting.swipeLeft()
        XCTAssertTrue(app.buttons["Sleep"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.buttons["Archive"].exists, "a working chat isn't archived")
        snapshot(app, "activity-working-swipe")
        waiting.swipeRight()
        waiting.press(forDuration: 1.0)
        XCTAssertFalse(item(app, "Archive").isEnabled, "Archive waits for it to stop")
        let stop = app.buttons["Stop Waiting"]
        XCTAssertTrue(stop.waitForExistence(timeout: 5))
        snapshot(app, "activity-stop-waiting")
        stop.tap()
        // Its turn reads as finished: it leaves Working (its only chat, so
        // the shelf goes) for the list.
        XCTAssertTrue(section(app, "activity.working").waitForNonExistence(timeout: 10))
        let back = row(app, "chat-ios-keyboard", down: false)
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
