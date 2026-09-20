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
        XCTAssertEqual(HarnessCatalog.defaultReasoning(for: astra), "high")

        let short = ModelInfo(id: "short", label: "Short", description: nil,
                              reasoningLevels: ["low", "medium"])
        XCTAssertEqual(HarnessCatalog.defaultReasoning(for: short), "medium")
    }

    func testChoiceFallsBackToAdvertisedDefault() {
        let option = HarnessCatalog.defaultModel(for: "codex").options[0]
        XCTAssertEqual(HarnessCatalog.selectedChoice(for: option, selectedId: "fast").label, "Fast")
        XCTAssertEqual(HarnessCatalog.selectedChoice(for: option, selectedId: "stale").id, "default")
    }

    func testNormalizeDropsDefaultWhenRealRowsExistButKeepsItAlone() {
        let defaultRow = ModelInfo(id: "default", label: "Default", description: nil, reasoningLevels: [])
        let realRow = ModelInfo(id: "titan", label: "Titan", description: nil, reasoningLevels: [])

        XCTAssertEqual(HarnessCatalog.normalize(harness: "other", models: [defaultRow, realRow]).map(\.id),
                       ["titan"])
        XCTAssertEqual(HarnessCatalog.normalize(harness: "other", models: [defaultRow]).map(\.id),
                       ["default"])
    }

    func testNormalizeFoldsOrphanLongContextVariant() {
        let row = ModelInfo(id: "titan[1m]", label: "Titan (1M context)",
                            description: "A model", reasoningLevels: ["high"])
        let normalized = HarnessCatalog.normalize(harness: "other", models: [row])

        XCTAssertEqual(normalized.first?.id, "titan")
        XCTAssertEqual(normalized.first?.label, "Titan")
        XCTAssertEqual(normalized.first?.options.first?.id, "contextWindow")
        XCTAssertEqual(normalized.first?.options.first?.defaultChoice, "1m")
    }

    func testNormalizeDropsLongContextVariantWhenBaseIsListed() {
        let base = ModelInfo(id: "claude-opus-5", label: "Opus", description: nil, reasoningLevels: [])
        let variant = ModelInfo(id: "claude-opus-5[1m]", label: "Opus (1M context)",
                                description: nil, reasoningLevels: [])

        XCTAssertEqual(HarnessCatalog.normalize(harness: "claude-code", models: [base, variant]).map(\.id),
                       ["claude-opus-5"])
    }

    func testNormalizeUsesCuratedAliasLabel() {
        let row = ModelInfo(id: "opus", label: "Opus", description: nil, reasoningLevels: [])
        XCTAssertEqual(HarnessCatalog.normalize(harness: "claude-code", models: [row]).first?.label,
                       "Opus 5")
    }

    func testResolvePreservesUnknownConfiguredModel() {
        let catalog = HarnessCatalog.models(for: "claude-code")
        XCTAssertEqual(HarnessCatalog.resolve(modelId: "claude-opus-5[1m]", in: catalog,
                                              harness: "claude-code").id, "claude-opus-5")
        let unknown = HarnessCatalog.resolve(modelId: "foo-9", in: catalog, harness: "claude-code")
        XCTAssertEqual(unknown.id, "foo-9")
        XCTAssertEqual(unknown.label, "foo-9")
    }

    func testClaudeCatalogStartsWithFable51AndContextWindow() {
        let first = HarnessCatalog.models(for: "claude-code").first
        XCTAssertEqual(first?.id, "claude-fable-5-1")
        XCTAssertEqual(first?.options.first?.id, "contextWindow")
    }
}
