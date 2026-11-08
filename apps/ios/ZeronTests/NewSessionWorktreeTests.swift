import XCTest
@testable import Zeron

/// A new session only asks its host for a worktree in a git project: the host
/// refuses one anywhere else, leaving an empty chat behind.
final class NewSessionWorktreeTests: XCTestCase {
    private func project(git: Bool) -> ProjectView {
        ProjectView(id: "project", name: "notes", path: "/code/notes", colorIndex: 0, deviceId: "mac", deviceName: nil, deviceOnline: true, isCloud: false, githubRepo: nil, gitDetected: git, createdAtMs: 0, groupKey: "notes", groupName: "notes", indicator: .idle, unseenCount: 0, sessions: [])
    }

    private func draft(worktree: Bool, branch: String? = nil) -> NewSessionDraft {
        var draft = NewSessionDraft()
        draft.worktree = worktree
        draft.branch = branch
        return draft
    }

    func testAGitProjectGetsTheWorktreeItWasAsked() throws {
        let spec = try XCTUnwrap(AppModel.worktreeSpec(draft: draft(worktree: true), project: project(git: true)))
        XCTAssertEqual(spec.repoPath, "/code/notes")
        XCTAssertEqual(spec.base, "HEAD")
        XCTAssertEqual(spec.spaceId, "project")
    }

    func testAWorktreeStartsOffThePickedBranch() {
        let spec = AppModel.worktreeSpec(draft: draft(worktree: true, branch: "release"), project: project(git: true))
        XCTAssertEqual(spec?.base, "release")
    }

    func testAPickKeptFromAGitProjectIsNotSentForAPlainFolder() {
        XCTAssertNil(AppModel.worktreeSpec(draft: draft(worktree: true), project: project(git: false)))
    }

    func testNoPickNoWorktree() {
        XCTAssertNil(AppModel.worktreeSpec(draft: draft(worktree: false), project: project(git: true)))
    }

    func testNoProjectNoWorktree() {
        XCTAssertNil(AppModel.worktreeSpec(draft: draft(worktree: true), project: nil))
    }
}
