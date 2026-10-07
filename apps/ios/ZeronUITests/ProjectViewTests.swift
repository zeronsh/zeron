import XCTest

final class ProjectViewTests: XCTestCase {
    override func setUp() { continueAfterFailure = false }
    override func tearDown() { XCUIDevice.shared.orientation = .portrait }
    private var pad: Bool { UIDevice.current.userInterfaceIdiom == .pad }

    private func launch(_ extra: [String] = []) -> XCUIApplication {
        if pad { XCUIDevice.shared.orientation = .landscapeLeft }
        let app = XCUIApplication()
        app.launchArguments = ["-AppleLanguages", "(en)", "-AppleLocale", "en_US", "-demo", "-fast", "-harness", "mock"] + extra
        app.launch()
        XCTAssertTrue(app.buttons["sessions-options"].waitForExistence(timeout: 10))
        return app
    }

    private func snapshot(_ name: String) {
        let image = XCTAttachment(screenshot: XCUIScreen.main.screenshot())
        image.name = "project-view-\(pad ? "ipad" : "iphone")-\(name)"
        image.lifetime = .keepAlways
        add(image)
    }

    private func setting(_ app: XCUIApplication, _ menu: String, _ title: String) {
        app.buttons["sessions-options"].tap()
        let submenu = app.buttons[menu]
        XCTAssertTrue(submenu.waitForExistence(timeout: 5)); submenu.tap()
        let choice = app.buttons[title]
        XCTAssertTrue(choice.waitForExistence(timeout: 5))
        if menu == "Show", title == "Harness" { snapshot("show-options") }
        // Tap the visible item without XCTest scrolling its menu container.
        XCTAssertTrue(choice.isHittable)
        choice.coordinate(withNormalizedOffset: CGVector(dx: 0.5, dy: 0.5)).tap()
    }

    private func reveal(_ app: XCUIApplication, _ element: XCUIElement) {
        for _ in 0..<8 {
            if element.isHittable { return }
            app.collectionViews.firstMatch.swipeUp()
        }
        XCTAssertTrue(element.isHittable)
    }

    private func picker(_ app: XCUIApplication, query: String) {
        app.openProjectScopePicker()
        XCTAssertTrue(app.tables["project-picker"].waitForExistence(timeout: 5))
        let search = app.searchFields["project-search"]
        XCTAssertTrue(search.waitForExistence(timeout: 5))
        search.tap(); search.typeText(query)
    }

    private func selectProject(_ app: XCUIApplication, _ id: String, query: String) {
        picker(app, query: query)
        let row = app.cells["project-id:\(id)"]
        XCTAssertTrue(row.waitForExistence(timeout: 5)); row.tap()
        XCTAssertTrue(app.tables["project-picker"].waitForNonExistence(timeout: 5))
        app.returnToSessions()
    }

    private func closePicker(_ app: XCUIApplication) {
        app.buttons["project-picker-close"].tap()
        XCTAssertTrue(app.tables["project-picker"].waitForNonExistence(timeout: 5))
        app.returnToSessions()
    }

    private func manage(_ app: XCUIApplication, _ id: String, _ action: String) {
        let button = app.buttons["project-options-\(id)"]
        XCTAssertTrue(button.waitForExistence(timeout: 5)); button.tap()
        let item = app.buttons[action]
        XCTAssertTrue(item.waitForExistence(timeout: 5)); item.tap()
    }

    private func replaceName(_ app: XCUIApplication, _ name: String) {
        let field = app.textFields["project-name"]
        XCTAssertTrue(field.waitForExistence(timeout: 5)); field.tap()
        let previous = field.value as? String ?? ""
        field.typeText(String(repeating: XCUIKeyboardKey.delete.rawValue, count: previous.count) + name)
    }

    func testListOrganizationSortingAndScope() {
        let app = launch()
        if pad {
            let chat = app.cells["session-chat-deploy"]; reveal(app, chat); chat.tap()
            XCTAssertTrue(app.navigationBars.staticTexts["Wrangler deploy hygiene"].waitForExistence(timeout: 5))
        }
        app.buttons["sessions-options"].tap(); snapshot("view-options")
        app.buttons["Organize"].tap(); app.buttons["By project"].tap()
        for id in ["pinned", "p0", "mobile"] {
            let header = app.cells["section-\(id)"]
            if header.isHittable, header.value as? String == "Expanded" { header.tap() }
        }
        let projectGroups = app.cells.matching(NSPredicate(format: "identifier BEGINSWITH %@", "section-project:"))
        XCTAssertGreaterThan(projectGroups.count, 0)
        app.assertProjectScope("All projects")
        snapshot("by-project")
        setting(app, "Sort", "Created")
        XCTAssertTrue((app.buttons["sessions-options"].value as? String ?? "").hasSuffix("By project, Created"))
        setting(app, "Organize", "By device")
        XCTAssertGreaterThan(app.cells.matching(NSPredicate(format: "identifier BEGINSWITH %@", "section-device:")).count, 0)
        snapshot("by-device")
        setting(app, "Organize", "In one list")
        selectProject(app, "space-edge", query: "hetzner edge")
        let chat = app.cells["session-chat-deploy"]
        XCTAssertTrue(chat.waitForExistence(timeout: 5))
        XCTAssertFalse(app.cells["session-chat-veil"].exists)
        XCTAssertTrue((app.buttons["sessions-options"].value as? String ?? "").hasSuffix("In one list, Created"))
        if pad { XCTAssertTrue(app.navigationBars.staticTexts["Wrangler deploy hygiene"].exists) }
    }

    func testMetadataVisibilityUpdatesExistingRowsAndRestoresThem() {
        let app = launch()
        let row = app.cells["session-chat-veil"]
        XCTAssertTrue(row.waitForExistence(timeout: 5))
        XCTAssertTrue(row.label.contains("zeron"))
        let height = row.frame.height
        for field in ["Branch", "Pull request", "Harness", "Project icon", "Project label"] { setting(app, "Show", field) }
        XCTAssertFalse(row.label.contains("zeron"))
        XCTAssertFalse(row.label.contains("veil-fade"))
        XCTAssertFalse(row.label.contains("claude-code"))
        XCTAssertLessThan(row.frame.height, height)
        snapshot("minimal-metadata")
        for field in ["Branch", "Pull request", "Harness", "Project icon", "Project label"] { setting(app, "Show", field) }
        XCTAssertTrue(row.label.contains("zeron"))
        XCTAssertTrue(row.label.contains("veil-fade"))
        XCTAssertEqual(row.frame.height, height, accuracy: 1)
    }

    func testProjectRenameDeleteConfirmationAndRefresh() {
        let app = launch()
        if pad {
            let chat = app.cells["session-chat-deploy"]; reveal(app, chat); chat.tap()
        }
        selectProject(app, "space-zeron", query: "MacBook zeron")
        picker(app, query: "MacBook zeron")
        app.buttons["project-options-space-zeron"].tap(); snapshot("project-management")
        app.buttons["Rename project…"].tap()
        replaceName(app, "Cancelled rename")
        app.alerts["Rename project"].buttons["Cancel"].tap()
        XCTAssertTrue(app.cells["project-id:space-zeron"].label.contains("zeron"))
        manage(app, "space-zeron", "Rename project…")
        replaceName(app, "Renamed demo project")
        app.alerts["Rename project"].buttons["Rename"].tap()
        XCTAssertTrue(app.alerts["Rename project"].waitForNonExistence(timeout: 5))
        XCTAssertTrue(app.cells["project-id:space-zeron"].label.contains("Renamed demo project"))
        closePicker(app)
        app.assertProjectScope("Renamed demo project")
        XCTAssertTrue(app.cells["session-chat-veil"].label.contains("Renamed demo project"))
        picker(app, query: "Renamed demo project")
        manage(app, "space-zeron", "Delete project…")
        let confirmation = app.alerts["Delete project?"]
        XCTAssertTrue(confirmation.waitForExistence(timeout: 5))
        XCTAssertTrue(confirmation.staticTexts.matching(NSPredicate(format: "label CONTAINS %@", "archived sessions")).firstMatch.exists)
        snapshot("delete-confirmation")
        confirmation.buttons["Cancel"].tap()
        XCTAssertTrue(app.cells["project-id:space-zeron"].exists)
        manage(app, "space-zeron", "Delete project…")
        app.alerts["Delete project?"].buttons["Delete project"].tap()
        XCTAssertTrue(app.cells["project-id:space-zeron"].waitForNonExistence(timeout: 5))
        closePicker(app)
        app.assertProjectScope("All projects")
        XCTAssertFalse(app.cells["session-chat-veil"].exists)
        if pad { XCTAssertTrue(app.navigationBars.staticTexts["Wrangler deploy hygiene"].exists) }
    }

    func testViewPreferencesRestoreIndependentlyOfScope() {
        let app = launch(["-persist-demo-scope", "-reset-demo-scope"])
        setting(app, "Organize", "By device")
        setting(app, "Sort", "Created")
        setting(app, "Show", "Project label")
        app.openProjectScopePicker(); app.cells["project-projectless"].tap(); app.returnToSessions()
        app.assertProjectScope("No project")
        app.terminate()
        app.launchArguments = app.launchArguments.filter { $0 != "-reset-demo-scope" }
        app.launch()
        XCTAssertTrue(app.buttons["sessions-options"].waitForExistence(timeout: 10))
        app.assertProjectScope("No project")
        XCTAssertTrue((app.buttons["sessions-options"].value as? String ?? "").hasSuffix("By device, Created"))
        XCTAssertGreaterThan(app.cells.matching(NSPredicate(format: "identifier BEGINSWITH %@", "section-device:")).count, 0)
        snapshot("restored-view")
    }
}
