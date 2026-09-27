import UIKit

/// A run pick as the pickers hold it — wire ids, possibly unresolved: a nil
/// model is the harness default, a nil effort the model's default.
struct ModelSelection: Equatable {
    var harness: String
    var model: String?
    var effort: String?
    /// Non-default option picks (option id → choice id), the `ChatConfig` shape.
    var options: [String: String] = [:]

    init(harness: String, model: String? = nil, effort: String? = nil, options: [String: String] = [:]) {
        self.harness = harness
        self.model = model
        self.effort = effort
        self.options = options
    }
}

/// Which list the model picker shows (desktop `ModelRail`).
enum ModelPickerTab: Hashable { case favorites; case provider(String) }

/// One model row (desktop `ModelRowData`).
struct ModelPickerRow: Equatable {
    let harness: String
    let providerLabel: String
    let model: ModelInfo
    /// The pick itself, absent from the catalog: shown, never choosable.
    var selectedOnly = false
    /// Its harness has another returned row with the same display label.
    var ambiguous = false
    var id: String { harness + "\u{1F}" + model.id }
}

/// Provider tabs and models as a run device reported them, with the desktop
/// pickers' resolution rules (`pickers.rs`): concrete effort is clamped to the
/// model's ladder and options are filtered to what it offers.
struct ModelCatalog: Equatable {
    struct Provider: Equatable {
        let id: String
        let label: String
        var reasoningLevels: [String] = []
        var models: [ModelInfo]
    }

    enum Setting: Hashable { case effort; case option(String) }

    struct SettingGroup: Equatable {
        struct Choice: Equatable {
            let id: String
            let label: String
            let isDefault: Bool
        }

        let setting: Setting
        let label: String
        let choices: [Choice]
        let selected: String?
        var selectedChoice: Choice? { choices.first { $0.id == selected } }
        /// Desktop `is_toggle`: a two-choice option is a card switch (Fast
        /// Mode), on at its non-default choice. Lead and Sidekick always pick
        /// from a list.
        var isToggle: Bool {
            guard case .option(let id) = setting else { return false }
            return choices.count == 2 && !ModelCatalog.pairOptions.contains(id)
        }
        var isOn: Bool { selectedChoice.map { !$0.isDefault } ?? false }
        var toggledChoice: Choice? { choices.first { $0.id != selected } }
    }

    static let serviceTier = "serviceTier"
    /// Devin Fusion's options that pick the pair it runs.
    private static let pairOptions: Set<String> = ["lead", "sidekick"]
    var providers: [Provider]

    static func fallback() -> ModelCatalog {
        ModelCatalog(providers: fallbackHarnesses().filter(\.offered).map { harness in
            Provider(id: harness.id, label: harness.label, reasoningLevels: harness.reasoningLevels, models: fallbackModels(harness: harness.id))
        })
    }

    func provider(_ harness: String) -> Provider? {
        providers.first { $0.id == harness }
    }

    func modelInfo(for selection: ModelSelection) -> ModelInfo? {
        guard let foundProvider = provider(selection.harness) else { return nil }
        guard let model = selection.model else { return foundProvider.models.first }
        return foundProvider.models.first { $0.id == model }
    }

    func ladder(for selection: ModelSelection) -> [String] {
        guard let model = modelInfo(for: selection), let foundProvider = provider(selection.harness) else { return [] }
        return model.reasoningLevels.isEmpty ? foundProvider.reasoningLevels : model.reasoningLevels
    }

    static func defaultEffort(_ ladder: [String]) -> String? {
        if ladder.contains("high") { return "high" }
        if ladder.contains("medium") { return "medium" }
        return ladder.first
    }

    func effort(for selection: ModelSelection) -> String? {
        guard modelInfo(for: selection) != nil else { return selection.effort }
        let levels = ladder(for: selection)
        if let effort = selection.effort, levels.contains(effort) { return effort }
        return Self.defaultEffort(levels)
    }

    func options(for selection: ModelSelection) -> [String: String] {
        guard let model = modelInfo(for: selection) else { return [:] }
        var filtered: [String: String] = [:]
        for option in model.options {
            if let choice = selection.options[option.id], option.choices.contains(where: { $0.id == choice }) {
                filtered[option.id] = choice
            }
        }
        return filtered
    }

    /// An unlisted model passes through. Its options survive only for a live
    /// chat's own config; a draft's were never validated against that model.
    func resolved(_ selection: ModelSelection, keepingUnlistedOptions: Bool) -> ModelSelection {
        guard let model = modelInfo(for: selection) else {
            return ModelSelection(
                harness: selection.harness,
                model: selection.model,
                effort: selection.effort,
                options: keepingUnlistedOptions ? selection.options : [:]
            )
        }
        return ModelSelection(harness: selection.harness, model: model.id, effort: effort(for: selection), options: options(for: selection))
    }

    /// Desktop `configured_in_place`: a model whose options pick what it runs
    /// (Devin Fusion) is set up from its own card, never the tray.
    static func isConfiguredInPlace(_ model: ModelInfo) -> Bool {
        model.options.contains { $0.id == "lead" }
    }

    /// The tray under the model list (desktop `setting_groups`): empty for a
    /// model configured in place.
    func settingGroups(for selection: ModelSelection) -> [SettingGroup] {
        guard let model = modelInfo(for: selection), !Self.isConfiguredInPlace(model) else { return [] }
        return groups(for: selection, model: model)
    }

    /// A model configured in place, read like Devin's own Fusion panel
    /// (desktop `card_order`): Lead, Effort, Sidekick, then switches.
    func cardGroups(for selection: ModelSelection) -> [SettingGroup] {
        guard let model = modelInfo(for: selection), Self.isConfiguredInPlace(model) else { return [] }
        var result = groups(for: selection, model: model)
        if let lead = result.firstIndex(where: { $0.setting == .option("lead") }), lead != 0 {
            result.insert(result.remove(at: lead), at: 0)
        }
        return result
    }

    /// Desktop `build_setting_groups`.
    private func groups(for selection: ModelSelection, model: ModelInfo) -> [SettingGroup] {
        let levels = ladder(for: selection)
        var result: [SettingGroup] = []
        if !levels.isEmpty {
            let defaultChoice = Self.defaultEffort(levels)
            result.append(SettingGroup(
                setting: .effort,
                label: "Effort",
                choices: levels.map { .init(id: $0, label: reasoningLabel(level: $0), isDefault: $0 == defaultChoice) },
                selected: effort(for: selection)
            ))
        }
        for option in model.options where !option.choices.isEmpty {
            let selected = selection.options[option.id].flatMap { choice in
                option.choices.contains(where: { $0.id == choice }) ? choice : nil
            } ?? option.defaultChoice
            result.append(SettingGroup(
                setting: .option(option.id),
                label: option.label,
                choices: option.choices.map { .init(id: $0.id, label: $0.label, isDefault: $0.id == option.defaultChoice) },
                selected: selected
            ))
        }
        return result
    }

    func picking(_ choice: String, for setting: Setting, in selection: ModelSelection) -> ModelSelection {
        var next = selection
        switch setting {
        case .effort:
            next.effort = choice
        case .option(let id):
            guard let option = modelInfo(for: selection)?.options.first(where: { $0.id == id }) else { return next }
            next.options[id] = choice == option.defaultChoice ? nil : choice
        }
        return next
    }

    func title(for selection: ModelSelection) -> String {
        if let model = modelInfo(for: selection) { return model.label }
        if let model = selection.model { return modelLabel(harness: selection.harness, model: model) }
        return fallbackModels(harness: selection.harness).first?.label ?? harnessLabel(harness: selection.harness)
    }

    /// The chip's second tone — effort, then service tier, each brighter off
    /// its default.
    func chipDetail(for selection: ModelSelection) -> [ComposerChip.Detail] {
        guard let model = modelInfo(for: selection) else {
            var detail: [ComposerChip.Detail] = []
            if let effort = selection.effort { detail.append(.init(text: reasoningLabel(level: effort))) }
            if let tier = selection.options[Self.serviceTier] { detail.append(.init(text: Self.serviceTierLabel(tier))) }
            return detail
        }
        var detail: [ComposerChip.Detail] = []
        let levels = ladder(for: selection)
        if let resolvedEffort = effort(for: selection) {
            detail.append(.init(text: reasoningLabel(level: resolvedEffort), emphasized: resolvedEffort != Self.defaultEffort(levels)))
        }
        if let tier = model.options.first(where: { $0.id == Self.serviceTier }) {
            let effective = selection.options[tier.id].flatMap { picked in
                tier.choices.first { $0.id == picked }
            } ?? tier.choices.first { $0.id == tier.defaultChoice }
            if let effective {
                detail.append(.init(text: effective.label, emphasized: effective.id != tier.defaultChoice))
            }
        }
        return detail
    }

    /// Force-includes the pick's harness (a dev session's mock harness, or one
    /// the device stopped offering); only the chat's own once it exists.
    func tabs(for selection: ModelSelection, locked: Bool) -> [Provider] {
        var result = providers
        if !result.contains(where: { $0.id == selection.harness }) {
            result.insert(Provider(id: selection.harness, label: harnessLabel(harness: selection.harness), models: fallbackModels(harness: selection.harness)), at: 0)
        }
        return locked ? result.filter { $0.id == selection.harness } : result
    }

    /// Desktop `scoped_model_rows`: the query never leaves the viewed tab.
    /// Label prefix beats label substring, then description hits; stars and
    /// catalog order break ties. Without a query, provider tabs put stars first.
    static func rows(
        _ tab: ModelPickerTab,
        in tabs: [Provider],
        query: String,
        selection: ModelSelection,
        favorites: Set<ModelFavorites.Key>
    ) -> [ModelPickerRow] {
        struct Candidate {
            var row: ModelPickerRow
            let index: Int
            let label: String
            let description: String
        }

        var candidates: [Candidate] = []
        var seen: Set<String> = []
        var index = 0
        for provider in tabs {
            for model in provider.models {
                defer { index += 1 }
                let key = ModelFavorites.Key(harness: provider.id, model: model.id)
                let inScope: Bool = switch tab {
                case .favorites: favorites.contains(key)
                case .provider(let id): provider.id == id
                }
                guard inScope else { continue }
                let row = ModelPickerRow(harness: provider.id, providerLabel: provider.label, model: model)
                guard seen.insert(row.id).inserted else { continue }
                candidates.append(Candidate(row: row, index: index, label: model.label.lowercased(), description: (model.description ?? "").lowercased()))
            }
        }

        let q = query.trimmingCharacters(in: .whitespacesAndNewlines).lowercased()
        if q.isEmpty {
            if case .provider = tab {
                candidates.sort {
                    let a = favorites.contains(.init(harness: $0.row.harness, model: $0.row.model.id)) ? 0 : 1
                    let b = favorites.contains(.init(harness: $1.row.harness, model: $1.row.model.id)) ? 0 : 1
                    return a == b ? $0.index < $1.index : a < b
                }
            }
        } else {
            func rank(_ candidate: Candidate) -> Int? {
                if candidate.label.hasPrefix(q) { return 0 }
                if candidate.label.contains(q) { return 1 }
                let hay = candidate.description + " " + candidate.label
                if hay.hasPrefix(q) { return 2 }
                if hay.contains(q) { return 3 }
                return nil
            }
            candidates = candidates.compactMap { candidate in rank(candidate).map { ($0, candidate) } }
                .sorted {
                    if $0.0 != $1.0 { return $0.0 < $1.0 }
                    let a = favorites.contains(.init(harness: $0.1.row.harness, model: $0.1.row.model.id)) ? 0 : 1
                    let b = favorites.contains(.init(harness: $1.1.row.harness, model: $1.1.row.model.id)) ? 0 : 1
                    return a == b ? $0.1.index < $1.1.index : a < b
                }
                .map { $0.1 }
        }

        var result = candidates.map(\.row)
        let selectedIsListed = selection.model.map { selected in
            tabs.first(where: { $0.id == selection.harness })?.models.contains(where: { $0.id == selected }) ?? false
        } ?? false
        if case .provider(let id) = tab, id == selection.harness, let selected = selection.model, !selectedIsListed {
            let label = modelLabel(harness: selection.harness, model: selected)
            if q.isEmpty || selected.lowercased().contains(q) || label.lowercased().contains(q) {
                let model = ModelInfo(id: selected, label: label, description: nil, reasoningLevels: [], options: [], defaultReasoning: nil)
                let providerLabel = tabs.first(where: { $0.id == selection.harness })?.label ?? harnessLabel(harness: selection.harness)
                result.insert(ModelPickerRow(harness: selection.harness, providerLabel: providerLabel, model: model, selectedOnly: true), at: 0)
            }
        }

        var counts: [String: Int] = [:]
        for row in result { counts[row.harness + "\u{1F}" + row.model.label, default: 0] += 1 }
        for i in result.indices {
            result[i].ambiguous = counts[result[i].harness + "\u{1F}" + result[i].model.label, default: 0] > 1
        }
        return result
    }

    private static func serviceTierLabel(_ id: String) -> String {
        switch id {
        case "default": return "Standard"
        case "fast", "priority": return "Fast"
        case "flex": return "Flex"
        case "ultrafast": return "Ultra Fast"
        default: return id.capitalized
        }
    }
}

/// Starred models (the picker's favorites tab) — the desktop's
/// composer-defaults favorites, kept on this device; memory-only in the demo
/// like the new-session draft.
enum ModelFavorites {
    struct Key: Hashable {
        let harness: String
        let model: String
    }

    private static var stored: [Key] = {
        guard AppModel.persistsNewSession else { return [] }
        let rows = UserDefaults.standard.array(forKey: "modelFavorites") as? [[String: String]] ?? []
        return rows.compactMap { row in
            guard let harness = row["harness"], let model = row["model"] else { return nil }
            return Key(harness: harness, model: model)
        }
    }()

    static var keys: Set<Key> { Set(stored) }
    static var isEmpty: Bool { stored.isEmpty }

    static func toggle(_ key: Key) {
        if let index = stored.firstIndex(of: key) {
            stored.remove(at: index)
        } else {
            stored.append(key)
        }
        if AppModel.persistsNewSession {
            UserDefaults.standard.set(stored.map { ["harness": $0.harness, "model": $0.model] }, forKey: "modelFavorites")
        }
    }
}
