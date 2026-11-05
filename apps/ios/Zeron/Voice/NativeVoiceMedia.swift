import Foundation

/// A single-generation platform bridge. The Rust call owns the callback object;
/// its back-reference is weak so releasing the call always closes native audio.
@MainActor
final class NativeVoiceMedia: VoiceMediaListener {
    weak var call: VoiceCall?
    private let peer = CodexVoicePeer()
    private var operations: [UInt64: Task<Void, Never>] = [:]
    private var closed = false
    private var activated = false
    var onFailure: (() -> Void)?

    init() {
        peer.onFailure = { [weak self] in
            guard let self else { return }
            self.close()
            self.call?.stop()
            self.onFailure?()
        }
    }

    nonisolated func onRequest(request: VoiceMediaRequest) {
        Task { @MainActor in self.perform(request) }
    }

    private func perform(_ request: VoiceMediaRequest) {
        if case .close = request.operation { close(); return }
        guard !closed else {
            call?.completeMedia(requestId: request.requestId, failure: .unavailable, sdp: nil, microphone: 0, speaker: 0)
            return
        }
        guard operations.count < 4 else { close(); call?.stop(); return }
        operations[request.requestId] = Task { @MainActor [weak self] in
            guard let self else { return }
            defer { self.operations[request.requestId] = nil }
            do {
                var sdp: String?
                var microphone: UInt16 = 0
                var speaker: UInt16 = 0
                switch request.operation {
                case .prepare: try await self.peer.prepare()
                case .offer: sdp = try await self.peer.offer()
                case .applyAnswer:
                    guard let answer = request.sdp else { throw CodexVoicePeer.Failure.unavailable }
                    try await self.peer.apply(answer: answer)
                case .setMuted:
                    try self.peer.setMuted(request.muted)
                    self.activated = true
                case .levels: (microphone, speaker) = try await self.peer.levels()
                case .close: self.close()
                }
                guard !self.closed, !Task.isCancelled else { return }
                self.call?.completeMedia(requestId: request.requestId, failure: nil, sdp: sdp, microphone: microphone, speaker: speaker)
            } catch {
                guard !self.closed else { return }
                let failure: VoiceMediaFailure = (error as? CodexVoicePeer.Failure) == .permissionDenied ? .permissionDenied : .unavailable
                self.call?.completeMedia(requestId: request.requestId, failure: failure, sdp: nil, microphone: 0, speaker: 0)
            }
        }
    }

    func muteLocally(_ muted: Bool) {
        // UI mute never waits for the Rust control queue or the relay.
        guard !closed, activated else { return }
        try? peer.setMuted(muted)
    }

    /// Proximity sensor state: the earpiece while the phone is at the ear.
    func setNearEar(_ near: Bool) {
        guard !closed else { return }
        peer.setNearEar(near)
    }

    func close() {
        guard !closed else { return }
        closed = true
        peer.close()
        for operation in operations.values { operation.cancel() }
        operations.removeAll()
    }
}
