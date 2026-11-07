import AVFoundation
import WebRTC

/// A call's two tones: one as it is placed, one as it ends.
///
/// They play through the call's own audio session. That session sounds with
/// the Ring/Silent switch set to silent, which a system sound does not, and
/// the tones take the call's route: a headset, Bluetooth or car when one is
/// connected, otherwise the loudspeaker in hand and the earpiece at the ear.
/// The session comes up as the call is placed, so the call finds its own
/// configuration in place and the route holds under the start tone; it goes
/// down once the last tone has played. Playback only: nothing here opens the
/// microphone or changes the volume.
@MainActor
final class CallTones {
    enum Tone: String {
        case start = "voice-start"
        case end = "voice-end"
    }

    /// The newest call's tones own the audio session: an earlier call's end
    /// tone must not take it down under them.
    private static weak var owner: CallTones?

    private let audio: CallToneAudio
    /// When the tone that is playing ends.
    private var quiet = ContinuousClock.now

    init(audio: CallToneAudio? = nil) { self.audio = audio ?? DeviceCallToneAudio() }

    /// The call is placed.
    func start() {
        Self.owner = self
        play(.start, nearEar: false)
    }

    /// The call is over. A hang-up or the end of a connected call `sounds`; a
    /// failure to connect does not. Either way the session goes down once the
    /// tone that is playing has ended.
    func end(sounds: Bool, nearEar: Bool) {
        if sounds { play(.end, nearEar: nearEar) }
        Task { [self, quiet] in
            try? await Task.sleep(until: quiet, clock: .continuous)
            guard CallTones.owner === self else { return }
            CallTones.owner = nil
            audio.deactivate()
        }
    }

    private func play(_ tone: Tone, nearEar: Bool) {
        guard audio.activate(), let length = audio.play(tone, nearEar: nearEar) else { return }
        quiet = .now + .seconds(length)
    }
}

/// What the tones need from the audio system; the tests substitute a silent one.
@MainActor
protocol CallToneAudio: AnyObject {
    /// Brings the call's audio session up. False when the system refuses it,
    /// as it does during a phone call.
    func activate() -> Bool
    /// Starts `tone` on the call's route: its length, or nil when it cannot play.
    func play(_ tone: CallTones.Tone, nearEar: Bool) -> TimeInterval?
    /// Takes the session down, unless a call's own audio has it by now.
    func deactivate()
}

@MainActor
final class DeviceCallToneAudio: CallToneAudio {
    private var player: AVAudioPlayer?

    func activate() -> Bool {
        // The one definition of the call's session; `prepare` applies it too.
        let configuration = CodexVoicePeer.installAudioConfiguration()
        let audio = RTCAudioSession.sharedInstance()
        audio.lockForConfiguration()
        defer { audio.unlockForConfiguration() }
        // A route may refuse the preferred rates; the tones need the category.
        try? audio.setConfiguration(configuration)
        guard audio.category == configuration.category else { return false }
        // Not through RTCAudioSession: its count of activations belongs to
        // the call, which activates on top of this and balances itself.
        return (try? audio.session.setActive(true)) != nil
    }

    func play(_ tone: CallTones.Tone, nearEar: Bool) -> TimeInterval? {
        guard let url = Bundle.main.url(forResource: tone.rawValue, withExtension: "wav"),
              let player = try? AVAudioPlayer(contentsOf: url) else { return nil }
        let audio = RTCAudioSession.sharedInstance()
        audio.lockForConfiguration()
        let loud = Self.loudspeaker(outputs: audio.currentRoute.outputs.map(\.portType), nearEar: nearEar)
        try? audio.overrideOutputAudioPort(loud ? .speaker : .none)
        audio.unlockForConfiguration()
        guard player.play() else { return nil }
        self.player = player
        return player.duration
    }

    func deactivate() {
        player?.stop()
        player = nil
        let audio = RTCAudioSession.sharedInstance()
        audio.lockForConfiguration()
        defer { audio.unlockForConfiguration() }
        // A call that is up has the session, and takes it down itself.
        guard !audio.isActive else { return }
        try? audio.overrideOutputAudioPort(.none)
        try? audio.session.setActive(false, options: .notifyOthersOnDeactivation)
    }

    /// The call's own rule (`CodexVoicePeer.applyRoute`): the loudspeaker in
    /// hand and the earpiece at the ear; a headset, Bluetooth or car route
    /// always wins.
    static func loudspeaker(outputs: [AVAudioSession.Port], nearEar: Bool) -> Bool {
        !nearEar && outputs.allSatisfy { $0 == .builtInReceiver || $0 == .builtInSpeaker }
    }
}
