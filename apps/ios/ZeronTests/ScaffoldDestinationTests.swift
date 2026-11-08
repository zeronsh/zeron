import XCTest
@testable import Zeron

final class ScaffoldDestinationTests: XCTestCase {
    func testDestinationsMatchTheDesktopPicker() {
        XCTAssertEqual(CrewDatabase.all.map(\.label), ["Scaffold (Local DB)", "Scaffold (Staging DB)", "Scaffold (Production DB)"])
        XCTAssertEqual(CrewDatabase.all.map(\.shortLabel), ["Local DB", "Staging DB", "Production DB"])
        for database in CrewDatabase.all {
            XCTAssertEqual(CrewDatabase(key: database.key), database)
        }
        XCTAssertEqual(CrewDatabase.all.map(\.key), ["local", "staging_snapshot", "production_snapshot"])
        XCTAssertNil(CrewDatabase(key: nil))
        XCTAssertNil(CrewDatabase(key: "staging"))
    }

    func testOnlyManagedRoutesRunInASandbox() {
        XCTAssertTrue(crewManagedModel(harness: "omp", model: "openai-codex/gpt-5.5"))
        XCTAssertTrue(crewManagedModel(harness: "omp", model: "anthropic/claude-opus-5"))
        XCTAssertTrue(crewManagedModel(harness: "antigravity", model: "gemini-3-pro"))
        XCTAssertFalse(crewManagedModel(harness: "omp", model: "gpt-5.5"), "provider-qualified only")
        XCTAssertFalse(crewManagedModel(harness: "claude-code", model: "anthropic/claude-opus-5"))
        XCTAssertFalse(crewManagedModel(harness: "not-a-harness", model: "openai/gpt-5.5"))
    }

    func testDraftsSavedBeforeSandboxesStillDecode() throws {
        let saved = Data(#"{"projectId":"p","worktree":false,"harness":"codex","model":"gpt-5.5"}"#.utf8)
        let old = try JSONDecoder().decode(NewSessionDraft.self, from: saved)
        XCTAssertNil(old.scaffold)
        var draft = old
        draft.scaffold = CrewDatabase.stagingSnapshot.key
        let reread = try JSONDecoder().decode(NewSessionDraft.self, from: JSONEncoder().encode(draft))
        XCTAssertEqual(CrewDatabase(key: reread.scaffold), .stagingSnapshot)
        XCTAssertEqual(reread.projectId, "p", "the device-side choice is kept for when a device is picked again")
    }
}
