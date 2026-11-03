import ActivityKit
import AppIntents
import Foundation

/// The voice call as a Live Activity: the Dynamic Island and the Lock Screen
/// keep showing the orchestrator call once Zeron leaves the screen. Shared by
/// the app, which starts and updates it, and the widget extension, which
/// draws it.
struct VoiceActivityAttributes: ActivityAttributes {
    struct ContentState: Codable, Hashable {
        var orb: VoiceActivityOrb
        var muted: Bool
        /// When the call went live; nil while it connects.
        var since: Date?
    }

    /// The execution host Codex runs on ("Fedora", "MacBook Pro").
    var host: String
}

/// The orb's call states. Each has the desktop orb's own drawing (exported
/// from `zeron-orb`) in the extension's asset catalog.
enum VoiceActivityOrb: String, Codable, Hashable {
    case connecting, listening, speaking, working, awaiting, muted

    /// What the call is doing, in the stage's words.
    var status: String {
        switch self {
        case .connecting: "Connecting…"
        case .listening: "Listening"
        case .speaking: "Speaking"
        case .working: "Working on it…"
        case .awaiting: "Codex needs your answer"
        case .muted: "Muted"
        }
    }
}

/// Live Activity buttons run in the app's process, where the call lives. The
/// widget extension compiles these too but never performs them.
enum VoiceActivityAction {
    case toggleMute, end

    @MainActor static var handler: ((VoiceActivityAction) -> Void)?
}

struct ToggleVoiceMuteIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "Mute or Unmute Voice"
    static var isDiscoverable: Bool { false }

    init() {}

    func perform() async throws -> some IntentResult {
        await MainActor.run { VoiceActivityAction.handler?(.toggleMute) }
        return .result()
    }
}

struct EndVoiceCallIntent: LiveActivityIntent {
    static let title: LocalizedStringResource = "End Voice Call"
    static var isDiscoverable: Bool { false }

    init() {}

    func perform() async throws -> some IntentResult {
        await MainActor.run { VoiceActivityAction.handler?(.end) }
        return .result()
    }
}
