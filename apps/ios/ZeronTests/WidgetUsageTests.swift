import XCTest
@testable import Zeron

final class WidgetUsageTests: XCTestCase {
    func testCachedCursorAndOpenCodeAccountsAreExcluded() {
        let snapshot = snapshot([
            account("claude-code"),
            account("cursor"),
            account("opencode"),
            account("opencode/openai"),
            account("opencode/opencode-go"),
            account("opencode/github-copilot"),
            account("codex"),
        ])

        XCTAssertEqual(snapshot.supportedUsage.map(\.source), ["claude-code", "codex"])
        XCTAssertEqual(snapshot.usageAccounts(showing: []).map(\.source), ["claude-code", "codex"])
    }

    func testPreviouslySelectedProvidersFallBackToAvailableAgents() {
        let snapshot = snapshot([
            account("claude-code"),
            account("cursor"),
            account("opencode/openai"),
            account("codex"),
        ])

        XCTAssertEqual(snapshot.usageAccounts(showing: ["cursor", "opencode/openai"]).map(\.source), [
            "claude-code", "codex",
        ])
    }

    func testMixedSelectionsKeepSupportedAgentOrderWithoutDuplicates() {
        let snapshot = snapshot([
            account("claude-code"),
            account("cursor"),
            account("opencode/opencode-go"),
            account("codex"),
            account("pi/anthropic"),
        ])

        XCTAssertEqual(snapshot.usageAccounts(showing: ["cursor", "pi/anthropic", "codex", "opencode/opencode-go", "codex"]).map(\.source), [
            "pi/anthropic", "codex",
        ])
    }

    func testExcludedProvidersNeverReturnAsAnEmptyStateFallback() {
        let snapshot = snapshot([account("cursor"), account("opencode/openai")])

        XCTAssertTrue(snapshot.usageAccounts(showing: []).isEmpty)
        XCTAssertTrue(snapshot.usageAccounts(showing: ["cursor"]).isEmpty)
    }

    private func account(_ source: String) -> WidgetSnapshot.UsageAccount {
        WidgetSnapshot.UsageAccount(
            id: "desktop/\(source)", source: source, agent: source,
            account: nil, plan: nil, host: "MacBook Pro", active: true,
            windows: [.init(label: "Week", used: 0.4, resetsAt: nil)],
            fetchedAt: nil, error: nil
        )
    }

    private func snapshot(_ usage: [WidgetSnapshot.UsageAccount]) -> WidgetSnapshot {
        var snapshot = WidgetSnapshot.empty
        snapshot.usage = usage
        return snapshot
    }
}
