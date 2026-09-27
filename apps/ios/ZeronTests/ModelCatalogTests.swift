import XCTest
@testable import Zeron

/// Pure catalog resolution, settings, tabs, search, and row ordering.
final class ModelCatalogTests: XCTestCase {
    private func choice(_ id: String, _ label: String) -> ModelOptionChoice {
        ModelOptionChoice(id: id, label: label)
    }

    private func option(_ id: String, _ label: String, _ choices: [ModelOptionChoice], default defaultChoice: String) -> ModelOption {
        ModelOption(id: id, label: label, choices: choices, defaultChoice: defaultChoice)
    }

    private func model(
        _ id: String,
        _ label: String,
        description: String? = nil,
        reasoning: [String] = [],
        options: [ModelOption] = []
    ) -> ModelInfo {
        ModelInfo(id: id, label: label, description: description, reasoningLevels: reasoning, options: options, defaultReasoning: nil)
    }

    private func catalog(_ providers: [ModelCatalog.Provider]) -> ModelCatalog {
        ModelCatalog(providers: providers)
    }

    func testEffortClampsToLadderDefault() {
        let models = catalog([.init(id: "codex", label: "Codex", models: [model("one", "One", reasoning: ["low", "medium", "high"])])])
        XCTAssertEqual(models.effort(for: .init(harness: "codex", model: "one", effort: "low")), "low")
        XCTAssertEqual(models.effort(for: .init(harness: "codex", model: "one", effort: "max")), "high")
        XCTAssertEqual(models.effort(for: .init(harness: "codex", model: "one")), "high")

        let short = catalog([.init(id: "codex", label: "Codex", models: [model("one", "One", reasoning: ["minimal", "low"])])])
        XCTAssertEqual(short.effort(for: .init(harness: "codex", model: "one", effort: "high")), "minimal")
    }

    func testHarnessLadderIsFallback() {
        let models = catalog([.init(id: "codex", label: "Codex", reasoningLevels: ["low", "high"], models: [model("one", "One")])])
        let selection = ModelSelection(harness: "codex", model: "one")
        XCTAssertEqual(models.ladder(for: selection), ["low", "high"])
        XCTAssertEqual(models.effort(for: selection), "high")
    }

    func testOptionsFilterInvalidIdsAndChoices() {
        let tier = option("serviceTier", "Service Tier", [choice("default", "Standard"), choice("fast", "Fast")], default: "default")
        let models = catalog([.init(id: "codex", label: "Codex", models: [model("one", "One", options: [tier])])])
        XCTAssertEqual(models.options(for: .init(harness: "codex", model: "one", options: ["serviceTier": "fast", "other": "x"])), ["serviceTier": "fast"])
        XCTAssertEqual(models.options(for: .init(harness: "codex", model: "one", options: ["serviceTier": "invalid"])), [:])
    }

    func testResolvedUnlistedModelCanKeepOptions() {
        let models = catalog([.init(id: "codex", label: "Codex", models: [model("known", "Known")])])
        let selection = ModelSelection(harness: "codex", model: "future", effort: "max", options: ["futureOption": "on"])
        XCTAssertEqual(models.resolved(selection, keepingUnlistedOptions: true), selection)
        XCTAssertEqual(models.resolved(selection, keepingUnlistedOptions: false).options, [:])
    }

    func testResolvedMakesSelectionConcrete() {
        let tier = option("serviceTier", "Service Tier", [choice("default", "Standard"), choice("fast", "Fast")], default: "default")
        let models = catalog([.init(id: "codex", label: "Codex", models: [model("first", "First", reasoning: ["low", "high"], options: [tier])])])
        let selection = ModelSelection(harness: "codex", effort: "max", options: ["serviceTier": "invalid"])
        XCTAssertEqual(
            models.resolved(selection, keepingUnlistedOptions: false),
            ModelSelection(harness: "codex", model: "first", effort: "high")
        )
    }

    func testTitleFallsBackToKnownLabels() {
        let models = catalog([])
        let unknownModel = ModelSelection(harness: "claude-code", model: "future-model")
        let modelTitle = models.title(for: unknownModel)
        XCTAssertEqual(modelTitle, modelLabel(harness: "claude-code", model: "future-model"))
        XCTAssertFalse(modelTitle.isEmpty)
        XCTAssertNotEqual(modelTitle, harnessLabel(harness: "claude-code"))

        guard let fallbackTitle = fallbackModels(harness: "claude-code").first?.label else {
            XCTFail("Claude fallback catalog is empty")
            return
        }
        XCTAssertEqual(models.title(for: .init(harness: "claude-code")), fallbackTitle)
    }

    func testSettingGroupsAndPicking() {
        let tier = option("serviceTier", "Service Tier", [choice("default", "Standard"), choice("fast", "Fast")], default: "default")
        let context = option("contextWindow", "Context Window", [choice("200k", "200K"), choice("1m", "1M")], default: "200k")
        let models = catalog([.init(id: "codex", label: "Codex", models: [model("one", "One", reasoning: ["low", "high"], options: [tier, context])])])
        let selection = ModelSelection(harness: "codex", model: "one", options: ["serviceTier": "fast"])
        let groups = models.settingGroups(for: selection)
        XCTAssertEqual(groups.map(\.label), ["Effort", "Service Tier", "Context Window"])
        XCTAssertEqual(groups[0].selectedChoice?.label, "High")
        XCTAssertTrue(groups[0].selectedChoice?.isDefault == true)
        XCTAssertEqual(groups[1].selectedChoice?.label, "Fast")
        XCTAssertFalse(groups[1].selectedChoice?.isDefault ?? true)
        XCTAssertEqual(groups[2].selectedChoice?.label, "200K")
        XCTAssertEqual(models.cardGroups(for: selection), [])

        let picked = models.picking("default", for: .option("serviceTier"), in: selection)
        XCTAssertNil(picked.options["serviceTier"])
    }

    /// Devin Fusion (desktop `configured_in_place`): no tray; its card reads
    /// Lead, Effort, Sidekick, then the Fast Mode switch.
    func testFusionSettingsLiveInItsCard() throws {
        let lead = option("lead", "Lead", [choice("fable", "Claude Fable 5.1"), choice("sol", "GPT-6 Sol")], default: "fable")
        let sidekick = option("sidekick", "Sidekick", [choice("swe-medium", "SWE-2 Medium"), choice("swe-high", "SWE-2 High")], default: "swe-medium")
        let speed = option("speed", "Fast Mode", [choice("standard", "Standard"), choice("fast", "Fast")], default: "standard")
        let fusion = model("fusion", "Fusion", reasoning: ["low", "medium", "high", "max"], options: [lead, sidekick, speed])
        let adaptive = model("adaptive", "Adaptive")
        let models = catalog([.init(id: "devin", label: "Devin", models: [adaptive, fusion])])
        XCTAssertTrue(ModelCatalog.isConfiguredInPlace(fusion))
        XCTAssertFalse(ModelCatalog.isConfiguredInPlace(adaptive))

        let selection = ModelSelection(harness: "devin", model: "fusion", options: ["sidekick": "swe-high"])
        XCTAssertEqual(models.settingGroups(for: selection), [])
        let card = models.cardGroups(for: selection)
        XCTAssertEqual(card.map(\.label), ["Lead", "Effort", "Sidekick", "Fast Mode"])
        XCTAssertEqual(card.map(\.isToggle), [false, false, false, true])
        XCTAssertEqual(card[0].selectedChoice?.label, "Claude Fable 5.1")
        XCTAssertEqual(card[1].selectedChoice?.label, "High")
        XCTAssertEqual(card[2].selectedChoice?.label, "SWE-2 High")
        XCTAssertFalse(card[3].isOn)

        let next = try XCTUnwrap(card[3].toggledChoice)
        let fast = models.picking(next.id, for: card[3].setting, in: selection)
        XCTAssertEqual(fast.options["speed"], "fast")
        let flipped = models.cardGroups(for: fast)[3]
        XCTAssertTrue(flipped.isOn)
        let back = try XCTUnwrap(flipped.toggledChoice)
        XCTAssertNil(models.picking(back.id, for: flipped.setting, in: fast).options["speed"])
        XCTAssertEqual(models.cardGroups(for: .init(harness: "devin", model: "adaptive")), [])
    }

    func testChipDetailLabelsAndEmphasis() {
        let tier = option("serviceTier", "Service Tier", [choice("default", "Standard"), choice("fast", "Fast")], default: "default")
        let models = catalog([.init(id: "codex", label: "Codex", models: [model("one", "One", reasoning: ["low", "high"], options: [tier])])])
        let defaults = models.chipDetail(for: .init(harness: "codex", model: "one"))
        XCTAssertEqual(defaults.map(\.text), ["High", "Standard"])
        XCTAssertEqual(defaults.map(\.emphasized), [false, false])
        let custom = models.chipDetail(for: .init(harness: "codex", model: "one", effort: "low", options: ["serviceTier": "fast"]))
        XCTAssertEqual(custom.map(\.text), ["Low", "Fast"])
        XCTAssertEqual(custom.map(\.emphasized), [true, true])

        let unknown = models.chipDetail(for: .init(harness: "codex", model: "future", effort: "xhigh", options: ["serviceTier": "ultrafast"]))
        XCTAssertEqual(unknown.map(\.text), ["X-High", "Ultra Fast"])
        XCTAssertEqual(unknown.map(\.emphasized), [false, false])
    }

    func testProviderRowsPutStarsFirstAndDedupe() {
        let duplicate = model("one", "One")
        let tabs = [ModelCatalog.Provider(id: "a", label: "A", models: [duplicate, model("two", "Two"), duplicate])]
        let favorites: Set<ModelFavorites.Key> = [.init(harness: "a", model: "two")]
        let rows = ModelCatalog.rows(.provider("a"), in: tabs, query: "", selection: .init(harness: "a"), favorites: favorites)
        XCTAssertEqual(rows.map { $0.model.id }, ["two", "one"])
    }

    func testFavoritesSpanProvidersInCatalogOrder() {
        let tabs = [
            ModelCatalog.Provider(id: "a", label: "A", models: [model("one", "One"), model("two", "Two")]),
            ModelCatalog.Provider(id: "b", label: "B", models: [model("three", "Three")]),
        ]
        let favorites: Set<ModelFavorites.Key> = [
            .init(harness: "a", model: "two"),
            .init(harness: "b", model: "three"),
        ]
        let rows = ModelCatalog.rows(.favorites, in: tabs, query: "", selection: .init(harness: "a"), favorites: favorites)
        XCTAssertEqual(rows.map { $0.model.id }, ["two", "three"])
    }

    func testSearchRankingAndScoping() {
        let tabs = [
            ModelCatalog.Provider(id: "a", label: "A", models: [
                model("prefix", "Fable One"),
                model("prefix-star", "Fable Star"),
                model("substring", "The Fable"),
                model("description-star", "Other", description: "Fable family"),
                model("description", "Last", description: "A fable model"),
            ]),
            ModelCatalog.Provider(id: "b", label: "B", models: [model("outside", "Fable Outside")]),
        ]
        let favorites: Set<ModelFavorites.Key> = [.init(harness: "a", model: "prefix-star")]
        let rows = ModelCatalog.rows(.provider("a"), in: tabs, query: "fable", selection: .init(harness: "a"), favorites: favorites)
        XCTAssertEqual(rows.map { $0.model.id }, ["prefix-star", "prefix", "substring", "description-star", "description"])
        XCTAssertFalse(rows.contains { $0.harness == "b" })
    }

    func testSelectedOnlyAndAmbiguousRows() {
        let tabs = [ModelCatalog.Provider(id: "claude-code", label: "Claude Code", models: [
            model("first", "Shared", description: "First"),
            model("second", "Shared", description: "Second"),
        ])]
        let ambiguous = ModelCatalog.rows(.provider("claude-code"), in: tabs, query: "", selection: .init(harness: "claude-code"), favorites: [])
        XCTAssertTrue(ambiguous.allSatisfy { $0.ambiguous })

        let selected = ModelCatalog.rows(.provider("claude-code"), in: tabs, query: "", selection: .init(harness: "claude-code", model: "claude-future"), favorites: [])
        XCTAssertEqual(selected.first?.model.id, "claude-future")
        XCTAssertTrue(selected.first?.selectedOnly == true)
    }

    func testTabsForceIncludeSelectionAndLock() {
        let models = catalog([.init(id: "claude-code", label: "Claude Code", models: [model("one", "One")])])
        let unlocked = models.tabs(for: .init(harness: "codex"), locked: false)
        XCTAssertEqual(unlocked.map(\.id), ["codex", "claude-code"])
        XCTAssertEqual(models.tabs(for: .init(harness: "codex"), locked: true).map(\.id), ["codex"])
    }
}
