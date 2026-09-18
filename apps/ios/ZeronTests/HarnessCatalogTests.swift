import XCTest
@testable import Zeron

final class HarnessCatalogTests: XCTestCase {
    func testCodexFallbackStartsWithAstraAndExposesItsTraits() {
        let models = HarnessCatalog.models(for: "codex")
        let astra = models.first

        XCTAssertEqual(astra?.id, "gpt-6-astra")
        XCTAssertEqual(astra?.label, "GPT-6-Astra")
        XCTAssertEqual(astra?.reasoningLevels,
                       ["low", "medium", "high", "xhigh", "max", "ultra"])
        XCTAssertEqual(astra?.options.first?.id, "serviceTier")
        XCTAssertEqual(astra?.options.first?.choices.map(\.id), ["default", "fast"])
    }

    func testDefaultReasoningMatchesDesktopPreference() {
        let astra = HarnessCatalog.defaultModel(for: "codex")
        XCTAssertEqual(astra.flatMap(HarnessCatalog.defaultReasoning), "high")

        let short = ModelInfo(id: "short", label: "Short", description: nil,
                              reasoningLevels: ["low", "medium"])
        XCTAssertEqual(HarnessCatalog.defaultReasoning(for: short), "medium")
    }

    func testChoiceFallsBackToAdvertisedDefault() {
        let option = HarnessCatalog.defaultModel(for: "codex")!.options[0]
        XCTAssertEqual(HarnessCatalog.selectedChoice(for: option, selectedId: "fast").label, "Fast")
        XCTAssertEqual(HarnessCatalog.selectedChoice(for: option, selectedId: "stale").id, "default")
    }

    func testOpenCodeHasNoInventedFallbackModels() {
        XCTAssertTrue(HarnessCatalog.models(for: "opencode").isEmpty)
        XCTAssertNil(HarnessCatalog.defaultModel(for: "opencode"))
        XCTAssertEqual(ModelCatalog.loaded([]).models.count, 0)
        XCTAssertEqual(ModelCatalog.failed("offline").models.count, 0)
    }

    func testCustomAgentAndModelIdsArePreserved() throws {
        let agent = try JSONDecoder().decode(AgentInfo.self,
            from: Data(#"{"id":"team/reviewer","label":"Reviewer","description":"Checks changes"}"#.utf8))
        XCTAssertEqual(agent.id, "team/reviewer")
        XCTAssertEqual(agent.description, "Checks changes")
        let live = ModelCatalog.loaded([
            ModelInfo(id: "custom/provider/model", label: "Custom model", description: nil,
                      reasoningLevels: [])
        ])
        XCTAssertEqual(live.models.map(\.id), ["custom/provider/model"])
    }

    func testOpenCodeSelectionKeepsExactIdOnlyInItsScope() {
        let early = [ModelInfo(id: "provider/early", label: "Early", description: nil,
                               reasoningLevels: [])]
        let selected = "custom/provider/model"
        XCTAssertEqual(HarnessCatalog.selectedOpenCodeModelId(
            models: early, storedId: selected, storedScope: "device/a/0", currentScope: "device/a/0"),
            selected)
        XCTAssertEqual(HarnessCatalog.selectedOpenCodeModelId(
            models: early, storedId: selected, storedScope: "device/a/0", currentScope: "device/b/0"),
            "provider/early")
        XCTAssertNil(HarnessCatalog.selectedOpenCodeModelId(
            models: [], storedId: selected, storedScope: "device/a/0", currentScope: "device/a/1"))
    }

    func testOldHostDoesNotAdvertiseAgentSelection() {
        let old = DeviceRow(id: "old", name: "Old", platform: "linux", capabilities: [])
        XCTAssertFalse(old.supports(EngineCapability.opencodeAgentSelectionV1))
    }
}
