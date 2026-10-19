import AppIntents
import Foundation

/// routes asked for by app intents. a control can launch the app cold, so a
/// route that arrives before the scene connects waits for it.
enum LinkRouter {
    @MainActor static var handler: ((URL) -> Void)? {
        didSet {
            guard let handler, let url = pending else { return }
            pending = nil
            handler(url)
        }
    }

    @MainActor private static var pending: URL?

    @MainActor static func open(_ url: URL) {
        if let handler { handler(url) } else { pending = url }
    }
}

/// control center, lock screen and action button entry points. they run in
/// the app's process (`supportedModes = .foreground`); the widget extension
/// compiles them too but never performs them.
struct NewSessionIntent: AppIntent {
    static let title: LocalizedStringResource = "New Session"
    static let description = IntentDescription("Start a new Zeron session.")
    static let supportedModes: IntentModes = .foreground

    init() {}

    func perform() async throws -> some IntentResult {
        await LinkRouter.open(ZeronLink.newSession)
        return .result()
    }
}

struct TalkToCodexIntent: AppIntent {
    static let title: LocalizedStringResource = "Talk to Codex"
    static let description = IntentDescription("Start a voice call with Codex on your default host.")
    static let supportedModes: IntentModes = .foreground

    init() {}

    func perform() async throws -> some IntentResult {
        await LinkRouter.open(ZeronLink.call)
        return .result()
    }
}
