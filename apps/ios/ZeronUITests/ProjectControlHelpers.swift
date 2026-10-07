import XCTest

extension XCUIApplication {
    func openProjectScopePicker(searchOnly: Bool = false) {
        let sidebar = searchFields["sidebar-search"]
        if sidebar.exists {
            let hideKeyboard = buttons["Hide keyboard"].firstMatch
            if hideKeyboard.exists && hideKeyboard.isHittable { hideKeyboard.tap() }
            buttons["sessions-options"].tap()
            buttons["Filter Sessions…"].tap()
        } else {
            if !buttons["search-options"].exists {
                let search = tabBars.buttons["Search"]
                XCTAssertTrue(search.waitForExistence(timeout: 5))
                search.tap()
            }
            let options = buttons["search-options"]
            XCTAssertTrue(options.waitForExistence(timeout: 5))
            options.tap()
            buttons[searchOnly ? "Search in project…" : "Filter Sessions…"].tap()
        }
        XCTAssertTrue(tables["project-picker"].waitForExistence(timeout: 5))
    }

    func returnToSessions() {
        guard !searchFields["sidebar-search"].exists else { return }
        let sessions = tabBars.buttons["Sessions"]
        if !sessions.isHittable {
            let close = buttons["Close"].firstMatch
            XCTAssertTrue(close.waitForExistence(timeout: 5))
            close.tap()
        }
        XCTAssertTrue(sessions.waitForExistence(timeout: 5))
        sessions.tap()
        XCTAssertTrue(buttons["sessions-options"].waitForExistence(timeout: 5))
    }

    func assertProjectScope(_ title: String, searchOnly: Bool = false, file: StaticString = #filePath, line: UInt = #line) {
        let options = buttons[searchOnly ? "search-options" : "sessions-options"]
        XCTAssertTrue(options.waitForExistence(timeout: 5), file: file, line: line)
        XCTAssertTrue((options.value as? String ?? "").hasPrefix(title + ";"), file: file, line: line)
    }
}
