import XCTest
@testable import Zeron

@MainActor
final class ProjectScopeTests: XCTestCase {
    private func demo() -> AppModel {
        AppModel(credentials: .demo(options: DemoOptions(fixture: .projectFilter, transcriptScale: .normal, streamSpeed: .fast, longReply: false)))
    }

    func testProjectSearchKeepsSharedRepositoryIdentityAndCheckoutHosts() throws {
        let app = demo()
        defer { app.signOutLocally() }
        let mac = try XCTUnwrap(app.projectOptions.first { $0.id == "space-zeron" })
        let vps = try XCTUnwrap(app.projects(matching: "hetzner zeron").first { $0.id == "space-zeron-vps" })
        XCTAssertEqual(mac.groupKey, vps.groupKey)
        XCTAssertEqual(mac.groupName, vps.groupName)
        XCTAssertEqual(mac.colorIndex, vps.colorIndex)
        XCTAssertNotEqual(mac.device, vps.device)
        XCTAssertNotEqual(mac.path, vps.path)
    }

    func testDraftFollowsScopeAndPreservesAnUnsentTarget() {
        let app = demo()
        defer { app.signOutLocally() }
        app.setSessionScope(.project(projectId: "space-zeron"))
        XCTAssertEqual(app.newDraft().projectId, "space-zeron")
        XCTAssertEqual(app.newDraft(scope: .project(projectId: "space-edge")).projectId, "space-edge", "Search tab can supply its independent current scope")
        app.lastDraft.projectId = "space-edge"
        app.newSessionText = "Resume this idea"
        XCTAssertEqual(app.newDraft().projectId, "space-edge")
        app.newSessionText = ""
        app.lastDraft.targetChosen = true
        XCTAssertEqual(app.newDraft().projectId, "space-edge")
        app.lastDraft.targetChosen = false
        app.setSessionScope(.projectless)
        XCTAssertNil(app.newDraft().projectId)
        XCTAssertNotNil(app.newDraft().hostId)
    }

    func testCreationMakesAnExplicitDifferentTargetVisible() {
        let app = demo()
        defer { app.signOutLocally() }
        app.setSessionScope(.project(projectId: "space-zeron"))
        var draft = app.newDraft()
        draft.projectId = "space-edge"
        draft.harness = "mock"
        let id = app.createSession(draft: draft, text: "Scope creation", images: [])
        XCTAssertNotNil(id)
        XCTAssertEqual(app.sessionScope, .project(projectId: "space-edge"))
        XCTAssertTrue(app.frontPage.sessions.contains { $0.id == id })
        app.setSessionScope(.all)
        XCTAssertNotNil(app.createSession(draft: draft, text: "All creation", images: []))
        XCTAssertEqual(app.sessionScope, .all)
    }

    func testEmptyProjectMutationNotifiesObserversAndUpdatesScopeLabel() async throws {
        let app = demo()
        defer { app.signOutLocally() }
        let added = expectation(description: "empty project added")
        var sawAdded = false
        let additionToken = app.observe {
            if !sawAdded, app.projectOptions.contains(where: { $0.path == "/Users/dev/empty-scope-test" }) { sawAdded = true; added.fulfill() }
        }
        let created = await app.createProject(deviceId: "dev-mac", path: "/Users/dev/empty-scope-test", gitDetected: false)
        await fulfillment(of: [added], timeout: 5)
        let id = try XCTUnwrap(created)
        XCTAssertTrue(app.projectOptions.contains { $0.id == id })
        app.setSessionScope(.project(projectId: id))
        let renamed = expectation(description: "empty project renamed")
        let deleted = expectation(description: "selected empty project deleted")
        var sawRename = false
        var sawDelete = false
        let token = app.observe {
            if !sawRename, app.scopeTitle(app.sessionScope) == "Renamed empty" { sawRename = true; renamed.fulfill() }
            if sawRename, !sawDelete, app.sessionScope == .all { sawDelete = true; deleted.fulfill() }
        }
        try app.client?.renameProject(spaceId: id, name: "Renamed empty")
        await fulfillment(of: [renamed], timeout: 5)
        try app.client?.deleteProject(spaceId: id)
        await fulfillment(of: [deleted], timeout: 5)
        withExtendedLifetime(token) {}
        withExtendedLifetime(additionToken) {}
    }

    func testPreferencesIsolateAccountOrgAndHistoryFromTheDraft() throws {
        let suite = "ProjectScopeTests.\(UUID())"
        let defaults = try XCTUnwrap(UserDefaults(suiteName: suite))
        defer { defaults.removePersistentDomain(forName: suite) }
        let a = WorkspacePreferences(userId: "user", orgId: "org-a", defaults: defaults)
        let b = WorkspacePreferences(userId: "user", orgId: "org-b", defaults: defaults)
        let other = WorkspacePreferences(userId: "other", orgId: "org-a", defaults: defaults)
        a.saveScope(.project(projectId: "stable-id"), "sessionsScope")
        a.saveScope(.projectless, "historyScope")
        var draft = NewSessionDraft(); draft.projectId = "draft-project"; draft.targetChosen = true
        a.draft = draft; a.text = "Unsent"
        let restored = WorkspacePreferences(userId: "user", orgId: "org-a", defaults: defaults)
        XCTAssertEqual(restored.scope("sessionsScope"), .project(projectId: "stable-id"))
        XCTAssertEqual(restored.scope("historyScope"), .projectless)
        XCTAssertEqual(restored.draft.projectId, "draft-project")
        XCTAssertEqual(restored.text, "Unsent")
        XCTAssertEqual(b.scope("sessionsScope"), .all)
        XCTAssertEqual(other.scope("historyScope"), .all)
        XCTAssertNil(b.draft.projectId)
        XCTAssertTrue(other.text.isEmpty)
    }
}
