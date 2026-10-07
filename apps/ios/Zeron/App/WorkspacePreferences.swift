import Foundation

/// Local navigation and new-session state belong to an account AND an org.
/// Session scope is independent of the unsent draft's execution target.
final class WorkspacePreferences {
    private let defaults: UserDefaults
    private let prefix: String

    init(userId: String, orgId: String, defaults: UserDefaults = .standard) {
        self.defaults = defaults
        // Encoding the tuple avoids ambiguous ids containing separators.
        let identity = try! JSONEncoder().encode([userId, orgId]).base64EncodedString()
        prefix = "workspace.\(identity)."
    }

    func scope(_ key: String) -> SessionScope {
        switch defaults.string(forKey: prefix + key) {
        case "projectless": return .projectless
        case let value? where value.hasPrefix("project:"): return .project(projectId: String(value.dropFirst(8)))
        default: return .all
        }
    }

    func saveScope(_ scope: SessionScope, _ key: String) {
        let value: String
        switch scope {
        case .all: value = "all"
        case .projectless: value = "projectless"
        case let .project(id): value = "project:" + id
        }
        defaults.set(value, forKey: prefix + key)
    }

    var viewPreferences: SessionViewPreferences {
        get {
            defaults.data(forKey: prefix + "sessionView")
                .flatMap { try? JSONDecoder().decode(SessionViewPreferences.self, from: $0) }
                ?? SessionViewPreferences()
        }
        set { defaults.set(try? JSONEncoder().encode(newValue), forKey: prefix + "sessionView") }
    }

    /// Call only after the old core directory proves this identity's ownership.
    func migrateLegacyViewPreferences() {
        guard defaults.object(forKey: prefix + "sessionView") == nil,
              let collapsed = defaults.stringArray(forKey: "collapsedSections") else { return }
        var value = SessionViewPreferences()
        value.collapsedSections = Set(collapsed)
        viewPreferences = value
        defaults.removeObject(forKey: "collapsedSections")
    }

    var draft: NewSessionDraft {
        get { defaults.data(forKey: prefix + "draft").flatMap { try? JSONDecoder().decode(NewSessionDraft.self, from: $0) } ?? NewSessionDraft() }
        set { defaults.set(try? JSONEncoder().encode(newValue), forKey: prefix + "draft") }
    }

    var text: String {
        get { defaults.string(forKey: prefix + "text") ?? "" }
        set { defaults.set(newValue, forKey: prefix + "text") }
    }

    /// Migrate only when the old core directory proves who owned the shared
    /// pre-scope draft. Never import another identity's draft.
    func migrateLegacyDraft() {
        guard defaults.object(forKey: prefix + "draft") == nil else { return }
        if let data = defaults.data(forKey: "newSessionDraft") {
            defaults.set(data, forKey: prefix + "draft")
            if draft.projectId != nil || draft.hostId != nil {
                var migrated = draft
                migrated.targetChosen = true
                draft = migrated
            }
            defaults.removeObject(forKey: "newSessionDraft")
        }
        if var drafts = defaults.dictionary(forKey: "drafts") as? [String: String], let text = drafts.removeValue(forKey: "new-session") {
            self.text = text
            defaults.set(drafts, forKey: "drafts")
        }
    }
}
