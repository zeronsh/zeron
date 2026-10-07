import XCTest

/// Native flows over offline Rust DemoHost + the command ledger. No browser
/// proxy or network fixture is involved.
final class ProjectFilterTests: XCTestCase {
    override func setUp() { continueAfterFailure = false }

    private var pad: Bool { UIDevice.current.userInterfaceIdiom == .pad }

    private func launch(_ args: [String] = []) -> XCUIApplication {
        let app = XCUIApplication()
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US", "-demo", "-fast", "-harness", "mock", "-project-filter-fixture"] + args
        app.launch()
        return app
    }

    private func snapshot(_ app: XCUIApplication, _ name: String) {
        let attachment = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        attachment.name = "project-filter-\(pad ? "ipad" : "iphone")-\(name)"
        attachment.lifetime = .keepAlways
        add(attachment)
    }

    private func choose(_ app: XCUIApplication, _ id: String, query: String? = nil, searchOnly: Bool = false) {
        app.openProjectScopePicker(searchOnly: searchOnly)
        XCTAssertTrue(app.tables["project-picker"].waitForExistence(timeout: 5))
        if let query {
            let search = app.searchFields["project-search"]
            search.tap(); search.typeText(query)
        }
        let row = app.cells["project-\(id)"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        row.tap()
        XCTAssertTrue(app.tables["project-picker"].waitForNonExistence(timeout: 5))
        if !searchOnly { app.returnToSessions() }
    }

    private func closeDraft(_ app: XCUIApplication) {
        // The sheet's explicit close affordance avoids a keyboard-dependent
        // swipe. iPad replaces the canvas by opening a sidebar session.
        if pad {
            app.cells["session-chat-deploy"].tap()
        } else {
            app.buttons["new-session-close"].tap()
        }
    }

    func testPhoneUsesNativeSearchWithoutDuplicateListControls() throws {
        try XCTSkipIf(pad, "iPhone tab shell")
        let app = launch()
        XCTAssertTrue(app.buttons["sessions-options"].waitForExistence(timeout: 10))
        XCTAssertFalse(app.searchFields["sessions-search"].exists)
        XCTAssertFalse(app.buttons["session-scope"].exists)
        XCTAssertFalse(app.buttons["session-view-options"].exists)
        snapshot(app, "sessions")
        app.tabBars.buttons["Search"].tap()
        let search = app.searchFields["history-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        XCTAssertGreaterThan(search.frame.minY, app.navigationBars.firstMatch.frame.maxY + 100, "native field remains below the results")
        search.typeText("zeron")
        XCTAssertTrue(app.cells["session-chat-veil"].waitForExistence(timeout: 5))
        snapshot(app, "native-search")
        app.returnToSessions()
        XCTAssertFalse(app.searchFields["sessions-search"].exists)
    }

    func testPickerSearchAndSameNameIdentity() {
        if pad { XCUIDevice.shared.orientation = .landscapeLeft }
        let app = launch()
        XCTAssertTrue(app.navigationBars["Sessions"].waitForExistence(timeout: 10))
        XCTAssertTrue(app.navigationBars["Sessions"].staticTexts["Sessions"].isHittable)
        snapshot(app, "entry")
        app.openProjectScopePicker()
        XCTAssertTrue(app.tables["project-picker"].waitForExistence(timeout: 5))
        snapshot(app, "picker")
        let search = app.searchFields["project-search"]
        search.tap(); search.typeText("STUDIO archive/zeron")
        let duplicate = app.cells["project-id:space-duplicate"]
        XCTAssertTrue(duplicate.waitForExistence(timeout: 5))
        XCTAssertTrue(duplicate.label.contains("Offline"))
        XCTAssertFalse(app.cells["project-id:space-zeron"].exists)
        snapshot(app, "project-search-keyboard")
        duplicate.tap()
        app.returnToSessions()
        XCTAssertTrue(app.cells["session-chat-duplicate"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.cells["session-chat-veil"].exists)
        app.assertProjectScope("zeron")
        snapshot(app, "same-name-filtered")
        choose(app, "id:space-zeron", query: "MacBook zeron")
        XCTAssertTrue(app.cells["section-pinned"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.cells["session-chat-duplicate"].exists)
        XCTAssertFalse(app.cells["section-mobile"].exists)
    }

    func testScopedSearchFindsHitAfterGlobalSixtyAndArchive() {
        let app = launch()
        choose(app, "id:space-zeron", query: "MacBook zeron", searchOnly: !pad)
        if !pad {
            app.buttons["search-options"].tap(); app.buttons["Include archived"].tap()
        }
        let search = app.searchFields[pad ? "sidebar-search" : "history-search"]
        if !search.isHittable { app.collectionViews.firstMatch.swipeDown() }
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.tap(); search.typeText("needle")
        XCTAssertTrue(app.cells["session-chat-scoped-needle"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.cells["session-chat-global-needle-00"].exists)
        snapshot(app, "scoped-search-after-sixty")
        search.tap(); search.buttons["Clear text"].tap()
        search.tap(); search.typeText("OKLCH")
        XCTAssertTrue(app.staticTexts["session-empty-state"].waitForExistence(timeout: 5))
        if pad {
            let hideKeyboard = app.buttons["Hide keyboard"].firstMatch
            if hideKeyboard.exists && hideKeyboard.isHittable { hideKeyboard.tap() }
        }
        let options = app.buttons[pad ? "sessions-options" : "search-options"]
        XCTAssertTrue(options.isHittable)
        options.tap()
        let includeArchived = app.buttons["Include archived"]
        XCTAssertTrue(includeArchived.waitForExistence(timeout: 5))
        includeArchived.tap()
        XCTAssertTrue(app.cells["session-chat-oklch"].waitForExistence(timeout: 5))
        snapshot(app, "scoped-search-archive")
    }

    func testHistoryKeepsGlobalDefaultAndSupportsScope() throws {
        try XCTSkipIf(pad, "independent Search tab belongs to the compact/phone shell")
        let app = launch(["-route", "search"])
        let search = app.searchFields["history-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 10))
        app.assertProjectScope("All projects", searchOnly: true)
        search.tap(); search.typeText("OKLCH")
        XCTAssertTrue(app.cells["session-chat-oklch"].waitForExistence(timeout: 5), "global history includes archives by default")
        choose(app, "id:space-edge", query: "/srv/deploys/edge", searchOnly: true)
        XCTAssertEqual(search.value as? String, "OKLCH", "scope changes preserve the query")
        XCTAssertFalse(app.cells["session-chat-oklch"].exists)
        snapshot(app, "history-project-scope")
        choose(app, "all", searchOnly: true)
        XCTAssertTrue(app.cells["session-chat-oklch"].waitForExistence(timeout: 5))
        app.buttons[pad ? "sessions-options" : "search-options"].tap(); app.buttons["Include archived"].tap()
        XCTAssertTrue(app.cells["session-chat-oklch"].waitForNonExistence(timeout: 5))
    }

    func testNoProjectEmptyProjectAndNewDefault() {
        let app = launch()
        choose(app, "projectless")
        XCTAssertTrue(app.cells["session-chat-home"].waitForExistence(timeout: 5))
        XCTAssertFalse(app.cells["section-pinned"].exists)
        snapshot(app, "no-project")
        app.buttons["new-session"].tap()
        XCTAssertTrue(app.buttons["composer-chip-project"].waitForExistence(timeout: 5))
        XCTAssertEqual(app.buttons["composer-chip-project"].label, "No project")
        XCTAssertTrue(app.buttons["composer-chip-host"].exists)
        snapshot(app, "no-project-default")
        if pad {
            // Scope changes leave a pristine canvas in place and update it.
            choose(app, "id:space-empty", query: "accessible layouts")
        } else {
            closeDraft(app)
            choose(app, "id:space-empty", query: "accessible layouts")
        }
        XCTAssertTrue(app.staticTexts["session-empty-state"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.staticTexts["session-empty-state"].label.contains("Empty project"))
        snapshot(app, "empty-project")
        if pad, app.keyboards.firstMatch.exists {
            // The embedded canvas keeps the composer focused, and in
            // landscape that keyboard covers the sidebar's empty action.
            app.buttons["Hide keyboard"].tap()
        }
        app.buttons["session-empty-action"].tap()
        XCTAssertTrue(app.buttons["composer-chip-project"].label.hasPrefix("Empty project"))
        snapshot(app, "empty-project-new-default")
    }

    func testDraftKeepsTargetAndCreationRevealsDifferentProject() throws {
        try XCTSkipIf(pad, "phone sheet round-trip; iPad detail retention has a dedicated test")
        let app = launch()
        choose(app, "id:space-zeron", query: "MacBook zeron")
        app.buttons["new-session"].tap()
        let chip = app.buttons["composer-chip-project"]
        XCTAssertTrue(chip.waitForExistence(timeout: 5))
        XCTAssertEqual(chip.label, "zeron")
        chip.tap()
        let search = app.searchFields["project-search"]
        search.tap(); search.typeText("/srv/deploys/edge")
        app.cells["project-id:space-edge"].tap()
        let input = app.textViews["composer-input"]
        input.tap(); input.typeText("Keep project B draft")
        closeDraft(app)
        choose(app, "projectless")
        app.buttons["new-session"].tap()
        XCTAssertEqual(chip.label, "edge")
        XCTAssertEqual(input.value as? String, "Keep project B draft")
        snapshot(app, "restored-draft-target")
        app.buttons["composer-send"].tap()
        XCTAssertTrue(app.scrollViews["transcript"].waitForExistence(timeout: 10))
        app.navigationBars.buttons.element(boundBy: 0).tap()
        app.assertProjectScope("edge")
        XCTAssertTrue(app.cells.matching(NSPredicate(format: "identifier BEGINSWITH 'session-' AND label CONTAINS 'Keep project B draft'")).firstMatch.waitForExistence(timeout: 10))
        snapshot(app, "created-session-visible")
    }

    func testScopeRestoresAcrossRelaunch() {
        let app = launch(["-persist-demo-scope", "-reset-demo-scope"])
        choose(app, "projectless")
        app.terminate()
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US", "-demo", "-fast", "-harness", "mock", "-project-filter-fixture", "-persist-demo-scope"]
        app.launch()
        XCTAssertTrue(app.buttons["sessions-options"].waitForExistence(timeout: 10))
        app.assertProjectScope("No project")
        XCTAssertTrue(app.cells["session-chat-home"].exists)
        snapshot(app, "restored-scope")
    }

    func testLargeTypeLongNameAndNoSearchMatches() {
        let app = launch(["-UIPreferredContentSizeCategoryName", "UICTContentSizeCategoryAccessibilityXXXL"])
        app.openProjectScopePicker()
        let search = app.searchFields["project-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.tap(); search.typeText("accessible layouts")
        let row = app.cells["project-id:space-empty"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        XCTAssertTrue(row.isHittable)
        snapshot(app, "large-type-long-name")
        // The clear button ends editing in the regular-width popover; delete
        // the query with the keyboard so focus stays in the field instead.
        search.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: 24))
        search.typeText("no such project anywhere")
        XCTAssertTrue(app.staticTexts["No matching projects"].waitForExistence(timeout: 5))
        XCTAssertTrue(app.buttons["new-project"].isHittable)
        snapshot(app, "no-project-matches")
        if pad {
            // iPad's software keyboard has its own hide control. Dragging a
            // short, single-row table would select the focused row instead.
            app.buttons["Hide keyboard"].tap()
            // iPadOS keeps a collapsed ~66pt keyboard strip in the
            // accessibility tree even after the keys slide away.
            if !app.keyboards.firstMatch.waitForNonExistence(timeout: 1) {
                XCTAssertLessThan(app.keyboards.firstMatch.frame.height, 100, "keyboard keys are gone")
            }
        } else {
            // The short table sits at its scroll edge, so a downward swipe
            // would drag the single-detent sheet away at accessibility sizes.
            // Dismiss the keyboard with the search bar's own close control.
            let searchClose = app.navigationBars["Projects"].buttons["Close"]
            XCTAssertTrue(searchClose.waitForExistence(timeout: 3))
            searchClose.tap()
            XCTAssertTrue(app.keyboards.firstMatch.waitForNonExistence(timeout: 3), "picker keyboard dismissed")
        }
        XCTAssertTrue(app.tables["project-picker"].exists, "picker stays open")
        snapshot(app, "large-type-keyboard-dismissed")
    }

    func testNewProjectReusesFolderBrowserAndSelectsTheResult() {
        let app = launch(["-no-projects"])
        app.openProjectScopePicker()
        XCTAssertTrue(app.staticTexts["No projects yet"].waitForExistence(timeout: 5))
        snapshot(app, "no-projects")
        app.buttons["new-project"].tap()
        let use = app.buttons["use-folder"]
        XCTAssertTrue(use.waitForExistence(timeout: 5))
        let enabled = XCTNSPredicateExpectation(predicate: NSPredicate(format: "enabled == true"), object: use)
        XCTAssertEqual(XCTWaiter.wait(for: [enabled], timeout: 5), .completed)
        snapshot(app, "folder-browser")
        use.tap()
        app.returnToSessions()
        XCTAssertTrue(app.buttons["sessions-options"].waitForExistence(timeout: 10))
        XCTAssertFalse((app.buttons["sessions-options"].value as? String ?? "").hasPrefix("All projects;"))
        XCTAssertTrue(app.staticTexts["session-empty-state"].exists)
        snapshot(app, "new-empty-project")
    }
}
