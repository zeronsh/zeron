import AVFoundation
import WebRTC
import XCTest
@testable import Zeron

/// Configuration only: nothing here records, plays or involves a second app,
/// so it cannot show that a real call survives another app's video.
final class VoiceAudioSessionTests: XCTestCase {
    @MainActor
    func testCallAudioIsAMixableVoiceChat() {
        let configuration = CodexVoicePeer.installAudioConfiguration()
        XCTAssertEqual(configuration.category, AVAudioSession.Category.playAndRecord.rawValue)
        XCTAssertEqual(configuration.mode, AVAudioSession.Mode.voiceChat.rawValue)
        XCTAssertEqual(configuration.categoryOptions, [.allowBluetoothHFP, .mixWithOthers])
    }

    /// WebRTC re-applies its global configuration when the audio unit starts:
    /// the option must be there, replace a stale one, and outlive a call.
    @MainActor
    func testWebRTCKeepsTheMixableConfigurationAcrossCalls() {
        RTCAudioSessionConfiguration.setWebRTC(RTCAudioSessionConfiguration())
        XCTAssertFalse(RTCAudioSessionConfiguration.webRTC().categoryOptions.contains(.mixWithOthers))

        CodexVoicePeer.installAudioConfiguration()
        XCTAssertTrue(RTCAudioSessionConfiguration.webRTC().categoryOptions.contains(.mixWithOthers))

        CodexVoicePeer().close()
        XCTAssertTrue(RTCAudioSessionConfiguration.webRTC().categoryOptions.contains(.mixWithOthers))
    }

    /// What the device session holds once WebRTC has applied and activated its
    /// global configuration, the step it repeats when the audio unit starts.
    @MainActor
    func testActivatedSessionIsMixable() throws {
        CodexVoicePeer.installAudioConfiguration()
        let audio = RTCAudioSession.sharedInstance()
        audio.lockForConfiguration()
        defer { audio.unlockForConfiguration() }
        do {
            try audio.setConfiguration(RTCAudioSessionConfiguration.webRTC(), active: true)
        } catch {
            throw XCTSkip("This host cannot activate a call audio session: \(error.localizedDescription)")
        }
        defer { try? audio.setActive(false) }
        let session = AVAudioSession.sharedInstance()
        XCTAssertEqual(session.category, .playAndRecord)
        XCTAssertEqual(session.mode, .voiceChat)
        XCTAssertTrue(session.categoryOptions.contains(.mixWithOthers))
    }
}
