import XCTest

/// End-to-end flows over the Rust core's demo workspace (real registry,
/// session docs, command ledger and simulated host — no network).
final class SessionFlowTests: XCTestCase {
    override func setUp() {
        continueAfterFailure = false
    }

    private func launch(_ args: [String] = [], fast: Bool = true) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-demo"] + (fast ? ["-fast"] : []) + args
        app.launch()
        return app
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let shot = XCTAttachment(screenshot: app.screenshot())
        shot.name = name
        shot.lifetime = .keepAlways
        add(shot)
    }

    func testFrontPageShowsFoldersAndRecents() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        let pinned = app.cells["section-pinned"]
        XCTAssertTrue(pinned.exists)
        snapshot(app, "front-page")
        // Sections fold in place, like the desktop sidebar.
        let wasCollapsed = pinned.value as? String == "Collapsed"
        pinned.tap()
        XCTAssertEqual(pinned.value as? String, wasCollapsed ? "Expanded" : "Collapsed")
        pinned.tap()
        XCTAssertEqual(pinned.value as? String, wasCollapsed ? "Collapsed" : "Expanded")
        // Long-press → Open drills into the folder (reorder lives there).
        pinned.press(forDuration: 0.8)
        app.buttons["Open"].tap()
        XCTAssertTrue(app.navigationBars["Pinned"].waitForExistence(timeout: 5))
        snapshot(app, "pinned-folder")
    }

    func testSendEchoesAndStreamsReply() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Summarize the launch post in three bullets.")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3), "keyboard dismissed on send")
        // Optimistic echo appears immediately, then the host streams a reply.
        let echo = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'You: Summarize the launch post'")).firstMatch
        XCTAssertTrue(echo.waitForExistence(timeout: 3))
        snapshot(app, "streaming")
        // The host's reply lands after the echo and the turn settles.
        let reply = app.staticTexts.matching(identifier: "row-markdown").element(boundBy: 0)
        XCTAssertTrue(reply.waitForExistence(timeout: 15))
        XCTAssertTrue(app.buttons["Send message"].waitForExistence(timeout: 30))
        snapshot(app, "reply-complete")
    }

    /// Leaving a session with a long draft and coming back rests the
    /// composer as a preview of whole lines (ending in "…"), not a scrolled
    /// box with the next line sliced through the bottom edge.
    func testLongDraftRestsAsWholeLinePreview() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("let's fix the new worktree icon it makes no sense, also let's make it so that there's no descriptions the line is too long and wraps onto a fourth line")
        snapshot(app, "draft-focused")
        app.navigationBars.buttons.element(boundBy: 0).tap()
        let row = app.cells["session-chat-deploy"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3))
        snapshot(app, "draft-resting")
        // Two whole lines plus the capsule's insets — never a third.
        XCTAssertLessThan(input.frame.height, 90)
        input.tap()
        XCTAssertGreaterThan(input.frame.height, 90, "focusing restores the full draft")
        snapshot(app, "draft-reopened")
    }

    func testQuestionPanelAnswersAndResumes() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Pick a tone for the post ?ask")
        app.buttons["composer-send"].tap()
        let option = app.buttons["question-option-0"]
        XCTAssertTrue(option.waitForExistence(timeout: 20))
        snapshot(app, "question")
        option.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 10), "composer returns after answering")
    }

    func testQueueWhileWorking() {
        let app = launch(["-route", "chat:chat-home", "-longreply"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Write a long reply please.")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.buttons["Stop response"].waitForExistence(timeout: 5))
        input.tap()
        input.typeText("Then add a TL;DR.")
        XCTAssertTrue(app.buttons["Queue message"].waitForExistence(timeout: 2))
        app.buttons["Queue message"].tap()
        XCTAssertTrue(app.buttons["More queue actions"].waitForExistence(timeout: 5))
        snapshot(app, "queued")
        // A second queued message, then the composer put away.
        input.tap()
        input.typeText("And link the docs, with a much longer follow-up line that has to wrap or fade somewhere.")
        app.buttons["Queue message"].tap()
        // Regression: a long queued message pushed its row's buttons off the card.
        let second = app.buttons.matching(NSPredicate(format: "label == 'More queue actions'")).element(boundBy: 1)
        XCTAssertTrue(second.waitForExistence(timeout: 5))
        XCTAssertTrue(second.isHittable, "the long row's actions stay on the card")
        XCTAssertLessThanOrEqual(second.frame.maxX, app.windows.firstMatch.frame.maxX)
        snapshot(app, "queued-two")
        app.scrollViews["transcript"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3)).tap()
        sleep(1)
        snapshot(app, "queued-resting")
    }

    func testJumpToLatestAfterScrollingUp() {
        let app = launch(["-route", "chat:chat-veil", "-big"])
        let transcript = app.scrollViews["transcript"]
        XCTAssertTrue(transcript.waitForExistence(timeout: 10))
        transcript.swipeDown(velocity: .fast)
        transcript.swipeDown(velocity: .fast)
        let jump = app.buttons["jump-to-latest"]
        XCTAssertTrue(jump.waitForExistence(timeout: 5))
        snapshot(app, "scrolled-up")
        jump.tap()
    }

    /// Search → back to Sessions / Settings (recorded for the accessory
    /// transition).
    func testSearchTabRoundTrip() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 5))
        for back in ["Sessions", "Settings"] {
            app.tabBars.buttons[back].tap()
            sleep(1)
            app.tabBars.buttons["Search"].tap()
            sleep(2)
            // Close beside the field leaves search for the tab you came from.
            app.buttons["Close"].firstMatch.tap()
            sleep(2)
            XCTAssertTrue(accessory.waitForExistence(timeout: 5), "accessory back after search")
            XCTAssertTrue(accessory.isHittable, "accessory usable after search")
        }
    }

    /// Tapping a link in the transcript opens it (in-app Safari for web).
    func testTappingLinkOpensIt() {
        let app = launch(["-route", "chat:chat-cjk"])
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        let link = app.descendants(matching: .any).matching(NSPredicate(format: "identifier == 'transcript-link' AND value CONTAINS 'wikipedia'")).firstMatch
        XCTAssertTrue(link.waitForExistence(timeout: 10), "link exposed")
        for _ in 0..<4 where !link.isHittable { app.scrollViews["transcript"].swipeUp() }
        link.tap()
        // SFSafariViewController's toolbar.
        XCTAssertTrue(app.buttons["Done"].waitForExistence(timeout: 8) || app.buttons["Close"].waitForExistence(timeout: 1), "link opened")
        snapshot(app, "link-opened")
    }

    /// Settings turns notifications on (system prompt), and tapping a
    /// session notification opens that session. The notification is sent
    /// from the host (`xcrun simctl push`, same payload as the edge) once the
    /// test signals it's in the background via /tmp/zeron-push-ready.
    func testNotificationTapOpensSession() throws {
        // Needs a host-side sender (see the comment above); skip without one.
        try XCTSkipUnless(FileManager.default.fileExists(atPath: "/tmp/zeron-push-host"), "no host push sender")
        let ready = "/tmp/zeron-push-ready"
        try? FileManager.default.removeItem(atPath: ready)
        let app = launch()
        XCTAssertTrue(app.tabBars.buttons["Settings"].waitForExistence(timeout: 10))
        app.tabBars.buttons["Settings"].tap()
        let toggle = app.switches["toggle-notify:enabled"]
        XCTAssertTrue(toggle.waitForExistence(timeout: 5))
        if (toggle.value as? String) != "1" {
            toggle.tap()
            let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
            let allow = springboard.buttons["Allow"]
            if allow.waitForExistence(timeout: 5) { allow.tap() }
        }
        XCTAssertTrue(app.switches["toggle-notify:done"].waitForExistence(timeout: 5), "kinds shown once on")
        snapshot(app, "notification-settings")
        XCUIDevice.shared.press(.home)
        FileManager.default.createFile(atPath: ready, contents: Data())
        let springboard = XCUIApplication(bundleIdentifier: "com.apple.springboard")
        let banner = springboard.descendants(matching: .any).matching(NSPredicate(format: "label CONTAINS 'Run finished'")).firstMatch
        guard banner.waitForExistence(timeout: 45) else {
            throw XCTSkip("no notification delivered (host didn't push)")
        }
        banner.tap()
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label CONTAINS 'Wrangler deploy hygiene'")).firstMatch.waitForExistence(timeout: 10), "tapped session opened")
        snapshot(app, "notification-opened")
    }

    func testTabsAndSearch() {
        let app = launch()
        XCTAssertTrue(app.staticTexts["Sessions"].waitForExistence(timeout: 10))
        app.tabBars.buttons["Settings"].tap()
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 5))
        snapshot(app, "settings")
        app.tabBars.buttons["Sessions"].tap()
        XCTAssertTrue(app.navigationBars["Sessions"].waitForExistence(timeout: 5))
    }

    func testNewSessionFromAccessory() {
        let app = launch()
        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 10))
        accessory.tap()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        snapshot(app, "new-session")
        input.typeText("Add a dark mode toggle to settings")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10), "hands off into the new session")
        // Sending puts the keyboard away; the handoff ends with the sheet gone
        // and the message in the transcript.
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3), "keyboard dismissed on send")
        XCTAssertTrue(app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'You: Add a dark mode toggle'")).firstMatch.waitForExistence(timeout: 5))
        sleep(1)
        XCTAssertFalse(app.buttons["Close"].exists, "sheet dismissed after the motion")
        snapshot(app, "new-session-created")
    }

    func testFileMentionSuggestions() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Look at @lay")
        let first = app.buttons["mention-0"]
        XCTAssertTrue(first.waitForExistence(timeout: 5))
        snapshot(app, "mentions")
        first.tap()
        XCTAssertTrue((input.value as? String ?? "").contains("@mod.rs"), "token inserted: \(input.value ?? "")")
    }

    func testArchiveWithUndo() {
        let app = launch()
        let cell = app.cells["session-chat-deploy"]
        XCTAssertTrue(cell.waitForExistence(timeout: 10))
        // Recent sits below the inline sections; bring the row clear of the accessory.
        app.collectionViews.firstMatch.swipeUp()
        cell.swipeLeft()
        app.buttons["Archive"].tap()
        let undo = app.buttons["toast-action"]
        XCTAssertTrue(undo.waitForExistence(timeout: 3))
        snapshot(app, "archive-undo")
        undo.tap()
        XCTAssertTrue(app.cells["session-chat-deploy"].waitForExistence(timeout: 5), "unarchived session returns")
    }

    func testNewProjectFolderBrowser() {
        // New projects are created from the new-session project picker.
        let app = launch(["-route", "new"])
        let chip = app.buttons["composer-chip-project"]
        XCTAssertTrue(chip.waitForExistence(timeout: 10))
        chip.tap()
        let add = app.buttons["New Project…"]
        XCTAssertTrue(add.waitForExistence(timeout: 5))
        add.tap()
        let use = app.buttons["use-folder"]
        XCTAssertTrue(use.waitForExistence(timeout: 5))
        let firstFolder = app.collectionViews.cells.element(boundBy: 0)
        XCTAssertTrue(firstFolder.waitForExistence(timeout: 5))
        snapshot(app, "new-project")
        firstFolder.tap()
        XCTAssertTrue(use.waitForExistence(timeout: 5))
        use.tap()
        XCTAssertTrue(chip.waitForExistence(timeout: 10), "sheet dismissed after creating")
        XCTAssertFalse(use.exists)
    }

    func testToolGroupExpandsAndShowsDetail() {
        let app = launch(["-route", "chat:chat-veil"])
        let expand = app.buttons["Expand"].firstMatch
        XCTAssertTrue(expand.waitForExistence(timeout: 10))
        expand.tap()
        let detail = app.buttons["Show details"].firstMatch
        XCTAssertTrue(detail.waitForExistence(timeout: 5))
        snapshot(app, "tools-expanded")
        // Details open inline under the row, like desktop.
        detail.tap()
        XCTAssertTrue(app.buttons["Hide details"].firstMatch.waitForExistence(timeout: 5))
        sleep(1)
        snapshot(app, "tool-detail")
    }

    /// Regression: picking a reasoning effort from the composer chip crashed.
    func testEffortPickerInSession() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        let chip = app.buttons["composer-chip-effort"]
        XCTAssertTrue(chip.waitForExistence(timeout: 5))
        chip.tap()
        let item = app.collectionViews.buttons.element(boundBy: 0)
        XCTAssertTrue(item.waitForExistence(timeout: 5), "effort levels load")
        snapshot(app, "effort-menu")
        item.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        XCTAssertEqual(app.state, .runningForeground)
        chip.tap()
        XCTAssertTrue(app.collectionViews.buttons.element(boundBy: 0).waitForExistence(timeout: 5))
        XCTAssertEqual(app.state, .runningForeground)
    }

    /// Regression: the model chip read "Claude Code" (the harness) until the
    /// host's model list came back over the relay.
    func testModelChipNamesAModelAtOnce() {
        let app = launch(["-route", "new"])
        let chip = app.buttons["composer-chip-model"]
        XCTAssertTrue(chip.waitForExistence(timeout: 10))
        XCTAssertFalse(chip.label.contains("Claude Code"), "chip shows a model, got \(chip.label)")
    }

    /// Closing the new-session page keeps what was typed and the options
    /// picked; sending clears the text for next time.
    func testNewSessionRemembersDraftWhenClosed() {
        let app = launch()
        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 10))
        accessory.tap()
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        input.typeText("Half-written idea")
        let effort = app.buttons["composer-chip-effort"]
        XCTAssertTrue(effort.waitForExistence(timeout: 5))
        effort.tap()
        app.buttons["Low"].firstMatch.tap()
        XCTAssertTrue(effort.label.contains("Low"))
        // Drag the sheet away by its top edge.
        let bar = app.navigationBars["New Session"]
        let top = bar.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.2))
        top.press(forDuration: 0.05, thenDragTo: app.windows.firstMatch.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.98)))
        XCTAssertTrue(input.waitForNonExistence(timeout: 5), "sheet dismissed")

        accessory.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        XCTAssertEqual(input.value as? String, "Half-written idea", "typed text comes back")
        XCTAssertTrue(app.buttons["composer-chip-effort"].label.contains("Low"), "picked effort comes back")

        // Sending uses it up: the next page starts empty.
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        app.navigationBars.buttons.element(boundBy: 0).tap()
        XCTAssertTrue(accessory.waitForExistence(timeout: 5))
        accessory.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        XCTAssertNotEqual(input.value as? String, "Half-written idea")
    }

    func testEffortPickerInNewSession() {
        let app = launch(["-route", "new"])
        let chip = app.buttons["composer-chip-effort"]
        XCTAssertTrue(chip.waitForExistence(timeout: 10))
        chip.tap()
        let item = app.collectionViews.buttons.element(boundBy: 0)
        XCTAssertTrue(item.waitForExistence(timeout: 5))
        item.tap()
        XCTAssertTrue(chip.waitForExistence(timeout: 5))
        XCTAssertEqual(app.state, .runningForeground)
    }

    /// Regression: "+" in the resting capsule did nothing (the focus tap
    /// morphed the composer under the finger and cancelled the menu).
    func testAttachMenuOpens() {
        for route in [["-route", "chat:chat-deploy"], ["-route", "new"]] {
            let app = launch(route)
            let attach = app.buttons["composer-attach"]
            XCTAssertTrue(attach.waitForExistence(timeout: 10))
            attach.tap()
            XCTAssertTrue(app.buttons["Photo Library"].waitForExistence(timeout: 5), "attach menu opens (\(route))")
            snapshot(app, "attach-menu")
            app.terminate()
        }
        // Card state (focused, with a draft): the toolbar row must not cover "+".
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Draft")
        app.buttons["composer-attach"].tap()
        XCTAssertTrue(app.buttons["Photo Library"].waitForExistence(timeout: 5), "attach menu opens from the card")
    }

    /// Regression: the model chip's menu completed off the main thread.
    func testModelPickerInSession() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        let chip = app.buttons["composer-chip-model"]
        XCTAssertTrue(chip.waitForExistence(timeout: 5))
        chip.tap()
        let item = app.collectionViews.buttons.element(boundBy: 0)
        XCTAssertTrue(item.waitForExistence(timeout: 5), "models load")
        item.tap()
        XCTAssertTrue(input.waitForExistence(timeout: 5))
        XCTAssertEqual(app.state, .runningForeground)
    }

    /// Regression: a drag that starts at the tail re-latched follow at once,
    /// so the list sprang back to the bottom on release while streaming.
    func testScrollingUpWhileStreamingStaysPut() {
        let app = launch(["-route", "chat:chat-veil", "-big", "-longreply"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Walk me through the veil")
        app.buttons["composer-send"].tap()
        let transcript = app.scrollViews["transcript"]
        sleep(2)
        let from = transcript.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.3))
        from.press(forDuration: 0.05, thenDragTo: transcript.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.75)), withVelocity: .slow, thenHoldForDuration: 0.1)
        sleep(3)
        XCTAssertTrue(app.buttons["jump-to-latest"].isHittable, "stays scrolled up while the reply streams")
        snapshot(app, "scrolled-up-streaming")
    }

    /// Desktop tool rows: file badges, inline stats/diff detail, thoughts,
    /// and a live group streaming in.
    func testToolGroupsRenderLikeDesktop() {
        let app = launch(["-route", "chat:chat-veil", "-big"])
        let transcript = app.scrollViews["transcript"]
        XCTAssertTrue(transcript.waitForExistence(timeout: 10))
        for _ in 0..<1 {
            let expand = app.buttons["Expand"].firstMatch
            guard expand.exists, expand.isHittable else { break }
            expand.tap()
            sleep(1)
        }
        snapshot(app, "tool-groups")
        let details = app.buttons.matching(identifier: "Show details")
        if details.count > 1 {
            details.element(boundBy: 1).tap()
            sleep(1)
            snapshot(app, "tool-row-detail")
        }
        app.terminate()
        let live = launch(["-route", "chat:chat-home"], fast: false)
        let input = live.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Refactor the layout pass")
        live.buttons["composer-send"].tap()
        for k in 0..<6 {
            sleep(1)
            snapshot(live, "live-tools-\(k)")
        }
    }

    /// Regression: a half swipe-back (cancelled) detached the session, so the
    /// streaming reply froze until you left and came back; the cancelled pop
    /// could also leave "New session" showing over the session.
    func testCancelledSwipeBackKeepsStreaming() {
        let app = launch(["-route", "chat:chat-deploy"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Summarize the launch post in three bullets.")
        app.buttons["composer-send"].tap()
        sleep(1)
        // Drag from the left edge a third of the way and let go: the pop cancels.
        let window = app.windows.firstMatch
        let start = window.coordinate(withNormalizedOffset: CGVector(dx: 0.01, dy: 0.5))
        start.press(forDuration: 0.05, thenDragTo: window.coordinate(withNormalizedOffset: CGVector(dx: 0.3, dy: 0.5)), withVelocity: .slow, thenHoldForDuration: 0.3)
        sleep(1)
        XCTAssertTrue(input.exists, "still on the session")
        XCTAssertFalse(app.buttons["new-session"].isHittable, "no New session accessory over the session")
        // The reply keeps streaming to completion.
        XCTAssertTrue(app.staticTexts.matching(identifier: "row-markdown").element(boundBy: 0).waitForExistence(timeout: 20))
        XCTAssertTrue(app.buttons["Send message"].waitForExistence(timeout: 40), "turn settles (updates still arriving)")
    }

    /// Tapping the transcript dismisses the keyboard and the composer rests
    /// as the capsule again (a draft stays in it).
    func testTapOutsideMinimizesComposer() {
        let app = launch(["-route", "chat:chat-deploy"])
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Half-written thought")
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 3))
        XCTAssertTrue(app.buttons["composer-chip-model"].isHittable, "card toolbar while composing")
        app.scrollViews["transcript"].coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.35)).tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3), "keyboard dismissed")
        sleep(1)
        XCTAssertFalse(app.buttons["composer-chip-model"].isHittable, "composer rests as the capsule")
        // (The sim keyboard's autocorrect may append to the typed text.)
        XCTAssertTrue((input.value as? String ?? "").hasPrefix("Half-written"), "draft kept")
        snapshot(app, "composer-minimized")
    }

    /// Regression: leaving a session with a queue and coming back showed no
    /// queue (the panel's glass never re-materialized on first render).
    func testQueueSurvivesNavigatingAway() {
        let app = launch(["-route", "chat:chat-home", "-longreply"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Write a long reply please.")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.buttons["Stop response"].waitForExistence(timeout: 5))
        input.tap()
        input.typeText("Then add a TL;DR.")
        app.buttons["Queue message"].tap()
        XCTAssertTrue(app.buttons["More queue actions"].waitForExistence(timeout: 5))
        app.navigationBars.buttons.element(boundBy: 0).tap()
        let accessory = app.buttons["new-session"]
        XCTAssertTrue(accessory.waitForExistence(timeout: 5))
        XCTAssertGreaterThan(accessory.frame.minY, app.windows.firstMatch.frame.height * 0.6, "accessory sits above the tab bar")
        app.cells["session-chat-home"].firstMatch.tap()
        let more = app.buttons["More queue actions"]
        XCTAssertTrue(more.waitForExistence(timeout: 5))
        XCTAssertTrue(more.isHittable, "queue shows again after coming back")
        snapshot(app, "queue-after-return")
    }

    /// Wallpaper controls in Settings: effect + remove appear once one is set.
    func testWallpaperSettings() throws {
        let path = "/tmp/wall-test.png"
        try XCTSkipUnless(FileManager.default.fileExists(atPath: path), "needs a host image at \(path)")
        let app = launch(["-wallpaper", path, "-wallpaper-effect", "halftone", "-route", "more"])
        XCTAssertTrue(app.navigationBars["Settings"].waitForExistence(timeout: 10))
        let effect = app.staticTexts["Effect"]
        for _ in 0..<4 where !effect.isHittable { app.swipeUp() }
        XCTAssertTrue(effect.waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Halftone'")).firstMatch.exists)
        snapshot(app, "wallpaper-settings")
        // Scroll the row clear of the bottom accessory / tab bar before tapping.
        let remove = app.staticTexts["Remove Wallpaper"]
        let limit = app.windows.firstMatch.frame.height * 0.7
        for _ in 0..<4 where remove.frame.maxY > limit { app.swipeUp() }
        remove.tap()
        XCTAssertTrue(app.staticTexts["Choose Wallpaper…"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.staticTexts["Effect"].exists)
    }

    /// The new-session headline stays above the composer, keyboard up.
    func testNewSessionHeadlineStaysAboveComposer() {
        let app = launch(["-route", "new"])
        let hero = app.staticTexts["What are we building?"]
        XCTAssertTrue(hero.waitForExistence(timeout: 5))
        app.textViews["composer-input"].tap()
        XCTAssertTrue(app.keyboards.firstMatch.waitForExistence(timeout: 5))
        sleep(1)
        let composer = app.textViews["composer-input"]
        XCTAssertLessThan(hero.frame.maxY, composer.frame.minY - 8, "headline clear of the composer")
        snapshot(app, "new-session-headline")
    }

    /// Sending images shows upload progress as a ring + percentage on the
    /// thumbnails, never a banner above the composer.
    func testImageUploadShowsRingOnThumbnail() {
        let size = CGSize(width: 240, height: 180)
        UIPasteboard.general.image = UIGraphicsImageRenderer(size: size).image { ctx in
            UIColor.systemTeal.setFill(); ctx.fill(CGRect(origin: .zero, size: size))
            UIColor.systemPink.setFill(); ctx.fill(CGRect(x: 60, y: 40, width: 120, height: 100))
        }
        let app = launch(["-route", "chat:chat-deploy"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        app.buttons["composer-attach"].tap()
        let paste = app.buttons["Paste Image"]
        XCTAssertTrue(paste.waitForExistence(timeout: 5))
        paste.tap()
        let allow = XCUIApplication(bundleIdentifier: "com.apple.springboard").buttons["Allow Paste"]
        if allow.waitForExistence(timeout: 2) { allow.tap() }
        input.tap()
        input.typeText("What's in this image?")
        app.buttons["composer-send"].tap()
        let ring = app.descendants(matching: .any)["upload-progress"].firstMatch
        XCTAssertTrue(ring.waitForExistence(timeout: 5), "ring on the pending thumbnail")
        snapshot(app, "upload-ring")
        XCTAssertFalse(app.staticTexts.matching(NSPredicate(format: "label BEGINSWITH 'Uploading'")).firstMatch.exists, "no upload banner")
        XCTAssertTrue(app.staticTexts.matching(identifier: "row-markdown").element(boundBy: 0).waitForExistence(timeout: 20), "the turn runs once uploaded")
    }

    /// Desktop runway: an immediate send glides the prompt to the top and the
    /// reply streams into the reserved space below it.
    func testSendGlidesPromptToTopWithRunway() {
        let app = launch(["-route", "chat:chat-veil", "-big"], fast: false)
        let input = app.textViews["composer-input"]
        XCTAssertTrue(input.waitForExistence(timeout: 10))
        input.tap()
        input.typeText("Runway check: summarize the veil work")
        app.buttons["composer-send"].tap()
        let prompt = app.descendants(matching: .any).matching(NSPredicate(format: "label BEGINSWITH 'You: Runway check'")).firstMatch
        XCTAssertTrue(prompt.waitForExistence(timeout: 5))
        sleep(1)
        let top = app.scrollViews["transcript"].frame.minY
        XCTAssertLessThan(prompt.frame.minY - top, 200, "prompt glided to the top of the transcript")
        snapshot(app, "runway")
        XCTAssertFalse(app.buttons["jump-to-latest"].isHittable, "held runway is 'the bottom'")
    }

    /// Regression: pull to refresh on the front page spun forever (the list
    /// stayed pushed down by the spinner).
    func testPullToRefreshEnds() {
        let app = launch()
        let list = app.collectionViews.firstMatch
        let first = app.cells["section-pinned"]
        XCTAssertTrue(first.waitForExistence(timeout: 10))
        sleep(1)
        let restingY = first.frame.minY
        let start = list.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.25))
        start.press(forDuration: 0.05, thenDragTo: list.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.85)))
        sleep(4)
        XCTAssertEqual(first.frame.minY, restingY, accuracy: 4, "refresh ended and the list settled back")
    }
}
