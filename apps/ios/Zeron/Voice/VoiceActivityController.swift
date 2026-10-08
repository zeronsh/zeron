import ActivityKit
import Foundation

/// Mirrors the call into its Live Activity (`ZeronLiveActivity`): requested
/// when a call starts (the app is in front then), updated as the orb changes
/// state and ended with the call. The Dynamic Island only shows it once Zeron
/// leaves the screen. Its mute and end buttons drive this controller's call.
@MainActor
final class VoiceActivityController {
    private weak var voice: RemoteVoiceController?
    private var token: AnyObject?
    private var activity: Activity<VoiceActivityAttributes>?
    private var sent: VoiceActivityAttributes.ContentState?
    private var sentAt = Date.distantPast
    private var pending: Task<Void, Never>?

    /// Orb states flip with every sentence; the island needs at most one
    /// update a second to keep up.
    private static let interval: TimeInterval = 1

    func attach(to voice: RemoteVoiceController) {
        self.voice = voice
        token = voice.observe { [weak self] in self?.sync() }
        VoiceActivityAction.handler = { [weak voice] action in
            switch action {
            case .toggleMute: voice?.toggleMute()
            case .end: voice?.stop()
            }
        }
        // A call never outlives its process: anything still showing is stale.
        Task {
            for stale in Activity<VoiceActivityAttributes>.activities where stale.id != activity?.id {
                await stale.end(nil, dismissalPolicy: .immediate)
            }
        }
    }

    private func sync() {
        guard let voice, voice.live else { return end() }
        let state = VoiceActivityAttributes.ContentState(orb: Self.orb(voice), muted: voice.muted, since: voice.activeSince)
        guard let activity else { return start(host: voice.hostName, state: state) }
        guard state != sent else {
            pending?.cancel()
            pending = nil
            return
        }
        pending?.cancel()
        let delay = Self.interval - Date().timeIntervalSince(sentAt)
        // Mute, end and the call going live answer at once; orb flips settle.
        let urgent = state.muted != sent?.muted || state.since != sent?.since
        pending = Task { [weak self] in
            if !urgent, delay > 0 {
                try? await Task.sleep(for: .seconds(delay))
                if Task.isCancelled { return }
            }
            self?.send(state, to: activity)
        }
    }

    private func start(host: String, state: VoiceActivityAttributes.ContentState) {
        guard ActivityAuthorizationInfo().areActivitiesEnabled else { return }
        do {
            activity = try Activity.request(
                attributes: VoiceActivityAttributes(host: host),
                content: ActivityContent(state: state, staleDate: nil),
                pushType: nil
            )
            sent = state
            sentAt = Date()
        } catch {
            activity = nil
        }
    }

    private func send(_ state: VoiceActivityAttributes.ContentState, to activity: Activity<VoiceActivityAttributes>) {
        guard activity.id == self.activity?.id else { return }
        sent = state
        sentAt = Date()
        Task { await activity.update(ActivityContent(state: state, staleDate: nil)) }
    }

    private func end() {
        pending?.cancel()
        pending = nil
        guard let activity else { return }
        self.activity = nil
        let last = sent
        sent = nil
        Task { await activity.end(last.map { ActivityContent(state: $0, staleDate: nil) }, dismissalPolicy: .immediate) }
    }

    private static func orb(_ voice: RemoteVoiceController) -> VoiceActivityOrb {
        guard let state = voice.state, state.phase == .active else { return .connecting }
        switch state.orb {
        case .speaking: return .speaking
        case .working: return .working
        case .awaitingInput: return .awaiting
        case .listening: return .listening
        case .connecting: return .connecting
        case .muted, .idle: return .muted
        }
    }
}
