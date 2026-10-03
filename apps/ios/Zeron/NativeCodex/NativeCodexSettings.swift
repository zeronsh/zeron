import UIKit

struct NativeCodexSettings: Codable, Equatable {
    var model = ""
    var effort: String?
    var serviceTier: String?

    func normalized(catalog: [[String: Any]]) -> Self {
        var value = self
        guard !catalog.isEmpty else { return value }
        let spec = catalog.first { ($0["model"] as? String ?? $0["id"] as? String) == model }
            ?? catalog.first { $0["isDefault"] as? Bool == true } ?? catalog[0]
        value.model = spec["model"] as? String ?? spec["id"] as? String ?? model
        let efforts = (spec["supportedReasoningEfforts"] as? [[String: Any]] ?? []).compactMap { $0["reasoningEffort"] as? String }
        if let effort, !efforts.contains(effort) { value.effort = nil }
        let tiers = (spec["serviceTiers"] as? [[String: Any]] ?? []).compactMap { $0["id"] as? String }
        if let serviceTier, !tiers.contains(serviceTier) { value.serviceTier = nil }
        return value
    }

    func turnParameters(catalog: [[String: Any]]) -> [String: Any] {
        let value = normalized(catalog: catalog)
        let spec = catalog.first { ($0["model"] as? String ?? $0["id"] as? String) == value.model }
        var params: [String: Any] = ["serviceTier": value.serviceTier as Any? ?? NSNull()]
        if !value.model.isEmpty { params["model"] = value.model }
        if let effort = value.effort ?? spec?["defaultReasoningEffort"] as? String { params["effort"] = effort }
        return params
    }
}

@MainActor
extension NativeCodexSession {
    func settingsChips(draft: Bool) -> [ComposerChip] {
        let settings = settingsForUI(draft: draft)
        let spec = models.first { ($0["model"] as? String ?? $0["id"] as? String) == settings.model }
        var chips = [ComposerChip(id: draft ? "native-model" : "model", title: spec?["displayName"] as? String ?? (settings.model.isEmpty ? "Default model" : settings.model), symbol: nil, icon: BrandMarks.image(for: "codex", side: 13))]
        if !(spec?["supportedReasoningEfforts"] as? [[String: Any]] ?? []).isEmpty {
            chips.append(ComposerChip(id: "effort", title: settings.effort.map(Self.effortLabel) ?? "Default reasoning", symbol: "gauge.with.dots.needle.67percent"))
        }
        chips.append(ComposerChip(id: "service-tier", title: settings.serviceTier.flatMap { tier in (spec?["serviceTiers"] as? [[String: Any]])?.first { $0["id"] as? String == tier }?["name"] as? String } ?? "Automatic tier", symbol: "speedometer"))
        return chips
    }

    static func effortLabel(_ value: String) -> String { value == "xhigh" ? "Extra high" : value.capitalized }

    func settingsMenu(_ id: String, draft: Bool) -> UIMenu? {
        let settings = settingsForUI(draft: draft)
        let spec = models.first { ($0["model"] as? String ?? $0["id"] as? String) == settings.model }
        let disabled: UIMenuElement.Attributes = !draft && running ? .disabled : []
        func action(_ title: String, subtitle: String? = nil, selected: Bool, edit: @escaping (inout NativeCodexSettings) -> Void) -> UIAction {
            UIAction(title: title, subtitle: subtitle, attributes: disabled, state: selected ? .on : .off) { [weak self] _ in
                var updated = settings
                edit(&updated)
                self?.setSettings(updated, draft: draft)
            }
        }
        switch id {
        case "model", "native-model":
            return UIMenu(title: "Model", children: models.compactMap { spec in
                guard let model = spec["model"] as? String ?? spec["id"] as? String else { return nil }
                return action(spec["displayName"] as? String ?? model, selected: settings.model == model) { $0.model = model }
            })
        case "effort":
            let options = spec?["supportedReasoningEfforts"] as? [[String: Any]] ?? []
            return UIMenu(title: "Reasoning", children: [action("Model default", selected: settings.effort == nil) { $0.effort = nil }] + options.compactMap { option in
                guard let effort = option["reasoningEffort"] as? String else { return nil }
                return action(Self.effortLabel(effort), subtitle: option["description"] as? String, selected: settings.effort == effort) { $0.effort = effort }
            })
        case "service-tier":
            let options = spec?["serviceTiers"] as? [[String: Any]] ?? []
            return UIMenu(title: "Service tier", children: [action("Automatic", subtitle: "Let Codex choose the default tier", selected: settings.serviceTier == nil) { $0.serviceTier = nil }] + options.compactMap { option in
                guard let tier = option["id"] as? String else { return nil }
                return action(option["name"] as? String ?? tier, subtitle: option["description"] as? String, selected: settings.serviceTier == tier) { $0.serviceTier = tier }
            })
        default: return nil
        }
    }
}
