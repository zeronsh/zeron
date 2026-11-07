import AVFoundation
import WebRTC
import XCTest
@testable import Zeron

/// Lifecycle and configuration only. Nothing here plays a sound, and a
/// simulator has no Ring/Silent switch, earpiece or headset: that the tones
/// are heard in Silent Mode is a check for a phone.
final class CallTonesTests: XCTestCase {
    /// Records what the tones ask of the audio system; plays nothing.
    @MainActor
    private final class Audio: CallToneAudio {
        var events: [String] = []
        var refuses = false
        var length: TimeInterval = 0.02
        func activate() -> Bool { events.append("activate"); return !refuses }
        func play(_ tone: CallTones.Tone, nearEar: Bool) -> TimeInterval? {
            events.append(tone.rawValue + (nearEar ? " at the ear" : ""))
            return length
        }
        func deactivate() { events.append("deactivate") }
    }

    @MainActor
    func testAPlacedCallSoundsOnceAndKeepsItsSession() async throws {
        let audio = Audio()
        let tones = CallTones(audio: audio)
        tones.start()
        try await Task.sleep(for: .milliseconds(200))
        // Still up while the call connects: the call's own audio comes next.
        XCTAssertEqual(audio.events, ["activate", "voice-start"])
    }

    @MainActor
    func testAHangUpSoundsOnceThenReleasesTheSession() async throws {
        let audio = Audio()
        let tones = CallTones(audio: audio)
        tones.start()
        tones.end(sounds: true, nearEar: true)
        XCTAssertEqual(audio.events, ["activate", "voice-start", "activate", "voice-end at the ear"])
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(audio.events, ["activate", "voice-start", "activate", "voice-end at the ear", "deactivate"])
    }

    @MainActor
    func testAFailureToConnectIsSilentAndReleasesAfterTheStartTone() async throws {
        let audio = Audio()
        audio.length = 1
        let tones = CallTones(audio: audio)
        tones.start()
        tones.end(sounds: false, nearEar: false)
        try await Task.sleep(for: .milliseconds(100))
        XCTAssertEqual(audio.events, ["activate", "voice-start"], "the start tone plays out")
        try await Task.sleep(for: .milliseconds(1200))
        XCTAssertEqual(audio.events, ["activate", "voice-start", "deactivate"])
    }

    @MainActor
    func testAnEarlierEndToneLeavesANewerCallsSessionUp() async throws {
        let earlier = Audio(), newer = Audio()
        let first = CallTones(audio: earlier)
        first.start()
        first.end(sounds: true, nearEar: false)
        let second = CallTones(audio: newer)
        second.start()
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertFalse(earlier.events.contains("deactivate"))
        XCTAssertFalse(newer.events.contains("deactivate"))
        second.end(sounds: false, nearEar: false)
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertFalse(earlier.events.contains("deactivate"))
        XCTAssertEqual(newer.events, ["activate", "voice-start", "deactivate"])
    }

    @MainActor
    func testARefusedSessionPlaysNothing() async throws {
        let audio = Audio()
        audio.refuses = true
        let tones = CallTones(audio: audio)
        tones.start()
        tones.end(sounds: true, nearEar: false)
        try await Task.sleep(for: .milliseconds(300))
        XCTAssertEqual(audio.events, ["activate", "activate", "deactivate"])
    }

    @MainActor
    func testTonesTakeTheLoudspeakerOnlyFromTheEarpieceInHand() {
        XCTAssertTrue(DeviceCallToneAudio.loudspeaker(outputs: [.builtInReceiver], nearEar: false))
        XCTAssertFalse(DeviceCallToneAudio.loudspeaker(outputs: [.builtInReceiver], nearEar: true))
        for external: AVAudioSession.Port in [.headphones, .bluetoothHFP, .bluetoothA2DP, .carAudio, .usbAudio] {
            XCTAssertFalse(DeviceCallToneAudio.loudspeaker(outputs: [external], nearEar: false), external.rawValue)
        }
    }

    func testBothTonesAreBundledAndDistinct() throws {
        let files = try [CallTones.Tone.start, .end].map { tone -> Data in
            let url = try XCTUnwrap(Bundle.main.url(forResource: tone.rawValue, withExtension: "wav"), tone.rawValue)
            let duration = try AVAudioPlayer(contentsOf: url).duration
            XCTAssertGreaterThan(duration, 0.2, tone.rawValue)
            XCTAssertLessThan(duration, 2, tone.rawValue)
            return try Data(contentsOf: url)
        }
        XCTAssertNotEqual(files[0], files[1])
    }

    /// Apple: `playAndRecord` audio "continues with the Silent switch set to
    /// silent"; the default category is silenced by it.
    @MainActor
    func testTheTonesSessionIsTheCallsOwn() throws {
        let audio = DeviceCallToneAudio()
        guard audio.activate() else { throw XCTSkip("This host cannot activate a call audio session.") }
        defer { audio.deactivate() }
        let session = AVAudioSession.sharedInstance()
        XCTAssertEqual(session.category, .playAndRecord)
        XCTAssertEqual(session.mode, .voiceChat)
        XCTAssertTrue(session.categoryOptions.contains(.mixWithOthers))
        XCTAssertFalse(RTCAudioSession.sharedInstance().isActive, "the call's count of activations is not the tones' to take")
    }

    /// RTCAudioSession counts activations and takes the device session down
    /// on the last release only. A peer that closes without having activated
    /// has nothing to release: otherwise the count runs negative, later calls
    /// leave the session up, and the tones can never take it down.
    @MainActor
    func testClosingAPeerThatNeverActivatedReleasesNothing() throws {
        CodexVoicePeer().close()
        CodexVoicePeer().close()
        let audio = RTCAudioSession.sharedInstance()
        audio.lockForConfiguration()
        defer { audio.unlockForConfiguration() }
        do { try audio.setActive(true) } catch {
            throw XCTSkip("This host cannot activate an audio session: \(error.localizedDescription)")
        }
        try? audio.setActive(false)
        XCTAssertFalse(audio.isActive, "a balanced activation takes the session down")
    }
}
