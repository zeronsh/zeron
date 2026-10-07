import XCTest
@testable import Zeron

@MainActor
final class ProjectViewTests: XCTestCase {
    private func demo() -> AppModel {
        AppModel(credentials: .demo(options: DemoOptions(fixture: .projectFilter, transcriptScale: .normal, streamSpeed: .fast, longReply: false)))
    }

    func testSessionControlsReuseOptionsAndPreserveTheOpenMenuDuringRefresh() throws {
        let app = demo(); defer { app.signOutLocally() }
        let list = SessionsViewController(app: app)
        list.loadViewIfNeeded()
        XCTAssertNil(list.navigationItem.searchController, "phone session search belongs to the native Search tab")
        let options = try XCTUnwrap(list.navigationItem.rightBarButtonItem)
        let menu = try XCTUnwrap(options.menu)
        list.reload(animated: false)
        XCTAssertTrue(options.menu === menu, "live row refresh keeps an open Options menu")
        var preferences = app.viewPreferences
        preferences.showHarness = false
        app.setViewPreferences(preferences)
        XCTAssertFalse(options.menu === menu, "a changed preference updates menu state")
        app.setSessionScope(.projectless)
        XCTAssertEqual(list.navigationItem.subtitle, "No project")
        let search = SearchViewController(app: app)
        search.loadViewIfNeeded()
        XCTAssertNotNil(search.navigationItem.searchController)
        XCTAssertNil(search.navigationItem.subtitle, "Search keeps its independent global default")
    }

    func testViewPreferencesRestoreWithoutChangingScopeOrDraftAndIsolateIdentity() throws {
        let suite = "ProjectViewTests.\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let owner = WorkspacePreferences(userId: "user", orgId: "org", defaults: defaults)
        owner.saveScope(.project(projectId: "selected-project"), "sessionsScope")
        var draft = NewSessionDraft(); draft.projectId = "unsent-project"; draft.targetChosen = true
        owner.draft = draft; owner.text = "Unsent idea"
        var view = SessionViewPreferences()
        view.organization = .byDevice; view.sort = .created
        view.showBranch = false; view.showHarness = false
        view.collapsedSections = ["project:selected-project", "pinned"]
        owner.viewPreferences = view
        let restored = WorkspacePreferences(userId: "user", orgId: "org", defaults: defaults)
        XCTAssertEqual(restored.viewPreferences, view)
        XCTAssertEqual(restored.scope("sessionsScope"), .project(projectId: "selected-project"))
        XCTAssertEqual(restored.draft.projectId, "unsent-project")
        XCTAssertEqual(restored.text, "Unsent idea")
        for identity in [("user", "other-org"), ("other-user", "org")] {
            XCTAssertEqual(WorkspacePreferences(userId: identity.0, orgId: identity.1, defaults: defaults).viewPreferences, SessionViewPreferences())
        }
        restored.saveScope(.projectless, "sessionsScope")
        XCTAssertEqual(restored.viewPreferences, view)
    }

    func testOlderAndUnknownViewFieldsKeepKnownChoices() throws {
        let data = Data(#"{"organization":"future-layout","sort":"created","showBranch":false}"#.utf8)
        let view = try JSONDecoder().decode(SessionViewPreferences.self, from: data)
        XCTAssertEqual(view.organization, .inOneList)
        XCTAssertEqual(view.sort, .created)
        XCTAssertFalse(view.showBranch)
        XCTAssertTrue(view.showProjectIcon)
        XCTAssertTrue(view.collapsedSections.isEmpty)
    }

    func testGroupingAndSortingPreservePinsSectionsAndUnsentTarget() throws {
        let app = demo(); defer { app.signOutLocally() }
        let pins = app.sessions(inFolder: "pinned").map(\.id)
        let sections = app.frontPage.folders.map(\.id)
        let recent = Set(app.frontPage.sessions.map(\.id))
        app.lastDraft.projectId = "space-edge"; app.newSessionText = "Keep this target"
        var view = app.viewPreferences; view.organization = .byProject; view.sort = .created
        app.setViewPreferences(view)
        XCTAssertEqual(app.sessions(inFolder: "pinned").map(\.id), pins)
        XCTAssertEqual(app.frontPage.folders.map(\.id), sections)
        XCTAssertEqual(Set(app.frontPage.groups.flatMap(\.sessions).map(\.id)), recent)
        XCTAssertEqual(Set(app.frontPage.groups.map(\.id)).count, app.frontPage.groups.count)
        XCTAssertTrue(app.frontPage.groups.contains { $0.projectId == "space-duplicate" })
        XCTAssertEqual(app.lastDraft.projectId, "space-edge")
        XCTAssertEqual(app.newSessionText, "Keep this target")
        let times = try app.frontPage.sessions.map { try XCTUnwrap(app.row($0.id)).createdAtMs }
        XCTAssertTrue(zip(times, times.dropFirst()).allSatisfy { $0.0 >= $0.1 })
        app.setSessionScope(.project(projectId: "space-zeron"))
        XCTAssertEqual(app.viewPreferences, view)
        for row in app.frontPage.groups.flatMap(\.sessions) {
            XCTAssertEqual(app.row(row.id)?.project?.id, "space-zeron")
        }
    }

    func testMetadataOnlyChangeNotifiesAndReusedCellRestoresItsAppearance() throws {
        let app = demo(); defer { app.signOutLocally() }
        let row = try XCTUnwrap(app.session("chat-veil"))
        let cell = SessionCell(frame: CGRect(x: 0, y: 0, width: 360, height: 62))
        cell.configure(row)
        let fullHeight = SessionCell.height(for: row, preferences: app.viewPreferences)
        XCTAssertTrue(cell.accessibilityLabel?.contains(row.projectName) == true)
        var notifications = 0
        let token = app.observe { notifications += 1 }
        var view = app.viewPreferences
        view.showProjectLabel = false; view.showProjectIcon = false; view.showBranch = false
        view.showPullRequest = false; view.showHarness = false
        app.setViewPreferences(view)
        XCTAssertGreaterThan(notifications, 0)
        cell.configure(row, preferences: view)
        XCTAssertFalse(cell.accessibilityLabel?.contains(row.projectName) == true)
        if let branch = row.branch { XCTAssertFalse(cell.accessibilityLabel?.contains(branch) == true) }
        XCTAssertLessThan(SessionCell.height(for: row, preferences: view), fullHeight)
        cell.configure(row)
        XCTAssertTrue(cell.accessibilityLabel?.contains(row.projectName) == true)
        if let branch = row.branch { XCTAssertTrue(cell.accessibilityLabel?.contains(branch) == true) }
        withExtendedLifetime(token) {}
    }

    func testProjectManagementRefreshesStableScopeAndPreservesOtherProjects() throws {
        let app = demo(); defer { app.signOutLocally() }
        var view = app.viewPreferences; view.organization = .byProject; app.setViewPreferences(view)
        app.setSessionScope(.project(projectId: "space-zeron"))
        let impact = try XCTUnwrap(app.projectDeletionSummary("space-zeron"))
        XCTAssertGreaterThan(impact.archivedCount, 0)
        XCTAssertGreaterThan(impact.sessionCount, impact.archivedCount)
        try app.renameProject("space-zeron", name: "Renamed demo project")
        XCTAssertEqual(app.sessionScope, .project(projectId: "space-zeron"))
        XCTAssertEqual(app.scopeTitle(app.sessionScope), "Renamed demo project")
        XCTAssertEqual(app.session("chat-veil")?.projectName, "Renamed demo project")
        try app.deleteProject("space-zeron")
        XCTAssertEqual(app.sessionScope, .all)
        XCTAssertEqual(app.viewPreferences, view)
        XCTAssertNil(app.session("chat-veil"))
        XCTAssertNotNil(app.session("chat-deploy"))
        XCTAssertFalse(app.projectOptions.contains { $0.id == "space-zeron" })
    }

    func testInvalidManagementActionLeavesProjectAndScopeUntouched() {
        let app = demo(); defer { app.signOutLocally() }
        app.setSessionScope(.project(projectId: "space-zeron"))
        let before = app.projectOptions
        XCTAssertThrowsError(try app.renameProject("missing-project", name: "New name"))
        XCTAssertThrowsError(try app.deleteProject("missing-project"))
        XCTAssertEqual(app.projectOptions, before)
        XCTAssertEqual(app.sessionScope, .project(projectId: "space-zeron"))
        XCTAssertNotNil(app.session("chat-veil"))
    }

    func testProjectChoiceRemainsActivatableBesideItsManagementButton() async throws {
        let app = demo(); defer { app.signOutLocally() }
        var selected: SessionScope?
        let picker = ProjectPickerViewController(app: app, selection: .all, mode: .filter) { selected = $0 }
        let scene = try XCTUnwrap(UIApplication.shared.connectedScenes.first as? UIWindowScene)
        let previousKeyWindow = scene.windows.first { $0.isKeyWindow }
        let window = UIWindow(windowScene: scene)
        window.frame = CGRect(x: 0, y: 0, width: 440, height: 720)
        window.rootViewController = UINavigationController(rootViewController: picker)
        window.makeKeyAndVisible()
        defer { window.isHidden = true; previousKeyWindow?.makeKey() }
        picker.view.layoutIfNeeded()
        let table = try XCTUnwrap(picker.view.subviews.first { $0 is UITableView } as? UITableView)
        let ready = XCTNSPredicateExpectation(predicate: NSPredicate { _, _ in
            table.numberOfSections > 1 && table.numberOfRows(inSection: 1) > 0
        }, object: nil)
        await fulfillment(of: [ready], timeout: 3)
        table.scrollToRow(at: IndexPath(row: 0, section: 1), at: .middle, animated: false)
        table.layoutIfNeeded()
        let cell = try XCTUnwrap(table.cellForRow(at: IndexPath(row: 0, section: 1)))
        let choice = try XCTUnwrap(cell.accessibilityElements?.first as? UIAccessibilityElement)
        XCTAssertTrue(choice.accessibilityActivate())
        XCTAssertNotNil(selected)
        XCTAssertNotNil((cell.accessibilityElements?.last as? UIButton)?.menu)
    }
}
