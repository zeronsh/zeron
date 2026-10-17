import AppIntents
import WidgetKit

/// the usage widget's settings. only the extension compiles this: nothing
/// in the app performs or donates it.
struct UsageConfigurationIntent: WidgetConfigurationIntent {
    static let title: LocalizedStringResource = "Usage Limits"
    static let description = IntentDescription("Choose which agents' plan limits to show.")

    @Parameter(title: "Agents", description: "Leave empty to show the agents closest to their limits.")
    var agents: [UsageSourceEntity]?

    init() {}
}

/// an agent, or one provider login of an agent that keeps several
/// (`UsageAccount.source`), across every host and login.
struct UsageSourceEntity: AppEntity, Identifiable {
    static let typeDisplayRepresentation: TypeDisplayRepresentation = "Agent"
    static let defaultQuery = UsageSourceQuery()

    let id: String
    let name: String

    var displayRepresentation: DisplayRepresentation {
        DisplayRepresentation(title: "\(name)")
    }
}

struct UsageSourceQuery: EntityQuery {
    func entities(for identifiers: [String]) async throws -> [UsageSourceEntity] {
        let known = Self.known()
        // a picked agent with nothing to show right now (host offline, login
        // removed) stays picked under its id rather than vanishing.
        return identifiers.filter(WidgetSnapshot.supportsUsageSource)
            .map { id in known.first { $0.id == id } ?? UsageSourceEntity(id: id, name: id) }
    }

    func suggestedEntities() async throws -> [UsageSourceEntity] {
        Self.known()
    }

    private static func known() -> [UsageSourceEntity] {
        guard let usage = WidgetStore.load()?.supportedUsage else { return [] }
        var seen = Set<String>()
        return usage.map { UsageSourceEntity(id: $0.source, name: $0.agent) }
            .filter { seen.insert($0.id).inserted }
            .sorted { $0.name.localizedStandardCompare($1.name) == .orderedAscending }
    }
}

