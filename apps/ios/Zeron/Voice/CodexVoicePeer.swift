import AVFoundation
import UIKit
import WebRTC

/// Native audio endpoint. Credentials and tools stay on the execution host.
/// The ordered oai-events channel and gathered offer mirror the pinned helper.
///
/// Audio behaves like a phone call: loudspeaker while the phone is in hand,
/// the earpiece while it is held to the ear, and whatever headset or car the
/// user connected otherwise. The call survives the screen locking (background
/// audio); an interruption such as an incoming call ends it.
@MainActor
final class CodexVoicePeer: NSObject {
    enum Failure: Error { case closed, unavailable, permissionDenied, timeout }
    private static let factory: RTCPeerConnectionFactory = {
        RTCInitializeSSL()
        return RTCPeerConnectionFactory()
    }()
    private var peer: RTCPeerConnection?
    private var channel: RTCDataChannel?
    private var track: RTCAudioTrack?
    private var closed = false
    private var active = false
    private var nearEar = false
    private var observers: [NSObjectProtocol] = []
    var onFailure: (() -> Void)?

    func prepare() async throws {
        guard !closed else { throw Failure.closed }
        guard await AVAudioApplication.requestRecordPermission() else { throw Failure.permissionDenied }
        guard !closed, UIApplication.shared.applicationState != .background else { throw Failure.closed }
        let audio = RTCAudioSession.sharedInstance()
        audio.useManualAudio = true
        audio.isAudioEnabled = false
        let configuration = RTCAudioSessionConfiguration.webRTC()
        configuration.category = AVAudioSession.Category.playAndRecord.rawValue
        configuration.mode = AVAudioSession.Mode.voiceChat.rawValue
        // No defaultToSpeaker: the route follows the proximity sensor instead.
        configuration.categoryOptions = [.allowBluetoothHFP]
        // WebRTC re-applies its *global* configuration when the audio unit
        // starts; a session-only change would be overwritten mid-call.
        RTCAudioSessionConfiguration.setWebRTC(configuration)
        audio.lockForConfiguration()
        defer { audio.unlockForConfiguration() }
        try audio.setConfiguration(configuration)
        let config = RTCConfiguration()
        config.sdpSemantics = .unifiedPlan
        config.bundlePolicy = .maxBundle
        guard let connection = Self.factory.peerConnection(with: config, constraints: RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil), delegate: self) else { throw Failure.unavailable }
        peer = connection
        let source = Self.factory.audioSource(with: RTCMediaConstraints(mandatoryConstraints: nil, optionalConstraints: nil))
        let track = Self.factory.audioTrack(with: source, trackId: "voice-audio")
        track.isEnabled = false
        self.track = track
        connection.add(track, streamIds: ["voice"])
        let dataConfig = RTCDataChannelConfiguration()
        dataConfig.isOrdered = true
        channel = connection.dataChannel(forLabel: "oai-events", configuration: dataConfig)
        guard channel != nil else { throw Failure.unavailable }
        channel?.delegate = self
        observers.append(NotificationCenter.default.addObserver(forName: AVAudioSession.interruptionNotification, object: nil, queue: .main) { [weak self] notification in
            // A phone call, Siri or an alarm takes the audio: end, never resume.
            guard (notification.userInfo?[AVAudioSessionInterruptionTypeKey] as? UInt) == AVAudioSession.InterruptionType.began.rawValue else { return }
            MainActor.assumeIsolated { self?.fail() }
        })
        observers.append(NotificationCenter.default.addObserver(forName: AVAudioSession.routeChangeNotification, object: nil, queue: .main) { [weak self] notification in
            // Our own override reports a change too; everything else (a headset
            // leaving, WebRTC re-applying its category) re-routes the call.
            guard (notification.userInfo?[AVAudioSessionRouteChangeReasonKey] as? UInt) != AVAudioSession.RouteChangeReason.override.rawValue else { return }
            MainActor.assumeIsolated { self?.applyRoute() }
        })
    }

    func offer() async throws -> String {
        guard let peer, !closed else { throw Failure.closed }
        let offer: RTCSessionDescription = try await callback { complete in
            peer.offer(for: RTCMediaConstraints(mandatoryConstraints: ["OfferToReceiveAudio": "true"], optionalConstraints: nil)) { description, error in
                complete(description.map(Result.success) ?? .failure(error ?? Failure.unavailable))
            }
        }
        let _: Void = try await callback { complete in
            peer.setLocalDescription(offer) { error in complete(error.map(Result.failure) ?? .success(())) }
        }
        try await waitUntil { peer.iceGatheringState == .complete }
        guard let sdp = peer.localDescription?.sdp, sdp.utf8.count <= 65_536 else { throw Failure.unavailable }
        return sdp
    }

    func apply(answer: String) async throws {
        guard let peer, !closed, !answer.isEmpty, answer.utf8.count <= 65_536 else { throw Failure.closed }
        let _: Void = try await callback { complete in
            peer.setRemoteDescription(RTCSessionDescription(type: .answer, sdp: answer)) { error in
                complete(error.map(Result.failure) ?? .success(()))
            }
        }
        try await waitUntil { peer.connectionState == .connected && self.channel?.readyState == .open }
    }

    func setMuted(_ muted: Bool) throws {
        guard !closed, peer?.connectionState == .connected else { throw Failure.closed }
        // Called only after the host confirms the current generation.
        if !active {
            let audio = RTCAudioSession.sharedInstance()
            audio.lockForConfiguration()
            defer { audio.unlockForConfiguration() }
            try audio.setActive(true)
            audio.isAudioEnabled = true
            active = true
            applyRoute(locked: true)
        }
        track?.isEnabled = !muted
    }

    /// The proximity sensor: held to the ear, play through the earpiece.
    func setNearEar(_ near: Bool) {
        guard nearEar != near else { return }
        nearEar = near
        applyRoute()
    }

    /// Loudspeaker in hand, earpiece at the ear; a headset, Bluetooth or car
    /// route always wins.
    private func applyRoute(locked: Bool = false) {
        guard active, !closed else { return }
        let audio = RTCAudioSession.sharedInstance()
        let external = audio.session.currentRoute.outputs.contains {
            $0.portType != .builtInReceiver && $0.portType != .builtInSpeaker
        }
        if !locked { audio.lockForConfiguration() }
        defer { if !locked { audio.unlockForConfiguration() } }
        try? audio.overrideOutputAudioPort(external || nearEar ? .none : .speaker)
    }

    func levels() async throws -> (UInt16, UInt16) {
        guard let peer, !closed, peer.connectionState != .failed, peer.connectionState != .closed else { throw Failure.closed }
        // Nobody sees the orb with the screen off; skip the statistics walk.
        guard UIApplication.shared.applicationState != .background else { return (0, 0) }
        let report: RTCStatisticsReport = try await callback { complete in peer.statistics { complete(.success($0)) } }
        var microphone: Double = 0
        var speaker: Double = 0
        for stat in report.statistics.values {
            guard let level = stat.values["audioLevel"] as? NSNumber else { continue }
            if stat.type == "media-source" { microphone = max(microphone, level.doubleValue) }
            if stat.type == "inbound-rtp" { speaker = max(speaker, level.doubleValue) }
        }
        return (UInt16(min(1, max(0, microphone)) * 65535), UInt16(min(1, max(0, speaker)) * 65535))
    }

    func close() {
        guard !closed else { return }
        closed = true
        track?.isEnabled = false
        RTCAudioSession.sharedInstance().isAudioEnabled = false
        channel?.close()
        peer?.close()
        channel = nil; peer = nil; track = nil
        for observer in observers { NotificationCenter.default.removeObserver(observer) }
        observers.removeAll()
        let audio = RTCAudioSession.sharedInstance()
        audio.lockForConfiguration()
        try? audio.overrideOutputAudioPort(.none)
        try? audio.setActive(false)
        audio.unlockForConfiguration()
        active = false
    }

    private func fail() { guard !closed else { return }; close(); onFailure?() }
    private func waitUntil(_ ready: () -> Bool) async throws {
        let deadline = ContinuousClock.now.advanced(by: .seconds(25))
        while !ready() {
            guard !closed else { throw Failure.closed }
            guard ContinuousClock.now < deadline else { throw Failure.timeout }
            try await Task.sleep(for: .milliseconds(20))
        }
        guard !closed else { throw Failure.closed }
    }
    private func callback<T>(_ start: (@escaping (Result<T, Error>) -> Void) -> Void) async throws -> T {
        let slot = VoiceContinuation<T>()
        return try await withTaskCancellationHandler {
            try await withCheckedThrowingContinuation { continuation in
                slot.install(continuation)
                start { slot.finish($0) }
            }
        } onCancel: { slot.finish(.failure(CancellationError())) }
    }
}

/// A cancelled operation can receive a late RTC callback, exactly once.
final class VoiceContinuation<T>: @unchecked Sendable {
    private let lock = NSLock()
    private var continuation: CheckedContinuation<T, Error>?
    private var result: Result<T, Error>?
    private var finished = false
    func install(_ continuation: CheckedContinuation<T, Error>) {
        lock.lock()
        if let result { lock.unlock(); continuation.resume(with: result); return }
        self.continuation = continuation
        lock.unlock()
    }
    func finish(_ result: Result<T, Error>) {
        lock.lock()
        guard !finished else { lock.unlock(); return }
        finished = true
        self.result = result
        let continuation = self.continuation
        self.continuation = nil
        lock.unlock()
        continuation?.resume(with: result)
    }
}

extension CodexVoicePeer: RTCPeerConnectionDelegate, RTCDataChannelDelegate {
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didChange stateChanged: RTCSignalingState) {}
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didAdd stream: RTCMediaStream) {}
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didRemove stream: RTCMediaStream) {}
    nonisolated func peerConnectionShouldNegotiate(_ peerConnection: RTCPeerConnection) {}
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceConnectionState) {
        if newState == .failed || newState == .closed || newState == .disconnected { Task { @MainActor [weak self] in self?.fail() } }
    }
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didChange newState: RTCIceGatheringState) {}
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didGenerate candidate: RTCIceCandidate) {}
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didRemove candidates: [RTCIceCandidate]) {}
    nonisolated func peerConnection(_ peerConnection: RTCPeerConnection, didOpen dataChannel: RTCDataChannel) { dataChannel.close() }
    nonisolated func dataChannelDidChangeState(_ dataChannel: RTCDataChannel) {
        if dataChannel.readyState == .closed || dataChannel.readyState == .closing { Task { @MainActor [weak self] in self?.fail() } }
    }
    nonisolated func dataChannel(_ dataChannel: RTCDataChannel, didReceiveMessageWith buffer: RTCDataBuffer) {
        // The host owns canonical transcripts and handoffs. Do not persist a second copy.
    }
}
